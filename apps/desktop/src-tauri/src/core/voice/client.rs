use std::collections::{hash_map::Entry, BTreeMap, HashMap};
use std::convert::TryInto;
use std::io::ErrorKind;
use std::marker::PhantomData;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::BytesMut;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use mumble_protocol::control::{msgs, ClientControlCodec, ControlPacket};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::{Clientbound, VoicePacket, VoicePacketPayload};
use mumble_protocol::Serverbound;
use native_tls::TlsConnector as NativeTlsConnector;
use opus2::{Application, Bitrate, Channels, Decoder as OpusDecoder, Encoder as OpusEncoder};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, sleep, MissedTickBehavior};
use tokio_native_tls::{TlsConnector, TlsStream};
use tokio_util::codec::{Decoder, Framed};

use tauri::AppHandle;

use super::audio_in::{self, InputCapture};
use super::audio_out::{self, OutputPlayback};
use super::vad::VoiceActivityDetector;
use crate::core::config::{
    AppConfig, DEFAULT_USER_PASSWORD, SUPERUSER_AUTH_PASSWORD, SUPERUSER_AUTH_USERNAME,
    SUPERUSER_TRIGGER_NICKNAME,
};
use crate::core::events::{
    self, ConnectionEvent, ConnectionState, MessageEvent, RosterEvent, SelfEvent, SpeakingEvent,
};

type ControlFramed = Framed<TlsStream<TcpStream>, ClientControlCodec>;
type ControlSink = SplitSink<ControlFramed, ControlPacket<Serverbound>>;
type ControlStream = SplitStream<ControlFramed>;

const OPUS_SAMPLE_RATE: u32 = 48_000;
const OPUS_FRAME_SAMPLES: usize = 960;
const OPUS_MAX_PACKET_SIZE: usize = 1024;
const OPUS_MAX_DECODED_SAMPLES: usize = 5760;
const OPUS_SEQ_STEP: u64 = OPUS_FRAME_SAMPLES as u64;
const OPUS_BITRATE_BPS: i32 = 32_000;
const OPUS_COMPLEXITY: i32 = 8;
const OPUS_PACKET_LOSS_PCT: i32 = 8;
const MEDIA_TICK_MS: u64 = 20;
const UDP_PING_INTERVAL_SECS: u64 = 5;
const VOICE_HANGOVER_FRAMES: u32 = 4;
const VAD_THRESHOLD: f32 = 0.015;
const UDP_DECRYPT_FAILURE_THRESHOLD: u32 = 12;
const UDP_DEGRADED_WINDOW_MS: u64 = 10_000;
const RX_JITTER_TARGET_FRAMES: usize = 3;
const RX_JITTER_MAX_FRAMES: usize = 8;
const RX_GAP_PLC_TRIGGER_FRAMES: u64 = 2;

#[derive(Clone)]
pub struct VoiceSharedState {
    pub connection: Arc<RwLock<ConnectionEvent>>,
    pub roster: Arc<RwLock<RosterEvent>>,
    pub self_state: Arc<RwLock<SelfEvent>>,
}

pub struct VoiceService {
    worker: Option<tauri::async_runtime::JoinHandle<()>>,
    command_tx: Option<mpsc::UnboundedSender<VoiceCommand>>,
}

impl VoiceService {
    pub fn new() -> Self {
        Self {
            worker: None,
            command_tx: None,
        }
    }

    pub async fn connect(
        &mut self,
        app: AppHandle,
        config: AppConfig,
        shared: VoiceSharedState,
    ) -> Result<(), String> {
        self.disconnect().await;

        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let handle = tauri::async_runtime::spawn(run_voice_worker(app, config, shared, command_rx));

        self.command_tx = Some(command_tx);
        self.worker = Some(handle);
        Ok(())
    }

    pub async fn disconnect(&mut self) {
        if let Some(tx) = self.command_tx.take() {
            let _ = tx.send(VoiceCommand::Disconnect);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
    }

    pub fn set_mute(&self, muted: bool) {
        self.send_command(VoiceCommand::SetMute(muted));
    }

    pub fn set_deafen(&self, deafened: bool) {
        self.send_command(VoiceCommand::SetDeafen(deafened));
    }

    pub fn set_ptt(&self, enabled: bool) {
        self.send_command(VoiceCommand::SetPtt(enabled));
    }

    pub fn set_ptt_hotkey(&self, hotkey: String) {
        self.send_command(VoiceCommand::SetPttHotkey(hotkey));
    }

    pub fn set_input_device(&self, device_id: String) {
        self.send_command(VoiceCommand::SetInputDevice(device_id));
    }

    pub fn set_output_device(&self, device_id: String) {
        self.send_command(VoiceCommand::SetOutputDevice(device_id));
    }

    pub fn send_message(&self, message: String) -> Result<(), String> {
        self.send_command_result(VoiceCommand::SendMessage(message))
    }

    fn send_command(&self, command: VoiceCommand) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(command);
        }
    }

    fn send_command_result(&self, command: VoiceCommand) -> Result<(), String> {
        let Some(tx) = &self.command_tx else {
            return Err("voice service is not connected".to_string());
        };
        tx.send(command)
            .map_err(|_| "voice worker is not running".to_string())
    }
}

enum VoiceCommand {
    Disconnect,
    SetMute(bool),
    SetDeafen(bool),
    SetPtt(bool),
    SetPttHotkey(String),
    SetInputDevice(String),
    SetOutputDevice(String),
    SendMessage(String),
}

struct LiveConnection {
    sink: ControlSink,
    stream: ControlStream,
    server_addr: SocketAddr,
}

struct ProtocolUser {
    session: u32,
    name: String,
    channel_id: u32,
    muted: bool,
    deafened: bool,
    speaking: bool,
    speaking_at: Option<Instant>,
}

impl ProtocolUser {
    fn new(session: u32) -> Self {
        Self {
            session,
            name: format!("User {}", session),
            channel_id: 0,
            muted: false,
            deafened: false,
            speaking: false,
            speaking_at: None,
        }
    }
}

struct ProtocolRoster {
    channels: HashMap<u32, String>,
    users: HashMap<u32, ProtocolUser>,
    self_session: Option<u32>,
    active_channel_id: Option<u32>,
    default_channel_name: String,
    default_channel_join_requested: bool,
}

impl ProtocolRoster {
    fn new(default_channel_name: String) -> Self {
        Self {
            channels: HashMap::new(),
            users: HashMap::new(),
            self_session: None,
            active_channel_id: None,
            default_channel_name,
            default_channel_join_requested: false,
        }
    }

    fn set_self_session(&mut self, session: u32) {
        self.self_session = Some(session);
    }

