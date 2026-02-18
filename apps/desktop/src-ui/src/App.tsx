import { type ChangeEvent, type FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Check,
  Circle,
  Hash,
  LoaderCircle,
  LogOut,
  Mic,
  MicOff,
  Plus,
  RefreshCw,
  Send,
  Settings2,
  Trash2,
  Volume2,
  VolumeOff,
  Waves,
} from 'lucide-react'

import {
  bootstrap,
  checkForUpdate,
  connect,
  disconnect,
  deleteSoundboardClip,
  importSoundboardClip,
  installCachedUpdate,
  listSoundboardClips,
  playSoundboardClip,
  refreshDevices,
  sendMessage,
  setDeafen,
  setInputDevice,
  setMute,
  setOutputDevice,
  setPtt,
  setPttHotkey,
  setServerEndpoint,
  subscribeCoreEvents,
} from '@/lib/core'
import { getVersion } from '@tauri-apps/api/app'
import type {
  AppConfig,
  ConnectionEvent,
  ConnectionState,
  DevicesEvent,
  MessageEvent,
  RosterEvent,
  SelfEvent,
  SoundboardClip,
  UpdateInfo,
} from '@/lib/types'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Badge, type BadgeProps } from '@/components/ui/badge'
import { BadgeIcons } from '@/components/badge-icons'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from '@/components/ui/sheet'
import { Slider } from '@/components/ui/slider'
import { Switch } from '@/components/ui/switch'
import {
  BADGE_MANIFEST,
  MAX_BADGES_PER_USER,
  normalizeBadgeCodes,
  resolveBadgeByCode,
} from '@/lib/badges'
import { cn } from '@/lib/utils'

const INITIAL_CONNECTION: ConnectionEvent = { state: 'disconnected' }
const SUPERUSER_TRIGGER_NICKNAME = 'spaceKomo'
const INITIAL_ROSTER: RosterEvent = {
  channel: { id: '0', name: 'Game Night' },
  users: [],
}
const INITIAL_DEVICES: DevicesEvent = {
  inputs: [],
  outputs: [],
}
const INITIAL_SELF_STATE: SelfEvent = {
  muted: false,
  deafened: false,
  ptt_enabled: false,
  transmitting: false,
}

type MicMeterStatus = 'idle' | 'active' | 'denied' | 'unavailable'

const METER_BAR_COUNT = 20
const METER_FFT_SIZE = 512
const METER_GAIN = 3.8
const METER_SMOOTHING = 0.24
const CHAT_HISTORY_LIMIT = 300
const SOUNDBOARD_UPLOAD_ACCEPT = '.wav,.mp3,.ogg,audio/wav,audio/mpeg,audio/ogg'
const HARMONY_DEFAULT_SERVER_ENDPOINT = {
  host: 'ec2-3-133-108-176.us-east-2.compute.amazonaws.com',
  port: 64738,
}
const RAILWAY_SERVER_PROFILE = {
  name: 'Harmony',
  host: 'shinkansen.proxy.rlwy.net',
  port: 34004,
}

type ChatDeliveryState = 'pending' | 'confirmed'
type ServerPreset = 'harmony-default' | 'railway' | 'custom'

type ChatMessage = MessageEvent & {
  local_id: string
  is_local_echo: boolean
  delivery_state: ChatDeliveryState
  delivery_error?: string | null
}

interface MessageGroup {
  actorName: string
  actorSession?: string
  actorBadgeCodes: string[]
  messages: ChatMessage[]
  firstTimestamp: number
}

const MESSAGE_GROUP_WINDOW_MS = 5 * 60 * 1000 // 5 minutes

const normalizeChatText = (value: string) => value.replace(/\s+/g, ' ').trim()
const normalizeActorName = (value: string) => value.trim().toLowerCase()
const normalizeChannelId = (channelId?: string) => channelId ?? ''
const normalizeOutgoingChannelId = (channelId: string) => (channelId === '0' ? undefined : channelId)
const normalizeServerHost = (host: string) => host.trim().toLowerCase()
const formatClipDuration = (durationMs: number) => `${(durationMs / 1000).toFixed(durationMs >= 10_000 ? 0 : 1)}s`

const trimMessageHistory = (messages: ChatMessage[]) =>
  messages.length > CHAT_HISTORY_LIMIT
    ? messages.slice(messages.length - CHAT_HISTORY_LIMIT)
    : messages

const createServerChatMessage = (payload: MessageEvent): ChatMessage => ({
  ...payload,
  local_id: `srv-${payload.timestamp_ms}-${Math.random().toString(36).slice(2, 9)}`,
  is_local_echo: false,
  delivery_state: 'confirmed',
  delivery_error: null,
})

const reconcileIncomingMessage = (
  previousMessages: ChatMessage[],
  payload: MessageEvent
): ChatMessage[] => {
  const normalizedIncomingMessage = normalizeChatText(payload.message)
  const normalizedIncomingChannel = normalizeChannelId(payload.channel_id)
  const normalizedIncomingActor = normalizeActorName(payload.actor_name)
  let fallbackIndex = -1

  for (let index = 0; index < previousMessages.length; index += 1) {
    const candidate = previousMessages[index]
    if (!candidate.is_local_echo || candidate.delivery_state !== 'pending') {
      continue
    }
    if (normalizeChatText(candidate.message) !== normalizedIncomingMessage) {
      continue
    }
    const channelsMatch =
      normalizedIncomingChannel.length === 0 ||
      normalizeChannelId(candidate.channel_id) === normalizedIncomingChannel
    if (!channelsMatch) {
      continue
    }
    if (normalizeActorName(candidate.actor_name) === normalizedIncomingActor) {
      fallbackIndex = index
      break
    }
    if (fallbackIndex === -1) {
      fallbackIndex = index
    }
  }

  if (fallbackIndex === -1) {
    return trimMessageHistory([...previousMessages, createServerChatMessage(payload)])
  }

  const nextMessages = [...previousMessages]
  nextMessages[fallbackIndex] = {
    ...nextMessages[fallbackIndex],
    actor_session: payload.actor_session ?? nextMessages[fallbackIndex].actor_session,
    actor_name: payload.actor_name,
    channel_id: payload.channel_id ?? nextMessages[fallbackIndex].channel_id,
    message: payload.message,
    timestamp_ms: payload.timestamp_ms,
    delivery_state: 'confirmed',
    delivery_error: null,
  }

  return trimMessageHistory(nextMessages)
}

