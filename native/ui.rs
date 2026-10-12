use crate::{
    entity_browser::EntityBrowser,
    log_search::fuzzy_match,
    platform::{Viewport, ViewportBounds, ViewportState, WindowsSupervisor},
    recent_projects::RecentProjects,
    runtime::{GameRunner, LaunchSpec, LogEntry, PlayMode},
    search_input::{SearchChanged, SearchInput},
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
        let store = self.theme.read(cx);
        let spacing = &store.theme.spacing;
        let scale = window.scale_factor();
        let width = 1280. / scale
            + 8.
            + spacing.sidebar_width
            + spacing.gap
            + 2. * spacing.workspace_padding;
        let height = 720. / scale + 48. + 180. + spacing.gap + 2. * spacing.workspace_padding;
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
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
                    .border_b_4()
                    .border_color(rgb(theme.colors.muted.0))
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
#[derive(Clone, Copy, PartialEq)]
enum ScrollSection {
    Console,
    Hierarchy,
    Properties,
}

struct Workspace {
    theme: Entity<ThemeStore>,
    runner: GameRunner,
    viewport: Viewport,
    bounds: Rc<Cell<Option<ViewportBounds>>>,
    status: String,
    console: Vec<LogEntry>,
    entities: EntityBrowser,
    search: Entity<SearchInput>,
    console_scroll: ScrollHandle,
    hierarchy_scroll: ScrollHandle,
    properties_scroll: ScrollHandle,
    scroll_drag: Option<(ScrollSection, f32)>,
    started: Instant,
}

impl Workspace {
    fn new(
        project: PathBuf,
        theme: Entity<ThemeStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&theme, |_, _, cx| cx.notify()).detach();
        let search = cx.new(|cx| SearchInput::new(theme.clone(), cx));
        cx.subscribe(&search, |view, _, _: &SearchChanged, cx| {
            view.console_scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
        })
        .detach();
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
            console: Vec::new(),
            entities: EntityBrowser::default(),
            search,
            console_scroll: ScrollHandle::new(),
            hierarchy_scroll: ScrollHandle::new(),
            properties_scroll: ScrollHandle::new(),
            scroll_drag: None,
            started: Instant::now(),
        }
    }

    fn play(&mut self, mode: PlayMode) {
        if self.runner.is_active() {
            return;
        }
        self.entities = EntityBrowser::default();
        self.hierarchy_scroll.set_offset(point(px(0.), px(0.)));
        self.properties_scroll.set_offset(point(px(0.), px(0.)));
        self.scroll_drag = None;
        self.console.clear();
        self.console_scroll.set_offset(point(px(0.), px(0.)));
        match self.runner.play(&WindowsSupervisor, mode) {
            Ok(()) => {
                self.started = Instant::now();
                self.status = if mode == PlayMode::HotReload {
                    "Starting dotnet watch (hot reload)..."
                } else {
                    "Starting dotnet..."
                }
                .into();
                self.runner.log.push(self.status.clone());
            }
            Err(e) => {
                eprintln!("Workspace: {e}");
                self.runner.log.push(format!("Error: {e}"));
                self.status = e;
            }
        }
    }

    fn stop(&mut self) {
        self.runner.stop();
        self.entities = EntityBrowser::default();
        self.hierarchy_scroll.set_offset(point(px(0.), px(0.)));
        self.properties_scroll.set_offset(point(px(0.), px(0.)));
        self.scroll_drag = None;
        self.console.clear();
        self.console_scroll.set_offset(point(px(0.), px(0.)));
        self.viewport.clear();
        self.status = "Stopped. Press Play to restart.".into();
    }

    fn poll(&mut self, window: &mut Window) -> Result<(), String> {
        if let Some(status) = self.runner.poll_exit()? {
            self.stop();
            self.status = format!("Game exited ({status}). Press Play to restart.");
            println!("Game exited ({status}).");
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
                self.runner.log.push("Game viewport attached.");
                self.status = if self.runner.is_hot_reload() {
                    "Running with hot reload. Click the game to focus input."
                } else {
                    "Running. Click the game to focus input."
                }
                .into();
                println!(
                    "GPUI viewport attached at {}, {} (1280x720)",
                    bounds.x, bounds.y
                );
            }
            ViewportState::Closed if self.runner.is_hot_reload() => {
                self.viewport.clear();
                self.started = Instant::now();
                self.status = "Hot reload: waiting for the game window...".into();
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
            self.runner.log.push(format!("Error: {e}"));
            eprintln!("Workspace: {e}");
        }
        self.entities.update(self.runner.inspection_state());
        self.runner.select_entity(self.entities.selected.as_deref());
        if let Some(output) = self.runner.log.take_changed() {
            self.console = output;
            self.console_scroll.scroll_to_bottom();
            cx.notify();
        }
        if self.runner.is_active() || self.status != previous_status {
            cx.notify();
        }
    }
}

