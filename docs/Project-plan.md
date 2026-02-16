PRD: “GameNight Voice” (Windows desktop voice chat client)

1) Overview

Product: A lightweight, fast Windows desktop voice chat app for private game nights.
Core idea: Ship a custom-branded client that connects to a self-hosted Mumble server (Murmur).
Why: Maximum “I built this” wow-factor with minimal infrastructure complexity and low ongoing cost.

⸻

2) Goals
	1.	One-click join: Install → open → press “Join Game Night” → you’re in voice.
	2.	Low latency, stable voice: Prioritize quality and reliability over features.
	3.	Lightweight + fast: Small installer, quick cold start, low CPU/RAM footprint.
	4.	Simple for friends: No server address configuration required; no accounts required for v1.

3) Non-goals (v1)
	•	Text chat, DMs, emojis, file sharing, screen share
	•	Public servers / server discovery
	•	Moderation tooling beyond basic mute/deafen
	•	Cross-platform (Windows-only in v1)
	•	Custom audio effects / soundboard

⸻

4) Target users & use cases

Users: You + a small friend group (2–15 typical, design for up to ~30).
Primary use case: Weekly/occasional game nights, jump into one shared voice channel quickly.

⸻

5) UX principles
	•	Zero setup for guests: hardcode server + default channel; nickname prompt only.
	•	Frictionless: minimal screens, minimal options, “it just works.”
	•	Visible state: who’s connected, who’s talking, your mute/deafen/PTT state.

⸻

6) User stories
	1.	As a user, I can set a nickname and join the main voice channel in one click.
	2.	As a user, I can mute/unmute myself.
	3.	As a user, I can deafen/undeafen (mute incoming audio).
	4.	As a user, I can enable push-to-talk and hold a hotkey to speak.
	5.	As a user, I can see who’s in the channel and who is currently speaking.
	6.	As a user, I can adjust my mic input device and speaker output device.
	7.	As a user, I can see connection status and reconnect if dropped.

⸻

7) Functional requirements (v1)

7.1 Onboarding & identity
	•	On first launch: prompt for nickname (required).
	•	Persist nickname locally.
	•	Optional: “Remember me” toggle (default on).
	•	No login/account system.

7.2 Connection & rooms
	•	“Join Game Night” button connects to a single configured server and joins a default channel.
	•	Support selecting from a small list of channels (optional for v1; can be v1.1).
	•	Reconnect behavior:
	•	If connection drops, show “Reconnecting…” and attempt exponential backoff.
	•	Offer a “Reconnect now” button.

7.3 Voice controls
	•	Mute: stops sending mic audio.
	•	Deafen: mutes incoming audio + optionally auto-mutes mic.
	•	Push-to-talk (PTT):
	•	Toggle PTT mode on/off.
	•	Configure a hotkey (default: Left Alt or Mouse button if supported).
	•	Visual indicator when transmitting.

7.4 Audio devices & levels
	•	Select input (mic) device
	•	Select output (speaker/headset) device
	•	Mic level meter (simple visualization)
	•	Output volume slider (global)
	•	Per-user volume slider (nice-to-have; if not easy, v1.1)

7.5 Presence & speaking indicators
	•	Show list of connected users in current channel
	•	Highlight who is speaking (voice activity detection events or audio level thresholds)
	•	Show self status icons: muted/deafened/PTT

7.6 Settings
	•	Basic settings screen:
	•	Mic device, speaker device
	•	PTT toggle + hotkey
	•	Startup behavior (launch on startup: optional v1.1)
	•	Store settings locally (app data directory).

⸻

8) Non-functional requirements
	•	Performance
	•	Cold start to usable UI: < 1.5s on typical gaming PC
	•	Idle CPU low; voice processing efficient
	•	Installer size
	•	Target small footprint; avoid bundling heavy runtimes
	•	Reliability
	•	Survive network hiccups; reconnect gracefully
	•	Security
	•	TLS connection to server if supported/configured
	•	No open inbound ports on client
	•	Store only local preferences; no sensitive personal data

⸻

9) Technical plan & architecture

9.1 Recommended framework
Use Tauri for a lightweight Windows desktop app:
	•	UI: HTML/CSS/JS (React optional)
	•	Backend: Rust for system-level access + audio client integration
	•	Output: native installer, generally smaller/lighter than Electron

Note: While the UI uses web tech, it is not a browser app; it’s a native desktop shell.

