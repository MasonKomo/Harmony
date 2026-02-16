import { type FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
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
  connect,
  disconnect,
  refreshDevices,
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
  RosterEvent,
  SelfEvent,
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

function App() {
  const [config, setConfig] = useState<AppConfig | null>(null)
  const [connection, setConnection] = useState<ConnectionEvent>(INITIAL_CONNECTION)
  const [roster, setRoster] = useState<RosterEvent>(INITIAL_ROSTER)
  const [devices, setDevices] = useState<DevicesEvent>(INITIAL_DEVICES)
  const [selfState, setSelfState] = useState<SelfEvent>(INITIAL_SELF_STATE)
  const [micLevel, setMicLevel] = useState(0)
  const [micMeterStatus, setMicMeterStatus] = useState<MicMeterStatus>('idle')
  const [nicknameInput, setNicknameInput] = useState('')
  const [hotkeyInput, setHotkeyInput] = useState('AltLeft')
  const [outputVolume, setOutputVolume] = useState([80])
  const [loading, setLoading] = useState(true)
  const [actionBusy, setActionBusy] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const mountedRef = useRef(false)
  const audioContextRef = useRef<AudioContext | null>(null)
  const analyserRef = useRef<AnalyserNode | null>(null)
  const sourceNodeRef = useRef<MediaStreamAudioSourceNode | null>(null)
  const mediaStreamRef = useRef<MediaStream | null>(null)
  const meterAnimationRef = useRef<number | null>(null)

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

  useEffect(() => {
    let stopListeners: (() => void) | null = null
    let mounted = true

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

      stopListeners = await subscribeCoreEvents({
        connection: (payload) => setConnection(payload),
        roster: (payload) => setRoster(payload),
        devices: (payload) => setDevices(payload),
        self: (payload) => setSelfState(payload),
      })
    }

    setup()

    return () => {
      mounted = false
      stopListeners?.()
    }
  }, [])

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
    setSelfState((prev) => ({ ...prev, ptt_enabled: nextEnabled }))
    try {
      await setPtt(nextEnabled)
      setConfig((prev) => (prev ? { ...prev, ptt_enabled: nextEnabled } : prev))
    } catch (error) {
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
                <CardDescription>Upcoming feature (disabled for now)</CardDescription>
              </CardHeader>
              <CardContent className="flex min-h-0 flex-1 flex-col gap-4">
                <ScrollArea className="min-h-0 flex-1 rounded-md border border-dashed bg-secondary/20 p-4">
                  <div className="space-y-2 text-sm text-muted-foreground">
                    <p>This pane will host in-channel text chat.</p>
                    <p>Voice controls remain active in the left pane.</p>
                  </div>
                </ScrollArea>
                <div className="space-y-2">
                  <Label htmlFor="text-chat-input">Message</Label>
                  <Input
                    id="text-chat-input"
                    placeholder="Text chat is disabled for now"
                    disabled
                  />
                </div>
                <Button disabled className="w-full">
                  Send
                </Button>
              </CardContent>
            </Card>
          ) : null}
        </div>
      </div>
    </div>
  )
}

export default App
