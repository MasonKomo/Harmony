export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'reconnecting'

export interface ServerConfig {
  host: string
  port: number
  password?: string | null
  default_channel: string
  allow_insecure_tls: boolean
}

export interface AppConfig {
  nickname: string
  remember_me: boolean
  ptt_enabled: boolean
  ptt_hotkey: string
  input_device?: string | null
  output_device?: string | null
  output_volume: number
  auto_mute_on_deafen: boolean
  server: ServerConfig
}

export interface ConnectionEvent {
  state: ConnectionState
  reason?: string
}

export interface ChannelInfo {
  id: string
  name: string
}

export interface RosterUser {
  id: string
  name: string
  muted: boolean
  deafened: boolean
  speaking: boolean
}

export interface RosterEvent {
  channel: ChannelInfo
  users: RosterUser[]
}

export interface SpeakingEvent {
  user_id: string
  speaking: boolean
  level?: number
}

export interface DeviceInfo {
  id: string
  name: string
}

export interface DevicesEvent {
  inputs: DeviceInfo[]
  outputs: DeviceInfo[]
}

export interface SelfEvent {
  muted: boolean
  deafened: boolean
  ptt_enabled: boolean
  transmitting: boolean
}

export interface MessageEvent {
  actor_session?: string
  actor_name: string
  channel_id?: string
  message: string
  timestamp_ms: number
}

export interface UpdateInfo {
  version: string
  currentVersion: string
  notes?: string | null
  date?: string | null
}

export interface BootstrapState {
  config: AppConfig
  connection: ConnectionEvent
  roster: RosterEvent
  devices: DevicesEvent
  self_state: SelfEvent
}
