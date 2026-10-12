use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{mem::size_of, ptr::null_mut};
use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
fn error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
}
mod cursor;
pub use cursor::{install_cleanup as install_pointer_cleanup, restore_pointer};
mod process;
mod viewport;
pub use process::WindowsSupervisor;
pub use viewport::{Viewport, ViewportBounds, ViewportState};
pub fn initialize() {
    // DirectComposition's topmost surface covers child HWNDs. Use GPUI's HWND swap chain.
    // Set before GPUI starts any threads; this setting is local to this process.
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1");
    }
}
pub fn native_window(window: &Window) -> Result<HWND, String> {
    match HasWindowHandle::window_handle(window)
        .map_err(|e| e.to_string())?
        .as_raw()
    {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as HWND),
        _ => Err("GPUI did not expose a Win32 window".into()),
    }
}
pub fn prepare_viewport(host: HWND) -> Result<(), String> {
    unsafe {
        if GetWindowLongPtrW(host, GWL_EXSTYLE) & WS_EX_NOREDIRECTIONBITMAP as isize != 0 {
            return Err("GPUI compositor is incompatible with the native viewport".into());
        }
        let style = GetWindowLongPtrW(host, GWL_STYLE);
        SetWindowLongPtrW(host, GWL_STYLE, style | WS_CLIPCHILDREN as isize);
    }
    Ok(())
}

mod inspection;
pub(crate) use inspection::{configure_inspection, connect_inspection};
