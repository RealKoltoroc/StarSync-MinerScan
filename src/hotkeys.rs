use std::sync::mpsc::{self, Receiver};

use anyhow::{Context, Result};
use eframe::egui;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};

#[cfg(target_os = "windows")]
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ToggleScanner,
    ToggleCalibration,
    ToggleMoveMode,
}

#[derive(Debug, Clone, Copy)]
enum Binding {
    NativeNumpadEnter,
    NativeNumpadAdd,
    NativeNumpadSubtract,
    Registered(HotKey),
}

pub struct HotkeyController {
    manager: GlobalHotKeyManager,
    scanner: Binding,
    calibration: Binding,
    move_mode: Binding,
    receiver: Receiver<GlobalHotKeyEvent>,
    #[cfg(target_os = "windows")]
    native_receiver: Receiver<HotkeyAction>,
}

impl HotkeyController {
    pub fn new(
        ctx: egui::Context,
        scanner: &str,
        calibration: &str,
        move_mode: &str,
    ) -> Result<Self> {
        let manager =
            GlobalHotKeyManager::new().context("Could not create global hotkey manager")?;
        let scanner = parse_binding(scanner, "scanner")?;
        let calibration = parse_binding(calibration, "calibration")?;
        let move_mode = parse_binding(move_mode, "move mode")?;

        let registered = registered_hotkeys(scanner, calibration, move_mode);
        if !registered.is_empty() {
            manager
                .register_all(&registered)
                .context("Could not register one or more MinerScan hotkeys")?;
        }

        let (tx, receiver) = mpsc::channel();
        let repaint_ctx = ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event| {
            let _ = tx.send(event);
            repaint_ctx.request_repaint();
        }));

        #[cfg(target_os = "windows")]
        let native_receiver = install_native_numpad_observer(ctx)?;

        let controller = Self {
            manager,
            scanner,
            calibration,
            move_mode,
            receiver,
            #[cfg(target_os = "windows")]
            native_receiver,
        };
        #[cfg(target_os = "windows")]
        controller.update_native_bindings();
        Ok(controller)
    }

    pub fn rebind(&mut self, scanner: &str, calibration: &str, move_mode: &str) -> Result<()> {
        let new_scanner = parse_binding(scanner, "scanner")?;
        let new_calibration = parse_binding(calibration, "calibration")?;
        let new_move_mode = parse_binding(move_mode, "move mode")?;

        let old_registered = self.registered_hotkeys();
        let new_registered = registered_hotkeys(new_scanner, new_calibration, new_move_mode);

        if !old_registered.is_empty() {
            let _ = self.manager.unregister_all(&old_registered);
        }
        if !new_registered.is_empty() {
            if let Err(error) = self.manager.register_all(&new_registered) {
                if !old_registered.is_empty() {
                    let _ = self.manager.register_all(&old_registered);
                }
                return Err(error).context("Could not register new MinerScan hotkeys");
            }
        }

        self.scanner = new_scanner;
        self.calibration = new_calibration;
        self.move_mode = new_move_mode;
        #[cfg(target_os = "windows")]
        self.update_native_bindings();
        Ok(())
    }

    fn registered_hotkeys(&self) -> Vec<HotKey> {
        registered_hotkeys(self.scanner, self.calibration, self.move_mode)
    }

    pub fn drain_actions(&self) -> Vec<HotkeyAction> {
        let mut actions = Vec::new();

        while let Ok(event) = self.receiver.try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if registered_matches(self.scanner, event.id) {
                actions.push(HotkeyAction::ToggleScanner);
            } else if registered_matches(self.calibration, event.id) {
                actions.push(HotkeyAction::ToggleCalibration);
            } else if registered_matches(self.move_mode, event.id) {
                actions.push(HotkeyAction::ToggleMoveMode);
            }
        }

        #[cfg(target_os = "windows")]
        while let Ok(action) = self.native_receiver.try_recv() {
            actions.push(action);
        }

        actions
    }

    #[cfg(target_os = "windows")]
    fn update_native_bindings(&self) {
        if let Some(lock) = NATIVE_DISPATCH.get() {
            if let Ok(mut guard) = lock.lock() {
                if let Some(dispatch) = guard.as_mut() {
                    dispatch.scanner = matches!(self.scanner, Binding::NativeNumpadEnter);
                    dispatch.calibration = matches!(self.calibration, Binding::NativeNumpadAdd);
                    dispatch.move_mode = matches!(self.move_mode, Binding::NativeNumpadSubtract);
                }
            }
        }
    }
}

fn registered_matches(binding: Binding, id: u32) -> bool {
    matches!(binding, Binding::Registered(hotkey) if hotkey.id() == id)
}

fn registered_hotkeys(scanner: Binding, calibration: Binding, move_mode: Binding) -> Vec<HotKey> {
    [scanner, calibration, move_mode]
        .into_iter()
        .filter_map(|binding| match binding {
            Binding::Registered(hotkey) => Some(hotkey),
            _ => None,
        })
        .collect()
}