    fn apply_channel_state(&mut self, msg: &msgs::ChannelState) -> bool {
        if !msg.has_channel_id() {
            return false;
        }

        let channel_id = msg.get_channel_id();
        let new_name = if msg.has_name() {
            msg.get_name().to_string()
        } else {
            self.channels
                .get(&channel_id)
                .cloned()
                .unwrap_or_else(|| format!("Channel {}", channel_id))
        };

        if self.channels.get(&channel_id) == Some(&new_name) {
            return false;
        }

        self.channels.insert(channel_id, new_name);
        true
    }

    fn remove_channel(&mut self, channel_id: u32) -> bool {
        self.channels.remove(&channel_id).is_some()
    }

    fn apply_user_state(&mut self, msg: &msgs::UserState) -> (bool, Option<SelfEvent>) {
        if !msg.has_session() {
            return (false, None);
        }

        let session = msg.get_session();
        let user = self
            .users
            .entry(session)
            .or_insert_with(|| ProtocolUser::new(session));
        let mut changed = false;

        if msg.has_name() {
            let next_name = msg.get_name().to_string();
            if user.name != next_name {
                user.name = next_name;
                changed = true;
            }
        }

        if msg.has_channel_id() {
            let next_channel = msg.get_channel_id();
            if user.channel_id != next_channel {
                user.channel_id = next_channel;
                changed = true;
            }
        }

        let next_muted =
            (msg.has_mute() && msg.get_mute()) || (msg.has_self_mute() && msg.get_self_mute());
        if user.muted != next_muted {
            user.muted = next_muted;
            changed = true;
        }

        let next_deafened =
            (msg.has_deaf() && msg.get_deaf()) || (msg.has_self_deaf() && msg.get_self_deaf());
        if user.deafened != next_deafened {
            user.deafened = next_deafened;
            changed = true;
        }

        let mut self_event = None;
        if self.self_session == Some(session) {
            self.active_channel_id = Some(user.channel_id);
            self_event = Some(SelfEvent {
                muted: user.muted,
                deafened: user.deafened,
                ptt_enabled: false,
                transmitting: false,
            });
        }

        (changed, self_event)
    }

    fn remove_user(&mut self, session: u32) -> bool {
        self.users.remove(&session).is_some()
    }

    fn maybe_mark_speaking(&mut self, session: u32) -> Option<SpeakingEvent> {
        let user = self.users.get_mut(&session)?;
        user.speaking_at = Some(Instant::now());
        if user.speaking {
            return None;
        }
        user.speaking = true;
        Some(SpeakingEvent {
            user_id: session.to_string(),
            speaking: true,
            level: Some(1.0),
        })
    }

    fn expire_speaking(&mut self, max_age: Duration) -> Vec<SpeakingEvent> {
        let now = Instant::now();
        let mut updates = Vec::new();
        for user in self.users.values_mut() {
            if !user.speaking {
                continue;
            }
            let Some(last_tick) = user.speaking_at else {
                continue;
            };
            if now.duration_since(last_tick) <= max_age {
                continue;
            }
            user.speaking = false;
            user.speaking_at = None;
            updates.push(SpeakingEvent {
                user_id: user.session.to_string(),
                speaking: false,
                level: Some(0.0),
            });
        }
        updates
    }

    fn target_channel_id(&self) -> Option<u32> {
        if let Some(channel_id) = self.active_channel_id {
            return Some(channel_id);
        }
        self.self_session
            .and_then(|session| self.users.get(&session).map(|user| user.channel_id))
    }

    fn default_channel_id(&self) -> Option<u32> {
        self.channels.iter().find_map(|(channel_id, name)| {
            (name == &self.default_channel_name).then_some(*channel_id)
        })
    }

    fn build_roster_event(&self) -> RosterEvent {
        let channel_id = self.target_channel_id().unwrap_or(0);
        let channel_name = self
            .channels
            .get(&channel_id)
            .cloned()
            .unwrap_or_else(|| self.default_channel_name.clone());

        let mut users = self
            .users
            .values()
            .filter(|user| channel_id == 0 || user.channel_id == channel_id)
            .map(|user| events::RosterUser {
                id: user.session.to_string(),
                name: user.name.clone(),
                muted: user.muted,
                deafened: user.deafened,
                speaking: user.speaking,
            })
            .collect::<Vec<_>>();

        users.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));

        RosterEvent {
            channel: events::ChannelInfo {
                id: channel_id.to_string(),
                name: channel_name,
            },
            users,
        }
    }

    fn user_name_for_session(&self, session: u32) -> String {
        self.users
            .get(&session)
            .map(|user| user.name.clone())
            .unwrap_or_else(|| format!("User {}", session))
    }
}

struct MonoRateConverter {
    ratio: f64,
    source_pos: f64,
    pending: Vec<f32>,
}

impl MonoRateConverter {
    fn new(input_rate: u32, output_rate: u32) -> Self {
        let safe_input = input_rate.max(1);
        let safe_output = output_rate.max(1);
        Self {
            ratio: safe_input as f64 / safe_output as f64,
            source_pos: 0.0,
            pending: Vec::with_capacity(OPUS_FRAME_SAMPLES * 2),
        }
    }

    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }

        if (self.ratio - 1.0).abs() <= f64::EPSILON {
            output.extend_from_slice(input);
            return;
        }

        self.pending.extend_from_slice(input);
        if self.pending.len() < 2 {
            return;
        }

        while self.source_pos + 1.0 < self.pending.len() as f64 {
            let left_idx = self.source_pos.floor() as usize;
            let frac = (self.source_pos - left_idx as f64) as f32;
            let left = self.pending[left_idx];
            let right = self.pending[left_idx + 1];
            output.push(left + (right - left) * frac);
            self.source_pos += self.ratio;
        }

        let consumed = self.source_pos.floor() as usize;
        if consumed > 0 {
            self.pending.drain(..consumed);
            self.source_pos -= consumed as f64;
        }
    }
}

#[derive(Default)]
struct InboundVoiceStream {
    expected_seq: Option<u64>,
    started: bool,
    buffered: BTreeMap<u64, Vec<u8>>,
    last_packet_at: Option<Instant>,
}

enum DecodeAction {
    Frame(Vec<u8>),
    ConcealLoss,
}

#[derive(Clone, Copy)]
struct UdpTransportStats {
    good: u32,
    late: u32,
    lost: u32,
}

