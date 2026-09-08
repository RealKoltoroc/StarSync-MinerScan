#[cfg(target_os = "windows")]
mod imp {
    use std::{process, ptr};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
        },
    };

    pub fn main_hwnd() -> Option<HWND> {
        find_window_with_title("StarSync MinerScan")
    }

    fn find_window_with_title(wanted: &str) -> Option<HWND> {
        struct TitleFindData<'a> {
            pid: u32,
            hwnd: HWND,
            wanted: &'a str,
        }

        unsafe extern "system" fn title_enum_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
            let data = unsafe { &mut *(lparam as *mut TitleFindData<'_>) };
            let mut pid = 0_u32;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if pid != data.pid {
                return 1;
            }
            let mut buf = [0_u16; 256];
            let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
            if len <= 0 {
                return 1;
            }
            let title = String::from_utf16_lossy(&buf[..len as usize]);
            if title == data.wanted {
                data.hwnd = hwnd;
                return 0;
            }
            1
        }

        let mut data = TitleFindData {
            pid: process::id(),
            hwnd: ptr::null_mut(),
            wanted,
        };
        unsafe {
            EnumWindows(Some(title_enum_proc), &mut data as *mut _ as LPARAM);
        }
        (!data.hwnd.is_null()).then_some(data.hwnd)
    }

    pub fn hide_main_window() -> bool {
        let Some(hwnd) = main_hwnd() else {
            return false;
        };
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
        true
    }

    pub fn show_main_window() -> bool {
        let Some(hwnd) = main_hwnd() else {
            return false;
        };
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
        true
    }

    pub fn is_main_window_visible() -> bool {
        main_hwnd().is_some_and(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 })
    }

    pub fn is_main_window_minimized() -> bool {
        main_hwnd().is_some_and(|hwnd| unsafe { IsIconic(hwnd) != 0 })
    }
}

#[cfg(target_os = "windows")]
pub use imp::*;

#[cfg(not(target_os = "windows"))]
pub fn hide_main_window() -> bool {
    false
}
#[cfg(not(target_os = "windows"))]
pub fn show_main_window() -> bool {
    false
}
#[cfg(not(target_os = "windows"))]
pub fn is_main_window_visible() -> bool {
    true
}
#[cfg(not(target_os = "windows"))]
pub fn is_main_window_minimized() -> bool {
    false
}
