# MG Interface

A Rust application using GPUI 0.2.2 to host a standalone MonoGame game on Windows.

## Run

Requires Rust's MSVC toolchain, Visual Studio C++ build tools, and the game's .NET SDK and MonoGame content tools.

```powershell
cargo run
```

The app opens the project launcher with no project selected. Choose **Open Project** at the top or click a recent project. A separate workspace opens with the game stopped. Press **Play** to build and run it; **Stop** terminates the launched process tree, including during startup. Stopping or a game exit leaves the workspace and selected project open. Closing the workspace cleans up the game. Play and Stop appear only in the workspace.

Open Project uses GPUI's asynchronous native file picker; choose the game's `.csproj`. Cancelling keeps the launcher open, and invalid selections show a validation error.

The ten most recent projects persist in `%APPDATA%\mg-interface\recent-projects.json`. Missing projects are marked unavailable; invalid selections leave the launcher open. Logs appear in the terminal. Stop is termination; pause is not implemented.

The game remains an independent MonoGame project. The shared runner prepares `dotnet run --project <project> --configuration Debug`, using the project directory as its working directory. No game-side integration is needed:

```powershell
dotnet run --project C:\path\to\your\game.csproj
```

## Structure

- `native/theme.rs`: configurable appearance, validation and shared style helpers.
- `themes/default.toml`: editable theme with live reloading.
- `native/ui.rs`: GPUI launcher, recent-project list, workspace layout and playback controls.
- `native/runtime.rs`: shared project validation, dotnet command preparation, process ownership and lifecycle. The process supervisor receives a prepared `Command`.
- `native/recent_projects.rs`: platform-independent history persistence.
- `native/platform/windows.rs`: GPUI window handle and renderer compatibility setup.
- `native/platform/windows/process.rs`: generic Windows process supervision and Job Object cleanup.
- `native/platform/windows/viewport.rs`: game-window discovery, embedding and positioning using bounds from GPUI layout.

The UI and game runner are separate from Windows integration. A future platform adapter must provide native dialogs, process supervision and viewport hosting; it can reuse the dotnet runner, history and GPUI views. Other platforms currently report unsupported hosting.

Windows initialization enables the process-local `GPUI_DISABLE_DIRECT_COMPOSITION` setting before GPUI starts. GPUI's default topmost DirectComposition surface covers native child windows; its HWND swap-chain renderer allows the embedded game to show.

The viewport remains fixed at **1280 x 720 physical pixels**. GPUI layout coordinates are converted using the window's scale factor. Small windows can clip the game. Docking, variable viewport size, multiple game windows and overlays over the native child are not implemented.

## Verification

```powershell
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

Run the actual app's automated lifecycle check with an explicit project and isolated history:

```powershell
$env:MG_INTERFACE_GPUI_SMOKE_TEST = '1'
$env:MG_INTERFACE_SMOKE_PROJECT = (Resolve-Path C:\path\to\your\game.csproj).Path
$env:MG_INTERFACE_STATE_DIR = Join-Path $PWD 'target\gpui-app-smoke-state'
cargo run
Remove-Item Env:\MG_INTERFACE_GPUI_SMOKE_TEST
Remove-Item Env:\MG_INTERFACE_SMOKE_PROJECT
Remove-Item Env:\MG_INTERFACE_STATE_DIR
```

This checks the launcher, recent selection, separate idle workspace, Stop during startup, native attachment and size, layout movement, Stop and replay, then cleanup. Pixel rendering and mouse/keyboard focus still require manual inspection. Check game rendering and input, minimize/restore and closing the workspace during startup and playback.

A GPUI diagnostic executable is also included:

```powershell
cargo run --features gpui-trial --bin mg-interface-gpui-trial -- C:\path\to\your\game.csproj
```

## Live themes

Edit `themes/default.toml` while the app is running. The launcher and workspace reload colors, fonts, button/panel styles and spacing within about 250 ms. No rebuild or game restart is needed. The native game keeps its fixed backbuffer; theme spacing changes reposition its viewport.

Use `#RRGGBB` colors and numeric sizes in GPUI logical pixels. All fields in the example are required; unknown keys and invalid sizes are rejected. Invalid or missing files display a theme error and retain the last valid appearance (built-in defaults at startup). Saving a valid file clears the error automatically.

To use another file, set its path before launching:

```powershell
$env:MG_INTERFACE_THEME = (Resolve-Path .\themes\default.toml).Path
cargo run
```

Without this override, the app uses `themes/default.toml` in the source checkout, independent of the working directory. The terminal prints the watched path. For a copied executable, ship a theme file beside it and set `MG_INTERFACE_THEME` to that file.

`native/theme.rs` owns parsing, validation, the shared theme store and reusable style helpers. Views consume these helpers; game lifecycle logic does not depend on theme values. The app polls file contents on a background executor, detecting normal saves and atomic file replacements.

For a live-reload integration check, copy the theme under `target`, set `MG_INTERFACE_THEME` to that disposable copy, and set `MG_INTERFACE_THEME_SMOKE_TEST=1` alongside the lifecycle smoke variables above. The check edits that copy, verifies changed colors and layout during playback, rejects an invalid save, restores the theme, and exercises Stop/replay and cleanup.