struct MediaRuntime {
    udp_socket: Option<std::net::UdpSocket>,
    crypt_state: Option<ClientCryptState>,
    input_capture: Option<InputCapture>,
    input_converter: Option<MonoRateConverter>,
    output_playback: Option<OutputPlayback>,
    capture_48k: Vec<f32>,
    encoder: OpusEncoder,
    decoders: HashMap<u32, OpusDecoder>,
    inbound_streams: HashMap<u32, InboundVoiceStream>,
    seq_num: u64,
    transmitting: bool,
    silence_frames: u32,
    vad: VoiceActivityDetector,
    muted: bool,
    deafened: bool,
    ptt_enabled: bool,
    ptt_hotkey: String,
    udp_consecutive_decrypt_failures: u32,
    last_udp_audio_rx_at: Option<Instant>,
    udp_degraded_until: Option<Instant>,
}

impl MediaRuntime {
    fn new(
        config: &AppConfig,
        initial_self: &SelfEvent,
        server_addr: SocketAddr,
    ) -> Result<Self, String> {
        let udp_socket = match create_udp_socket(server_addr) {
            Ok(socket) => Some(socket),
            Err(err) => {
                log::warn!("failed to initialize UDP socket: {err}");
                None
            }
        };

        let input_capture = match audio_in::start_input_capture(config.input_device.as_deref()) {
            Ok(capture) => Some(capture),
            Err(err) => {
                log::warn!("failed to start input capture: {err}");
                None
            }
        };
        let input_converter = input_capture
            .as_ref()
            .map(|capture| MonoRateConverter::new(capture.sample_rate(), OPUS_SAMPLE_RATE));

        let output_playback =
            match audio_out::start_output_playback(config.output_device.as_deref()) {
                Ok(playback) => Some(playback),
                Err(err) => {
                    log::warn!("failed to start output playback: {err}");
                    None
                }
            };

        let mut encoder = OpusEncoder::new(OPUS_SAMPLE_RATE, Channels::Mono, Application::Voip)
            .map_err(|err| format!("failed to create opus encoder: {err}"))?;
        configure_encoder(&mut encoder)
            .map_err(|err| format!("failed to configure opus encoder: {err}"))?;

        Ok(Self {
            udp_socket,
            crypt_state: None,
            input_capture,
            input_converter,
            output_playback,
            capture_48k: Vec::with_capacity(OPUS_FRAME_SAMPLES * 8),
            encoder,
            decoders: HashMap::new(),
            inbound_streams: HashMap::new(),
            seq_num: 0,
            transmitting: false,
            silence_frames: 0,
            vad: VoiceActivityDetector::new(VAD_THRESHOLD),
            muted: initial_self.muted,
            deafened: initial_self.deafened,
            ptt_enabled: initial_self.ptt_enabled,
            ptt_hotkey: config.ptt_hotkey.clone(),
            udp_consecutive_decrypt_failures: 0,
            last_udp_audio_rx_at: None,
            udp_degraded_until: None,
        })
    }

    fn apply_crypt_setup(
        &mut self,
        msg: &msgs::CryptSetup,
    ) -> Result<Option<msgs::CryptSetup>, String> {
        let key = msg.get_key();
        let client_nonce = msg.get_client_nonce();
        let server_nonce = msg.get_server_nonce();

        if key.len() == 16 && client_nonce.len() == 16 && server_nonce.len() == 16 {
            let key: [u8; 16] = key
                .try_into()
                .map_err(|_| "invalid crypt setup key length".to_string())?;
            let client_nonce: [u8; 16] = client_nonce
                .try_into()
                .map_err(|_| "invalid crypt setup client nonce length".to_string())?;
            let server_nonce: [u8; 16] = server_nonce
                .try_into()
                .map_err(|_| "invalid crypt setup server nonce length".to_string())?;
            self.crypt_state = Some(ClientCryptState::new_from(key, client_nonce, server_nonce));
            return Ok(None);
        }

        if !server_nonce.is_empty() {
            let nonce: [u8; 16] = server_nonce
                .try_into()
                .map_err(|_| "invalid crypt setup server nonce length".to_string())?;
            if let Some(state) = self.crypt_state.as_mut() {
                state.set_decrypt_nonce(&nonce);
            }
            return Ok(None);
        }

        if key.is_empty() && client_nonce.is_empty() && server_nonce.is_empty() {
            if let Some(state) = self.crypt_state.as_ref() {
                let mut response = msgs::CryptSetup::new();
                response.set_client_nonce(state.get_encrypt_nonce().to_vec());
                return Ok(Some(response));
            }
        }

        Ok(None)
    }

    fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    fn set_deafened(&mut self, deafened: bool) {
        self.deafened = deafened;
    }

    fn set_ptt(&mut self, enabled: bool) {
        self.ptt_enabled = enabled;
    }

    fn set_ptt_hotkey(&mut self, hotkey: String) {
        self.ptt_hotkey = hotkey;
    }

    fn set_input_device(&mut self, device_id: String) {
        match audio_in::start_input_capture(Some(device_id.as_str())) {
            Ok(capture) => {
                self.input_converter = Some(MonoRateConverter::new(
                    capture.sample_rate(),
                    OPUS_SAMPLE_RATE,
                ));
                self.input_capture = Some(capture);
            }
            Err(err) => {
                log::warn!("failed to switch input device: {err}");
            }
        }
    }

    fn set_output_device(&mut self, device_id: String) {
        match audio_out::start_output_playback(Some(device_id.as_str())) {
            Ok(playback) => {
                self.output_playback = Some(playback);
            }
            Err(err) => {
                log::warn!("failed to switch output device: {err}");
            }
        }
    }

    fn transport_stats(&mut self) -> Option<UdpTransportStats> {
        if !self.can_send_udp_voice() {
            return None;
        }
        let crypt = self.crypt_state.as_ref()?;
        Some(UdpTransportStats {
            good: crypt.get_good(),
            late: crypt.get_late(),
            lost: crypt.get_lost(),
        })
    }

    fn send_udp_ping(&mut self) -> Result<(), String> {
        if !self.can_send_udp_voice() {
            return Ok(());
        }
        self.send_udp_packet(VoicePacket::Ping {
            timestamp: epoch_millis(),
        })
    }

