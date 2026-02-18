pub mod config;
pub mod events;
pub mod soundboard;
pub mod voice;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tokio::sync::{Mutex, RwLock};

use config::{load_config, save_config_to_path, AppConfig};
use events::{
    emit_connection, emit_devices, emit_roster, emit_self, ConnectionEvent, ConnectionState,
    DevicesEvent, SelfEvent,
};
use soundboard::{SoundboardClip, SoundboardStore};
use voice::hotkeys::Hotkey;
use voice::{list_input_devices, list_output_devices, AudioQualityMetrics, VoiceService, VoiceSharedState};

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapState {
    pub config: AppConfig,
    pub connection: ConnectionEvent,
    pub roster: events::RosterEvent,
    pub devices: DevicesEvent,
    pub self_state: SelfEvent,
}

pub struct AppCore {
    config_path: PathBuf,
    config_is_dev_override: bool,
    pub config: Arc<RwLock<AppConfig>>,
    pub connection: Arc<RwLock<ConnectionEvent>>,
    pub roster: Arc<RwLock<events::RosterEvent>>,
    pub devices: Arc<RwLock<DevicesEvent>>,
    pub self_state: Arc<RwLock<SelfEvent>>,
    pub voice: Mutex<VoiceService>,
    pub soundboard: Mutex<SoundboardStore>,
}

impl AppCore {
    pub fn new() -> Result<Self, String> {
        let loaded = load_config().map_err(|err| err.to_string())?;
        let devices = read_devices_event();
        let roster = events::RosterEvent {
            channel: events::ChannelInfo {
                id: "0".to_string(),
                name: loaded.config.server.default_channel.clone(),
            },
            users: Vec::new(),
        };

        let self_state = SelfEvent {
            muted: false,
            deafened: false,
            ptt_enabled: loaded.config.ptt_enabled,
            transmitting: false,
        };

        Ok(Self {
            config_path: loaded.path,
            config_is_dev_override: loaded.is_dev_override,
            config: Arc::new(RwLock::new(loaded.config)),
            connection: Arc::new(RwLock::new(ConnectionEvent::default())),
            roster: Arc::new(RwLock::new(roster)),
            devices: Arc::new(RwLock::new(devices)),
            self_state: Arc::new(RwLock::new(self_state)),
            voice: Mutex::new(VoiceService::new()),
            soundboard: Mutex::new(SoundboardStore::load()?),
        })
    }

    pub async fn bootstrap(&self) -> BootstrapState {
        BootstrapState {
            config: self.config.read().await.clone(),
            connection: self.connection.read().await.clone(),
            roster: self.roster.read().await.clone(),
            devices: self.devices.read().await.clone(),
            self_state: self.self_state.read().await.clone(),
        }
    }

    pub async fn emit_initial_events(&self, app: &AppHandle) -> Result<(), String> {
        let connection = self.connection.read().await.clone();
        let roster = self.roster.read().await.clone();
        let devices = self.devices.read().await.clone();
        let self_state = self.self_state.read().await.clone();

        emit_connection(app, &connection)?;
        emit_roster(app, &roster)?;
        emit_devices(app, &devices)?;
        emit_self(app, &self_state)?;
        Ok(())
    }

    async fn persist_config(&self) -> Result<(), String> {
        if self.config_is_dev_override {
            return Ok(());
        }
        let snapshot = self.config.read().await.clone();
        save_config_to_path(&self.config_path, &snapshot).map_err(|err| err.to_string())
    }

    fn voice_shared_state(&self) -> VoiceSharedState {
        VoiceSharedState {
            connection: Arc::clone(&self.connection),
            roster: Arc::clone(&self.roster),
            self_state: Arc::clone(&self.self_state),
        }
    }

    async fn refresh_devices(&self, app: &AppHandle) -> Result<DevicesEvent, String> {
        let refreshed = read_devices_event();
        {
            let mut devices = self.devices.write().await;
            *devices = refreshed.clone();
        }
        emit_devices(app, &refreshed)?;
        Ok(refreshed)
    }
}

