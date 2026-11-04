# AGENTS.md

ClipFlow is a Windows-only native desktop app: one Rust crate at the repo root, UI in **GPUI** (`gpui-kit`). The Tauri/React/WebView2 shell is gone — there is no Node, npm, frontend build, or `src-tauri/` directory. `media/*` is the old capture engine (DXGI Desktop Duplication, Media Foundation, WASAPI, ring buffer) kept intact.

## Commands

Run from the repo root. Windows is the shipping target:

```powershell
cargo check --all-targets          # CI step 1
cargo test                         # CI step 2
cargo clippy --all-targets         # CI step 3
cargo check --release              # CI step 4: compiles gpui HLSL shaders
cargo run                          # debug build keeps a console for log output
cargo build --release              # -> target/release/clipflow.exe
```

On macOS/Linux, match CI by appending `--features headless-sim` to **every** command, including single-test runs. The feature forces the synthetic capture backend and platform stubs (it also does so on Windows, which is useful for testing non-Windows code paths).

```bash
cargo test --features headless-sim some_test_name
```

Single test on Windows: `cargo test some_test_name`.

Prerequisites: Rust **1.98+** (the GPUI stack uses `Option::map_or_default`), Visual Studio C++ build tools + Windows SDK. Release builds need `fxc.exe` (HLSL); gpui finds it via the SDK registry, or set `GPUI_FXC_PATH`.

Packaging (version pinned deliberately — the config schema rejects unknown fields):

```powershell
cargo install cargo-packager --version 0.11.8 --locked
cargo packager --release
```

## Architecture

- `src/main.rs` → `clipflow_lib::run()` → `app::run()` in `src/app.rs`. Bootstrap order matters: logging, `AppState`, autostart reconciliation, gpui init/theme, tray + global hotkeys **on the main thread**, window, runtime loops, launch tasks.
- `src/ui/deck.rs` is the single root view; `capture_card`, `library`, `settings`, `theme`, `thumbs` are its components. There is one window.
- `src/state.rs` (`AppState`) replaces the old Tauri commands. Its methods are synchronous and potentially blocking. **Never call them from the render path** — use `cx.background_spawn(...)` and land back with `this.update(...)` + `cx.notify()`, as `src/runtime/*` does.
- `src/runtime/*` holds background loops (4 Hz stats, 2 s foreground poll, ECO, tray/hotkey events). `src/policy/*` is pure, unit-tested UI/session logic (ported from the old Vitest suites).
- Platform cfg convention, used throughout: real code under `#[cfg(all(windows, not(feature = "headless-sim")))]`, stubs under `#[cfg(any(not(windows), feature = "headless-sim"))]`. Preserve this in new platform code.

## Gotchas

- **Never `use gpui_kit::*;` in a module containing `#[test]`** — gpui re-exports a `test` attribute macro that shadows the built-in and crashes rustc. Import names explicitly.
- Tests needing a GPUI `Context` live in `src/ui/deck_tests.rs`, not inline in `deck.rs`; a `#[test]` there overflows the macro recursion budget (`#![recursion_limit = "512"]` exists for this).
- `AppState::new()` loads and saves the real `%APPDATA%\ClipFlow\settings.json`. Tests must construct `AppState` by hand (see `state.rs` tests) and never touch user config.
- Global hotkeys are OS-exclusive per process. Tests drive a recorder trait instead of registering real hotkeys; keep it that way.
- Settings JSON is camelCase and byte-compatible with the old Tauri build — keep field names, defaults, and clamps. Data files: `%APPDATA%\ClipFlow\{settings.json,gallery_state.json,clip_meta.json}`; clips default to `%USERPROFILE%\Videos\ClipFlow`.
- Many features are **deferred but still persisted/inert**: REC HUD, video playback, trimmer/split/PNG/GIF UI, per-game profile editor, STATS tab, onboarding, signed auto-update. The settings fields and engine code exist; do not assume the UI is wired. `CHANGELOG.md` is the authoritative list.
- Keep the release profile (`opt-level = "s"`, `lto`, `panic = "abort"`, `strip`) — it produces the intended single ~6 MB exe.
- Release: bump the `version` in `Cargo.toml` before pushing a `vX.Y.Z` tag, or `release.yml` fails. Installer names and `latest.json` derive from the manifest version.
- `build.rs` embeds the icon via `rc.exe` (missing compiler is only a warning), delay-loads Media Foundation, and must **not** add a second manifest (causes `CVT1100: duplicate resource`; gpui already embeds one).
- Windows FFI splits crate versions: the engine pins `windows 0.58`, UI/tray pull `windows-sys 0.61`. `platform/win32.rs` passes HWNDs as `isize`; don't hand typed HWNDs across that boundary.
- Logging uses `RUST_LOG` via `env_logger`, defaulting to `clipflow=info,warn`. Release builds are `windows_subsystem = "windows"` (no console), so log output is discarded there.

## References

- `README.md` — build/packaging and the macOS/Linux headless workflow.
- `CHANGELOG.md` — Tauri→GPUI migration, remaining deferred features.
- `.github/workflows/ci.yml`, `release.yml` — exact gates and release/version rules.