    fn poll_udp_inbound(
        &mut self,
        app: &AppHandle,
        roster: &mut ProtocolRoster,
    ) -> Result<bool, String> {
        if self.udp_socket.is_none() || self.crypt_state.is_none() {
            return Ok(false);
        }

        let mut roster_changed = false;
        loop {
            let mut buf = [0_u8; 2048];
            let len = {
                let Some(socket) = self.udp_socket.as_ref() else {
                    return Ok(roster_changed);
                };
                match socket.recv(&mut buf) {
                    Ok(len) => len,
                    Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                    Err(err) => return Err(format!("udp receive failed: {err}")),
                }
            };

            let mut encrypted = BytesMut::from(&buf[..len]);
            let decrypt_result = {
                let Some(crypt) = self.crypt_state.as_mut() else {
                    continue;
                };
                crypt.decrypt(&mut encrypted)
            };
            let packet = match decrypt_result {
                Ok(Ok(packet)) => {
                    self.udp_consecutive_decrypt_failures = 0;
                    packet
                }
                Ok(Err(err)) => {
                    log::debug!("invalid decrypted udp packet: {err}");
                    self.mark_udp_decrypt_failure();
                    continue;
                }
                Err(err) => {
                    log::debug!("failed to decrypt udp packet: {err:?}");
                    self.mark_udp_decrypt_failure();
                    continue;
                }
            };

            match packet {
                VoicePacket::Ping { timestamp } => {
                    let _ = self.send_udp_packet(VoicePacket::Ping { timestamp });
                }
                VoicePacket::Audio { .. } => {
                    if self.handle_incoming_voice(packet, app, roster)? {
                        roster_changed = true;
                    }
                    self.mark_udp_audio_rx();
                }
            }
        }

        Ok(roster_changed)
    }

    async fn pump_capture_and_send(
        &mut self,
        sink: &mut ControlSink,
        app: &AppHandle,
        shared: &VoiceSharedState,
    ) -> Result<(), String> {
        let mut drained = Vec::new();
        if let Some(capture) = &self.input_capture {
            capture.drain_samples(&mut drained);
        }

        if !drained.is_empty() {
            if let Some(converter) = self.input_converter.as_mut() {
                converter.process(&drained, &mut self.capture_48k);
            } else {
                self.capture_48k.extend(drained);
            }
        }

        let mut sent_voice_frame = false;
        while self.capture_48k.len() >= OPUS_FRAME_SAMPLES {
            let frame = self
                .capture_48k
                .drain(..OPUS_FRAME_SAMPLES)
                .collect::<Vec<f32>>();
            let level = rms_level(&frame);
            let should_tx = self.should_transmit(level);

            if should_tx {
                self.silence_frames = 0;
                let encoded = self.encode_frame(&frame)?;
                let packet = VoicePacket::Audio {
                    _dst: PhantomData,
                    target: 0,
                    session_id: (),
                    seq_num: self.seq_num,
                    payload: VoicePacketPayload::Opus(encoded.into(), false),
                    position_info: None,
                };
                self.seq_num = self.seq_num.wrapping_add(OPUS_SEQ_STEP);
                self.send_voice_packet(packet, sink).await?;
                sent_voice_frame = true;
            } else if self.transmitting {
                self.silence_frames = self.silence_frames.saturating_add(1);
                if self.silence_frames >= VOICE_HANGOVER_FRAMES {
                    self.send_termination_packet(sink).await?;
                    self.silence_frames = 0;
                    self.set_transmitting_state(app, shared, false).await?;
                }
            }
        }

        if sent_voice_frame {
            self.set_transmitting_state(app, shared, true).await?;
        }

        Ok(())
    }

    async fn set_transmitting_state(
        &mut self,
        app: &AppHandle,
        shared: &VoiceSharedState,
        transmitting: bool,
    ) -> Result<(), String> {
        if self.transmitting == transmitting {
            return Ok(());
        }
        self.transmitting = transmitting;

        let next = {
            let mut self_state = shared.self_state.write().await;
            self_state.transmitting = transmitting;
            self_state.clone()
        };
        let _ = events::emit_self(app, &next);
        Ok(())
    }

    async fn send_termination_packet(&mut self, sink: &mut ControlSink) -> Result<(), String> {
        let silence = vec![0_f32; OPUS_FRAME_SAMPLES];
        let encoded = self.encode_frame(&silence)?;
        let packet = VoicePacket::Audio {
            _dst: PhantomData,
            target: 0,
            session_id: (),
            seq_num: self.seq_num,
            payload: VoicePacketPayload::Opus(encoded.into(), true),
            position_info: None,
        };
        self.seq_num = self.seq_num.wrapping_add(OPUS_SEQ_STEP);
        self.send_voice_packet(packet, sink).await
    }

