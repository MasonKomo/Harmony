# Building Harmony Locally (Windows + macOS)

This doc covers:

1. Running/building the app locally on Windows
2. Running/building the app locally on macOS
3. Building the Windows installer (`.exe` via NSIS)
4. Building the macOS installer (`.dmg`)

---

## Prerequisites

### Required on both platforms

- Node.js 20+ (npm included)
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

```bash
xcode-select --install
```

---

## Local App Build/Run - Windows

From repo root:

```powershell
cd apps/desktop/src-ui
npm ci
npm run tauri:dev
```

Production binary (no installer):

```powershell
cd ../src-tauri
cargo build --release
```

Output binary:

- `apps/desktop/src-tauri/target/release/harmony-desktop.exe`

---

## Local App Build/Run - macOS

From repo root:

```bash
cd apps/desktop/src-ui
npm ci
npm run tauri:dev
```

Production binary (no installer):

```bash
cd ../src-tauri
cargo build --release
```

Output binary:

- `apps/desktop/src-tauri/target/release/harmony-desktop`

---

## Build Windows Installer (`.exe`)

From repo root:

```powershell
cd apps/desktop/src-ui
npm ci
npm run tauri:build
```

What this does in current config:

- Builds the UI
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
cd apps/desktop/src-tauri
$env:HARMONY_UPDATER_KEY_PASSWORD = "replace-with-strong-password"
npm --prefix ../src-ui exec -- tauri signer generate -- --ci -w "$env:USERPROFILE\.tauri\harmony-updater.key" -p "$env:HARMONY_UPDATER_KEY_PASSWORD"
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
cd apps/desktop/src-ui
npm ci
npm run tauri:build
```

This generates (under `apps/desktop/src-tauri/target/release/bundle/nsis/`):

- `Harmony_<version>_x64-setup.exe`
- `Harmony_<version>_x64-setup.exe.sig`

### 3) Publish release assets + latest.json

From repo root:

```powershell
cd apps/desktop/src-tauri
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/publish-github-update.ps1 `
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
cd apps/desktop/src-tauri
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/release-day.ps1 `
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
cd apps/desktop/src-tauri
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/publish-github-update.ps1 `
  -ReleaseTag "v0.1.0" `
  -DryRun
```

If `latest.json` was previously published with BOM and app update checks fail with a decode error, regenerate and re-upload the asset:

```powershell
cd apps/desktop/src-tauri
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/publish-github-update.ps1 `
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

Current default `apps/desktop/src-tauri/tauri.conf.json` is Windows-oriented (`nsis` target + Windows OpenSSL runtime copy step).  
For macOS packaging, create a macOS-specific Tauri config once, then build with it.

### 1) Create `apps/desktop/src-tauri/tauri.macos.conf.json`

Start by copying `tauri.conf.json`, then make these required changes:

- `build.beforeBuildCommand`:
  - from: `npm --prefix src-ui run build && powershell -NoProfile -ExecutionPolicy Bypass -File src-tauri/scripts/prepare-openssl-runtime.ps1`
  - to: `npm --prefix src-ui run build`
- `bundle.targets`:
  - from: `"nsis"`
  - to: `["app", "dmg"]`
- remove Windows-only bundle resources:
  - delete `bundle.resources`

### 2) Build with the macOS config

From repo root:

```bash
cd apps/desktop/src-tauri
npm --prefix ../src-ui ci
npm --prefix ../src-ui exec -- tauri build --config tauri.macos.conf.json
```

Outputs:

- App bundle: `apps/desktop/src-tauri/target/release/bundle/macos/Harmony.app`
- Installer: `apps/desktop/src-tauri/target/release/bundle/dmg/Harmony_0.1.0_*.dmg`

Notes:

- Local DMG builds are unsigned unless you configure Apple signing/notarization.
- Unsigned apps may show Gatekeeper warnings on other machines.
