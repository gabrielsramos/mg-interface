use crate::{
    platform::{Viewport, ViewportBounds, ViewportState, WindowsSupervisor},
    recent_projects::RecentProjects,
    runtime::{GameRunner, LaunchSpec},
    theme::{ThemeStore, styles::ThemeStyled},
};
use gpui::{prelude::*, *};
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

struct Launcher {
    picker_pending: bool,
    theme: Entity<ThemeStore>,
    recents: RecentProjects,
    status: String,
}
impl Launcher {
    fn new(theme: Entity<ThemeStore>, cx: &mut Context<Self>) -> Self {
        cx.observe(&theme, |_, _, cx| cx.notify()).detach();
        let directory = std::env::var_os("MG_INTERFACE_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("mg-interface")
            });
        let (recents, warning) = RecentProjects::load(&directory);
        Self {
            picker_pending: false,
            theme,
            recents,
            status: warning
                .unwrap_or_else(|| "Select a standalone MonoGame .csproj to begin.".into()),
        }
    }
    fn load(&mut self, project: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let project = match crate::runtime::validate_project(project) {
            Ok(project) => project,
            Err(e) => {
                self.status = e;
                cx.notify();
                return;
            }
        };
        let bounds = Bounds::centered(None, size(px(1600.), px(880.)), cx);
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| Workspace::new(project.clone(), self.theme.clone(), window, cx))
            },
        ) {
            Ok(_) => {
                if let Err(e) = self.recents.remember(project) {
                    eprintln!("{e}");
                }
                window.remove_window();
            }
            Err(e) => {
                self.status = format!("Could not open workspace: {e}");
                cx.notify();
            }
        }
    }
    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker_pending {
            return;
        }
        self.picker_pending = true;
        // GPUI schedules this dialog after the input callback releases the app
        // borrow. Calling a blocking Win32 dialog here re-enters GPUI's task loop.
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open standalone MonoGame project (.csproj)".into()),
        });
        cx.spawn_in(window, async move |entity, cx| {
            let result = match selection.await {
                Ok(result) => result.map_err(|error| format!("Project picker failed: {error}")),
                Err(error) => Err(format!("Project picker closed unexpectedly: {error}")),
            };
            // A closed launcher is allowed: never retain it just for the dialog.
            let _ = entity.update_in(cx, |view, window, cx| {
                view.picker_pending = false;
                match result {
                    Ok(Some(paths)) => {
                        if let Some(project) = paths.into_iter().next() {
                            view.load(project, window, cx);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => view.status = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
impl Render for Launcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.theme.read(cx);
        let theme = &store.theme;
        div()
            .size_full()
            .flex()
            .flex_col()
            .app_style(theme)
            .when_some(store.error.clone(), |view, error| {
                view.child(
                    div()
                        .small_style(theme)
                        .text_color(rgb(theme.colors.error.0))
                        .child(error),
                )
            })
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .h(px(theme.spacing.launcher_toolbar_height))
                    .border_b_1()
                    .border_color(rgb(theme.colors.secondary.0))
                    .px(px(theme.spacing.page_padding))
                    .flex()
                    .items_center()
                    .gap(px(theme.spacing.gap))
                    .justify_between()
                    .child("MonoGame Interface")
                    .child(
                        div()
                            .id("open-project")
                            .flex_shrink_0()
                            .button_style(theme, true, !self.picker_pending)
                            .cursor_pointer()
                            .on_click(cx.listener(|view, _, window, cx| view.open(window, cx)))
                            .child(if self.picker_pending {
                                "Opening..."
                            } else {
                                "Open Project"
                            }),
                    ),
            )
            .child(
                div()
                    .p(px(theme.spacing.page_padding))
                    .flex()
                    .flex_col()
                    .gap(px(theme.spacing.gap))
                    .flex_1()
                    .min_h(px(0.))
                    .id("recent-projects")
                    .overflow_y_scroll()
                    .child("Recent projects")
                    .child(div().small_style(theme).child(self.status.clone()))
                    .when(self.recents.paths().is_empty(), |view| {
                        view.child("Your recent projects will appear here.")
                    })
                    .children(
                        self.recents
                            .paths()
                            .iter()
                            .enumerate()
                            .map(|(index, project)| {
                                let project = project.clone();
                                let available = project.is_file();
                                div()
                                    .id(("recent", index))
                                    .panel_style(theme)
                                    .cursor_pointer()
                                    .child(project.display().to_string())
                                    .when(!available, |view| view.child(" (unavailable)"))
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.load(project.clone(), window, cx)
                                    }))
                            }),
                    ),
            )
    }
}
struct Workspace {
    theme: Entity<ThemeStore>,
    runner: GameRunner,
    viewport: Viewport,
    bounds: Rc<Cell<Option<ViewportBounds>>>,
    status: String,
    started: Instant,
    attached: Option<Instant>,
    inset: bool,
    smoke: bool,
    smoke_stage: u8,
    smoke_origin_y: Option<i32>,
}

impl Workspace {
    fn new(
        project: PathBuf,
        theme: Entity<ThemeStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&theme, |_, _, cx| cx.notify()).detach();
        let mut runner = GameRunner::default();
        let status = match runner.select(LaunchSpec {
            project,
            args: Vec::new(),
        }) {
            Ok(()) => "Ready. Press Play to launch the selected standalone game.".into(),
            Err(e) => e,
        };
        cx.spawn_in(window, async move |entity, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(30))
                    .await;
                if entity
                    .update_in(cx, |view, window, cx| {
                        view.tick(window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            theme,
            runner,
            viewport: Viewport::default(),
            bounds: Rc::new(Cell::new(None)),
            status,
            started: Instant::now(),
            attached: None,
            inset: false,
            smoke: std::env::var_os("MG_INTERFACE_GPUI_SMOKE_TEST").is_some(),
            smoke_stage: 0,
            smoke_origin_y: None,
        }
    }

    fn play(&mut self) {
        if self.runner.is_active() {
            return;
        }
        match self.runner.play(&WindowsSupervisor) {
            Ok(()) => {
                self.started = Instant::now();
                self.status = "Starting dotnet...".into();
            }
            Err(e) => self.status = e,
        }
    }

    fn stop(&mut self) {
        self.runner.stop();
        self.viewport.clear();
        self.attached = None;
        self.status = "Stopped. Press Play to restart.".into();
    }

    fn poll(&mut self, window: &mut Window) -> Result<(), String> {
        if let Some(status) = self.runner.poll_exit()? {
            self.stop();
            self.status = format!("Game exited ({status}). Press Play to restart.");
        }
        let Some(pid) = self.runner.process_id() else {
            return Ok(());
        };
        let Some(bounds) = self.bounds.get() else {
            return Ok(());
        };
        let host = crate::platform::native_window(window)?;
        crate::platform::prepare_viewport(host)?;
        match self.viewport.poll(pid, host, bounds)? {
            ViewportState::Attached => {
                self.attached = Some(Instant::now());
                self.status = "Running. Click the game to focus input.".into();
                println!(
                    "GPUI viewport attached at {}, {} (1280x720)",
                    bounds.x, bounds.y
                );
            }
            ViewportState::Closed => self.stop(),
            ViewportState::Waiting if self.started.elapsed() > Duration::from_secs(120) => {
                return Err("No game window appeared within 120 seconds".into());
            }
            _ => {}
        }
        Ok(())
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let previous_status = self.status.clone();
        if let Err(e) = self.poll(window) {
            self.stop();
            self.status = e.clone();
            eprintln!("Workspace: {e}");
            if self.smoke {
                std::process::exit(1);
            }
        }
        if self.smoke {
            if self.smoke_stage == 0 {
                assert_eq!(
                    cx.windows().len(),
                    1,
                    "Only the workspace should remain open"
                );
                if self.runner.project().is_none() {
                    eprintln!("No valid smoke project");
                    std::process::exit(1);
                }
                assert!(
                    !self.runner.is_active(),
                    "Loading a project must not start the game"
                );
                self.play();
                self.stop();
                assert!(
                    self.runner.project().is_some(),
                    "Stop must retain the project"
                );
                self.play();
                self.smoke_stage = 1;
            } else if self
                .attached
                .is_some_and(|time| time.elapsed() > Duration::from_secs(2))
            {
                match self.smoke_stage {
                    1 => {
                        self.smoke_origin_y = self.bounds.get().map(|bounds| bounds.y);
                        self.inset = true;
                        if std::env::var_os("MG_INTERFACE_THEME_SMOKE_TEST").is_some() {
                            let path = &self.theme.read(cx).path;
                            assert!(
                                path.starts_with(
                                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target")
                                ),
                                "Theme smoke requires a disposable file under target"
                            );
                            let source = include_str!("../themes/default.toml")
                                .replace("#2563eb", "#ff00ff")
                                .replace("sidebar_width = 240", "sidebar_width = 300");
                            std::fs::write(path, source).expect("Write smoke theme");
                        }
                        self.smoke_stage = 2;
                        self.attached = Some(Instant::now());
                    }
                    2 => {
                        if self.bounds.get().map(|bounds| bounds.y) == self.smoke_origin_y {
                            self.stop();
                            eprintln!("GPUI layout did not move the embedded viewport");
                            std::process::exit(1);
                        }
                        if std::env::var_os("MG_INTERFACE_THEME_SMOKE_TEST").is_some() {
                            let store = self.theme.read(cx);
                            assert_eq!(store.theme.colors.accent.0, 0xff00ff);
                            assert_eq!(store.theme.spacing.sidebar_width, 300.);
                            assert!(store.error.is_none());
                            std::fs::write(&store.path, "[invalid")
                                .expect("Write invalid smoke theme");
                            self.smoke_stage = 4;
                            self.attached = Some(Instant::now());
                        } else {
                            self.stop();
                            self.play();
                            self.smoke_stage = 3;
                        }
                    }
                    4 => {
                        let store = self.theme.read(cx);
                        assert!(store.error.is_some());
                        assert_eq!(store.theme.colors.accent.0, 0xff00ff);
                        std::fs::write(&store.path, include_str!("../themes/default.toml"))
                            .expect("Restore smoke theme");
                        self.smoke_stage = 5;
                        self.attached = Some(Instant::now());
                    }
                    5 => {
                        let store = self.theme.read(cx);
                        assert!(store.error.is_none());
                        assert_eq!(store.theme.colors.accent.0, 0x2563eb);
                        println!(
                            "Live theme smoke passed: file reload, updated color and layout, invalid-file retention, recovery with game still running."
                        );
                        self.stop();
                        self.play();
                        self.smoke_stage = 3;
                    }
                    3 => {
                        assert!(self.runner.is_active());
                        println!(
                            "App smoke passed: launcher, recent selection, idle workspace, startup stop, attach, layout movement, stop, replay, cleanup."
                        );
                        window.remove_window();
                    }
                    _ => {}
                }
            } else if !self.runner.is_active() && self.smoke_stage > 0 {
                eprintln!("Game exited before GPUI smoke completed");
                std::process::exit(1);
            }
        }
        if self.runner.is_active() || self.status != previous_status {
            cx.notify();
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let tracked = self.bounds.clone();
        let project = self
            .runner
            .project()
            .map(|p| p.project.display().to_string())
            .unwrap_or_else(|| "No valid project".into());
        let store = self.theme.read(cx);
        let theme = &store.theme;
        div()
            .size_full()
            .flex()
            .flex_col()
            .app_style(theme)
            .when_some(store.error.clone(), |view, error| {
                view.child(
                    div()
                        .small_style(theme)
                        .text_color(rgb(theme.colors.error.0))
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .gap(px(theme.spacing.toolbar_gap))
                    .items_center()
                    .h(px(theme.spacing.workspace_toolbar_height))
                    .px(px(theme.spacing.workspace_padding))
                    .child(
                        div()
                            .id("play")
                            .button_style(theme, true, !self.runner.is_active())
                            .cursor_pointer()
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.play();
                                cx.notify();
                            }))
                            .child("Play"),
                    )
                    .child(
                        div()
                            .id("stop")
                            .button_style(theme, false, self.runner.is_active())
                            .cursor_pointer()
                            .on_click(cx.listener(|view, _, _, cx| {
                                if view.runner.is_active() {
                                    view.stop();
                                    cx.notify();
                                }
                            }))
                            .child("Stop"),
                    )
                    .child("MonoGame workspace"),
            )
            .child(
                div()
                    .px(px(theme.spacing.workspace_padding))
                    .h(px(theme.spacing.status_height))
                    .small_style(theme)
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap(px(theme.spacing.gap))
                    .px(px(theme.spacing.workspace_padding))
                    .child(
                        div()
                            .w(px(theme.spacing.sidebar_width))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(theme.spacing.gap))
                            .panel_style(theme)
                            .child("Project")
                            .child(div().small_style(theme).child(project))
                            .child(
                                div()
                                    .small_style(theme)
                                    .child("Click the game to focus keyboard and mouse input."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .mt(px(if self.inset { 24. } else { 0. }))
                            .child(
                                div()
                                    .relative()
                                    .bg(rgb(theme.colors.viewport.0))
                                    .child(
                                        canvas(
                                            move |bounds, _, _| {
                                                tracked.set(Some(ViewportBounds {
                                                    x: (f32::from(bounds.origin.x) * scale).round()
                                                        as i32,
                                                    y: (f32::from(bounds.origin.y) * scale).round()
                                                        as i32,
                                                    width: 1280,
                                                    height: 720,
                                                }));
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .w(px(1280. / scale))
                                        .h(px(720. / scale)),
                                    )
                                    .when(!self.runner.is_active(), |area| {
                                        area.child(
                                            div()
                                                .absolute()
                                                .top(px(theme.panels.padding))
                                                .left(px(theme.panels.padding))
                                                .child("Press Play to run the game."),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .mt(px(theme.spacing.gap / 2.))
                                    .small_style(theme)
                                    .child(
                                        "Standalone MonoGame • fixed 1280 × 720 physical pixels",
                                    ),
                            ),
                    ),
            )
    }
}

pub fn run() {
    crate::platform::initialize();
    Application::new().run(|cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let theme = crate::theme::start(cx);
        let bounds = Bounds::centered(None, size(px(900.), px(650.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx: &mut Context<Launcher>| {
                    let mut launcher = Launcher::new(theme, cx);
                    if std::env::var_os("MG_INTERFACE_GPUI_SMOKE_TEST").is_some() {
                        let project = PathBuf::from(
                            std::env::var_os("MG_INTERFACE_SMOKE_PROJECT")
                                .expect("Smoke requires an explicit project"),
                        );
                        cx.spawn_in(window, async move |entity, cx| {
                            entity
                                .update_in(cx, |view: &mut Launcher, window, cx| {
                                    assert_eq!(cx.windows().len(), 1);
                                    view.recents
                                        .remember(project)
                                        .expect("Smoke history write failed");
                                    let recent = view.recents.paths()[0].clone();
                                    view.load(recent, window, cx);
                                })
                                .expect("Smoke launcher disappeared");
                        })
                        .detach();
                        launcher.status = "Smoke test".into();
                    }
                    launcher
                })
            },
        )
        .expect("Could not open project launcher");
        cx.activate(true);
    });
}