    async fn send_voice_packet(
        &mut self,
        packet: VoicePacket<Serverbound>,
        sink: &mut ControlSink,
    ) -> Result<(), String> {
        if self.can_send_udp_voice() {
            match self.send_udp_packet(packet.clone()) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    log::warn!("udp voice send failed; tunneling voice over tcp: {err}");
                    self.degrade_udp_path("udp_send_failed");
                }
            }
        }

        sink.send(ControlPacket::<Serverbound>::from(packet))
            .await
            .map_err(|err| format!("failed to send tunneled voice packet: {err}"))
    }

    fn can_send_udp(&self) -> bool {
        self.udp_socket.is_some() && self.crypt_state.is_some()
    }

    fn can_send_udp_voice(&mut self) -> bool {
        if !self.can_send_udp() {
            return false;
        }

        let now = Instant::now();
        if let Some(until) = self.udp_degraded_until {
            if now < until {
                return false;
            }
            self.udp_degraded_until = None;
            self.udp_consecutive_decrypt_failures = 0;
            log::info!("udp degrade window expired; retrying udp voice path");
        }

        true
    }

    fn mark_udp_decrypt_failure(&mut self) {
        self.udp_consecutive_decrypt_failures = self.udp_consecutive_decrypt_failures.saturating_add(1);
        if self.udp_consecutive_decrypt_failures < UDP_DECRYPT_FAILURE_THRESHOLD {
            return;
        }
        self.degrade_udp_path("udp_decrypt_failures");
    }

    fn mark_udp_audio_rx(&mut self) {
        self.udp_consecutive_decrypt_failures = 0;
        self.last_udp_audio_rx_at = Some(Instant::now());
        if self.udp_degraded_until.take().is_some() {
            log::info!("udp audio receive recovered; re-enabling udp voice path");
        }
    }

    fn degrade_udp_path(&mut self, reason: &str) {
        let now = Instant::now();
        self.udp_consecutive_decrypt_failures = 0;
        self.udp_degraded_until = Some(now + Duration::from_millis(UDP_DEGRADED_WINDOW_MS));

        let since_last_audio_ms = self
            .last_udp_audio_rx_at
            .map(|last| now.duration_since(last).as_millis());

        match since_last_audio_ms {
            Some(ms) => log::warn!(
                "degrading udp voice path ({reason}); last udp audio rx was {ms}ms ago"
            ),
            None => log::warn!("degrading udp voice path ({reason}); no udp audio received yet"),
        }
    }

    fn send_udp_packet(&mut self, packet: VoicePacket<Serverbound>) -> Result<(), String> {
        let Some(socket) = self.udp_socket.as_ref() else {
            return Err("udp socket not initialized".to_string());
        };
        let Some(crypt_state) = self.crypt_state.as_mut() else {
            return Err("udp crypt state not initialized".to_string());
        };

        let mut encrypted = BytesMut::with_capacity(OPUS_MAX_PACKET_SIZE);
        crypt_state.encrypt(packet, &mut encrypted);
        socket
            .send(&encrypted)
            .map_err(|err| format!("udp send failed: {err}"))?;
        Ok(())
    }

    fn should_transmit(&mut self, level: f32) -> bool {
        if self.muted || self.deafened {
            return false;
        }

        // Hotkey press detection is not wired yet; do not block audio path.
        if self.ptt_enabled {
            return self.vad.is_speaking(level);
        }

        self.vad.is_speaking(level)
    }

    fn encode_frame(&mut self, frame: &[f32]) -> Result<Vec<u8>, String> {
        let mut pcm = Vec::with_capacity(frame.len());
        for &sample in frame {
            let clamped = sample.clamp(-1.0, 1.0);
            pcm.push((clamped * i16::MAX as f32) as i16);
        }

        let mut packet = vec![0_u8; OPUS_MAX_PACKET_SIZE];
        let written = self
            .encoder
            .encode(&pcm, &mut packet)
            .map_err(|err| format!("opus encode failed: {err}"))?;
        packet.truncate(written);
        Ok(packet)
    }

    fn handle_incoming_voice(
        &mut self,
        packet: VoicePacket<Clientbound>,
        app: &AppHandle,
        roster: &mut ProtocolRoster,
    ) -> Result<bool, String> {
        let VoicePacket::Audio {
            session_id,
            seq_num,
            payload,
            ..
        } = packet
        else {
            return Ok(false);
        };

        let mut changed = false;
        if let Some(update) = roster.maybe_mark_speaking(session_id) {
            let _ = events::emit_speaking(app, &update);
            changed = true;
        }

        if let VoicePacketPayload::Opus(frame, _) = payload {
            let actions = self.queue_inbound_voice(session_id, seq_num, frame.to_vec());
            self.play_inbound_actions(session_id, actions)?;
        }

        Ok(changed)
    }

    fn drain_inbound_playout(&mut self) -> Result<(), String> {
        let session_ids = self.inbound_streams.keys().copied().collect::<Vec<_>>();
        for session_id in session_ids {
            let force_gap_conceal = self
                .inbound_streams
                .get(&session_id)
                .and_then(|stream| stream.last_packet_at)
                .map(|last_packet| last_packet.elapsed() >= Duration::from_millis(MEDIA_TICK_MS))
                .unwrap_or(false);
            let actions = {
                let Some(stream) = self.inbound_streams.get_mut(&session_id) else {
                    continue;
                };
                collect_decode_actions(stream, force_gap_conceal)
            };
            self.play_inbound_actions(session_id, actions)?;
        }
        Ok(())
    }

    fn queue_inbound_voice(
        &mut self,
        session_id: u32,
        seq_num: u64,
        frame: Vec<u8>,
    ) -> Vec<DecodeAction> {
        let stream = self.inbound_streams.entry(session_id).or_default();
        if let Some(expected) = stream.expected_seq {
            if seq_num < expected {
                log::debug!(
                    "dropping late voice frame for session {session_id}: seq={seq_num} expected={expected}"
                );
                return Vec::new();
            }
        }

        stream.buffered.entry(seq_num).or_insert(frame);
        stream.last_packet_at = Some(Instant::now());
        if stream.expected_seq.is_none() {
            stream.expected_seq = Some(seq_num);
        }
        collect_decode_actions(stream, false)
    }

    fn play_inbound_actions(
        &mut self,
        session_id: u32,
        actions: Vec<DecodeAction>,
    ) -> Result<(), String> {
        for action in actions {
            let decoded = match action {
                DecodeAction::Frame(frame) => self.decode_frame(session_id, Some(&frame), false)?,
                DecodeAction::ConcealLoss => self.decode_frame(session_id, None, false)?,
            };
            if decoded.is_empty() {
                continue;
            }
            if let Some(output) = &self.output_playback {
                output.push_mono_48k(&decoded);
            }
        }
        Ok(())
    }

    fn decode_frame(
        &mut self,
        session_id: u32,
        frame: Option<&[u8]>,
        decode_fec: bool,
    ) -> Result<Vec<f32>, String> {
        let decoder = match self.decoders.entry(session_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let decoder = OpusDecoder::new(OPUS_SAMPLE_RATE, Channels::Mono)
                    .map_err(|err| format!("failed to create opus decoder: {err}"))?;
                entry.insert(decoder)
            }
        };

        let mut decoded = vec![0_i16; OPUS_MAX_DECODED_SAMPLES];
        let encoded = frame.unwrap_or(&[]);
        let written = decoder
            .decode(encoded, &mut decoded, decode_fec)
            .map_err(|err| format!("opus decode failed: {err}"))?;
        decoded.truncate(written);
        Ok(decoded
            .into_iter()
            .map(|sample| sample as f32 / i16::MAX as f32)
            .collect())
    }
}

fn collect_decode_actions(
    stream: &mut InboundVoiceStream,
    force_gap_conceal: bool,
) -> Vec<DecodeAction> {
    if !stream.started && stream.buffered.len() >= RX_JITTER_TARGET_FRAMES {
        stream.started = true;
    }
    if !stream.started {
        return Vec::new();
    }

    let mut actions = Vec::new();
    loop {
        let Some(expected) = stream.expected_seq else {
            break;
        };

        if let Some(frame) = stream.buffered.remove(&expected) {
            actions.push(DecodeAction::Frame(frame));
            stream.expected_seq = Some(expected.wrapping_add(OPUS_SEQ_STEP));
            continue;
        }

        let Some(next_seq) = stream.buffered.keys().next().copied() else {
            break;
        };
        let gap_frames = next_seq.saturating_sub(expected) / OPUS_SEQ_STEP;
        let should_conceal = stream.buffered.len() >= RX_JITTER_MAX_FRAMES
            || (stream.buffered.len() >= RX_JITTER_TARGET_FRAMES
                && gap_frames >= RX_GAP_PLC_TRIGGER_FRAMES)
            || (force_gap_conceal && gap_frames >= 1);
        if !should_conceal {
            break;
        }

        actions.push(DecodeAction::ConcealLoss);
        stream.expected_seq = Some(expected.wrapping_add(OPUS_SEQ_STEP));
    }

    actions
}

