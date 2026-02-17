# Building Harmony Locally (Windows + macOS)

This doc covers:

1. Running/building the app locally on Windows
2. Running/building the app locally on macOS
3. Building the Windows installer (`.exe` via NSIS)
4. Building the macOS installer (`.dmg`)

---

## Prerequisites

### Required on both platforms

- Node.js `20.19+` or `22.12+` (npm included)
- Rust (stable) via `rustup`

```powershell
# Verify tooling
node -v
npm -v
rustc -V
cargo -V
```

### Windows-only prerequisites

- Visual Studio 2022 Build Tools (Desktop development with C++)
- OpenSSL runtime DLLs available on PATH or in `C:\Windows\System32`:
  - `libssl-3-x64.dll`
  - `libcrypto-3-x64.dll`

### macOS-only prerequisites

- Xcode Command Line Tools
- CMake (`brew install cmake`)

```bash
xcode-select --install
```

### Command style (important)

All command snippets below assume you are in repo root and use `--prefix` paths.

- Good: `npm ci --prefix apps/desktop/src-ui`
- Bad: `cd apps/desktop/src-ui` and then `npm ci --prefix apps/desktop/src-ui` (this doubles the path and fails with ENOENT)

On Apple Silicon, avoid x86_64 Homebrew-only OpenSSL/Opus for local arm64 builds. The macOS scripts now force bundled Opus and vendored OpenSSL for architecture-safe linking.

---

## Local App Build/Run - Windows

From repo root:

```powershell
npm ci --prefix apps/desktop/src-ui
npm --prefix apps/desktop/src-ui run tauri:dev:windows
```

Production binary (no installer):

```powershell
cargo build --release --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Output binary:

- `apps/desktop/src-tauri/target/release/harmony-desktop.exe`

---

## Local App Build/Run - macOS

From repo root:

```bash
npm ci --prefix apps/desktop/src-ui
npm --prefix apps/desktop/src-ui run tauri:dev:macos
```

Production binary (no installer):

```bash
cargo build --release --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Output binary:

- `apps/desktop/src-tauri/target/release/harmony-desktop`

---

## Build Windows Installer (`.exe`)

From repo root:

```powershell
npm ci --prefix apps/desktop/src-ui
npm --prefix apps/desktop/src-ui run tauri:build:windows
```

What this does in current config:

- Builds the UI
- Uses `apps/desktop/src-tauri/tauri.windows.conf.json`
- Runs `apps/desktop/src-tauri/scripts/prepare-openssl-runtime.ps1`
- Bundles with NSIS (`bundle.targets = "nsis"`)

Installer output:

- `apps/desktop/src-tauri/target/release/bundle/nsis/Harmony_0.1.0_x64-setup.exe`

---

## Publish Windows Auto-Update (GitHub Releases)

This project now uses Tauri updater + GitHub Releases for in-app updates.  
Runtime endpoint is:

- `https://github.com/MasonKomo/Harmony/releases/latest/download/latest.json`

### 1) One-time setup (updater signing key)

From repo root:

```powershell
$env:HARMONY_UPDATER_KEY_PASSWORD = "replace-with-strong-password"
npm --prefix apps/desktop/src-ui exec -- tauri signer generate -- --ci -w "$env:USERPROFILE\.tauri\harmony-updater.key" -p "$env:HARMONY_UPDATER_KEY_PASSWORD"
```

Then copy the generated public key contents into:

- `apps/desktop/src-tauri/tauri.conf.json` -> `plugins.updater.pubkey`

Important:

- Keep the private key safe. If you lose it, existing installs cannot trust future updates.
- Never commit private key material.

### 2) Build signed installer + updater artifacts

From repo root:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content -Raw "$env:USERPROFILE\.tauri\harmony-updater.key").Trim()
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "$env:HARMONY_UPDATER_KEY_PASSWORD"
npm ci --prefix apps/desktop/src-ui
npm --prefix apps/desktop/src-ui run tauri:build:windows
```

This generates (under `apps/desktop/src-tauri/target/release/bundle/nsis/`):

- `Harmony_<version>_x64-setup.exe`
- `Harmony_<version>_x64-setup.exe.sig`

### 3) Publish release assets + latest.json

From repo root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File apps/desktop/src-tauri/scripts/publish-github-update.ps1 `
  -ReleaseTag "v0.1.0" `
  -Repo "MasonKomo/Harmony" `
  -ReleaseNotes "Harmony 0.1.0"
```

This script:

- creates `latest.json` with `windows-x86_64` URL + signature
- writes `latest.json` as UTF-8 **without BOM** (required by Tauri updater JSON decoding)
- creates the GitHub release tag if missing
- uploads `.exe`, `.sig`, and `latest.json` with `--clobber`

### One-command release day flow

Use the release wrapper script to validate key/pubkey, build, and publish in one run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File apps/desktop/src-tauri/scripts/release-day.ps1 `
  -Version "0.1.0" `
  -ReleaseNotes "Harmony 0.1.0" `
  -KeyPassword "$env:HARMONY_UPDATER_KEY_PASSWORD"
```

Useful flags:

- `-DryRun`: generate `latest.json` only (no GitHub release upload)
- `-SkipNpmCi`: skip `npm ci` when dependencies are already installed
- `-SkipBuild`: skip build and only publish existing artifacts

Optional local smoke check (no GitHub upload):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File apps/desktop/src-tauri/scripts/publish-github-update.ps1 `
  -ReleaseTag "v0.1.0" `
  -DryRun
```

If `latest.json` was previously published with BOM and app update checks fail with a decode error, regenerate and re-upload the asset:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File apps/desktop/src-tauri/scripts/publish-github-update.ps1 `
  -ReleaseTag "v0.1.0" `
  -Repo "MasonKomo/Harmony" `
  -ReleaseNotes "Harmony 0.1.0"
```

### 4) Verify update path before broad rollout

1. Install an older Harmony build on a clean Windows test machine.
2. Publish a newer release via the steps above.
3. Launch old build, open Settings, click **Check now** under **App updates**.
4. Confirm it downloads, installs, and relaunches into the new version without uninstalling.

---

## Build macOS Installer (`.dmg`)

`apps/desktop/src-tauri/tauri.conf.json` is platform-neutral.  
For platform-specific packaging overrides, scripts use:

- `apps/desktop/src-tauri/tauri.windows.conf.json` (Windows installer flow)
- `apps/desktop/src-tauri/tauri.macos.conf.json` (macOS installer flow)

### Build with the macOS config

From repo root:

```bash
npm ci --prefix apps/desktop/src-ui
npm --prefix apps/desktop/src-ui run tauri:build:macos
```

Outputs:

- App bundle: `apps/desktop/src-tauri/target/release/bundle/macos/Harmony.app`
- Installer: `apps/desktop/src-tauri/target/release/bundle/dmg/Harmony_0.1.0_*.dmg`

Notes:

- Local DMG builds are unsigned unless you configure Apple signing/notarization.
- Unsigned apps may show Gatekeeper warnings on other machines.
