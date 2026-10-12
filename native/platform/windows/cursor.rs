use gpui::App;
use windows_sys::Win32::UI::{
    Input::KeyboardAndMouse::{GetCapture, ReleaseCapture},
    WindowsAndMessaging::{IDC_ARROW, LoadCursorW, SetCursor, ShowCursor},
};

pub fn install_cleanup(cx: &mut App) {
    cx.on_window_closed(|_| restore_pointer()).detach();
    cx.on_app_quit(|_| {
        restore_pointer();
        async {}
    })
    .detach();
}

pub fn restore_pointer() {
    unsafe {
        if !GetCapture().is_null() {
            ReleaseCapture();
        }
        let arrow = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        if !arrow.is_null() {
            SetCursor(arrow);
        }
        restore_visibility(|show| ShowCursor(i32::from(show)));
    }
}

fn restore_visibility(mut show_cursor: impl FnMut(bool) -> i32) {
    // Probe the thread's display counter without increasing an already visible count.
    let mut count = show_cursor(true);
    if count > 0 {
        show_cursor(false);
    } else {
        while count < 0 {
            count = show_cursor(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::restore_visibility;

    #[test]
    fn hidden_cursor_is_restored_and_repeated_cleanup_does_not_drift() {
        for initial in [-5, -1, 0, 3] {
            let mut count = initial;
            for _ in 0..3 {
                restore_visibility(|show| {
                    count += if show { 1 } else { -1 };
                    count
                });
                assert_eq!(count, initial.max(0));
            }
        }
    }
}