fn read_devices_event() -> DevicesEvent {
    DevicesEvent {
        inputs: list_input_devices()
            .into_iter()
            .map(|device| events::DeviceInfo {
                id: device.id,
                name: device.name,
            })
            .collect(),
        outputs: list_output_devices()
            .into_iter()
            .map(|device| events::DeviceInfo {
                id: device.id,
                name: device.name,
            })
            .collect(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ConnectArgs {
    nickname: String,
    #[serde(default)]
    badge_codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetMuteArgs {
    muted: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetDeafenArgs {
    deafened: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetPttArgs {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetPttHotkeyArgs {
    hotkey: String,
}

#[derive(Debug, Deserialize)]
pub struct SetInputDeviceArgs {
    device_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SetOutputDeviceArgs {
    device_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SetServerEndpointArgs {
    host: String,
    port: u16,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageArgs {
    message: String,
}

#[derive(Debug, Deserialize)]
pub struct ImportSoundboardClipArgs {
    label: String,
    file_name: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteSoundboardClipArgs {
    clip_id: String,
}

#[derive(Debug, Deserialize)]
pub struct PlaySoundboardClipArgs {
    clip_id: String,
}

#[tauri::command]
pub async fn bootstrap(state: State<'_, AppCore>) -> Result<BootstrapState, String> {
    Ok(state.bootstrap().await)
}

#[tauri::command]
pub async fn connect(
    app: AppHandle,
    state: State<'_, AppCore>,
    args: ConnectArgs,
) -> Result<(), String> {
    let nickname = args.nickname.trim().to_string();
    if nickname.is_empty() {
        return Err("nickname is required".to_string());
    }
    let badge_codes = normalize_badge_codes(args.badge_codes);

    {
        let mut config = state.config.write().await;
        config.nickname = nickname.clone();
        config.badge_profiles.insert(nickname, badge_codes);
    }
    state.persist_config().await?;

    let config_snapshot = state.config.read().await.clone();
    let shared = state.voice_shared_state();
    {
        let mut voice = state.voice.lock().await;
        voice.connect(app.clone(), config_snapshot, shared).await?;
    }
    state.emit_initial_events(&app).await?;
    Ok(())
}

const MAX_BADGE_CODES_PER_USER: usize = 5;
const MAX_BADGE_CODE_LEN: usize = 32;

fn normalize_badge_codes(raw_codes: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();

    for raw in raw_codes {
        let code = raw.trim().to_ascii_lowercase();
        if code.is_empty() || code.len() > MAX_BADGE_CODE_LEN {
            continue;
        }
        if !code.bytes().all(|value| {
            value.is_ascii_lowercase() || value.is_ascii_digit() || value == b'-' || value == b'_'
        }) {
            continue;
        }
        if !seen.insert(code.clone()) {
            continue;
        }
        normalized.push(code);
        if normalized.len() >= MAX_BADGE_CODES_PER_USER {
            break;
        }
    }

    normalized
}

#[tauri::command]
pub async fn disconnect(app: AppHandle, state: State<'_, AppCore>) -> Result<(), String> {
    {
        let mut voice = state.voice.lock().await;
        voice.disconnect().await;
    }

    let disconnected = ConnectionEvent {
        state: ConnectionState::Disconnected,
        reason: None,
    };
    {
        let mut connection = state.connection.write().await;
        *connection = disconnected.clone();
    }
    emit_connection(&app, &disconnected)?;
    Ok(())
}

#[tauri::command]
pub async fn set_mute(
    app: AppHandle,
    state: State<'_, AppCore>,
    args: SetMuteArgs,
) -> Result<(), String> {
    let next = {
        let mut self_state = state.self_state.write().await;
        self_state.muted = args.muted;
        self_state.clone()
    };
    emit_self(&app, &next)?;

    let voice = state.voice.lock().await;
    voice.set_mute(args.muted);
    Ok(())
}

#[tauri::command]
pub async fn set_deafen(
    app: AppHandle,
    state: State<'_, AppCore>,
    args: SetDeafenArgs,
) -> Result<(), String> {
    let auto_mute = state.config.read().await.auto_mute_on_deafen;

    let next = {
        let mut self_state = state.self_state.write().await;
        self_state.deafened = args.deafened;
        if auto_mute && args.deafened {
            self_state.muted = true;
        }
        self_state.clone()
    };
    emit_self(&app, &next)?;

    let voice = state.voice.lock().await;
    voice.set_deafen(args.deafened);
    if auto_mute && args.deafened {
        voice.set_mute(true);
    }
    Ok(())
}

#[tauri::command]
pub async fn set_ptt(
    app: AppHandle,
    state: State<'_, AppCore>,
    args: SetPttArgs,
) -> Result<(), String> {
    {
        let mut config = state.config.write().await;
        config.ptt_enabled = args.enabled;
    }
    state.persist_config().await?;

    let next = {
        let mut self_state = state.self_state.write().await;
        self_state.ptt_enabled = args.enabled;
        self_state.clone()
    };
    emit_self(&app, &next)?;

    let voice = state.voice.lock().await;
    voice.set_ptt(args.enabled);
    Ok(())
}

#[tauri::command]
pub async fn set_ptt_hotkey(
    _app: AppHandle,
    state: State<'_, AppCore>,
    args: SetPttHotkeyArgs,
) -> Result<(), String> {
    let Some(parsed_hotkey) = Hotkey::parse(&args.hotkey) else {
        return Err("hotkey cannot be empty".to_string());
    };

    {
        let mut config = state.config.write().await;
        config.ptt_hotkey = parsed_hotkey.0.clone();
    }
    state.persist_config().await?;

    let voice = state.voice.lock().await;
    voice.set_ptt_hotkey(parsed_hotkey.0);
    Ok(())
}

#[tauri::command]
pub async fn set_input_device(
    _app: AppHandle,
    state: State<'_, AppCore>,
    args: SetInputDeviceArgs,
) -> Result<(), String> {
    {
        let mut config = state.config.write().await;
        config.input_device = Some(args.device_id.clone());
    }
    state.persist_config().await?;

    let voice = state.voice.lock().await;
    voice.set_input_device(args.device_id);
    Ok(())
}

#[tauri::command]
pub async fn set_output_device(
    _app: AppHandle,
    state: State<'_, AppCore>,
    args: SetOutputDeviceArgs,
) -> Result<(), String> {
    {
        let mut config = state.config.write().await;
        config.output_device = Some(args.device_id.clone());
    }
    state.persist_config().await?;

    let voice = state.voice.lock().await;
    voice.set_output_device(args.device_id);
    Ok(())
}

#[tauri::command]
pub async fn set_server_endpoint(
    _app: AppHandle,
    state: State<'_, AppCore>,
    args: SetServerEndpointArgs,
) -> Result<(), String> {
    let host = args.host.trim().to_string();
    if host.is_empty() {
        return Err("server host cannot be empty".to_string());
    }
    if args.port == 0 {
        return Err("server port must be greater than 0".to_string());
    }

    {
        let mut config = state.config.write().await;
        config.server.host = host;
        config.server.port = args.port;
    }
    state.persist_config().await?;
    Ok(())
}

#[tauri::command]
pub async fn refresh_devices(
    app: AppHandle,
    state: State<'_, AppCore>,
) -> Result<DevicesEvent, String> {
    state.refresh_devices(&app).await
}

#[tauri::command]
pub async fn get_audio_quality_metrics(state: State<'_, AppCore>) -> Result<AudioQualityMetrics, String> {
    let voice = state.voice.lock().await;
    Ok(voice.audio_quality_metrics())
}

#[tauri::command]
pub async fn send_message(
    _app: AppHandle,
    state: State<'_, AppCore>,
    args: SendMessageArgs,
) -> Result<(), String> {
    let message = args.message.trim().to_string();
    if message.is_empty() {
        return Err("message cannot be empty".to_string());
    }

    let voice = state.voice.lock().await;
    voice.send_message(message)
}

#[tauri::command]
pub async fn list_soundboard_clips(state: State<'_, AppCore>) -> Result<Vec<SoundboardClip>, String> {
    let soundboard = state.soundboard.lock().await;
    Ok(soundboard.list_clips())
}

#[tauri::command]
pub async fn import_soundboard_clip(
    state: State<'_, AppCore>,
    args: ImportSoundboardClipArgs,
) -> Result<SoundboardClip, String> {
    let mut soundboard = state.soundboard.lock().await;
    soundboard.import_custom_clip(&args.label, &args.file_name, &args.bytes)
}

#[tauri::command]
pub async fn delete_soundboard_clip(
    state: State<'_, AppCore>,
    args: DeleteSoundboardClipArgs,
) -> Result<(), String> {
    let mut soundboard = state.soundboard.lock().await;
    soundboard.delete_custom_clip(&args.clip_id)
}

#[tauri::command]
pub async fn play_soundboard_clip(
    state: State<'_, AppCore>,
    args: PlaySoundboardClipArgs,
) -> Result<(), String> {
    let samples_48k = {
        let soundboard = state.soundboard.lock().await;
        soundboard
            .samples_for_clip(&args.clip_id)
            .ok_or_else(|| "clip not found".to_string())?
    };
    let voice = state.voice.lock().await;
    voice.queue_soundboard_samples(samples_48k)
}
