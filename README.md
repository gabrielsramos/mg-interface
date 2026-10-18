# MG Interface

A Rust application using GPUI 0.2.2 to host a standalone MonoGame game on Windows.

## Run

Requires Rust's MSVC toolchain, Visual Studio C++ build tools, and the game's .NET SDK and MonoGame content tools.

```powershell
cargo run
```

The app opens the project launcher with no project selected. Choose **Open Project** at the top or click a recent project. A separate workspace opens with the game stopped. Press **Play** to build and run it; **Play with Hot Reload** (the triangle inside a refresh ring) launches `dotnet watch` and marks the viewport with the theme accent border. Supported edits apply while the game runs; edits requiring a restart automatically restart and reattach the game window. **Stop** terminates the launched process tree, including during startup. Stopping or a game exit leaves the workspace and selected project open. Closing the workspace cleans up the game. Play and Stop appear only in the workspace.

Open Project uses GPUI's asynchronous native file picker; choose the game's `.csproj`. Cancelling keeps the launcher open, and invalid selections show a validation error.

The ten most recent projects persist in `%APPDATA%\mg-interface\recent-projects.json`. Missing projects are marked unavailable; invalid selections leave the launcher open. The Console panel below the game viewport displays build output, game stdout/stderr, and lifecycle messages. The search field filters messages and timestamps as you type using case-insensitive fuzzy matching; clear it or press Escape to show all logs. Each message has a local HH:MM:SS timestamp on the left and a separator line; a draggable scrollbar appears on the right. It follows new output and retains the latest 200 messages during a run, clears when the game stops, and starts fresh on Play; process output is also mirrored to the terminal. Stop is termination; pause is not implemented.

The game remains an independent MonoGame project. The shared runner prepares `dotnet run --project <project> --configuration Debug`, using the project directory as its working directory. No game-side integration is needed:

```powershell
dotnet run --project C:\path\to\your\game.csproj
```

## Structure

- `native/theme.rs`: configurable appearance, validation and shared style helpers.
- `themes/default.toml`: editable theme with live reloading.
- `native/ui.rs`: GPUI launcher, recent-project list, workspace layout and playback controls.
- `native/debug_windows.rs`: debug-window catalog and selection state.
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

For manual verification, check game rendering and input, selection highlighting, scrolling, minimize/restore, Stop/replay, and closing the workspace during startup or playback.

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

## Scene inspection

An optional mgcore scene bridge now sends snapshots over a named pipe. The workspace sidebar shows a live World/UI entity tree and read-only properties. Click a name to select it and an arrow to collapse its children. Selection survives updates and clears on removal, Stop, or disconnect. See [SCENE_INSPECTION.md](SCENE_INSPECTION.md) for game adaptation and protocol.

Games launched with `mgcore.GameHost.Run(game)` can also define custom debug windows with labels, buttons, separators, and checkboxes. Play automatically discovers window classes in the game assembly and builds their schemas once the game connects. The right sidebar has a window dropdown; button and checkbox callbacks run on the game thread. Checkbox changes return the confirmed value without rebuilding windows. Replay the game to rebuild windows, label snapshots, and context after changes. The existing entity inspector stays on the left, and the game keeps its fixed viewport size. See [SCENE_INSPECTION.md](SCENE_INSPECTION.md#debug-windows) for the API and protocol.
