#[cfg(target_os = "windows")]
mod imp {
    use std::{mem, ptr, sync::OnceLock};

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
            PAINTSTRUCT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW,
            HWND_TOPMOST, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
            SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_ERASEBKGND, WM_PAINT, WNDCLASSW,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
        },
    };

    const SEGMENTS: usize = 8;
    static CLASS_NAME: OnceLock<Vec<u16>> = OnceLock::new();

    fn class_name() -> &'static [u16] {
        CLASS_NAME.get_or_init(|| {
            "StarSyncMinerScanCalibrationSegment\0"
                .encode_utf16()
                .collect()
        })
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = unsafe { mem::zeroed() };
                let dc = unsafe { BeginPaint(hwnd, &mut ps) };
                let color = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as u32;
                let brush = unsafe { CreateSolidBrush(color) };
                unsafe {
                    FillRect(dc, &ps.rcPaint, brush);
                    DeleteObject(brush);
                    EndPaint(hwnd, &ps);
                }
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn register_class() -> bool {
        let class = class_name();
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        unsafe { RegisterClassW(&wc) != 0 }
    }

    fn color_ref(rgb: [u8; 3]) -> u32 {
        (rgb[0] as u32) | ((rgb[1] as u32) << 8) | ((rgb[2] as u32) << 16)
    }

    #[derive(Default)]
    pub struct CalibrationOverlay {
        windows: Vec<HWND>,
    }

    impl CalibrationOverlay {
        fn ensure_created(&mut self) -> bool {
            if self.windows.len() == SEGMENTS {
                return true;
            }
            let _ = register_class();
            let class = class_name();
            let instance = unsafe { GetModuleHandleW(ptr::null()) };
            let ex_style = WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
            for _ in self.windows.len()..SEGMENTS {
                let hwnd = unsafe {
                    CreateWindowExW(
                        ex_style,
                        class.as_ptr(),
                        ptr::null(),
                        WS_POPUP,
                        0,
                        0,
                        1,
                        1,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        instance,
                        ptr::null(),
                    )
                };
                if hwnd.is_null() {
                    self.hide();
                    return false;
                }
                self.windows.push(hwnd);
            }
            true
        }

        pub fn update(
            &mut self,
            x: i32,
            y: i32,
            width: u32,
            height: u32,
            rgb: [u8; 3],
            visible: bool,
        ) {
            if !visible {
                self.hide();
                return;
            }
            if !self.ensure_created() {
                return;
            }

            let w = width.max(20) as i32;
            let h = height.max(20) as i32;
            let thickness = 1_i32;
            let horizontal = ((w as f32 * 0.25).round() as i32).max(6);
            let vertical = ((h as f32 * 0.20).round() as i32).max(6);
            let color = color_ref(rgb);
            let segments = [
                (x, y, horizontal, thickness),
                (x + w - horizontal, y, horizontal, thickness),
                (x, y + h - thickness, horizontal, thickness),
                (x + w - horizontal, y + h - thickness, horizontal, thickness),
                (x, y, thickness, vertical),
                (x, y + h - vertical, thickness, vertical),
                (x + w - thickness, y, thickness, vertical),
                (x + w - thickness, y + h - vertical, thickness, vertical),
            ];

            for (hwnd, (sx, sy, sw, sh)) in self.windows.iter().copied().zip(segments) {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, color as isize);
                    SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        sx,
                        sy,
                        sw.max(1),
                        sh.max(1),
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                    InvalidateRect(hwnd, ptr::null(), 1);
                }
            }
        }

        pub fn hide(&self) {
            for hwnd in &self.windows {
                unsafe {
                    ShowWindow(*hwnd, SW_HIDE);
                }
            }
        }
    }

    impl Drop for CalibrationOverlay {
        fn drop(&mut self) {
            for hwnd in self.windows.drain(..) {
                unsafe {
                    DestroyWindow(hwnd);
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use imp::CalibrationOverlay;

#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct CalibrationOverlay;

#[cfg(not(target_os = "windows"))]
impl CalibrationOverlay {
    pub fn update(
        &mut self,
        _x: i32,
        _y: i32,
        _width: u32,
        _height: u32,
        _rgb: [u8; 3],
        _visible: bool,
    ) {
    }

    pub fn hide(&self) {}
}