fn parse_binding(value: &str, label: &str) -> Result<Binding> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "numpadenter" | "numpad enter" | "num enter" => Ok(Binding::NativeNumpadEnter),
        "numpadadd" | "numpad add" | "num +" | "numpad +" => Ok(Binding::NativeNumpadAdd),
        "numpadsubtract" | "numpad subtract" | "num -" | "numpad -" => {
            Ok(Binding::NativeNumpadSubtract)
        }
        _ => value
            .trim()
            .parse::<HotKey>()
            .map(Binding::Registered)
            .with_context(|| format!("Invalid {label} hotkey '{value}'")),
    }
}

#[cfg(target_os = "windows")]
struct NativeDispatch {
    tx: mpsc::Sender<HotkeyAction>,
    ctx: egui::Context,
    scanner: bool,
    calibration: bool,
    move_mode: bool,
}

#[cfg(target_os = "windows")]
static NATIVE_DISPATCH: OnceLock<Mutex<Option<NativeDispatch>>> = OnceLock::new();

#[cfg(target_os = "windows")]
fn install_native_numpad_observer(ctx: egui::Context) -> Result<Receiver<HotkeyAction>> {
    use std::thread;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG,
        SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
    };

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_ADD, VK_RETURN, VK_SUBTRACT};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, WM_KEYDOWN, WM_SYSKEYDOWN,
        };

        if code >= 0 && (wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN) {
            let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            if let Some(lock) = NATIVE_DISPATCH.get() {
                if let Ok(guard) = lock.lock() {
                    if let Some(dispatch) = guard.as_ref() {
                        let action = if dispatch.scanner
                            && event.vkCode == VK_RETURN as u32
                            && (event.flags & LLKHF_EXTENDED) != 0
                        {
                            Some(HotkeyAction::ToggleScanner)
                        } else if dispatch.calibration && event.vkCode == VK_ADD as u32 {
                            Some(HotkeyAction::ToggleCalibration)
                        } else if dispatch.move_mode && event.vkCode == VK_SUBTRACT as u32 {
                            Some(HotkeyAction::ToggleMoveMode)
                        } else {
                            None
                        };
                        if let Some(action) = action {
                            let _ = dispatch.tx.send(action);
                            dispatch.ctx.request_repaint();
                        }
                    }
                }
            }
        }

        // Passive observer only: never consume the key. Star Citizen and any other
        // foreground application continue to receive the original keyboard event.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    let (tx, receiver) = mpsc::channel();
    let dispatch = NATIVE_DISPATCH.get_or_init(|| Mutex::new(None));
    *dispatch
        .lock()
        .map_err(|_| anyhow::anyhow!("Could not initialize native hotkey dispatch"))? =
        Some(NativeDispatch {
            tx,
            ctx,
            scanner: false,
            calibration: false,
            move_mode: false,
        });

    static HOOK_THREAD_STARTED: OnceLock<()> = OnceLock::new();
    if HOOK_THREAD_STARTED.set(()).is_ok() {
        thread::Builder::new()
            .name("minerscan-passive-hotkeys".to_owned())
            .spawn(move || unsafe {
                let hook =
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0);
                if hook.is_null() {
                    return;
                }
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            })
            .context("Could not start passive MinerScan hotkey observer")?;
    }

    // Keep imports referenced in this cfg block explicit for compile-time API validation.
    let _ = (
        CallNextHookEx as unsafe extern "system" fn(_, _, _, _) -> _,
        KBDLLHOOKSTRUCT::default,
        LLKHF_EXTENDED,
        WM_KEYDOWN,
        WM_SYSKEYDOWN,
    );

    Ok(receiver)
}

#[cfg(target_os = "windows")]
pub fn arrow_state() -> (bool, bool, bool, bool) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_DOWN, VK_LEFT, VK_RIGHT, VK_UP,
    };
    unsafe {
        (
            GetAsyncKeyState(VK_LEFT as i32) != 0,
            GetAsyncKeyState(VK_RIGHT as i32) != 0,
            GetAsyncKeyState(VK_UP as i32) != 0,
            GetAsyncKeyState(VK_DOWN as i32) != 0,
        )
    }
}

#[cfg(not(target_os = "windows"))]
pub fn arrow_state() -> (bool, bool, bool, bool) {
    (false, false, false, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_numpad_hotkeys_are_passive_native_bindings() {
        assert!(matches!(
            parse_binding("NumpadEnter", "scanner").unwrap(),
            Binding::NativeNumpadEnter
        ));
        assert!(matches!(
            parse_binding("NumpadAdd", "calibration").unwrap(),
            Binding::NativeNumpadAdd
        ));
        assert!(matches!(
            parse_binding("NumpadSubtract", "move").unwrap(),
            Binding::NativeNumpadSubtract
        ));
    }

    #[test]
    fn ordinary_enter_is_not_the_numpad_enter_binding() {
        assert!(matches!(
            parse_binding("Enter", "scanner").unwrap(),
            Binding::Registered(_)
        ));
    }

    #[test]
    #[ignore = "installs real Windows low-level keyboard hook"]
    fn default_numpad_hotkeys_register() {
        let ctx = egui::Context::default();
        let controller = HotkeyController::new(ctx, "NumpadEnter", "NumpadAdd", "NumpadSubtract");
        assert!(controller.is_ok());
    }
}
