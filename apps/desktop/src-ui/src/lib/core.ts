import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

import type {
  BootstrapState,
  ConnectionEvent,
  DevicesEvent,
  RosterEvent,
  SelfEvent,
  SpeakingEvent,
} from '@/lib/types'

type EventHandlers = {
  connection?: (payload: ConnectionEvent) => void
  roster?: (payload: RosterEvent) => void
  speaking?: (payload: SpeakingEvent) => void
  devices?: (payload: DevicesEvent) => void
  self?: (payload: SelfEvent) => void
}

export async function bootstrap(): Promise<BootstrapState> {
  return invoke<BootstrapState>('bootstrap')
}

export async function connect(nickname: string): Promise<void> {
  return invoke<void>('connect', { args: { nickname } })
}

export async function disconnect(): Promise<void> {
  return invoke<void>('disconnect')
}

export async function setMute(muted: boolean): Promise<void> {
  return invoke<void>('set_mute', { args: { muted } })
}

export async function setDeafen(deafened: boolean): Promise<void> {
  return invoke<void>('set_deafen', { args: { deafened } })
}

export async function setPtt(enabled: boolean): Promise<void> {
  return invoke<void>('set_ptt', { args: { enabled } })
}

export async function setPttHotkey(hotkey: string): Promise<void> {
  return invoke<void>('set_ptt_hotkey', { args: { hotkey } })
}

export async function setInputDevice(deviceId: string): Promise<void> {
  return invoke<void>('set_input_device', { args: { device_id: deviceId } })
}

export async function setOutputDevice(deviceId: string): Promise<void> {
  return invoke<void>('set_output_device', { args: { device_id: deviceId } })
}

export async function refreshDevices(): Promise<DevicesEvent> {
  return invoke<DevicesEvent>('refresh_devices')
}

export async function subscribeCoreEvents(handlers: EventHandlers): Promise<() => void> {
  const unlisten = await Promise.all([
    listen<ConnectionEvent>('core/connection', (event) => handlers.connection?.(event.payload)),
    listen<RosterEvent>('core/roster', (event) => handlers.roster?.(event.payload)),
    listen<SpeakingEvent>('core/speaking', (event) => handlers.speaking?.(event.payload)),
    listen<DevicesEvent>('core/devices', (event) => handlers.devices?.(event.payload)),
    listen<SelfEvent>('core/self', (event) => handlers.self?.(event.payload)),
  ])

  return () => {
    for (const unregister of unlisten) {
      unregister()
    }
  }
}
