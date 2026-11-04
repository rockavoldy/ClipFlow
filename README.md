# ClipFlow

<p align="center">
  <strong>Ultra-Lightweight Instant Replay & Game Clipping Engine for Windows 10/11</strong>
</p>

<p align="center">
  <a href="https://buymeacoffee.com/jaimitus">
    <img src="https://img.shields.io/badge/Buy%20Me%20A%20Coffee-Donate-orange.svg?style=for-the-badge&logo=buy-me-a-coffee" alt="Buy Me A Coffee" />
  </a>
  <img src="https://img.shields.io/badge/Platform-Windows%2010%20%2F%2011-blue?style=for-the-badge&logo=windows" alt="Windows 10/11" />
  <img src="https://img.shields.io/badge/Built%20With-Rust%20%2B%20GPUI-orange?style=for-the-badge&logo=rust" alt="Rust + GPUI" />
  <img src="https://img.shields.io/badge/License-MIT-brightgreen?style=for-the-badge" alt="MIT License" />
</p>

---

![ClipFlow Studio](ClipFlow_UI.png)

---

## ⚡ Overview

**ClipFlow** is a zero-bloat, high-performance Instant Replay and Game Clipping desktop app for Windows 10 and 11. 

Designed for gamers and creators who demand maximum FPS and instant clipping without background resource hogging, ClipFlow continuously records your active display into a rolling memory buffer directly on your GPU. Pressing `Alt + C` instantly flushes your buffer to an MP4 video file in **under 50 milliseconds**.

> ### 🧱 UI rebuilt on GPUI
> The whole shell moved from **Tauri v2 + React/WebView2** to a **native GPUI**
> desktop app: one Rust view tree, no WebView2 runtime, no Node build step, and
> a single self-contained `clipflow.exe`. The capture engine — DXGI Desktop
> Duplication, the Media Foundation hardware encoder, WASAPI loopback and the
> rolling ring buffer — is unchanged.
>
> A few 1.4.x features are **deferred** in this build while the new shell beds
> in: the REC HUD/overlay, video playback, the trimmer (trim / split / PNG
> snapshot / GIF export), the per-game profile editor, the STATS tab, onboarding
> and the signed auto-updater. Their settings and engine code are retained, and
> [CHANGELOG.md](CHANGELOG.md) lists them explicitly so nothing is discovered as
> a surprise.
>
> Full list in [CHANGELOG.md](CHANGELOG.md).

---

## ✨ Key Features

- **⚡ Sub-50ms Instant Clip Flush:** Snapshot your last 30–120 seconds of gameplay to `%USERPROFILE%\Videos\ClipFlow` in under 50 ms. Zero rendering queues or waiting.
- **🎮 Per-Game Capture Settings:** Clips are tagged with the foreground game and, optionally, routed into `Videos/ClipFlow/<game>/`, and the last 30 s can be saved automatically when a game exits focus. *(The profile editor UI is deferred; profiles still seed, persist and apply.)*
- **🏷️ Game-Tagged Clips & Folders:** Every save is tagged with the game in the foreground and (optionally) routed into `Videos/ClipFlow/<game>/`. Auto-save the last 30 s whenever a game exits focus.
- **🔇 Game-Focus Friendly:** Saving with `Alt + C` never steals focus from a fullscreen game — the window stays put and you get a native toast instead. Audio capture retries automatically when a game holds the device in exclusive mode, so clips never come out silent.
- **⌨️ Fully Customisable Hotkeys:** Rebind save and arm/disarm from the UI — click, press, done. Persisted in one JSON file.
- **🎞️ H.264 or HEVC (H.265):** Pick your codec; HEVC cuts storage in half while staying fully hardware-accelerated.
- **🖥️ DXGI Desktop Duplication & Direct3D 11:** Zero-copy GPU surface acquisition at 60–144 FPS with sub-millisecond frame latency.
- **🎯 Native Hardware Encoding MFT:** Leverages Media Foundation hardware encoders (NVIDIA NVENC, AMD AMF, Intel QSV) for near-zero CPU usage.
- **🎙️ Synchronized WASAPI Audio Loopback:** Dual capture for system output audio and microphone, encoded to AAC-LC at 48 kHz stereo on a unified QPC clock.
- **🛡️ 100% Private & Zero Telemetry:** No user account, no login, no cloud upload, no telemetry sockets, and no background updater service. The only network call is a version probe against GitHub Releases (silent at launch, or on demand from Settings) — no analytics, no tracking.
- **🔄 Release Notifications:** Settings → About & Updates probes the Releases feed and opens the download page. *(Signed in-app installation is deferred with the updater.)*
- **💧 Ultra-Low Memory Footprint:** Keeps encoded packets in a circular RAM buffer (<100 MB RAM usage for 60 s history).
- **🔄 Resilient GPU Recovery:** Automatically detects resolution switches, UAC prompts, or driver TDR resets (`DXGI_ERROR_DEVICE_RESET`) and re-arms Desktop Duplication seamlessly.

---