function App() {
  const [config, setConfig] = useState<AppConfig | null>(null)
  const [connection, setConnection] = useState<ConnectionEvent>(INITIAL_CONNECTION)
  const [roster, setRoster] = useState<RosterEvent>(INITIAL_ROSTER)
  const [devices, setDevices] = useState<DevicesEvent>(INITIAL_DEVICES)
  const [selfState, setSelfState] = useState<SelfEvent>(INITIAL_SELF_STATE)
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [soundboardClips, setSoundboardClips] = useState<SoundboardClip[]>([])
  const [soundboardOpen, setSoundboardOpen] = useState(false)
  const [soundboardBusy, setSoundboardBusy] = useState(false)
  const [soundboardPlayingId, setSoundboardPlayingId] = useState<string | null>(null)
  const [chatInput, setChatInput] = useState('')
  const [micLevel, setMicLevel] = useState(0)
  const [micMeterStatus, setMicMeterStatus] = useState<MicMeterStatus>('idle')
  const [nicknameInput, setNicknameInput] = useState('')
  const [selectedBadgeCodes, setSelectedBadgeCodes] = useState<string[]>([])
  const [hotkeyInput, setHotkeyInput] = useState('AltLeft')
  const [outputVolume, setOutputVolume] = useState([80])
  const [loading, setLoading] = useState(true)
  const [actionBusy, setActionBusy] = useState(false)
  const [chatBusy, setChatBusy] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null)
  const [updateBusy, setUpdateBusy] = useState(false)
  const [updateNotice, setUpdateNotice] = useState<string | null>(null)
  const [serverPresetBusy, setServerPresetBusy] = useState(false)
  const [appVersion, setAppVersion] = useState<string | null>(null)
  const mountedRef = useRef(false)
  const chatBottomRef = useRef<HTMLDivElement | null>(null)
  const chatInputRef = useRef<HTMLInputElement | null>(null)
  const audioContextRef = useRef<AudioContext | null>(null)
  const analyserRef = useRef<AnalyserNode | null>(null)
  const sourceNodeRef = useRef<MediaStreamAudioSourceNode | null>(null)
  const mediaStreamRef = useRef<MediaStream | null>(null)
  const meterAnimationRef = useRef<number | null>(null)
  const localMessageCounterRef = useRef(0)
  const loadedBadgeProfileRef = useRef<string | null>(null)
  const soundboardFileInputRef = useRef<HTMLInputElement | null>(null)

  const stopMicMeter = useCallback(() => {
    if (meterAnimationRef.current !== null) {
      cancelAnimationFrame(meterAnimationRef.current)
      meterAnimationRef.current = null
    }

    if (sourceNodeRef.current) {
      sourceNodeRef.current.disconnect()
      sourceNodeRef.current = null
    }
    analyserRef.current = null

    if (mediaStreamRef.current) {
      for (const track of mediaStreamRef.current.getTracks()) {
        track.stop()
      }
      mediaStreamRef.current = null
    }

    const audioContext = audioContextRef.current
    audioContextRef.current = null
    if (audioContext && audioContext.state !== 'closed') {
      void audioContext.close().catch(() => undefined)
    }
  }, [])

  const runUpdateCheck = useCallback(async (showUpToDateNotice: boolean) => {
    setUpdateBusy(true)
    if (showUpToDateNotice) {
      setUpdateNotice('Checking for updates...')
    }
    try {
      const available = await checkForUpdate()
      if (!available) {
        setUpdateInfo(null)
        if (showUpToDateNotice) {
          setUpdateNotice('You are on the latest version.')
        }
        return
      }
      setUpdateInfo(available)
      setUpdateNotice(`Update ${available.version} is available (current ${available.currentVersion}).`)
    } catch (error) {
      setUpdateNotice(`Update check failed: ${String(error)}`)
    } finally {
      setUpdateBusy(false)
    }
  }, [])

  const handleInstallUpdate = useCallback(async () => {
    if (!updateInfo) {
      return
    }
    setUpdateBusy(true)
    setUpdateNotice(`Installing update ${updateInfo.version}...`)
    try {
      await installCachedUpdate()
    } catch (error) {
      setUpdateNotice(`Update install failed: ${String(error)}`)
    } finally {
      setUpdateBusy(false)
    }
  }, [updateInfo])

  const refreshSoundboard = useCallback(async () => {
    const clips = await listSoundboardClips()
    setSoundboardClips(clips)
  }, [])

  useEffect(() => {
    let stopListeners: (() => void) | null = null
    let mounted = true
    let teardownRequested = false

    const setup = async () => {
      try {
        const snapshot = await bootstrap()
        if (!mounted) {
          return
        }

        setConfig(snapshot.config)
        setConnection(snapshot.connection)
        setRoster(snapshot.roster)
        setDevices(snapshot.devices)
        setSelfState(snapshot.self_state)
        setNicknameInput(snapshot.config.nickname)
        setSelectedBadgeCodes(
          normalizeBadgeCodes(snapshot.config.badge_profiles[snapshot.config.nickname] ?? [])
        )
        loadedBadgeProfileRef.current = snapshot.config.nickname
        setHotkeyInput(snapshot.config.ptt_hotkey)
        setOutputVolume([snapshot.config.output_volume])
        try {
          const clips = await listSoundboardClips()
          if (mounted) {
            setSoundboardClips(clips)
          }
        } catch (error) {
          if (mounted) {
            setErrorMessage(String(error))
          }
        }
      } catch (error) {
        if (mounted) {
          setErrorMessage(String(error))
        }
      } finally {
        if (mounted) {
          setLoading(false)
        }
      }

      const registeredListeners = await subscribeCoreEvents({
        connection: (payload) => {
          if (!mounted) {
            return
          }
          setConnection(payload)
        },
        roster: (payload) => {
          if (!mounted) {
            return
          }
          setRoster(payload)
        },
        devices: (payload) => {
          if (!mounted) {
            return
          }
          setDevices(payload)
        },
        self: (payload) => {
          if (!mounted) {
            return
          }
          setSelfState(payload)
        },
        speaking: (payload) => {
          if (!mounted) {
            return
          }
          setRoster((prev) => ({
            ...prev,
            users: prev.users.map((user) =>
              user.id === payload.user_id ? { ...user, speaking: payload.speaking } : user
            ),
          }))
        },
        message: (payload) => {
          if (!mounted) {
            return
          }
          setMessages((prev) => reconcileIncomingMessage(prev, payload))
        },
      })

      if (teardownRequested || !mounted) {
        registeredListeners()
        return
      }

      stopListeners = registeredListeners
      void runUpdateCheck(false)
    }

    void setup()

    return () => {
      mounted = false
      teardownRequested = true
      stopListeners?.()
    }
  }, [runUpdateCheck])

  useEffect(() => {
    let active = true
    void getVersion()
      .then((version) => {
        if (active) {
          setAppVersion(version)
        }
      })
      .catch(() => {
        if (active) {
          setAppVersion(null)
        }
      })
    return () => {
      active = false
    }
  }, [])

  useEffect(() => {
    if (!config) {
      return
    }
    const profileKey = nicknameInput.trim()
    if (loadedBadgeProfileRef.current === profileKey) {
      return
    }
    loadedBadgeProfileRef.current = profileKey
    if (!profileKey) {
      setSelectedBadgeCodes([])
      return
    }
    setSelectedBadgeCodes(normalizeBadgeCodes(config.badge_profiles[profileKey] ?? []))
  }, [config, nicknameInput])

  const connectionLabel = useMemo(() => {
    const labels: Record<ConnectionState, string> = {
      connected: 'Connected',
      connecting: 'Connecting',
      disconnected: 'Disconnected',
      reconnecting: 'Reconnecting',
    }
    return labels[connection.state]
  }, [connection.state])

  const connectionBadgeVariant: BadgeProps['variant'] = useMemo(() => {
    switch (connection.state) {
      case 'connected':
        return 'success'
      case 'connecting':
      case 'reconnecting':
        return 'warning'
      default:
        return 'secondary'
    }
  }, [connection.state])

  const hasNickname = nicknameInput.trim().length > 0
  const isSuperuserRoute = nicknameInput.trim() === SUPERUSER_TRIGGER_NICKNAME
  const isConnectingLike =
    connection.state === 'connecting' || connection.state === 'reconnecting'
  const canJoin = hasNickname && !isConnectingLike && !actionBusy
  const joinedUserCount = roster.users.length
  const showConnectedLayout =
    connection.state === 'connected' || connection.state === 'reconnecting'
  const selectedBadgeSet = useMemo(() => new Set(selectedBadgeCodes), [selectedBadgeCodes])
  const rosterBadgesById = useMemo(
    () => new Map(roster.users.map((user) => [user.id, normalizeBadgeCodes(user.badge_codes)])),
    [roster.users]
  )
  const visibleMessages = useMemo(
    () => messages.filter((message) => !message.channel_id || message.channel_id === roster.channel.id),
    [messages, roster.channel.id]
  )
  const defaultSoundboardClips = useMemo(
    () => soundboardClips.filter((clip) => clip.source === 'default'),
    [soundboardClips]
  )
  const customSoundboardClips = useMemo(
    () => soundboardClips.filter((clip) => clip.source === 'custom'),
    [soundboardClips]
  )
  const canUseSoundboard = showConnectedLayout && connection.state !== 'disconnected'

  const groupedMessages = useMemo(() => {
    const groups: MessageGroup[] = []
    for (const msg of visibleMessages) {
      const lastGroup = groups[groups.length - 1]
      const sameUser = lastGroup?.actorSession === msg.actor_session && lastGroup?.actorName === msg.actor_name
      const lastMessage = lastGroup?.messages[lastGroup.messages.length - 1]
      const withinWindow = lastMessage && msg.timestamp_ms - lastMessage.timestamp_ms < MESSAGE_GROUP_WINDOW_MS

      if (sameUser && withinWindow) {
        lastGroup.messages.push(msg)
      } else {
        groups.push({
          actorName: msg.actor_name,
          actorSession: msg.actor_session,
          actorBadgeCodes: msg.actor_session
            ? normalizeBadgeCodes(rosterBadgesById.get(msg.actor_session) ?? [])
            : [],
          messages: [msg],
          firstTimestamp: msg.timestamp_ms,
        })
      }
    }
    return groups
  }, [rosterBadgesById, visibleMessages])

  const messageTimeFormatter = useMemo(
    () =>
      new Intl.DateTimeFormat(undefined, {
        hour: '2-digit',
        minute: '2-digit',
      }),
    []
  )
  const renderedMicLevel = selfState.muted ? 0 : micLevel
  const meterFallbackLabel =
    micMeterStatus === 'denied'
      ? 'Mic permission denied for level meter.'
      : 'Mic access required for level meter.'
  const selectedServerPreset = useMemo<ServerPreset>(() => {
    if (!config) {
      return 'custom'
    }

    const host = normalizeServerHost(config.server.host)
    if (host === normalizeServerHost(RAILWAY_SERVER_PROFILE.host) && config.server.port === RAILWAY_SERVER_PROFILE.port) {
      return 'railway'
    }
    if (
      host === normalizeServerHost(HARMONY_DEFAULT_SERVER_ENDPOINT.host) &&
      config.server.port === HARMONY_DEFAULT_SERVER_ENDPOINT.port
    ) {
      return 'harmony-default'
    }

    return 'custom'
  }, [config])
  const meterBars = useMemo(
    () =>
      Array.from({ length: METER_BAR_COUNT }, (_, index) => {
        const ratio = index / (METER_BAR_COUNT - 1)
        const envelope = Math.sin(ratio * Math.PI)
        const height = 3 + renderedMicLevel * (4 + envelope * 16)
        const opacity = Math.min(1, Math.max(0.14, renderedMicLevel * (0.45 + envelope * 0.55)))
        return {
          id: `meter-bar-${index}`,
          height: `${height.toFixed(2)}px`,
          opacity,
        }
      }),
    [renderedMicLevel]
  )

  useEffect(() => {
    if (!canUseSoundboard) {
      setSoundboardOpen(false)
      setSoundboardPlayingId(null)
    }
  }, [canUseSoundboard])

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      stopMicMeter()
    }
  }, [stopMicMeter])

  useEffect(() => {
    if (!showConnectedLayout) {
      stopMicMeter()
      setMicLevel(0)
      setMicMeterStatus('idle')
      return
    }

    if (!navigator.mediaDevices?.getUserMedia) {
      setMicLevel(0)
      setMicMeterStatus('unavailable')
      return
    }

    let cancelled = false
    setMicMeterStatus('idle')

    const startMicMeter = async () => {
      try {
        const stream = await navigator.mediaDevices.getUserMedia({
          audio: {
            echoCancellation: true,
            noiseSuppression: true,
            autoGainControl: true,
          },
        })

        if (cancelled || !mountedRef.current) {
          for (const track of stream.getTracks()) {
            track.stop()
          }
          return
        }

        const audioContextCtor =
          window.AudioContext ??
          (window as Window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
        if (!audioContextCtor) {
          for (const track of stream.getTracks()) {
            track.stop()
          }
          setMicLevel(0)
          setMicMeterStatus('unavailable')
          return
        }

        stopMicMeter()

        const context = new audioContextCtor()
        const analyser = context.createAnalyser()
        analyser.fftSize = METER_FFT_SIZE
        analyser.smoothingTimeConstant = 0.82

        const source = context.createMediaStreamSource(stream)
        source.connect(analyser)

        audioContextRef.current = context
        analyserRef.current = analyser
        sourceNodeRef.current = source
        mediaStreamRef.current = stream

        void context.resume().catch(() => undefined)

        const samples = new Uint8Array(analyser.fftSize)
        let smoothedLevel = 0

        const readLevel = () => {
          const currentAnalyser = analyserRef.current
          if (!currentAnalyser || cancelled || !mountedRef.current) {
            return
          }

          currentAnalyser.getByteTimeDomainData(samples)

          let sumSquares = 0
          for (const sample of samples) {
            const centered = (sample - 128) / 128
            sumSquares += centered * centered
          }
          const rms = Math.sqrt(sumSquares / samples.length)
          const scaledLevel = Math.min(1, rms * METER_GAIN)
          smoothedLevel = smoothedLevel * (1 - METER_SMOOTHING) + scaledLevel * METER_SMOOTHING

          setMicLevel(smoothedLevel)
          meterAnimationRef.current = requestAnimationFrame(readLevel)
        }

        setMicMeterStatus('active')
        readLevel()
      } catch (error) {
        setMicLevel(0)
        if (
          error instanceof DOMException &&
          (error.name === 'NotAllowedError' || error.name === 'SecurityError')
        ) {
          setMicMeterStatus('denied')
          return
        }
        setMicMeterStatus('unavailable')
      }
    }

    void startMicMeter()

    return () => {
      cancelled = true
      stopMicMeter()
    }
  }, [showConnectedLayout, stopMicMeter])

  useEffect(() => {
    if (!showConnectedLayout || visibleMessages.length === 0) {
      return
    }
    chatBottomRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' })
  }, [showConnectedLayout, visibleMessages.length])

  const handleJoin = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!hasNickname) {
      return
    }

    const nextNickname = nicknameInput.trim()
    const nextBadgeCodes = normalizeBadgeCodes(selectedBadgeCodes)
    setActionBusy(true)
    setErrorMessage(null)
    try {
      await connect(nextNickname, nextBadgeCodes)
      setConfig((prev) =>
        prev
          ? {
              ...prev,
              nickname: nextNickname,
              badge_profiles: {
                ...prev.badge_profiles,
                [nextNickname]: nextBadgeCodes,
              },
            }
          : prev
      )
    } catch (error) {
      setErrorMessage(String(error))
    } finally {
      setActionBusy(false)
    }
  }

  const handleDisconnect = async () => {
    setActionBusy(true)
    setErrorMessage(null)
    try {
      await disconnect()
    } catch (error) {
      setErrorMessage(String(error))
    } finally {
      setActionBusy(false)
    }
  }

  const handleMuteToggle = async (nextMuted: boolean) => {
    setErrorMessage(null)
    try {
      await setMute(nextMuted)
    } catch (error) {
      setErrorMessage(String(error))
    }
  }

  const handleDeafenToggle = async (nextDeafened: boolean) => {
    setErrorMessage(null)
    try {
      await setDeafen(nextDeafened)
    } catch (error) {
      setErrorMessage(String(error))
    }
  }

  const handlePttToggle = async (nextEnabled: boolean) => {
    setErrorMessage(null)
    const previousPttEnabled = selfState.ptt_enabled
    setSelfState((prev) => ({ ...prev, ptt_enabled: nextEnabled }))
    try {
      await setPtt(nextEnabled)
      setConfig((prev) => (prev ? { ...prev, ptt_enabled: nextEnabled } : prev))
    } catch (error) {
      setSelfState((prev) => ({ ...prev, ptt_enabled: previousPttEnabled }))
      setErrorMessage(String(error))
    }
  }

  const handlePttHotkeySave = async () => {
    if (!hotkeyInput.trim()) {
      setErrorMessage('PTT hotkey cannot be empty.')
      return
    }
    setErrorMessage(null)
    try {
      await setPttHotkey(hotkeyInput)
      setConfig((prev) => (prev ? { ...prev, ptt_hotkey: hotkeyInput } : prev))
    } catch (error) {
      setErrorMessage(String(error))
    }
  }

  const handleInputDevice = async (deviceId: string) => {
    setErrorMessage(null)
    try {
      await setInputDevice(deviceId)
      setConfig((prev) => (prev ? { ...prev, input_device: deviceId } : prev))
    } catch (error) {
      setErrorMessage(String(error))
    }
  }

  const handleOutputDevice = async (deviceId: string) => {
    setErrorMessage(null)
    try {
      await setOutputDevice(deviceId)
      setConfig((prev) => (prev ? { ...prev, output_device: deviceId } : prev))
    } catch (error) {
      setErrorMessage(String(error))
    }
  }

  const handleServerPresetChange = async (presetValue: string) => {
    if (!config) {
      return
    }

    let nextHost: string
    let nextPort: number

    if (presetValue === 'railway') {
      nextHost = RAILWAY_SERVER_PROFILE.host
      nextPort = RAILWAY_SERVER_PROFILE.port
    } else if (presetValue === 'harmony-default') {
      nextHost = HARMONY_DEFAULT_SERVER_ENDPOINT.host
      nextPort = HARMONY_DEFAULT_SERVER_ENDPOINT.port
    } else {
      return
    }

    if (
      normalizeServerHost(nextHost) === normalizeServerHost(config.server.host) &&
      nextPort === config.server.port
    ) {
      return
    }

    setServerPresetBusy(true)
    setErrorMessage(null)
    try {
      await setServerEndpoint(nextHost, nextPort)
      setConfig((prev) =>
        prev
          ? {
              ...prev,
              server: {
                ...prev.server,
                host: nextHost,
                port: nextPort,
              },
            }
          : prev
      )
    } catch (error) {
      setErrorMessage(String(error))
    } finally {
      setServerPresetBusy(false)
    }
  }

  const handleRefreshDevices = async () => {
    setErrorMessage(null)
    try {
      const next = await refreshDevices()
      setDevices(next)
    } catch (error) {
      setErrorMessage(String(error))
    }
  }

  const handleSoundboardPlay = async (clipId: string) => {
    if (!canUseSoundboard) {
      return
    }
    setErrorMessage(null)
    setSoundboardPlayingId(clipId)
    try {
      await playSoundboardClip(clipId)
    } catch (error) {
      setErrorMessage(String(error))
    } finally {
      setSoundboardPlayingId((current) => (current === clipId ? null : current))
    }
  }

  const handleSoundboardDelete = async (clipId: string) => {
    setSoundboardBusy(true)
    setErrorMessage(null)
    try {
      await deleteSoundboardClip(clipId)
      await refreshSoundboard()
    } catch (error) {
      setErrorMessage(String(error))
    } finally {
      setSoundboardBusy(false)
    }
  }

  const handleSoundboardUploadPick = () => {
    soundboardFileInputRef.current?.click()
  }

  const handleSoundboardUploadChange = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    event.currentTarget.value = ''
    if (!file) {
      return
    }

    setSoundboardBusy(true)
    setErrorMessage(null)
    try {
      const buffer = await file.arrayBuffer()
      const bytes = new Uint8Array(buffer)
      await importSoundboardClip('', file.name, bytes)
      await refreshSoundboard()
    } catch (error) {
      setErrorMessage(String(error))
    } finally {
      setSoundboardBusy(false)
    }
  }

  const handleBadgeAdd = (badgeCode: string) => {
    setSelectedBadgeCodes((prev) => normalizeBadgeCodes([...prev, badgeCode]))
  }

  const handleBadgeRemove = (badgeCode: string) => {
    setSelectedBadgeCodes((prev) => prev.filter((code) => code !== badgeCode))
  }

  const handleSendChat = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const message = chatInput.trim()
    if (!showConnectedLayout || !message || chatBusy) {
      return
    }

    setChatBusy(true)
    setErrorMessage(null)
    localMessageCounterRef.current += 1
    const localMessageId = `local-${Date.now()}-${localMessageCounterRef.current}`
    const pendingMessage: ChatMessage = {
      local_id: localMessageId,
      actor_name: nicknameInput.trim() || config?.nickname || 'You',
      channel_id: normalizeOutgoingChannelId(roster.channel.id),
      message,
      timestamp_ms: Date.now(),
      is_local_echo: true,
      delivery_state: 'pending',
      delivery_error: null,
    }
    setMessages((prev) => trimMessageHistory([...prev, pendingMessage]))
    setChatInput('')
    try {
      await sendMessage(message)
      setMessages((prev) =>
        prev.map((item) =>
          item.local_id === localMessageId
            ? { ...item, delivery_state: 'confirmed', delivery_error: null }
            : item
        )
      )
    } catch (error) {
      const errorText = String(error)
      setMessages((prev) =>
        prev.map((item) =>
          item.local_id === localMessageId ? { ...item, delivery_error: errorText } : item
        )
      )
      setErrorMessage(errorText)
    } finally {
      setChatBusy(false)
      requestAnimationFrame(() => {
        chatInputRef.current?.focus()
      })
    }
  }

  if (loading || !config) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background text-muted-foreground">
        <LoaderCircle className="mr-2 size-4 animate-spin" />
        Loading Harmony...
      </div>
    )
  }

  return (
    <div className="h-screen overflow-hidden bg-background">
      <div className="flex h-full w-full flex-col overflow-hidden">
        <header className="flex items-center justify-between border-b border-border/40 px-5 py-4">
          <div className="flex items-center gap-2">
            <p className="text-xs uppercase tracking-[0.18em] text-muted-foreground">Harmony</p>
            <span className="rounded-md border border-border/70 bg-secondary/40 px-2 py-0.5 font-mono text-[11px] text-muted-foreground">
              {appVersion ? `v${appVersion}` : 'v...'}
            </span>
          </div>
          <div className="flex items-center gap-3">
            <Badge variant={connectionBadgeVariant}>{connectionLabel}</Badge>
            <Sheet open={settingsOpen} onOpenChange={setSettingsOpen}>
              <SheetTrigger asChild>
                <Button variant="outline" size="sm">
                  <Settings2 className="size-4" />
                  Settings
                </Button>
              </SheetTrigger>
              <SheetContent side="right" className="w-full sm:max-w-md">
                <SheetHeader>
                  <SheetTitle>Audio + PTT</SheetTitle>
                  <SheetDescription>
                    Pick devices and tune push-to-talk for your setup.
                  </SheetDescription>
                </SheetHeader>

                <div className="mt-6 space-y-5">
                  <div className="space-y-2">
                    <Label htmlFor="input-device">Input device</Label>
                    <Select
                      value={config.input_device ?? ''}
                      onValueChange={(value) => void handleInputDevice(value)}
                    >
                      <SelectTrigger id="input-device">
                        <SelectValue placeholder="Select microphone" />
                      </SelectTrigger>
                      <SelectContent>
                        {devices.inputs.map((device) => (
                          <SelectItem key={device.id} value={device.id}>
                            {device.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor="output-device">Output device</Label>
                    <Select
                      value={config.output_device ?? ''}
                      onValueChange={(value) => void handleOutputDevice(value)}
                    >
                      <SelectTrigger id="output-device">
                        <SelectValue placeholder="Select speakers/headset" />
                      </SelectTrigger>
                      <SelectContent>
                        {devices.outputs.map((device) => (
                          <SelectItem key={device.id} value={device.id}>
                            {device.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>

                  <div className="space-y-2">
                    <Label>Output volume</Label>
                    <Slider
                      value={outputVolume}
                      onValueChange={(value) => setOutputVolume(value)}
                      max={100}
                      min={0}
                      step={1}
                    />
                    <p className="text-xs text-muted-foreground">{outputVolume[0]}%</p>
                  </div>

                  <div className="rounded-md border bg-secondary/40 p-3">
                    <div className="flex items-center justify-between">
                      <div>
                        <p className="text-sm font-medium">Push-to-talk</p>
                        <p className="text-xs text-muted-foreground">
                          Use hotkey transmission mode instead of open mic.
                        </p>
                      </div>
                      <Switch
                        checked={selfState.ptt_enabled}
                        onCheckedChange={(checked) => void handlePttToggle(checked)}
                      />
                    </div>
                    <div className="mt-3 flex gap-2">
                      <Input
                        value={hotkeyInput}
                        onChange={(event) => setHotkeyInput(event.target.value)}
                        placeholder="AltLeft"
                        className="flex-1"
                      />
                      <Button variant="secondary" onClick={handlePttHotkeySave}>
                        Save
                      </Button>
                    </div>
                  </div>

                  <div className="rounded-md border bg-secondary/40 p-3">
                    <div className="space-y-2">
                      <Label htmlFor="server-profile">Server profile</Label>
                      <Select
                        value={selectedServerPreset}
                        onValueChange={(value) => void handleServerPresetChange(value)}
                        disabled={serverPresetBusy}
                      >
                        <SelectTrigger id="server-profile">
                          <SelectValue placeholder="Select server profile" />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="harmony-default">Harmony default</SelectItem>
                          <SelectItem value="railway">Railway ({RAILWAY_SERVER_PROFILE.name})</SelectItem>
                          <SelectItem value="custom">Custom (current)</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                    <p className="mt-3 text-xs text-muted-foreground">
                      Server: {config.server.host}:{config.server.port}
                    </p>
                    <p className="text-xs text-muted-foreground">Channel target: {config.server.default_channel}</p>
                    <p className="mt-1 text-[11px] text-muted-foreground">
                      Reconnect after switching profiles to apply immediately.
                    </p>
                  </div>

                  <div className="rounded-md border bg-secondary/40 p-3">
                    <div className="flex items-center justify-between gap-2">
                      <div>
                        <p className="text-sm font-medium">Harmony Voice</p>
                        <p className="text-xs text-muted-foreground">
                          {updateInfo
                            ? `Update ${updateInfo.version} is ready to install.`
                            : `Version ${appVersion ?? '...'}`}
                        </p>
                      </div>
                      <Button
                        variant="secondary"
                        onClick={() => void runUpdateCheck(true)}
                        disabled={updateBusy}
                      >
                        {updateBusy ? (
                          <LoaderCircle className="size-4 animate-spin" />
                        ) : (
                          <RefreshCw className="size-4" />
                        )}
                        Check for updates
                      </Button>
                    </div>
                    {updateInfo ? (
                      <Button
                        className="mt-3 w-full"
                        onClick={() => void handleInstallUpdate()}
                        disabled={updateBusy}
                      >
                        Install update {updateInfo.version}
                      </Button>
                    ) : null}
                    {updateNotice ? (
                      <p className="mt-3 text-xs text-muted-foreground">{updateNotice}</p>
                    ) : null}
                  </div>

                  <Button variant="secondary" onClick={handleRefreshDevices} className="w-full">
                    <RefreshCw className="size-4" />
                    Refresh devices
                  </Button>
                </div>
              </SheetContent>
            </Sheet>
          </div>
        </header>

        {errorMessage ? (
          <div className="mx-5 mt-3 rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {errorMessage}
          </div>
        ) : null}

        {showConnectedLayout ? (
          <div className="flex min-h-0 flex-1 overflow-hidden bg-card/30">
            {/* Sidebar */}
            <aside className="flex min-h-0 w-60 flex-shrink-0 flex-col border-r border-border/50 bg-card/60">
              {/* Channel header */}
              <div className="border-b px-4 py-3">
                <div className="flex items-center gap-2">
                  <Waves className="size-4 text-primary" />
                  <h2 className="font-semibold">{roster.channel.name}</h2>
                </div>
                <p className="mt-0.5 text-xs text-muted-foreground">
                  {joinedUserCount} online
                </p>
              </div>

              {/* User list */}
              <ScrollArea className="flex-1">
                <div className="p-2">
                  <p className="mb-2 px-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                    Voice Connected — {joinedUserCount}
                  </p>
                  {roster.users.length === 0 ? (
                    <p className="px-2 text-xs text-muted-foreground">No users connected</p>
                  ) : (
                    <div className="space-y-0.5">
                      {roster.users.map((user) => (
                        <div
                          key={user.id}
                          className={cn(
                            'flex items-center gap-2 rounded-md px-2 py-1.5 transition-colors duration-150 hover:bg-secondary/50',
                            user.speaking && 'bg-emerald-500/10'
                          )}
                        >
                          <div className="relative">
                            <Avatar className="size-8">
                              <AvatarFallback className="text-xs">
                                {user.name.slice(0, 2).toUpperCase()}
                              </AvatarFallback>
                            </Avatar>
                            {user.speaking ? (
                              <span className="absolute -bottom-0.5 -right-0.5 size-3 rounded-full border-2 border-card bg-emerald-500" />
                            ) : null}
                          </div>
                          <div className="min-w-0 flex-1">
                            <div className="flex min-w-0 items-center gap-1.5">
                              <p className="truncate text-sm font-medium">{user.name}</p>
                              <BadgeIcons badgeCodes={user.badge_codes} size="sm" className="shrink-0" />
                            </div>
                            <div className="flex items-center gap-1">
                              {user.muted ? (
                                <MicOff className="size-3 text-destructive" />
                              ) : null}
                              {user.deafened ? (
                                <VolumeOff className="size-3 text-destructive" />
                              ) : null}
                              {!user.muted && !user.deafened && user.speaking ? (
                                <span className="text-[10px] text-emerald-500">Speaking</span>
                              ) : null}
                            </div>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </ScrollArea>

              {/* Voice controls */}
              <div className="mt-auto border-t bg-secondary/30 p-2">
                {/* Mic meter */}
                <div className="mb-2 rounded-md bg-background/50 px-2 py-1.5">
                  <div className="flex items-center justify-between text-[11px]">
                    <span className="text-muted-foreground">
                      {selfState.transmitting ? 'Transmitting' : 'Voice'}
                    </span>
                    <span className={cn('font-medium', selfState.transmitting ? 'text-emerald-500' : 'text-muted-foreground')}>
                      {selfState.muted ? 'Muted' : selfState.transmitting ? 'Live' : 'Idle'}
                    </span>
                  </div>
                  {(micMeterStatus === 'active' || micMeterStatus === 'idle') ? (
                    <div
                      className={cn(
                        'mt-1.5 flex h-5 items-end gap-0.5 rounded bg-secondary/50 px-1.5 py-0.5',
                        selfState.muted && 'opacity-50'
                      )}
                      aria-hidden="true"
                    >
                      {meterBars.map((bar) => (
                        <div
                          key={bar.id}
                          className={cn(
                            'flex-1 rounded-sm bg-primary transition-[height,opacity] duration-75 ease-linear',
                            selfState.muted && 'bg-muted-foreground'
                          )}
                          style={{ height: bar.height, opacity: bar.opacity }}
                        />
                      ))}
                    </div>
                  ) : (
                    <p className="mt-1 text-[10px] text-muted-foreground">{meterFallbackLabel}</p>
                  )}
                </div>

                {/* Control buttons */}
                <div className="flex gap-1">
                  <input
                    ref={soundboardFileInputRef}
                    type="file"
                    accept={SOUNDBOARD_UPLOAD_ACCEPT}
                    onChange={(event) => void handleSoundboardUploadChange(event)}
                    className="hidden"
                  />
                  <Popover open={soundboardOpen} onOpenChange={setSoundboardOpen}>
                    <PopoverTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        className="flex-1"
                        disabled={!canUseSoundboard || soundboardBusy}
                      >
                        <Waves className="size-4" />
                      </Button>
                    </PopoverTrigger>
                    <PopoverContent align="start" className="w-80 p-2">
                      <div className="mb-2 flex items-start justify-between gap-2 px-1">
                        <div>
                          <p className="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
                            Soundboard
                          </p>
                          <p className="text-[11px] text-muted-foreground">
                            Upload custom clips (local only). Everyone hears played clips.
                          </p>
                        </div>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="h-7 px-2"
                          disabled={soundboardBusy}
                          onClick={handleSoundboardUploadPick}
                        >
                          <Plus className="size-3.5" />
                          Add
                        </Button>
                      </div>

                      <div className="space-y-1">
                        <p className="px-1 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
                          Default
                        </p>
                        {defaultSoundboardClips.map((clip) => (
                          <Button
                            key={clip.id}
                            type="button"
                            variant={soundboardPlayingId === clip.id ? 'secondary' : 'ghost'}
                            size="sm"
                            className="h-8 w-full justify-between px-2"
                            onClick={() => void handleSoundboardPlay(clip.id)}
                            disabled={soundboardBusy || !canUseSoundboard}
                          >
                            <span className="truncate">{clip.label}</span>
                            <span className="text-[10px] text-muted-foreground">
                              {formatClipDuration(clip.duration_ms)}
                            </span>
                          </Button>
                        ))}
                      </div>

                      <div className="mt-2 space-y-1 border-t border-border/60 pt-2">
                        <p className="px-1 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
                          Custom
                        </p>
                        {customSoundboardClips.length === 0 ? (
                          <p className="px-1 py-1 text-[11px] text-muted-foreground">
                            No custom clips yet.
                          </p>
                        ) : (
                          customSoundboardClips.map((clip) => (
                            <div key={clip.id} className="flex items-center gap-1">
                              <Button
                                type="button"
                                variant={soundboardPlayingId === clip.id ? 'secondary' : 'ghost'}
                                size="sm"
                                className="h-8 flex-1 justify-between px-2"
                                onClick={() => void handleSoundboardPlay(clip.id)}
                                disabled={soundboardBusy || !canUseSoundboard}
                              >
                                <span className="truncate">{clip.label}</span>
                                <span className="text-[10px] text-muted-foreground">
                                  {formatClipDuration(clip.duration_ms)}
                                </span>
                              </Button>
                              <Button
                                type="button"
                                variant="ghost"
                                size="sm"
                                className="h-8 px-2 text-muted-foreground hover:text-destructive"
                                disabled={soundboardBusy}
                                onClick={() => void handleSoundboardDelete(clip.id)}
                              >
                                <Trash2 className="size-3.5" />
                              </Button>
                            </div>
                          ))
                        )}
                      </div>
                    </PopoverContent>
                  </Popover>
                  <Button
                    type="button"
                    variant={selfState.muted ? 'destructive' : 'ghost'}
                    size="sm"
                    className="flex-1"
                    onClick={() => void handleMuteToggle(!selfState.muted)}
                  >
                    {selfState.muted ? (
                      <MicOff className="size-4" />
                    ) : (
                      <Mic className="size-4" />
                    )}
                  </Button>
                  <Button
                    type="button"
                    variant={selfState.deafened ? 'destructive' : 'ghost'}
                    size="sm"
                    className="flex-1"
                    onClick={() => void handleDeafenToggle(!selfState.deafened)}
                  >
                    {selfState.deafened ? (
                      <VolumeOff className="size-4" />
                    ) : (
                      <Volume2 className="size-4" />
                    )}
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="flex-1 text-destructive hover:bg-destructive hover:text-destructive-foreground"
                    disabled={connection.state === 'disconnected' || actionBusy}
                    onClick={handleDisconnect}
                  >
                    <LogOut className="size-4" />
                  </Button>
                </div>

                {connection.reason ? (
                  <p className="mt-1.5 text-[10px] text-muted-foreground">{connection.reason}</p>
                ) : null}
              </div>
            </aside>

            {/* Main chat area */}
            <main className="flex min-h-0 flex-1 flex-col">
              {/* Chat header */}
              <div className="flex items-center gap-2 border-b border-border/50 px-4 py-3 shadow-sm">
                <Hash className="size-5 text-muted-foreground" />
                <h2 className="font-semibold">{roster.channel.name}</h2>
                <span className="ml-auto text-xs text-muted-foreground">
                  {visibleMessages.length} messages
                </span>
              </div>

              {/* Messages */}
              <ScrollArea className="min-h-0 flex-1">
                <div className="py-4 pr-4">
                  {groupedMessages.length === 0 ? (
                    <div className="flex flex-col items-center justify-center px-4 py-12 text-center">
                      <Hash className="mb-3 size-12 text-muted-foreground/50" />
                      <h3 className="font-semibold">Welcome to #{roster.channel.name}</h3>
                      <p className="mt-1 text-sm text-muted-foreground">
                        This is the start of your conversation.
                      </p>
                    </div>
                  ) : (
                    groupedMessages.map((group, groupIdx) => (
                      <div
                        key={`${group.actorSession ?? group.actorName}-${group.firstTimestamp}`}
                        className={cn(
                          'group flex gap-4 px-4 py-0.5 transition-colors duration-100 hover:bg-secondary/20',
                          groupIdx === 0 && 'mt-2'
                        )}
                      >
                        <Avatar className="mt-0.5 size-10 flex-shrink-0">
                          <AvatarFallback>
                            {group.actorName.slice(0, 2).toUpperCase()}
                          </AvatarFallback>
                        </Avatar>
                        <div className="min-w-0 flex-1">
                          <div className="flex items-baseline gap-2">
                            <div className="flex min-w-0 items-center gap-1.5">
                              <span className="truncate font-medium">{group.actorName}</span>
                              <BadgeIcons badgeCodes={group.actorBadgeCodes} size="sm" />
                            </div>
                            <span className="text-xs text-muted-foreground">
                              {messageTimeFormatter.format(new Date(group.firstTimestamp))}
                            </span>
                          </div>
                          {group.messages.map((msg) => (
                            <div key={msg.local_id} className="relative pr-6">
                              <p
                                className={cn(
                                  'whitespace-pre-wrap break-words text-[15px] leading-relaxed text-foreground/90',
                                  msg.delivery_error && 'text-destructive/80'
                                )}
                              >
                                {msg.message}
                              </p>
                              {msg.is_local_echo ? (
                                msg.delivery_state === 'confirmed' ? (
                                  <Check
                                    className="absolute right-1 top-1 size-3 text-emerald-500"
                                    aria-label="Message delivered"
                                  />
                                ) : (
                                  <Circle
                                    className={cn(
                                      'absolute right-1 top-1 size-3',
                                      msg.delivery_error ? 'text-destructive' : 'text-muted-foreground'
                                    )}
                                    aria-label={
                                      msg.delivery_error
                                        ? 'Message pending confirmation (send failed)'
                                        : 'Message pending confirmation'
                                    }
                                  />
                                )
                              ) : null}
                            </div>
                          ))}
                        </div>
                      </div>
                    ))
                  )}
                  <div ref={chatBottomRef} />
                </div>
              </ScrollArea>

              {/* Message input */}
              <div className="border-t border-border/50 p-4">
                <form onSubmit={handleSendChat} autoComplete="off">
                  <div className="flex items-center gap-2 rounded-lg bg-secondary/40 px-4 py-2.5 transition-colors focus-within:bg-secondary/60">
                    <Input
                      ref={chatInputRef}
                      id="text-chat-input"
                      placeholder={`Message #${roster.channel.name}`}
                      value={chatInput}
                      onChange={(event) => setChatInput(event.target.value)}
                      maxLength={1024}
                      autoComplete="off"
                      autoCorrect="off"
                      autoCapitalize="off"
                      spellCheck={false}
                      disabled={!showConnectedLayout || chatBusy}
                      className="flex-1 border-0 bg-transparent px-0 shadow-none focus-visible:ring-0"
                    />
                    <Button
                      type="submit"
                      size="sm"
                      variant="ghost"
                      className="size-8 p-0 text-muted-foreground hover:text-foreground"
                      disabled={!showConnectedLayout || chatBusy || chatInput.trim().length === 0}
                    >
                      <Send className="size-4" />
                    </Button>
                  </div>
                </form>
              </div>
            </main>
          </div>
        ) : (
          <div className="flex min-h-0 flex-1 items-center justify-center px-5">
            <Card className="w-full max-w-md bg-card/95">
              <CardHeader>
                <CardTitle className="flex items-center gap-2">
                  <Waves className="size-4 text-primary" />
                  Join voice
                </CardTitle>
                <CardDescription>
                  Enter your username to connect.
                </CardDescription>
              </CardHeader>
              <CardContent className="space-y-4">
                <form onSubmit={handleJoin} className="space-y-4">
                  <div className="space-y-2">
                    <Label htmlFor="username">Username</Label>
                    <Input
                      id="username"
                      placeholder="Your in-game name"
                      value={nicknameInput}
                      onChange={(event) => setNicknameInput(event.target.value)}
                      maxLength={32}
                    />
                  </div>
                  <div className="space-y-2">
                    <div className="flex items-center justify-between">
                      <Label htmlFor="badge-picker">Badges</Label>
                      <span className="text-[11px] text-muted-foreground">
                        {selectedBadgeCodes.length}/{MAX_BADGES_PER_USER}
                      </span>
                    </div>
                    <Select
                      onValueChange={(value) => {
                        handleBadgeAdd(value)
                      }}
                    >
                      <SelectTrigger id="badge-picker" disabled={selectedBadgeCodes.length >= MAX_BADGES_PER_USER}>
                        <SelectValue placeholder="Add a badge" />
                      </SelectTrigger>
                      <SelectContent>
                        {BADGE_MANIFEST.map((badge) => (
                          <SelectItem
                            key={badge.code}
                            value={badge.code}
                            disabled={
                              selectedBadgeSet.has(badge.code) ||
                              selectedBadgeCodes.length >= MAX_BADGES_PER_USER
                            }
                          >
                            <span className="inline-flex items-center gap-2">
                              <img
                                src={badge.src}
                                alt={badge.label}
                                className="size-4 rounded-sm border border-border/50 object-cover"
                              />
                              <span>{badge.label}</span>
                            </span>
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    {selectedBadgeCodes.length > 0 ? (
                      <div className="flex flex-wrap gap-1.5">
                        {selectedBadgeCodes.map((badgeCode) => {
                          const badge = resolveBadgeByCode(badgeCode)
                          return (
                            <button
                              key={badgeCode}
                              type="button"
                              onClick={() => handleBadgeRemove(badgeCode)}
                              className="inline-flex items-center gap-1.5 rounded-md border border-border/60 bg-secondary/35 px-2 py-1 text-xs transition-colors hover:bg-secondary/60"
                            >
                              <BadgeIcons badgeCodes={[badgeCode]} size="sm" />
                              <span>{badge?.label ?? badgeCode}</span>
                            </button>
                          )
                        })}
                      </div>
                    ) : (
                      <p className="text-xs text-muted-foreground">No badges selected.</p>
                    )}
                  </div>
                  {isSuperuserRoute ? (
                    <div className="rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
                      Superuser route active: this will authenticate as SuperUser.
                    </div>
                  ) : null}
                  <Button type="submit" className="w-full" disabled={!canJoin}>
                    {isConnectingLike ? (
                      <>
                        <LoaderCircle className="size-4 animate-spin" />
                        Connecting...
                      </>
                    ) : (
                      'Connect'
                    )}
                  </Button>
                </form>

                {connection.reason ? (
                  <p className="text-xs text-muted-foreground">{connection.reason}</p>
                ) : null}
              </CardContent>
            </Card>
          </div>
        )}
      </div>
    </div>
  )
}

export default App