impl Workspace {
    fn entities_panel(&self, cx: &mut Context<Self>) -> Div {
        let hierarchy_scrollbar = self.scrollbar(ScrollSection::Hierarchy, cx);
        let properties_scrollbar = self.scrollbar(ScrollSection::Properties, cx);
        let store = self.theme.read(cx);
        let theme = &store.theme;
        let mut tree = div()
            .id("entity-tree")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .min_h(px(0.))
            .track_scroll(&self.hierarchy_scroll)
            .overflow_y_scroll();
        for (ui, heading) in [(false, "World"), (true, "UI")] {
            tree = tree.child(
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .text_color(rgb(theme.colors.muted.0))
                    .child(heading),
            );
            for row in self.entities.rows(ui) {
                let selected = self.entities.selected.as_deref() == Some(&row.id);
                let select_id = row.id.clone();
                let toggle_id = row.id.clone();
                tree = tree.child(
                    div()
                        .id(SharedString::from(format!("entity-{}", row.id)))
                        .flex()
                        .items_center()
                        .min_w(px(0.))
                        .w_full()
                        .border_1()
                        .border_color(rgb(theme.colors.secondary.0))
                        .py(px(4.))
                        .pr(px(8.))
                        .pl(px(12. + (row.depth.min(12) * 12) as f32))
                        .when(selected, |view| view.bg(rgb(theme.colors.accent.0)))
                        .child(
                            div()
                                .id(SharedString::from(format!("expand-{}", row.id)))
                                .w(px(20.))
                                .flex_shrink_0()
                                .cursor_pointer()
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    if row.has_children {
                                        view.entities.toggle(&toggle_id);
                                        cx.notify();
                                    }
                                }))
                                .child(if !row.has_children {
                                    "·"
                                } else if row.collapsed {
                                    "▸"
                                } else {
                                    "▾"
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("select-{}", select_id)))
                                .flex_1()
                                .min_w(px(0.))
                                .cursor_pointer()
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.properties_scroll.set_offset(point(px(0.), px(0.)));
                                    view.entities.selected = Some(select_id.clone());
                                    view.runner.select_entity(Some(&select_id));
                                    cx.notify();
                                }))
                                .child(row.label),
                        ),
                );
            }
        }
        let properties = self.entities.properties();
        let inspector = div()
            .id("entity-properties")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .min_h(px(0.))
            .track_scroll(&self.properties_scroll)
            .overflow_y_scroll()
            .children(properties.into_iter().map(|(label, value)| {
                div()
                    .w_full()
                    .px(px(12.))
                    .py(px(5.))
                    .border_1()
                    .border_color(rgb(theme.colors.secondary.0))
                    .child(div().text_color(rgb(theme.colors.muted.0)).child(label))
                    .child(div().text_color(rgb(theme.colors.text.0)).child(value))
            }));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .small_style(theme)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(tree)
                    .child(hierarchy_scrollbar),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .border_t_4()
                    .border_color(rgb(theme.colors.muted.0))
                    .child(inspector)
                    .child(properties_scrollbar),
            )
    }

    fn scrollbar(
        &self,
        section: ScrollSection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let scroll = match section {
            ScrollSection::Console => self.console_scroll.clone(),
            ScrollSection::Hierarchy => self.hierarchy_scroll.clone(),
            ScrollSection::Properties => self.properties_scroll.clone(),
        };
        let entity = cx.entity();
        let store = self.theme.read(cx);
        let track_color = rgb(store.theme.colors.viewport.0);
        let thumb_color = rgb(store.theme.colors.muted.0);
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let height = f32::from(bounds.size.height);
                let max = f32::from(scroll.max_offset().height);
                let thumb_height = (height * height / (height + max).max(1.))
                    .max(24.)
                    .min(height);
                let travel = (height - thumb_height).max(0.);
                let offset = if max > 0. {
                    -f32::from(scroll.offset().y) / max * travel
                } else {
                    0.
                };
                let thumb = Bounds::new(
                    point(
                        bounds.origin.x + px(2.),
                        bounds.origin.y + px(offset.clamp(0., travel)),
                    ),
                    size(bounds.size.width - px(4.), px(thumb_height)),
                );
                window.paint_quad(fill(bounds, track_color));
                window.paint_quad(fill(thumb, thumb_color));
                window.on_mouse_event({
                    let entity = entity.clone();
                    let scroll = scroll.clone();
                    move |event: &MouseDownEvent, _, _, cx| {
                        if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                            return;
                        }
                        let grab = if thumb.contains(&event.position) {
                            f32::from(event.position.y - thumb.origin.y)
                        } else {
                            thumb_height / 2.
                        };
                        let fraction = ((f32::from(event.position.y - bounds.origin.y) - grab)
                            / travel.max(1.))
                        .clamp(0., 1.);
                        scroll.set_offset(point(px(0.), px(-fraction * max)));
                        entity.update(cx, |view, cx| {
                            view.scroll_drag = Some((section, grab));
                            cx.notify();
                        });
                    }
                });
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |_: &MouseUpEvent, _, _, cx| {
                        entity.update(cx, |view, _| view.scroll_drag = None);
                    }
                });
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                    if !event.dragging() {
                        return;
                    }
                    entity.update(cx, |view, cx| {
                        if let Some((_, grab)) =
                            view.scroll_drag.filter(|(active, _)| *active == section)
                        {
                            let fraction = ((f32::from(event.position.y - bounds.origin.y) - grab)
                                / travel.max(1.))
                            .clamp(0., 1.);
                            scroll.set_offset(point(px(0.), px(-fraction * max)));
                            cx.notify();
                        }
                    });
                });
            },
        )
        .w(px(12.))
        .h_full()
        .flex_shrink_0()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entities_panel = self.entities_panel(cx);
        let scale = window.scale_factor();
        let tracked = self.bounds.clone();
        let query = self.search.read(cx).query().to_owned();
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
                    .gap(px(theme.spacing.gap))
                    .p(px(theme.spacing.workspace_padding))
                    .child(
                        div()
                            .w(px(theme.spacing.sidebar_width))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(theme.spacing.gap))
                            .panel_style(theme)
                            .p(px(0.))
                            .overflow_hidden()
                            .h(px(720. / scale + 240.))
                            .child(entities_panel),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_shrink_0()
                            .w(px(1280. / scale + 8.))
                            .gap(px(theme.spacing.gap))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_shrink_0()
                                    .panel_style(theme)
                                    .p(px(0.))
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(theme.spacing.toolbar_gap))
                                            .items_center()
                                            .justify_center()
                                            .h(px(40.))
                                            .flex_shrink_0()
                                            .px(px(theme.spacing.workspace_padding))
                                            .child(
                                                div()
                                                    .id("play")
                                                    .button_style(
                                                        theme,
                                                        self.runner.is_active()
                                                            && !self.runner.is_hot_reload(),
                                                        true,
                                                    )
                                                    .p(px(0.))
                                                    .size(px(28.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(14.))
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(|view, _, _, cx| {
                                                        view.play(PlayMode::Normal);
                                                        cx.notify();
                                                    }))
                                                    .child("▶"),
                                            )
                                            .child(
                                                div()
                                                    .id("play-hot-reload")
                                                    .button_style(
                                                        theme,
                                                        self.runner.is_hot_reload(),
                                                        true,
                                                    )
                                                    .p(px(0.))
                                                    .size(px(28.))
                                                    .relative()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(|view, _, _, cx| {
                                                        view.play(PlayMode::HotReload);
                                                        cx.notify();
                                                    }))
                                                    .child(
                                                        div()
                                                            .absolute()
                                                            .left(px(0.))
                                                            .top(px(-3.))
                                                            .size(px(28.))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .text_size(px(34.))
                                                            .child("\u{27f3}"),
                                                    )
                                                    .child(
                                                        div().text_size(px(10.)).child("\u{25b6}"),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .id("stop")
                                                    .button_style(theme, false, true)
                                                    .p(px(0.))
                                                    .size(px(28.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(14.))
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(|view, _, _, cx| {
                                                        if view.runner.is_active() {
                                                            view.stop();
                                                            cx.notify();
                                                        }
                                                    }))
                                                    .child("■"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .relative()
                                            .p(px(2.))
                                            .bg(rgb(if self.runner.is_hot_reload() {
                                                theme.colors.accent.0
                                            } else {
                                                theme.colors.viewport.0
                                            }))
                                            .child(
                                                canvas(
                                                    move |bounds, _, _| {
                                                        tracked.set(Some(ViewportBounds {
                                                            x: (f32::from(bounds.origin.x) * scale)
                                                                .round()
                                                                as i32,
                                                            y: (f32::from(bounds.origin.y) * scale)
                                                                .round()
                                                                as i32,
                                                            width: 1280,
                                                            height: 720,
                                                        }));
                                                    },
                                                    |_, _, _, _| {},
                                                )
                                                .w(px(1280. / scale))
                                                .h(px(720. / scale)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(180.))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .panel_style(theme)
                                    .p(px(12.))
                                    .gap(px(8.))
                                    .child(self.search.clone())
                                    .child(
                                        div()
                                            .flex()
                                            .flex_1()
                                            .min_h(px(0.))
                                            .gap(px(8.))
                                            .child(
                                                div()
                                                    .id("console-output")
                                                    .flex_1()
                                                    .min_w(px(0.))
                                                    .h_full()
                                                    .overflow_y_scroll()
                                                    .track_scroll(&self.console_scroll)
                                                    .font_family("Consolas")
                                                    .text_size(px(theme.typography.small_font_size))
                                                    .children(
                                                        self.console
                                                            .iter()
                                                            .filter(|entry| {
                                                                fuzzy_match(
                                                                    &query,
                                                                    &format!(
                                                                        "{} {}",
                                                                        entry.timestamp,
                                                                        entry.message
                                                                    ),
                                                                )
                                                            })
                                                            .map(|entry| {
                                                                div()
                                                                    .flex()
                                                                    .w_full()
                                                                    .gap(px(12.))
                                                                    .py(px(6.))
                                                                    .border_b_1()
                                                                    .border_color(rgb(theme
                                                                        .colors
                                                                        .secondary
                                                                        .0))
                                                                    .child(
                                                                        div()
                                                                            .w(px(72.))
                                                                            .flex_shrink_0()
                                                                            .text_color(rgb(theme
                                                                                .colors
                                                                                .muted
                                                                                .0))
                                                                            .child(
                                                                                entry
                                                                                    .timestamp
                                                                                    .clone(),
                                                                            ),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .min_w(px(0.))
                                                                            .child(
                                                                                entry
                                                                                    .message
                                                                                    .clone(),
                                                                            ),
                                                                    )
                                                            }),
                                                    ),
                                            )
                                            .child(self.scrollbar(ScrollSection::Console, cx)),
                                    ),
                            ),
                    ),
            )
    }
}

pub fn run() {
    crate::platform::initialize();
    Application::new().run(|cx: &mut App| {
        crate::platform::install_pointer_cleanup(cx);
        crate::search_input::bind_keys(cx);
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
            |_, cx| cx.new(|cx| Launcher::new(theme, cx)),
        )
        .expect("Could not open project launcher");
        cx.activate(true);
    });
    crate::platform::restore_pointer();
}