## ⚔️ Why ClipFlow? (Feature Comparison)

ClipFlow was built to eliminate the bloat, mandatory accounts, and high RAM overhead common in modern clipping apps.

| Feature | ⚡ **ClipFlow** | NVIDIA ShadowPlay | Medal.tv | Outplayed / Overwolf | SteelSeries GG | OBS Replay Buffer |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| **Mandatory Account / Login** | ❌ **None (0%)** | ⚠️ Required | ⚠️ Required | ⚠️ Required | ⚠️ Required | ❌ None |
| **RAM Footprint** | ⚡ **< 100 MB** | 🟡 300–600 MB | 🔴 800 MB–1.5 GB | 🔴 1.0–2.0 GB | 🔴 700 MB–1.2 GB | 🟡 400–800 MB |
| **Telemetry / Tracking** | ❌ **Zero** | ⚠️ Yes | ⚠️ Yes | ⚠️ Yes | ⚠️ Yes | ❌ None |
| **Flush Latency (`Alt + C`)** | ⚡ **< 50 ms** | 🟡 ~500 ms | 🔴 2–5 seconds | 🔴 3–10 seconds | 🔴 2–5 seconds | 🟡 ~300 ms |
| **Lossless Stream-Copy Trim** | 🟡 Engine ready, UI deferred | ❌ Separate tool | 🟡 Re-encodes | 🟡 Re-encodes | 🟡 Re-encodes | ❌ None |
| **Single Portable Binary** | ✅ **Yes (.exe)** | ❌ GeForce Exp. | ❌ Heavy Installer | ❌ Overwolf App | ❌ Huge Suite | ❌ Installer |
| **Background Updater Service** | ❌ **None** | ⚠️ Yes | ⚠️ Yes | ⚠️ Yes | ⚠️ Yes | ❌ None |

---

## 🚀 Quick Start

### 1. Download & Run
Choose your preferred flavor from the [Releases](https://github.com/jaimitus/ClipFlow/releases) page:
- **Standalone Portable:** `ClipFlow_v1.4.2_Portable.exe` (Single executable, no installation needed)
- **Setup Installer:** `ClipFlow_1.4.2_x64-setup.exe` (NSIS Installer)
- **MSI Package:** `ClipFlow_1.4.2_x64_en-US.msi` (Windows Installer)

> 💡 All three are the same binary; the installers just add Start-menu shortcuts and the uninstaller.

### 2. Basic Controls
- `Alt + C`: **Save Instant Replay** (Flushes your active rolling buffer to disk).
- `Alt + Shift + C`: **Arm / Disarm Buffer** (Toggles active GPU recording).
- Both hotkeys are **customisable** — Settings → click the badge → press your combo.
- **Updates:** Settings → About & Updates → `Check for updates` compares your version against the newest release and opens the download page.

Saved clips are stored in `%USERPROFILE%\Videos\ClipFlow` (or `Videos\ClipFlow\<game>\` with *Organise by game*).

---

## 🛠️ Building from Source

### Prerequisites
- [Rust](https://www.rust-lang.org/tools/install) **1.98+** — the GPUI stack
  uses `Option::map_or_default`, stabilised in Rust 1.98.0 (the ClipFlow crate
  itself stays on edition 2021)
- C++ Build Tools for Visual Studio with the Windows 10/11 SDK
- No Node.js, no WebView2, no Tauri CLI

### Build Instructions

```powershell
# Clone repository
git clone https://github.com/jaimitus/ClipFlow.git
cd ClipFlow

# Build the release binary
cargo build --release

# Optional: build the installers (NSIS + MSI)
cargo install cargo-packager --locked
cargo packager --release
```

The binary lands in `target/release/clipflow.exe`; the installers land next to
it. `cargo packager` reads `[package.metadata.packager]` in `Cargo.toml`, so the
shipped version can never drift from the crate version.

The deck uses **Segoe UI** and **Consolas**, both shipped with Windows, so
there is nothing to bundle and nothing to download.

Release profile settings (`opt-level = "s"`, `lto`, `panic = "abort"`,
`strip`) are what keep it a single ~6 MB executable — keep them.

### Developing on macOS / Linux

The capture engine has a synthetic backend, and every platform module degrades to
a stub, so the whole app builds and its policy tests run on a non-Windows
machine:

```bash
cargo check --features headless-sim
cargo test  --features headless-sim
```

That path exercises the UI, the settings model and all policy tests, but not
DXGI, Media Foundation, WASAPI, the tray icon or the global hotkeys.

### Windows cross-check from a non-Windows host

`cargo check --target x86_64-pc-windows-msvc` type-checks the real Windows code
paths without linking. It needs `llvm-rc` on `PATH` (from Homebrew's `llvm` on
macOS) for the GPUI Windows manifest resource.

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).

---

<p align="center">
  Developed with ❤️ by <a href="https://github.com/jaimitus">jaimitus</a>. If you find ClipFlow useful, consider <a href="https://buymeacoffee.com/jaimitus">supporting the project</a>!
</p>
