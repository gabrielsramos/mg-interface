use super::{error, null_mut, size_of};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::ScreenToClient,
    System::Diagnostics::ToolHelp::*,
    UI::{Input::KeyboardAndMouse::SetFocus, WindowsAndMessaging::*},
};
use windows_sys::core::BOOL;

pub enum ViewportState {
    Waiting,
    Attached,
    Running,
    Closed,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ViewportBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
#[derive(Default)]
pub struct Viewport {
    game: HWND,
    bounds: Option<ViewportBounds>,
}
impl Viewport {
    pub fn clear(&mut self) {
        self.game = null_mut();
        self.bounds = None;
    }
    pub fn poll(
        &mut self,
        pid: u32,
        host: HWND,
        bounds: ViewportBounds,
    ) -> Result<ViewportState, String> {
        unsafe {
            if !self.game.is_null() {
                if IsWindow(self.game) == 0 {
                    self.game = null_mut();
                    return Ok(ViewportState::Closed);
                }
                if self.bounds != Some(bounds) {
                    self.position(host, bounds)?;
                }
                return Ok(ViewportState::Running);
            }
            let mut search = Search {
                pids: descendants(pid),
                hwnd: null_mut(),
            };
            EnumWindows(Some(find_window), &mut search as *mut _ as LPARAM);
            if search.hwnd.is_null() {
                return Ok(ViewportState::Waiting);
            }
            let game = search.hwnd;
            let old = GetWindowLongPtrW(game, GWL_STYLE) as u32;
            SetWindowLongPtrW(
                game,
                GWL_STYLE,
                ((old
                    & !(WS_POPUP
                        | WS_CAPTION
                        | WS_THICKFRAME
                        | WS_SYSMENU
                        | WS_MINIMIZEBOX
                        | WS_MAXIMIZEBOX))
                    | WS_CHILD) as isize,
            );
            SetLastError(0);
            let previous = SetParent(game, host);
            if previous.is_null() && GetLastError() != 0 {
                return Err(error("SetParent"));
            }
            self.game = game;
            self.position(host, bounds)?;
            if GetParent(game) != host {
                return Err("Embedded window parent is incorrect".into());
            }
            SetFocus(game);
        }
        Ok(ViewportState::Attached)
    }

    fn position(&mut self, host: HWND, bounds: ViewportBounds) -> Result<(), String> {
        unsafe {
            if SetWindowPos(
                self.game,
                null_mut(),
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
            ) == 0
            {
                return Err(error("SetWindowPos"));
            }
            let mut client: RECT = std::mem::zeroed();
            if GetClientRect(self.game, &mut client) == 0
                || client.right - client.left != bounds.width
                || client.bottom - client.top != bounds.height
            {
                return Err("Embedded client size is incorrect".into());
            }
            let mut outer: RECT = std::mem::zeroed();
            if GetWindowRect(self.game, &mut outer) == 0 {
                return Err(error("GetWindowRect"));
            }
            let mut origin = POINT {
                x: outer.left,
                y: outer.top,
            };
            if ScreenToClient(host, &mut origin) == 0 {
                return Err(error("ScreenToClient"));
            }
            if origin.x != bounds.x || origin.y != bounds.y {
                return Err("Embedded window position is incorrect".into());
            }
            self.bounds = Some(bounds);
        }
        Ok(())
    }
}
struct Search {
    pids: Vec<u32>,
    hwnd: HWND,
}
unsafe extern "system" fn find_window(hwnd: HWND, data: LPARAM) -> BOOL {
    unsafe {
        let search = &mut *(data as *mut Search);
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if search.pids.contains(&pid)
            && IsWindowVisible(hwnd) != 0
            && GetWindow(hwnd, GW_OWNER).is_null()
        {
            search.hwnd = hwnd;
            return 0;
        }
        1
    }
}

fn descendants(root: u32) -> Vec<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return vec![root];
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut pairs = Vec::new();
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                pairs.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        let mut pids = vec![root];
        loop {
            let before = pids.len();
            for &(pid, parent) in &pairs {
                if pids.contains(&parent) && !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
            if before == pids.len() {
                return pids;
            }
        }
    }
}
