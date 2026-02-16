use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use mumble_protocol::control::{msgs, ClientControlCodec, ControlPacket};
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::Serverbound;
use native_tls::TlsConnector as NativeTlsConnector;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, sleep, MissedTickBehavior};
use tokio_native_tls::{TlsConnector, TlsStream};
use tokio_util::codec::{Decoder, Framed};

use tauri::AppHandle;

use crate::core::config::{
    AppConfig, DEFAULT_USER_PASSWORD, SUPERUSER_AUTH_PASSWORD, SUPERUSER_AUTH_USERNAME,
    SUPERUSER_TRIGGER_NICKNAME,
};
use crate::core::events::{
    self, ConnectionEvent, ConnectionState, RosterEvent, SelfEvent, SpeakingEvent,
};

type ControlFramed = Framed<TlsStream<TcpStream>, ClientControlCodec>;
type ControlSink = SplitSink<ControlFramed, ControlPacket<Serverbound>>;
type ControlStream = SplitStream<ControlFramed>;

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

    fn send_command(&self, command: VoiceCommand) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(command);
        }
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
}

struct LiveConnection {
    sink: ControlSink,
    stream: ControlStream,
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

        let next_muted = (msg.has_mute() && msg.get_mute())
            || (msg.has_self_mute() && msg.get_self_mute());
        if user.muted != next_muted {
            user.muted = next_muted;
            changed = true;
        }

        let next_deafened = (msg.has_deaf() && msg.get_deaf())
            || (msg.has_self_deaf() && msg.get_self_deaf());
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
        self.channels
            .iter()
            .find_map(|(channel_id, name)| (name == &self.default_channel_name).then_some(*channel_id))
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

    while !should_exit {
        let connecting_state = if reconnect_attempt == 0 {
            ConnectionState::Connecting
        } else {
            ConnectionState::Reconnecting
        };
        set_connection_state(&app, &shared, connecting_state, latest_reason.clone()).await;

        let mut connection = match connect_mumble(&config).await {
            Ok(connection) => connection,
            Err(err) => {
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                latest_reason = Some(err);
                let delay = reconnect_delay(reconnect_attempt);

                tokio::select! {
                    maybe_cmd = command_rx.recv() => {
                        if matches!(maybe_cmd, None | Some(VoiceCommand::Disconnect)) {
                            should_exit = true;
                        }
                    }
                    _ = sleep(delay) => {}
                }
                continue;
            }
        };

        reconnect_attempt = 0;
        latest_reason = None;
        set_connection_state(&app, &shared, ConnectionState::Connected, None).await;

        let mut roster = ProtocolRoster::new(config.server.default_channel.clone());
        let mut ping_tick = interval(Duration::from_secs(10));
        ping_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut speaking_tick = interval(Duration::from_millis(180));
        speaking_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                maybe_cmd = command_rx.recv() => {
                    match maybe_cmd {
                        None | Some(VoiceCommand::Disconnect) => {
                            should_exit = true;
                            break;
                        }
                        Some(command) => {
                            if let Err(err) = handle_live_command(command, &mut connection.sink).await {
                                latest_reason = Some(err);
                                break;
                            }
                        }
                    }
                }
                _ = ping_tick.tick() => {
                    if let Err(err) = send_ping(&mut connection.sink).await {
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

                    if let Err(err) = handle_control_packet(
                        packet,
                        &app,
                        &shared,
                        &config,
                        &mut connection.sink,
                        &mut roster,
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
    }

    set_connection_state(&app, &shared, ConnectionState::Disconnected, latest_reason).await;
}

async fn connect_mumble(config: &AppConfig) -> Result<LiveConnection, String> {
    let address = format!("{}:{}", config.server.host, config.server.port);
    let tcp = TcpStream::connect(&address)
        .await
        .map_err(|err| format!("failed to connect TCP {}: {err}", address))?;

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

    Ok(LiveConnection { sink, stream })
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
        assert_eq!(profile.auth_password.as_deref(), Some(DEFAULT_USER_PASSWORD));
    }
}

async fn handle_live_command(command: VoiceCommand, sink: &mut ControlSink) -> Result<(), String> {
    match command {
        VoiceCommand::Disconnect => Ok(()),
        VoiceCommand::SetMute(muted) => send_self_state_update(sink, Some(muted), None).await,
        VoiceCommand::SetDeafen(deafened) => send_self_state_update(sink, None, Some(deafened)).await,
        VoiceCommand::SetPtt(_enabled) => Ok(()),
        VoiceCommand::SetPttHotkey(_hotkey) => Ok(()),
        VoiceCommand::SetInputDevice(_device_id) => Ok(()),
        VoiceCommand::SetOutputDevice(_device_id) => Ok(()),
    }
}

async fn handle_control_packet(
    packet: ControlPacket<mumble_protocol::Clientbound>,
    app: &AppHandle,
    shared: &VoiceSharedState,
    config: &AppConfig,
    sink: &mut ControlSink,
    roster: &mut ProtocolRoster,
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
                let current = shared.self_state.read().await;
                self_event.ptt_enabled = current.ptt_enabled;
                self_event.transmitting = current.transmitting;
                drop(current);
                {
                    let mut self_state = shared.self_state.write().await;
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
            if let VoicePacket::Audio { session_id, .. } = *packet {
                if let Some(update) = roster.maybe_mark_speaking(session_id) {
                    let _ = events::emit_speaking(app, &update);
                    roster_changed = true;
                }
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
                let current = shared.self_state.read().await;
                let next = SelfEvent {
                    muted: user.muted,
                    deafened: user.deafened,
                    ptt_enabled: current.ptt_enabled,
                    transmitting: current.transmitting,
                };
                drop(current);
                {
                    let mut self_state = shared.self_state.write().await;
                    *self_state = next.clone();
                }
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

async fn send_ping(sink: &mut ControlSink) -> Result<(), String> {
    let mut ping = msgs::Ping::new();
    ping.set_timestamp(epoch_millis());
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