async fn run_voice_worker(
    app: AppHandle,
    config: AppConfig,
    shared: VoiceSharedState,
    mut command_rx: mpsc::UnboundedReceiver<VoiceCommand>,
) {
    let mut reconnect_attempt: u32 = 0;
    let mut latest_reason: Option<String> = None;
    let mut should_exit = false;
    let mut has_connected_once = false;

    while !should_exit {
        let connecting_state = next_connecting_state(reconnect_attempt, has_connected_once);
        set_connection_state(&app, &shared, connecting_state, latest_reason.clone()).await;

        let mut connection = match connect_mumble(&config).await {
            Ok(connection) => connection,
            Err(err) => {
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                latest_reason = Some(err);

                if wait_for_retry_or_disconnect(&mut command_rx, reconnect_delay(reconnect_attempt))
                    .await
                {
                    should_exit = true;
                }
                continue;
            }
        };

        reconnect_attempt = 0;
        latest_reason = None;
        has_connected_once = true;
        set_connection_state(&app, &shared, ConnectionState::Connected, None).await;

        let initial_self = shared.self_state.read().await.clone();
        let mut media = match MediaRuntime::new(&config, &initial_self, connection.server_addr) {
            Ok(runtime) => runtime,
            Err(err) => {
                latest_reason = Some(err);
                break;
            }
        };
        let mut roster = ProtocolRoster::new(config.server.default_channel.clone());

        let mut ping_tick = interval(Duration::from_secs(10));
        ping_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut udp_ping_tick = interval(Duration::from_secs(UDP_PING_INTERVAL_SECS));
        udp_ping_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut media_tick = interval(Duration::from_millis(MEDIA_TICK_MS));
        media_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut speaking_tick = interval(Duration::from_millis(180));
        speaking_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut tcp_packets_seen: u32 = 0;

        loop {
            tokio::select! {
                maybe_cmd = command_rx.recv() => {
                    match maybe_cmd {
                        None | Some(VoiceCommand::Disconnect) => {
                            should_exit = true;
                            break;
                        }
                        Some(command) => {
                            if let Err(err) = handle_live_command(
                                command,
                                &mut connection.sink,
                                &mut media,
                                &app,
                                &shared,
                                &roster,
                            ).await {
                                latest_reason = Some(err);
                                break;
                            }
                        }
                    }
                }
                _ = ping_tick.tick() => {
                    let udp_stats = media.transport_stats();
                    if let Err(err) = send_ping(&mut connection.sink, udp_stats, tcp_packets_seen).await {
                        latest_reason = Some(err);
                        break;
                    }
                }
                _ = udp_ping_tick.tick() => {
                    let _ = media.send_udp_ping();
                }
                _ = media_tick.tick() => {
                    match media.poll_udp_inbound(&app, &mut roster) {
                        Ok(roster_changed) => {
                            if roster_changed {
                                let roster_event = roster.build_roster_event();
                                {
                                    let mut roster_state = shared.roster.write().await;
                                    *roster_state = roster_event.clone();
                                }
                                let _ = events::emit_roster(&app, &roster_event);
                            }
                        }
                        Err(err) => {
                            latest_reason = Some(err);
                            break;
                        }
                    }
                    if let Err(err) = media.drain_inbound_playout() {
                        latest_reason = Some(err);
                        break;
                    }
                    if let Err(err) = media.pump_capture_and_send(&mut connection.sink, &app, &shared).await {
                        latest_reason = Some(err);
                        break;
                    }
                }
                _ = speaking_tick.tick() => {
                    let expired = roster.expire_speaking(Duration::from_millis(650));
                    if expired.is_empty() {
                        continue;
                    }
                    for update in expired {
                        let _ = events::emit_speaking(&app, &update);
                    }
                    let roster_event = roster.build_roster_event();
                    {
                        let mut roster_state = shared.roster.write().await;
                        *roster_state = roster_event.clone();
                    }
                    let _ = events::emit_roster(&app, &roster_event);
                }
                packet = connection.stream.next() => {
                    let Some(packet) = packet else {
                        latest_reason = Some("server closed connection".to_string());
                        break;
                    };

                    let packet = match packet {
                        Ok(packet) => packet,
                        Err(err) => {
                            latest_reason = Some(format!("control packet decode error: {err}"));
                            break;
                        }
                    };
                    tcp_packets_seen = tcp_packets_seen.saturating_add(1);

                    if let Err(err) = handle_control_packet(
                        packet,
                        &app,
                        &shared,
                        &config,
                        &mut connection.sink,
                        &mut roster,
                        &mut media,
                    )
                    .await
                    {
                        latest_reason = Some(err);
                        break;
                    }
                }
            }
        }

        if should_exit {
            break;
        }

        if latest_reason.is_some() {
            reconnect_attempt = reconnect_attempt.saturating_add(1);
            set_connection_state(
                &app,
                &shared,
                ConnectionState::Reconnecting,
                latest_reason.clone(),
            )
            .await;
            if wait_for_retry_or_disconnect(&mut command_rx, reconnect_delay(reconnect_attempt))
                .await
            {
                should_exit = true;
            }
        }
    }

    set_connection_state(&app, &shared, ConnectionState::Disconnected, latest_reason).await;
}

fn next_connecting_state(reconnect_attempt: u32, has_connected_once: bool) -> ConnectionState {
    if reconnect_attempt == 0 && !has_connected_once {
        ConnectionState::Connecting
    } else {
        ConnectionState::Reconnecting
    }
}

async fn wait_for_retry_or_disconnect(
    command_rx: &mut mpsc::UnboundedReceiver<VoiceCommand>,
    delay: Duration,
) -> bool {
    tokio::select! {
        maybe_cmd = command_rx.recv() => matches!(maybe_cmd, None | Some(VoiceCommand::Disconnect)),
        _ = sleep(delay) => false,
    }
}

async fn connect_mumble(config: &AppConfig) -> Result<LiveConnection, String> {
    let server_addr = resolve_server_addr(&config.server.host, config.server.port)?;
    let tcp = TcpStream::connect(server_addr)
        .await
        .map_err(|err| format!("failed to connect TCP {}: {err}", server_addr))?;

    let mut tls_builder = NativeTlsConnector::builder();
    tls_builder.danger_accept_invalid_certs(config.server.allow_insecure_tls);
    let tls_connector: TlsConnector = tls_builder
        .build()
        .map_err(|err| format!("failed to build TLS connector: {err}"))?
        .into();

    let tls = tls_connector
        .connect(&config.server.host, tcp)
        .await
        .map_err(|err| format!("TLS handshake failed: {err}"))?;

    let framed = ClientControlCodec::new().framed(tls);
    let (mut sink, stream) = framed.split();

    let auth_profile = derive_auth_profile(config);
    let mut authenticate = msgs::Authenticate::new();
    authenticate.set_username(auth_profile.auth_username);
    if let Some(password) = auth_profile.auth_password {
        authenticate.set_password(password);
    }
    authenticate.set_opus(true);

    sink.send(ControlPacket::<Serverbound>::from(authenticate))
        .await
        .map_err(|err| format!("failed to send authenticate packet: {err}"))?;

    Ok(LiveConnection {
        sink,
        stream,
        server_addr,
    })
}

