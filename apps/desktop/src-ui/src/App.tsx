import { type FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Check,
  Circle,
  LoaderCircle,
  Mic,
  MicOff,
  RefreshCw,
  Settings2,
  Volume2,
  VolumeOff,
  Waves,
} from 'lucide-react'

import {
  bootstrap,
  checkForUpdate,
  connect,
  disconnect,
  installCachedUpdate,
  refreshDevices,
  sendMessage,
  setDeafen,
  setInputDevice,
  setMute,
  setOutputDevice,
  setPtt,
  setPttHotkey,
  subscribeCoreEvents,
} from '@/lib/core'
import type {
  AppConfig,
  ConnectionEvent,
  ConnectionState,
  DevicesEvent,
  MessageEvent,
  RosterEvent,
  SelfEvent,
  UpdateInfo,
} from '@/lib/types'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Badge, type BadgeProps } from '@/components/ui/badge'
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
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from '@/components/ui/sheet'
import { Slider } from '@/components/ui/slider'
import { Switch } from '@/components/ui/switch'
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

type ChatDeliveryState = 'pending' | 'confirmed'

type ChatMessage = MessageEvent & {
  local_id: string
  is_local_echo: boolean
  delivery_state: ChatDeliveryState
  delivery_error?: string | null
}

const normalizeChatText = (value: string) => value.replace(/\s+/g, ' ').trim()
const normalizeActorName = (value: string) => value.trim().toLowerCase()
const normalizeChannelId = (channelId?: string) => channelId ?? ''
const normalizeOutgoingChannelId = (channelId: string) => (channelId === '0' ? undefined : channelId)

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
  const [chatInput, setChatInput] = useState('')
  const [micLevel, setMicLevel] = useState(0)
  const [micMeterStatus, setMicMeterStatus] = useState<MicMeterStatus>('idle')
  const [nicknameInput, setNicknameInput] = useState('')
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
  const mountedRef = useRef(false)
  const chatBottomRef = useRef<HTMLDivElement | null>(null)
  const chatInputRef = useRef<HTMLInputElement | null>(null)
  const audioContextRef = useRef<AudioContext | null>(null)
  const analyserRef = useRef<AnalyserNode | null>(null)
  const sourceNodeRef = useRef<MediaStreamAudioSourceNode | null>(null)
  const mediaStreamRef = useRef<MediaStream | null>(null)
  const meterAnimationRef = useRef<number | null>(null)
  const localMessageCounterRef = useRef(0)

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
        setHotkeyInput(snapshot.config.ptt_hotkey)
        setOutputVolume([snapshot.config.output_volume])
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
  const visibleMessages = useMemo(
    () => messages.filter((message) => !message.channel_id || message.channel_id === roster.channel.id),
    [messages, roster.channel.id]
  )
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

    setActionBusy(true)
    setErrorMessage(null)
    try {
      await connect(nicknameInput.trim())
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

  const handleRefreshDevices = async () => {
    setErrorMessage(null)
    try {
      const next = await refreshDevices()
      setDevices(next)
    } catch (error) {
      setErrorMessage(String(error))
    }
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
    <div className="min-h-screen bg-background">
      <div className="mx-auto max-w-6xl px-5 py-6">
        <header className="mb-6 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <p className="text-xs uppercase tracking-[0.18em] text-muted-foreground">Harmony</p>
            <span className="rounded-md border border-border/70 bg-secondary/40 px-2 py-0.5 font-mono text-[11px] text-muted-foreground">
              v0.12
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

                  <div className="rounded-md border bg-secondary/40 p-3 text-xs text-muted-foreground">
                    <p>
                      Server: {config.server.host}:{config.server.port}
                    </p>
                    <p className="mt-1">Channel target: {config.server.default_channel}</p>
                  </div>

                  <div className="rounded-md border bg-secondary/40 p-3">
                    <div className="flex items-center justify-between gap-2">
                      <div>
                        <p className="text-sm font-medium">App updates</p>
                        <p className="text-xs text-muted-foreground">
                          {updateInfo
                            ? `Update ${updateInfo.version} is ready to install.`
                            : 'Check GitHub release updates for this build.'}
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
                        Check now
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
          <div className="mb-4 rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {errorMessage}
          </div>
        ) : null}

        <div
          className={cn(
            'transition-all duration-500 ease-out',
            showConnectedLayout
              ? 'grid gap-4 lg:grid-cols-[1fr_1fr]'
              : 'flex min-h-[calc(100vh-11rem)] items-center justify-center'
          )}
        >
          <div
            className={cn(
              'w-full transition-all duration-500 ease-out',
              showConnectedLayout ? 'lg:translate-x-0' : 'max-w-md lg:translate-x-10'
            )}
          >
            <Card
              className={cn(
                'bg-card/95 transition-all duration-500',
                showConnectedLayout ? 'flex h-[560px] flex-col overflow-hidden' : ''
              )}
            >
              {showConnectedLayout ? (
                <>
                  <CardHeader>
                    <CardTitle className="flex items-center gap-2">
                      <Waves className="size-4 text-primary" />
                      {roster.channel.name}
                    </CardTitle>
                    <CardDescription>{joinedUserCount} connected user(s)</CardDescription>
                  </CardHeader>
                  <CardContent className="flex min-h-0 flex-1 flex-col gap-4">
                    <ScrollArea className="min-h-0 flex-1 pr-2">
                      <div className="space-y-2">
                        {roster.users.length === 0 ? (
                          <div className="rounded-md border border-dashed p-4 text-sm text-muted-foreground">
                            Nobody in channel yet.
                          </div>
                        ) : (
                          roster.users.map((user) => (
                            <div
                              key={user.id}
                              className="flex items-center justify-between rounded-md border bg-secondary/30 px-3 py-2"
                            >
                              <div className="flex items-center gap-2">
                                <Avatar>
                                  <AvatarFallback>
                                    {user.name.slice(0, 2).toUpperCase()}
                                  </AvatarFallback>
                                </Avatar>
                                <div>
                                  <p className="text-sm font-medium">{user.name}</p>
                                  <p className="text-xs text-muted-foreground">ID: {user.id}</p>
                                </div>
                              </div>
                              <div className="flex gap-2">
                                {user.muted ? <Badge variant="danger">Muted</Badge> : null}
                                {user.deafened ? <Badge variant="danger">Deaf</Badge> : null}
                                {user.speaking ? <Badge variant="success">Speaking</Badge> : null}
                              </div>
                            </div>
                          ))
                        )}
                      </div>
                    </ScrollArea>

                    <div className="mt-auto space-y-3">
                      <div className="rounded-md border px-3 py-2">
                        <div className="flex items-center justify-between text-sm">
                          <span className="text-muted-foreground">Transmitting</span>
                          <Badge variant={selfState.transmitting ? 'success' : 'secondary'}>
                            {selfState.transmitting ? 'Live' : 'Idle'}
                          </Badge>
                        </div>
                        <div className="mt-2 flex items-center gap-3">
                          {micMeterStatus === 'active' || micMeterStatus === 'idle' ? (
                            <>
                              <div
                                className={cn(
                                  'flex h-8 flex-1 items-end gap-1 rounded-md border bg-secondary/30 px-2 py-1',
                                  selfState.muted ? 'opacity-55' : 'opacity-100'
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
                              <span className="w-12 text-right font-mono text-[11px] text-muted-foreground">
                                {selfState.muted ? 'Muted' : `${Math.round(renderedMicLevel * 100)}%`}
                              </span>
                            </>
                          ) : (
                            <p className="text-xs text-muted-foreground">{meterFallbackLabel}</p>
                          )}
                        </div>
                      </div>

                      <div className="grid grid-cols-3 gap-2">
                        <Button
                          type="button"
                          variant={selfState.muted ? 'destructive' : 'outline'}
                          onClick={() => void handleMuteToggle(!selfState.muted)}
                        >
                          {selfState.muted ? (
                            <MicOff className="size-4" />
                          ) : (
                            <Mic className="size-4" />
                          )}
                          {selfState.muted ? 'Muted' : 'Mute'}
                        </Button>
                        <Button
                          type="button"
                          variant={selfState.deafened ? 'destructive' : 'outline'}
                          onClick={() => void handleDeafenToggle(!selfState.deafened)}
                        >
                          {selfState.deafened ? (
                            <VolumeOff className="size-4" />
                          ) : (
                            <Volume2 className="size-4" />
                          )}
                          {selfState.deafened ? 'Deafened' : 'Deafen'}
                        </Button>
                        <Button
                          type="button"
                          variant="secondary"
                          disabled={connection.state === 'disconnected' || actionBusy}
                          onClick={handleDisconnect}
                        >
                          Disconnect
                        </Button>
                      </div>

                      {connection.reason ? (
                        <p className="text-xs text-muted-foreground">{connection.reason}</p>
                      ) : null}
                    </div>
                  </CardContent>
                </>
              ) : (
                <>
                  <CardHeader>
                    <CardTitle className="flex items-center gap-2">
                      <Waves className="size-4 text-primary" />
                      Join voice
                    </CardTitle>
                    <CardDescription>
                      Enter your username to connect to {config.server.default_channel}.
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
                </>
              )}
            </Card>
          </div>

          {showConnectedLayout ? (
            <Card className="bg-card/95 flex h-[560px] flex-col overflow-hidden">
              <CardHeader>
                <CardTitle>Text chat</CardTitle>
                <CardDescription>
                  Channel: {roster.channel.name} ({visibleMessages.length} messages this session)
                </CardDescription>
              </CardHeader>
              <CardContent className="flex min-h-0 flex-1 flex-col gap-4">
                <ScrollArea className="min-h-0 flex-1">
                  <div className="space-y-2 pr-4">
                    {visibleMessages.length === 0 ? (
                      <div className="rounded-md border border-dashed p-3 text-sm text-muted-foreground">
                        No channel messages yet.
                      </div>
                    ) : (
                      visibleMessages.map((message) => (
                        <div
                          key={message.local_id}
                          className={cn(
                            'rounded-md border bg-background/70 px-3 py-2',
                            message.is_local_echo ? 'relative pb-6' : '',
                            message.delivery_error ? 'border-destructive/50' : ''
                          )}
                        >
                          <div className="mb-1 flex items-center justify-between gap-3">
                            <div className="flex min-w-0 items-center gap-1.5">
                              <p className="truncate text-xs font-semibold">{message.actor_name}</p>
                            </div>
                            <p className="shrink-0 text-[11px] text-muted-foreground">
                              {messageTimeFormatter.format(new Date(message.timestamp_ms))}
                            </p>
                          </div>
                          <p className="whitespace-pre-wrap break-words text-sm">{message.message}</p>
                          {message.is_local_echo ? (
                            message.delivery_state === 'confirmed' ? (
                              <span
                                className="absolute bottom-2 right-2 inline-flex size-4 items-center justify-center rounded-full bg-emerald-500 text-white ring-1 ring-emerald-200/70 shadow-sm"
                                aria-label="Message delivered"
                                title="Delivered"
                              >
                                <Check className="size-2.5 stroke-[3]" />
                              </span>
                            ) : (
                              <Circle
                                className={cn(
                                  'absolute bottom-2 right-2 size-3.5',
                                  message.delivery_error ? 'text-destructive' : 'text-muted-foreground'
                                )}
                                aria-label={
                                  message.delivery_error
                                    ? 'Message pending confirmation (send failed)'
                                    : 'Message pending confirmation'
                                }
                              />
                            )
                          ) : null}
                        </div>
                      ))
                    )}
                    <div ref={chatBottomRef} />
                  </div>
                </ScrollArea>
                <form onSubmit={handleSendChat} className="space-y-2" autoComplete="off">
                  <Label htmlFor="text-chat-input">Message</Label>
                  <div className="flex items-center gap-2">
                    <Input
                      ref={chatInputRef}
                      id="text-chat-input"
                      placeholder="Type a message..."
                      value={chatInput}
                      onChange={(event) => setChatInput(event.target.value)}
                      maxLength={1024}
                      autoComplete="off"
                      autoCorrect="off"
                      autoCapitalize="off"
                      spellCheck={false}
                      disabled={!showConnectedLayout || chatBusy}
                    />
                    <Button
                      type="submit"
                      className="px-5"
                      disabled={!showConnectedLayout || chatBusy || chatInput.trim().length === 0}
                    >
                      Send
                    </Button>
                  </div>
                </form>
              </CardContent>
            </Card>
          ) : null}
        </div>
      </div>
    </div>
  )
}

export default App
