mod platform;
mod recent_projects;
mod runtime;
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
