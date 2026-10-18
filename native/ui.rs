use crate::{
    debug_windows::DebugWindowBrowser,
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
            + 2. * spacing.sidebar_width
            + 2. * spacing.gap
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
    windows_revision: Option<u64>,
    debug_windows: DebugWindowBrowser,
    windows_dropdown_open: bool,
    debug_window_scroll: ScrollHandle,
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
            windows_revision: None,
            debug_windows: DebugWindowBrowser::default(),
            windows_dropdown_open: false,
            debug_window_scroll: ScrollHandle::new(),
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
        self.reset_debug_windows();
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
        self.reset_debug_windows();
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
        let inspection = self.runner.inspection_state();
        if let Some(catalog) = &inspection.windows {
            if self.windows_revision != Some(catalog.revision) {
                self.windows_revision = Some(catalog.revision);
                let titles = catalog
                    .windows
                    .iter()
                    .map(|window| window.title.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.runner.log.push(format!(
                    "Debug windows loaded: {} found{}",
                    catalog.windows.len(),
                    if titles.is_empty() {
                        String::new()
                    } else {
                        format!(" ({titles})")
                    }
                ));
                for error in &catalog.errors {
                    self.runner.log.push(format!("Debug window error: {error}"));
                }
            }
        } else {
            self.windows_revision = None;
        }
        let previous_selection = self.debug_windows.selected.clone();
        self.debug_windows
            .update(inspection.windows.clone().filter(|_| inspection.connected));
        if self.debug_windows.selected != previous_selection {
            self.debug_window_scroll.set_offset(point(px(0.), px(0.)));
            self.windows_dropdown_open = false;
        }
        if !inspection.connected {
            self.windows_dropdown_open = false;
        }
        self.entities.update(inspection);
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
    fn reset_debug_windows(&mut self) {
        self.windows_revision = None;
        self.debug_windows.update(None);
        self.windows_dropdown_open = false;
        self.debug_window_scroll.set_offset(point(px(0.), px(0.)));
    }

    fn debug_element(
        &self,
        element: &crate::runtime::inspection::DebugElement,
        scope: (&str, u64),
        inspection: &crate::runtime::inspection::InspectionState,
        theme: &crate::theme::Theme,
        cx: &Context<Self>,
    ) -> Div {
        let (window_id, revision) = scope;
        let mut view = match element.element_type.as_str() {
            "row" => {
                let mut row = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(element.gap.unwrap_or(8.).clamp(0., 256.)));
                for child in element.children.as_deref().unwrap_or_default() {
                    row = row.child(self.debug_element(child, scope, inspection, theme, cx));
                }
                row
            }
            "space" => div().size(px(element.size.unwrap_or(8.).clamp(0., 4096.))),
            "separator" => div().w_full().h(px(1.)).bg(rgb(theme.colors.secondary.0)),
            "label" => div().child(element.text.clone()),
            "checkbox" => {
                let window_id = window_id.to_owned();
                let control_id = element.id.clone();
                let value = element.value.unwrap_or(false);
                let enabled = inspection.connected
                    && !inspection.checkbox_waiting()
                    && element.value.is_some();
                // Only the checkbox indicator and label are interactive; no button surface.
                div().child(
                    div()
                        .id(SharedString::from(format!(
                            "debug-checkbox-{window_id}-{control_id}"
                        )))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .py(px(2.))
                        .when(enabled, |view| view.cursor_pointer())
                        .when(!enabled, |view| view.opacity(0.5))
                        .child(
                            div()
                                .size(px(16.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .overflow_hidden()
                                .border_1()
                                .rounded(px(2.))
                                .border_color(rgb(if value {
                                    theme.colors.accent.0
                                } else {
                                    theme.colors.muted.0
                                }))
                                .text_size(px(12.))
                                .text_color(rgb(if value {
                                    theme.colors.accent.0
                                } else {
                                    theme.colors.muted.0
                                }))
                                .child(if value { "✓" } else { "" }),
                        )
                        .child(element.text.clone())
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if enabled
                                && !view.runner.set_debug_checkbox(
                                    revision,
                                    &window_id,
                                    &control_id,
                                    !value,
                                )
                            {
                                view.runner.log.push(
                                    "Debug checkbox unavailable: wait for the game or replay it.",
                                );
                            }
                            cx.notify();
                        })),
                )
            }
            "button" => {
                let window_id = window_id.to_owned();
                let control_id = element.id.clone();
                div().child(
                    div()
                        .id(SharedString::from(format!(
                            "debug-button-{window_id}-{control_id}"
                        )))
                        .button_style(theme, false, inspection.connected)
                        .child(element.text.clone())
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if !view
                                .runner
                                .invoke_debug_button(revision, &window_id, &control_id)
                            {
                                view.runner.log.push(
                                    "Debug button unavailable: replay the game and try again.",
                                );
                            }
                            cx.notify();
                        })),
                )
            }
            _ => div()
                .small_style(theme)
                .child(format!("Unsupported control: {}", element.element_type)),
        };
        if let Some(size) = element
            .font_size
            .filter(|size| size.is_finite() && (1. ..=128.).contains(size))
        {
            view = view.text_size(px(size));
        }
        if let Some(color) = element.color.as_deref().and_then(|color| {
            color
                .strip_prefix('#')
                .filter(|hex| hex.len() == 6)
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        }) {
            view = view.text_color(rgb(color));
        }
        view.flex_shrink_0()
    }
    fn debug_windows_panel(&self, scale: f32, cx: &mut Context<Self>) -> Div {
        let inspection = self.runner.inspection_state();
        let connected = inspection.connected;
        let store = self.theme.read(cx);
        let theme = &store.theme;
        let selected_title = self
            .debug_windows
            .selected_window()
            .map(|window| window.title.clone())
            .unwrap_or_else(|| "Select a window".into());
        let has_windows = self
            .debug_windows
            .catalog
            .as_ref()
            .is_some_and(|catalog| !catalog.windows.is_empty());
        let mut header = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(8.))
            .child(div().small_style(theme).child("Debug windows"))
            .child(
                div().flex().gap(px(6.)).items_center().child(
                    div()
                        .id("debug-window-selector")
                        .flex_1()
                        .min_w(px(0.))
                        .button_style(theme, false, connected && has_windows)
                        .child(format!("{selected_title} ▾"))
                        .on_click(cx.listener(|view, _, _, cx| {
                            if view.debug_windows.selected_window().is_some() {
                                view.windows_dropdown_open = !view.windows_dropdown_open;
                                cx.notify();
                            }
                        })),
                ),
            );
        if self.windows_dropdown_open {
            let mut choices = div()
                .id("debug-window-options")
                .max_h(px(220.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(4.));
            if let Some(catalog) = &self.debug_windows.catalog {
                for window in &catalog.windows {
                    let id = window.id.clone();
                    choices = choices.child(
                        div()
                            .id(SharedString::from(format!("window-option-{id}")))
                            .flex_shrink_0()
                            .button_style(
                                theme,
                                self.debug_windows.selected.as_ref() == Some(&id),
                                true,
                            )
                            .child(window.title.clone())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.debug_windows.selected = Some(id.clone());
                                view.windows_dropdown_open = false;
                                view.debug_window_scroll.set_offset(point(px(0.), px(0.)));
                                cx.notify();
                            })),
                    );
                }
            }
            header = header.child(choices);
        }
        let mut content = div()
            .id("debug-window-canvas")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.debug_window_scroll)
            .flex()
            .flex_col()
            .gap(px(10.));
        if let (Some(catalog), Some(window)) = (
            &self.debug_windows.catalog,
            self.debug_windows.selected_window(),
        ) {
            for element in &window.elements {
                content = content.child(self.debug_element(
                    element,
                    (&window.id, catalog.revision),
                    &inspection,
                    theme,
                    cx,
                ));
            }
        } else {
            content = content.child(div().small_style(theme).child(if !connected {
                "Waiting for the game connection."
            } else if self.debug_windows.catalog.is_some() {
                "No debug windows found."
            } else {
                "Loading debug windows..."
            }));
        }
        if let Some(catalog) = &self.debug_windows.catalog {
            for error in &catalog.errors {
                content = content.child(
                    div()
                        .small_style(theme)
                        .text_color(rgb(theme.colors.error.0))
                        .child(error.clone()),
                );
            }
        }
        div()
            .w(px(theme.spacing.sidebar_width))
            .h(px(720. / scale + 240.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .panel_style(theme)
            .overflow_hidden()
            .child(header)
            .child(content)
    }

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
        let debug_windows_panel = self.debug_windows_panel(scale, cx);
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
                    )
                    .child(debug_windows_panel),
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
