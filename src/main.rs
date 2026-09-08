#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod calibration_overlay;
mod data;
mod database_update;
#[cfg(target_os = "windows")]
mod dxgi_capture;
mod enrichment;
mod event_log;
mod hotkeys;
mod live_scan;
mod matcher;
mod models;
mod native_window;
#[allow(dead_code)]
mod ocr;
mod provider_cache;
mod provider_http;
mod report;
mod settings;
mod tray;

use std::sync::Arc;

use app::MinerScanApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let _ = settings::ensure_runtime_layout();
    notify_shell_icon_changed();
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("StarSync MinerScan")
            .with_icon(app_icon())
            .with_inner_size([920.0, 1080.0])
            .with_min_inner_size([820.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "StarSync MinerScan",
        native_options,
        Box::new(|cc| Ok(Box::new(MinerScanApp::new(cc)))),
    )
}

fn app_icon() -> Arc<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../assets/brand/StarSyncMinerScan.png"))
        .expect("embedded MinerScan icon")
        .to_rgba8();
    Arc::new(egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

#[cfg(target_os = "windows")]
fn notify_shell_icon_changed() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHCNE_UPDATEITEM, SHCNF_PATHW, SHChangeNotify};

    if let Ok(path) = std::env::current_exe() {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);
        unsafe {
            SHChangeNotify(
                SHCNE_UPDATEITEM as i32,
                SHCNF_PATHW,
                wide.as_ptr().cast(),
                std::ptr::null(),
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn notify_shell_icon_changed() {}
