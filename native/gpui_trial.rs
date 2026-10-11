#[cfg(windows)]
#[path = "platform/windows/process.rs"]
mod process;
#[cfg(windows)]
mod runtime;
#[cfg(windows)]
#[path = "platform/windows/viewport.rs"]
mod viewport;
#[cfg(windows)]
use std::{mem::size_of, ptr::null_mut};
#[cfg(windows)]
fn error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
}

#[cfg(windows)]
mod trial {
    use super::{
        process,
        runtime::{GameRunner, LaunchSpec},
        viewport::{Viewport, ViewportBounds, ViewportState},
    };
    use gpui::{prelude::*, *};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{
        cell::Cell,
        path::PathBuf,
        rc::Rc,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};

    struct Trial {
        runner: GameRunner,
        viewport: Viewport,
        bounds: Rc<Cell<Option<ViewportBounds>>>,
        status: String,
        started: Instant,
        attached: Option<Instant>,
        clicks: u32,
        inset: bool,
        smoke: bool,
        smoke_stage: u8,
        smoke_origin_y: Option<i32>,
    }

    fn native_window(window: &Window) -> Result<HWND, String> {
        match HasWindowHandle::window_handle(window)
            .map_err(|e| e.to_string())?
            .as_raw()
        {
            RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as HWND),
            _ => Err("GPUI did not expose a Win32 window".into()),
        }
    }

    impl Trial {
        fn new(project: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
                runner,
                viewport: Viewport::default(),
                bounds: Rc::new(Cell::new(None)),
                status,
                started: Instant::now(),
                attached: None,
                clicks: 0,
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
            match self.runner.play(&process::WindowsSupervisor) {
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
            let host = native_window(window)?;
            unsafe {
                if GetWindowLongPtrW(host, GWL_EXSTYLE) & WS_EX_NOREDIRECTIONBITMAP as isize != 0 {
                    return Err(
                        "GPUI compositor is incompatible with the native child viewport".into(),
                    );
                }
                let style = GetWindowLongPtrW(host, GWL_STYLE);
                SetWindowLongPtrW(host, GWL_STYLE, style | WS_CLIPCHILDREN as isize);
            }
            match self.viewport.poll(pid, host, bounds)? {
                ViewportState::Attached => {
                    self.attached = Some(Instant::now());
                    self.status = "Running inside GPUI. Try the side panel and Play/Stop.".into();
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
            if let Err(e) = self.poll(window) {
                self.stop();
                self.status = e.clone();
                eprintln!("GPUI trial: {e}");
                if self.smoke {
                    std::process::exit(1);
                }
            }
            if self.smoke {
                if self.smoke_stage == 0 {
                    if self.runner.project().is_none() {
                        eprintln!("No valid smoke project");
                        std::process::exit(1);
                    }
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
                            self.clicks += 1;
                            self.smoke_stage = 2;
                            self.attached = Some(Instant::now());
                        }
                        2 => {
                            if self.bounds.get().map(|bounds| bounds.y) == self.smoke_origin_y {
                                self.stop();
                                eprintln!("GPUI layout did not move the embedded viewport");
                                std::process::exit(1);
                            }
                            self.stop();
                            self.play();
                            self.smoke_stage = 3;
                        }
                        3 => {
                            self.stop();
                            println!(
                                "GPUI smoke passed: attach, panel redraw/layout movement, stop, replay, attach, cleanup."
                            );
                            cx.quit();
                        }
                        _ => {}
                    }
                } else if !self.runner.is_active() && self.smoke_stage > 0 {
                    eprintln!("Game exited before GPUI smoke completed");
                    std::process::exit(1);
                }
            }
            // Keep GPUI redrawing beside the live native child to exercise coexistence.
            cx.notify();
        }
    }

    impl Render for Trial {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let scale = window.scale_factor();
            let tracked = self.bounds.clone();
            let project = self
                .runner
                .project()
                .map(|p| p.project.display().to_string())
                .unwrap_or_else(|| "No valid project".into());
            div().size_full().flex().flex_col().bg(rgb(0x111827)).text_color(rgb(0xe5e7eb))
                .child(div().flex().gap_3().items_center().h(px(56.)).px_4()
                    .child(div().id("play").px_4().py_2().rounded_md().bg(rgb(0x2563eb)).cursor_pointer()
                        .on_click(cx.listener(|view, _, _, cx| { view.play(); cx.notify(); })).child("Play"))
                    .child(div().id("stop").px_4().py_2().rounded_md().bg(rgb(0x374151)).cursor_pointer()
                        .on_click(cx.listener(|view, _, _, cx| { view.stop(); cx.notify(); })).child("Stop"))
                    .child("GPUI viewport trial"))
                .child(div().px_4().h(px(32.)).text_sm().child(self.status.clone()))
                .child(div().flex().gap_4().px_4()
                    .child(div().w(px(240.)).flex_shrink_0().flex().flex_col().gap_4().p_4().rounded_md().bg(rgb(0x1f2937))
                        .child("Test panel").child(div().text_sm().child(project))
                        .child(div().id("counter").p_3().bg(rgb(0x374151)).rounded_md().cursor_pointer()
                            .on_click(cx.listener(|view, _, _, cx| { view.clicks += 1; cx.notify(); })).child(format!("Panel clicks: {}", self.clicks)))
                        .child(div().id("move").p_3().bg(rgb(0x374151)).rounded_md().cursor_pointer()
                            .on_click(cx.listener(|view, _, _, cx| { view.inset = !view.inset; cx.notify(); })).child("Move viewport"))
                        .child(div().text_sm().child("Click the game to focus input. Test minimize/restore and Stop/Play.")))
                    .child(div().flex().flex_col().mt(px(if self.inset { 24. } else { 0. }))
                        .child(canvas(move |bounds, _, _| {
                            tracked.set(Some(ViewportBounds { x: (f32::from(bounds.origin.x) * scale).round() as i32,
                                y: (f32::from(bounds.origin.y) * scale).round() as i32, width: 1280, height: 720 }));
                        }, |_, _, _, _| {}).w(px(1280. / scale)).h(px(720. / scale)))
                        .child(div().mt_2().text_sm().child("Standalone MonoGame • fixed 1280 × 720 physical pixels"))))
        }
    }

    pub fn run() {
        let Some(project) = std::env::args_os().nth(1).map(PathBuf::from) else {
            eprintln!(
                "Usage: cargo run --features gpui-trial --bin mg-interface-gpui-trial -- <game.csproj>"
            );
            std::process::exit(1);
        };
        // GPUI 0.2.2's DirectComposition target uses topmost=true, covering child
        // HWNDs. Its supported HWND swap-chain mode allows the game to show.
        // This is a process-local setting, applied before GPUI is initialized.
        // Windows environment mutation is thread-safe (set_var is unsafe for Unix).
        unsafe {
            std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1");
        }
        Application::new().run(move |cx: &mut App| {
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1600.), px(880.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Trial::new(project, window, cx)),
            )
            .expect("Could not open GPUI trial window");
            cx.activate(true);
        });
    }
}

#[cfg(windows)]
fn main() {
    trial::run();
}
#[cfg(not(windows))]
fn main() {
    eprintln!("The GPUI viewport trial currently supports Windows only.");
}
