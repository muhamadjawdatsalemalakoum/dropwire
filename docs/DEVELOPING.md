# Developing Dropwire

## Layout

```
core/        irohcore — the transfer engine (only crate that imports iroh / iroh-blobs)
src-tauri/   the desktop app shell (Tauri v2): commands, window, config, icons
ui/          the app frontend (vanilla HTML/CSS/JS; loaded by Tauri as frontendDist)
infra/       OPTIONAL self-hosted relay + DNS (not needed — the app is serverless by default)
www/         marketing landing page (static)
docs/        PRIVACY.md, this file, and other docs
```

The golden rule: **`iroh` / `iroh-blobs` types never appear outside `core/`.** The shell and UI
speak only `irohcore`'s stable API (`Core`, `Progress`, `CoreConfig`).

## Prerequisites

- **Rust** (stable) — https://rustup.rs
- **A C toolchain** for the native crypto/QUIC deps:
  - **Windows:** Visual Studio Build Tools with the *Desktop development with C++* workload, plus
    WebView2 (ships with Windows 11). Build from a shell that has the MSVC env loaded — either the
    "x64 Native Tools" prompt, or import `vcvars64.bat` before running cargo (see below).
  - **macOS:** Xcode command line tools.
  - **Linux:** `webkit2gtk` + `libsoup` dev packages (see Tauri's Linux prerequisites) and a C compiler.
- **Node** is *not* required — the UI is plain HTML/CSS/JS with no build step.

### Windows: loading the MSVC environment for cargo

```powershell
$vcvars = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cmd /c "`"$vcvars`" && set" | ForEach-Object { if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($matches[1])" -Value $matches[2] } }
# now `cargo` can find link.exe and the Windows SDK libs
```

## Engine (fast inner loop)

```sh
cargo test  -p irohcore                              # roundtrip (file + folder) tests
cargo test  -p irohcore -- --ignored resume_after_interrupt   # the 64 MB interrupt+resume test
cargo clippy -p irohcore --all-targets -- -D warnings
cargo fmt --all
```

The engine tests use `Infra::LocalOnly` (no relay/discovery) so they're hermetic and network-free.

## Running the desktop app

```sh
cargo run -p dropwire        # builds the shell + engine and opens the window
```

No dev server is needed (the UI is static and loaded from `../ui`). The app starts with the
serverless config: Mainline-DHT discovery + n0's free public relay fallback.

## Building installers (M5)

Requires the Tauri CLI:

```sh
cargo install tauri-cli --version "^2"
cargo tauri build           # produces platform installers under target/release/bundle/
```

Code signing (Windows cert, Apple Developer ID + notarization) is a release-time step — see the
release checklist. Unsigned local installers build fine for testing.

## Regenerating app icons

Icons in `src-tauri/icons/` are derived from `branding/icon.svg` (the lime "wire" mark on Wire
Black). To regenerate from a 1024px PNG source: `cargo tauri icon path/to/icon.png`.
