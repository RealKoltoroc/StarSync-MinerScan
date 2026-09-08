use std::sync::mpsc::{self, Receiver};

use anyhow::{Context, Result};
use eframe::egui;

use crate::native_window::show_main_window;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Close,
    StartScanner,
    StopScanner,
    LastReport,
}

pub struct TrayController {
    _tray: TrayIcon,
    show: MenuItem,
    close: MenuItem,
    start: MenuItem,
    stop: MenuItem,
    last_report: MenuItem,
    receiver: Receiver<MenuEvent>,
}

impl TrayController {
    pub fn new(ctx: egui::Context) -> Result<Self> {
        let icon_image =
            image::load_from_memory(include_bytes!("../assets/brand/StarSyncMinerScan.png"))
                .context("Could not decode MinerScan tray icon")?
                .to_rgba8();
        let (width, height) = icon_image.dimensions();
        let icon = Icon::from_rgba(icon_image.into_raw(), width, height)
            .context("Could not create MinerScan tray icon")?;

        let menu = Menu::new();
        let show = MenuItem::new("Show", true, None);
        let start = MenuItem::new("Start Scanner", true, None);
        let stop = MenuItem::new("Stop Scanner", false, None);
        let last_report = MenuItem::new("Last Report", false, None);
        let close = MenuItem::new("Close", true, None);
        menu.append(&show)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&start)?;
        menu.append(&stop)?;
        menu.append(&last_report)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&close)?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true)
            .with_tooltip("StarSync MinerScan")
            .with_icon(icon)
            .build()
            .context("Could not create MinerScan system tray icon")?;

        let (tx, receiver) = mpsc::channel();
        let show_id = show.id().clone();
        let close_id = close.id().clone();
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == close_id {
                // Close from the Windows tray is an explicit process termination.
                // Execute it directly in the native menu callback so it cannot be
                // delayed by a hidden/minimized eframe viewport or repaint cycle.
                std::process::exit(0);
            }
            if event.id == show_id {
                let _ = show_main_window();
            }
            let _ = tx.send(event);
            menu_ctx.request_repaint();
        }));
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                let _ = show_main_window();
                ctx.request_repaint();
            }
        }));

        Ok(Self {
            _tray: tray,
            show,
            close,
            start,
            stop,
            last_report,
            receiver,
        })
    }

    pub fn set_state(&self, scanner_active: bool, has_report: bool) {
        self.start.set_enabled(!scanner_active);
        self.stop.set_enabled(scanner_active);
        self.last_report.set_enabled(has_report);
    }

    pub fn set_visible(&self, visible: bool) {
        let _ = self._tray.set_visible(visible);
    }

    pub fn poll_action(&self) -> Option<TrayAction> {
        let event = self.receiver.try_recv().ok()?;
        if event.id == *self.show.id() {
            Some(TrayAction::Show)
        } else if event.id == *self.close.id() {
            Some(TrayAction::Close)
        } else if event.id == *self.start.id() {
            Some(TrayAction::StartScanner)
        } else if event.id == *self.stop.id() {
            Some(TrayAction::StopScanner)
        } else if event.id == *self.last_report.id() {
            Some(TrayAction::LastReport)
        } else {
            None
        }
    }
}
