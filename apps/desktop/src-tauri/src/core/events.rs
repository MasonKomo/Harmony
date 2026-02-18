use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

pub const EVENT_CONNECTION: &str = "core/connection";
pub const EVENT_ROSTER: &str = "core/roster";
pub const EVENT_SPEAKING: &str = "core/speaking";
pub const EVENT_DEVICES: &str = "core/devices";
pub const EVENT_SELF: &str = "core/self";
pub const EVENT_MESSAGE: &str = "core/message";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionEvent {
    pub state: ConnectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Default for ConnectionEvent {
    fn default() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RosterUser {
    pub id: String,
    pub name: String,
    pub badge_codes: Vec<String>,
    pub muted: bool,
    pub deafened: bool,
    pub speaking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RosterEvent {
    pub channel: ChannelInfo,
    pub users: Vec<RosterUser>,
}

impl Default for RosterEvent {
    fn default() -> Self {
        Self {
            channel: ChannelInfo {
                id: "0".to_string(),
                name: "Game Night".to_string(),
            },
            users: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakingEvent {
    pub user_id: String,
    pub speaking: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DevicesEvent {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfEvent {
    pub muted: bool,
    pub deafened: bool,
    pub ptt_enabled: bool,
    pub transmitting: bool,
}

impl Default for SelfEvent {
    fn default() -> Self {
        Self {
            muted: false,
            deafened: false,
            ptt_enabled: false,
            transmitting: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_session: Option<String>,
    pub actor_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    pub message: String,
    pub timestamp_ms: u64,
}

fn emit<R: Runtime, T: Serialize>(
    app: &AppHandle<R>,
    event_name: &str,
    payload: &T,
) -> Result<(), String> {
    app.emit(event_name, payload).map_err(|err| err.to_string())
}

pub fn emit_connection<R: Runtime>(
    app: &AppHandle<R>,
    payload: &ConnectionEvent,
) -> Result<(), String> {
    emit(app, EVENT_CONNECTION, payload)
}

pub fn emit_roster<R: Runtime>(app: &AppHandle<R>, payload: &RosterEvent) -> Result<(), String> {
    emit(app, EVENT_ROSTER, payload)
}

pub fn emit_speaking<R: Runtime>(
    app: &AppHandle<R>,
    payload: &SpeakingEvent,
) -> Result<(), String> {
    emit(app, EVENT_SPEAKING, payload)
}

pub fn emit_devices<R: Runtime>(app: &AppHandle<R>, payload: &DevicesEvent) -> Result<(), String> {
    emit(app, EVENT_DEVICES, payload)
}

pub fn emit_self<R: Runtime>(app: &AppHandle<R>, payload: &SelfEvent) -> Result<(), String> {
    emit(app, EVENT_SELF, payload)
}

pub fn emit_message<R: Runtime>(app: &AppHandle<R>, payload: &MessageEvent) -> Result<(), String> {
    emit(app, EVENT_MESSAGE, payload)
}