9.2 High-level components
	1.	Desktop UI (Tauri frontend)
	•	Screens: Welcome/Nickname, Main Voice Screen, Settings
	•	State: connection status, current channel, user list, speaking indicators
	2.	Voice Engine (Rust core)
	•	Implements/uses a Mumble protocol client
	•	Handles:
	•	connect/auth to server
	•	channel join
	•	audio capture/playback
	•	encode/decode (Opus)
	•	jitter buffer, packet timing
	•	VAD / speaking detection
	3.	Settings Store
	•	Local config file (JSON) in AppData
	4.	Update & release pipeline
	•	CI builds signed Windows installer (later)
	•	Versioned releases

9.3 Server-side (out of scope for the app agent, but required)
	•	One Murmur server (Mumble server)
	•	Configure:
	•	server address/port
	•	optional TLS cert
	•	default channel(s)
	•	user auth strategy (anonymous allowed for private server or simple password)

9.4 Data flow
	•	UI triggers “Join” → Rust core starts connection → on success emits events:
	•	connection_state_changed
	•	users_updated
	•	speaking_state_changed
	•	self_state_changed
	•	UI renders state + sends commands:
	•	set_mute(bool)
	•	set_deafen(bool)
	•	set_ptt(bool)
	•	set_ptt_hotkey(key)
	•	select_input_device(id)
	•	select_output_device(id)
	•	set_volume(user_id, value) (optional)

9.5 Event contract (what the coding agent should implement)
Events from core → UI
	•	core/connection: { state: "disconnected"|"connecting"|"connected"|"reconnecting", reason?: string }
	•	core/roster: { channel: {id,name}, users: [{id,name, muted, deafened, speaking}] }
	•	core/speaking: { user_id, speaking: bool, level?: number }
	•	core/devices: { inputs: [{id,name}], outputs: [{id,name}] }
	•	core/self: { muted, deafened, ptt_enabled, transmitting }

Commands from UI → core
	•	connect({ nickname })
	•	disconnect()
	•	set_mute({ muted })
	•	set_deafen({ deafened })
	•	set_ptt({ enabled })
	•	set_ptt_hotkey({ hotkey })
	•	set_input_device({ device_id })
	•	set_output_device({ device_id })
	•	(optional) set_user_volume({ user_id, volume })
	•	(optional) switch_channel({ channel_id })

⸻

10) Repository structure (hand this to an AI coding agent)
```
Harmony/
  apps/
    desktop/
      src-ui/                # UI (React or vanilla TS)
      src-tauri/
        src/
          main.rs            # Tauri entry
          core/
            mod.rs
            config.rs        # load/save settings
            events.rs        # event emitter bridge to UI
            voice/
              mod.rs
              client.rs      # mumble client wrapper
              audio_in.rs    # mic capture
              audio_out.rs   # speaker playback
              codec.rs       # opus encode/decode
              vad.rs         # speaking detection
              hotkeys.rs     # PTT global hotkey (Windows)
  docs/
    Project-plan.md
    protocol-notes.md        # mumble integration notes
  .github/workflows/
    build-windows.yml
```

Key implementation note for the agent: pick one of these approaches:
	•	A) Use an existing maintained Mumble client library and wrap it (preferred if available).
	•	B) If not, implement minimal Mumble protocol client for voice + roster (harder but doable).
	•	C) Hybrid: embed/bridge to an existing CLI/headless client and control it (fastest hack, least elegant).


⸻

12) Acceptance criteria (definition of done for v1)
	•	A new friend can install on Windows, enter nickname, click Join, and be in voice within 30 seconds.
	•	Mute/deafen/PTT all work reliably.
	•	Voice latency is “game-night acceptable” (subjective but should feel comparable to Discord).
	•	App stays stable for a 2-hour session with 5–10 users.
	•	Installer and app start quickly and don’t feel heavy.

⸻

13) Risks & mitigations
	•	Mumble client library availability/quality: choose approach A/B/C early; spike it in Milestone 2.
	•	Audio device handling on Windows: use stable audio I/O crates; build a simple device selection screen early.
	•	Hotkey reliability: keep PTT key config minimal; if global hotkeys are painful, start with “PTT when app focused” for v1 and add global later.

⸻

14) Hand-off notes to the AI coding agent
	•	Prioritize correctness + stability over features.
	•	Keep UI minimal; implement an event-driven bridge (Rust emits events → UI subscribes).
	•	Build with test server credentials/config in a local dev-config.json (excluded from git) and a config.sample.json.