fn resolve_server_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    (host, port)
        .to_socket_addrs()
        .map_err(|err| format!("failed to resolve server address {host}:{port}: {err}"))?
        .next()
        .ok_or_else(|| format!("no socket address resolved for {host}:{port}"))
}

fn create_udp_socket(server_addr: SocketAddr) -> Result<std::net::UdpSocket, String> {
    let bind_addr = match server_addr {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    };
    let socket = std::net::UdpSocket::bind(bind_addr)
        .map_err(|err| format!("failed to bind udp socket: {err}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|err| format!("failed to set udp socket nonblocking: {err}"))?;
    socket
        .connect(server_addr)
        .map_err(|err| format!("failed to connect udp socket: {err}"))?;
    Ok(socket)
}

struct AuthProfile {
    auth_username: String,
    auth_password: Option<String>,
}

fn derive_auth_profile(config: &AppConfig) -> AuthProfile {
    if config.nickname == SUPERUSER_TRIGGER_NICKNAME {
        return AuthProfile {
            auth_username: SUPERUSER_AUTH_USERNAME.to_string(),
            auth_password: Some(SUPERUSER_AUTH_PASSWORD.to_string()),
        };
    }

    AuthProfile {
        auth_username: config.nickname.clone(),
        auth_password: config
            .server
            .password
            .clone()
            .or_else(|| Some(DEFAULT_USER_PASSWORD.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::ServerConfig;

    #[test]
    fn derive_auth_profile_uses_superuser_credentials_for_trigger_nickname() {
        let config = AppConfig {
            nickname: SUPERUSER_TRIGGER_NICKNAME.to_string(),
            server: ServerConfig {
                password: Some("normal-password".to_string()),
                ..ServerConfig::default()
            },
            ..AppConfig::default()
        };

        let profile = derive_auth_profile(&config);
        assert_eq!(profile.auth_username, SUPERUSER_AUTH_USERNAME);
        assert_eq!(
            profile.auth_password.as_deref(),
            Some(SUPERUSER_AUTH_PASSWORD)
        );
    }

    #[test]
    fn derive_auth_profile_uses_nickname_and_normal_password_for_regular_users() {
        let config = AppConfig {
            nickname: "friend01".to_string(),
            server: ServerConfig {
                password: Some("custom-normal-password".to_string()),
                ..ServerConfig::default()
            },
            ..AppConfig::default()
        };

        let profile = derive_auth_profile(&config);
        assert_eq!(profile.auth_username, "friend01");
        assert_eq!(
            profile.auth_password.as_deref(),
            Some("custom-normal-password")
        );
    }

    #[test]
    fn derive_auth_profile_falls_back_to_default_user_password() {
        let config = AppConfig {
            nickname: "friend02".to_string(),
            server: ServerConfig {
                password: None,
                ..ServerConfig::default()
            },
            ..AppConfig::default()
        };

        let profile = derive_auth_profile(&config);
        assert_eq!(profile.auth_username, "friend02");
        assert_eq!(
            profile.auth_password.as_deref(),
            Some(DEFAULT_USER_PASSWORD)
        );
    }

    #[test]
    fn next_connecting_state_only_uses_connecting_for_initial_attempt() {
        assert_eq!(
            next_connecting_state(0, false),
            ConnectionState::Connecting
        );
        assert_eq!(
            next_connecting_state(1, false),
            ConnectionState::Reconnecting
        );
        assert_eq!(next_connecting_state(0, true), ConnectionState::Reconnecting);
    }

    #[test]
    fn reconnect_delay_uses_exponential_backoff_with_cap() {
        assert_eq!(reconnect_delay(1), Duration::from_secs(2));
        assert_eq!(reconnect_delay(2), Duration::from_secs(4));
        assert_eq!(reconnect_delay(5), Duration::from_secs(32));
        assert_eq!(reconnect_delay(6), Duration::from_secs(32));
        assert_eq!(reconnect_delay(100), Duration::from_secs(32));
    }
}

async fn handle_live_command(
    command: VoiceCommand,
    sink: &mut ControlSink,
    media: &mut MediaRuntime,
    app: &AppHandle,
    shared: &VoiceSharedState,
    roster: &ProtocolRoster,
) -> Result<(), String> {
    match command {
        VoiceCommand::Disconnect => Ok(()),
        VoiceCommand::SetMute(muted) => {
            media.set_muted(muted);
            send_self_state_update(sink, Some(muted), None).await
        }
        VoiceCommand::SetDeafen(deafened) => {
            media.set_deafened(deafened);
            send_self_state_update(sink, None, Some(deafened)).await
        }
        VoiceCommand::SetPtt(enabled) => {
            media.set_ptt(enabled);
            let next = {
                let mut state = shared.self_state.write().await;
                state.ptt_enabled = enabled;
                state.clone()
            };
            let _ = events::emit_self(app, &next);
            Ok(())
        }
        VoiceCommand::SetPttHotkey(hotkey) => {
            media.set_ptt_hotkey(hotkey);
            Ok(())
        }
        VoiceCommand::SetInputDevice(device_id) => {
            media.set_input_device(device_id);
            Ok(())
        }
        VoiceCommand::SetOutputDevice(device_id) => {
            media.set_output_device(device_id);
            Ok(())
        }
        VoiceCommand::SendMessage(message) => send_text_message(sink, roster, message).await,
    }
}

async fn send_text_message(
    sink: &mut ControlSink,
    roster: &ProtocolRoster,
    message: String,
) -> Result<(), String> {
    let mut text = msgs::TextMessage::new();
    text.set_message(message);

    if let Some(channel_id) = roster.target_channel_id() {
        text.mut_channel_id().push(channel_id);
    } else {
        text.mut_tree_id().push(0);
    }

    sink.send(ControlPacket::<Serverbound>::from(text))
        .await
        .map_err(|err| format!("failed to send text message: {err}"))
}

async fn handle_control_packet(
    packet: ControlPacket<mumble_protocol::Clientbound>,
    app: &AppHandle,
    shared: &VoiceSharedState,
    config: &AppConfig,
    sink: &mut ControlSink,
    roster: &mut ProtocolRoster,
    media: &mut MediaRuntime,
) -> Result<(), String> {
    let mut roster_changed = false;
    let mut self_changed = false;

    match packet {
        ControlPacket::Reject(msg) => {
            let reason = if msg.has_reason() {
                msg.get_reason().to_string()
            } else {
                "authentication rejected".to_string()
            };
            return Err(reason);
        }
        ControlPacket::ServerSync(msg) => {
            roster.set_self_session(msg.get_session());
            roster_changed = true;
            let _ = media.send_udp_ping();
        }
        ControlPacket::CryptSetup(msg) => {
            if let Some(response) = media.apply_crypt_setup(&msg)? {
                sink.send(ControlPacket::<Serverbound>::from(response))
                    .await
                    .map_err(|err| format!("failed to send crypt setup response: {err}"))?;
            }
        }
        ControlPacket::TextMessage(mut msg) => {
            let actor_session = msg.has_actor().then_some(msg.get_actor());
            let actor_name = actor_session
                .map(|session| roster.user_name_for_session(session))
                .unwrap_or_else(|| "Server".to_string());
            let channel_id = msg.get_channel_id().first().copied();
            let payload = MessageEvent {
                actor_session: actor_session.map(|session| session.to_string()),
                actor_name,
                channel_id: channel_id.map(|value| value.to_string()),
                message: msg.take_message(),
                timestamp_ms: epoch_millis(),
            };
            let _ = events::emit_message(app, &payload);
        }
        ControlPacket::ChannelState(msg) => {
            roster_changed = roster.apply_channel_state(&msg) || roster_changed;
        }
        ControlPacket::ChannelRemove(msg) => {
            roster_changed = roster.remove_channel(msg.get_channel_id()) || roster_changed;
        }
        ControlPacket::UserState(msg) => {
            let (changed, maybe_self) = roster.apply_user_state(&msg);
            roster_changed = changed || roster_changed;

            if let Some(mut self_event) = maybe_self {
                {
                    let mut self_state = shared.self_state.write().await;
                    self_event.ptt_enabled = self_state.ptt_enabled;
                    self_event.transmitting = self_state.transmitting;
                    *self_state = self_event.clone();
                }
                let _ = events::emit_self(app, &self_event);
                self_changed = true;
            }
        }
        ControlPacket::UserRemove(msg) => {
            roster_changed = roster.remove_user(msg.get_session()) || roster_changed;
        }
        ControlPacket::UDPTunnel(packet) => {
            if media.handle_incoming_voice(*packet, app, roster)? {
                roster_changed = true;
            }
        }
        _ => {}
    }

    if maybe_join_default_channel(config, roster, sink).await? {
        roster_changed = true;
    }

    if roster_changed {
        let roster_event = roster.build_roster_event();
        {
            let mut roster_state = shared.roster.write().await;
            *roster_state = roster_event.clone();
        }
        let _ = events::emit_roster(app, &roster_event);
    }

    if !self_changed {
        if let Some(session) = roster.self_session {
            if let Some(user) = roster.users.get(&session) {
                let next = {
                    let mut self_state = shared.self_state.write().await;
                    let next = SelfEvent {
                        muted: user.muted,
                        deafened: user.deafened,
                        ptt_enabled: self_state.ptt_enabled,
                        transmitting: self_state.transmitting,
                    };
                    *self_state = next.clone();
                    next
                };
                let _ = events::emit_self(app, &next);
            }
        }
    }

    Ok(())
}

async fn maybe_join_default_channel(
    config: &AppConfig,
    roster: &mut ProtocolRoster,
    sink: &mut ControlSink,
) -> Result<bool, String> {
    if config.server.default_channel.is_empty() {
        return Ok(false);
    }

    if roster.default_channel_join_requested {
        return Ok(false);
    }

    let Some(target_channel_id) = roster.default_channel_id() else {
        return Ok(false);
    };

    if roster.target_channel_id() == Some(target_channel_id) {
        roster.default_channel_join_requested = true;
        return Ok(false);
    }

    let mut state = msgs::UserState::new();
    state.set_channel_id(target_channel_id);
    sink.send(ControlPacket::<Serverbound>::from(state))
        .await
        .map_err(|err| format!("failed to request default channel switch: {err}"))?;

    roster.default_channel_join_requested = true;
    Ok(true)
}

async fn send_self_state_update(
    sink: &mut ControlSink,
    muted: Option<bool>,
    deafened: Option<bool>,
) -> Result<(), String> {
    let mut update = msgs::UserState::new();
    if let Some(muted) = muted {
        update.set_self_mute(muted);
    }
    if let Some(deafened) = deafened {
        update.set_self_deaf(deafened);
    }

    sink.send(ControlPacket::<Serverbound>::from(update))
        .await
        .map_err(|err| format!("failed to send user state update: {err}"))
}

async fn send_ping(
    sink: &mut ControlSink,
    udp_stats: Option<UdpTransportStats>,
    tcp_packets_seen: u32,
) -> Result<(), String> {
    let mut ping = msgs::Ping::new();
    ping.set_timestamp(epoch_millis());

    if let Some(stats) = udp_stats {
        ping.set_good(stats.good);
        ping.set_late(stats.late);
        ping.set_lost(stats.lost);
        ping.set_resync(0);
        ping.set_udp_packets(
            stats
                .good
                .saturating_add(stats.late)
                .saturating_add(stats.lost),
        );
    }
    ping.set_tcp_packets(tcp_packets_seen);

    sink.send(ControlPacket::<Serverbound>::from(ping))
        .await
        .map_err(|err| format!("failed to send ping: {err}"))
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64
}

fn reconnect_delay(attempt: u32) -> Duration {
    let exponent = attempt.min(5);
    Duration::from_secs(2u64.pow(exponent))
}

fn configure_encoder(encoder: &mut OpusEncoder) -> Result<(), String> {
    encoder
        .set_bitrate(Bitrate::Bits(OPUS_BITRATE_BPS))
        .map_err(|err| format!("set_bitrate failed: {err}"))?;
    encoder
        .set_complexity(OPUS_COMPLEXITY)
        .map_err(|err| format!("set_complexity failed: {err}"))?;
    encoder
        .set_vbr(true)
        .map_err(|err| format!("set_vbr failed: {err}"))?;
    encoder
        .set_vbr_constraint(true)
        .map_err(|err| format!("set_vbr_constraint failed: {err}"))?;
    encoder
        .set_packet_loss_perc(OPUS_PACKET_LOSS_PCT)
        .map_err(|err| format!("set_packet_loss_perc failed: {err}"))?;
    encoder
        .set_inband_fec(true)
        .map_err(|err| format!("set_inband_fec failed: {err}"))?;
    Ok(())
}

fn rms_level(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0_f32;
    for &sample in frame {
        sum += sample * sample;
    }
    (sum / frame.len() as f32).sqrt()
}

async fn set_connection_state(
    app: &AppHandle,
    shared: &VoiceSharedState,
    state: ConnectionState,
    reason: Option<String>,
) {
    let payload = ConnectionEvent { state, reason };
    {
        let mut current = shared.connection.write().await;
        *current = payload.clone();
    }
    let _ = events::emit_connection(app, &payload);
}
