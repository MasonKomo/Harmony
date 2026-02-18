export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'reconnecting'

export interface ServerConfig {
  host: string
  port: number
  password?: string | null
  default_channel: string
  allow_insecure_tls: boolean
}

export interface VoiceQualityConfig {
  opus_bitrate_bps: number
  packet_loss_perc: number
  jitter_target_frames: number
  jitter_max_frames: number
  inband_fec: boolean
}

export interface AppConfig {
  nickname: string
  badge_profiles: Record<string, string[]>
  remember_me: boolean
  ptt_enabled: boolean
  ptt_hotkey: string
  input_device?: string | null
  output_device?: string | null
  output_volume: number
  auto_mute_on_deafen: boolean
  voice_quality: VoiceQualityConfig
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
  badge_codes: string[]
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

export interface AudioQualityMetrics {
  connected: boolean
  input_device_name?: string | null
  input_sample_rate?: number | null
  output_device_name?: string | null
  output_sample_rate?: number | null
  tx_frames_encoded: number
  tx_packets_sent_udp: number
  tx_packets_sent_tcp: number
  tx_clip_samples: number
  tx_limiter_activations: number
  tx_bitrate_bps: number
  tx_packet_loss_percent: number
  rx_packets_received: number
  rx_frames_decoded: number
  rx_plc_frames: number
  rx_late_frames_dropped: number
  rx_gap_events: number
  rx_jitter_ms: number
  rx_jitter_target_frames: number
  rx_jitter_max_frames: number
  rx_buffered_peak_frames: number
  rx_mix_clip_samples: number
  rx_nan_samples: number
  output_underflow_events: number
  output_overflow_dropped_samples: number
  output_callback_overruns: number
  output_callback_max_duration_us: number
  output_clipped_samples: number
  output_peak_queue_samples: number
  output_queued_samples: number
  input_clipped_frames: number
  input_dropped_chunks: number
  input_delivered_chunks: number
  network_good_packets: number
  network_late_packets: number
  network_lost_packets: number
}

export interface MessageEvent {
  actor_session?: string
  actor_name: string
  channel_id?: string
  message: string
  timestamp_ms: number
}

export type SoundboardClipSource = 'default' | 'custom'

export interface SoundboardClip {
  id: string
  label: string
  source: SoundboardClipSource
  duration_ms: number
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
