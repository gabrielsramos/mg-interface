#[cfg(any(windows, test))]
mod debug_windows;
#[cfg(any(windows, test))]
mod entity_browser;
#[cfg(any(windows, test))]
mod log_search;
mod platform;
mod recent_projects;
mod runtime;
#[cfg(windows)]
mod search_input;
#[cfg(any(windows, test))]
mod theme;
#[cfg(windows)]
mod ui;
fn main() {
    #[cfg(windows)]
    ui::run();
    #[cfg(not(windows))]
    eprintln!("Viewport hosting is currently implemented for Windows only");
}
