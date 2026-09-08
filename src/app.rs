use std::{
    fs::File,
    io::BufReader,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use display_info::DisplayInfo;
use eframe::egui::{self, Color32, RichText, Stroke};
use rodio::{Decoder, OutputStream, Sink};

use crate::{
    calibration_overlay::CalibrationOverlay,
    data::ScUnpackedCatalog,
    database_update::{
        database_root, ensure_database_root, inbox_state, install_or_update_candidate,
        installed_dataset,
    },
    enrichment::{
        EnrichedMineral, EnrichmentConfig, EnrichmentEvent, EnrichmentWorker, RefineryOption,
        RefiningMethod, load_refinery_options, load_refining_methods, refinery_material_key,
    },
    event_log::EventLogger,
    hotkeys::{HotkeyAction, HotkeyController, arrow_state},
    live_scan::{LiveScanConfig, LiveScanEvent, LiveScanner},
    matcher::{match_signature, resolve_signature},
    models::{CaptureRegion, DetectionCandidate, SignatureCategory, SignatureDefinition},
    native_window::{
        hide_main_window, is_main_window_minimized, is_main_window_visible, show_main_window,
    },
    ocr::normalize_numeric,
    provider_cache::{
        ProviderCache, cache_root, clear_all_provider_caches, clear_provider_cache,
        ensure_provider_cache, provider_cache_size,
    },
    provider_http::cached_get_text,
    report::{latest_report, prepare_session_reports, reports_dir, write_enriched_report},
    settings::{
        AppSettings, default_refining_method_base_efficiency, load_settings, save_settings,
        settings_path,
    },
    tray::{TrayAction, TrayController},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppTab {
    Scanner,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomPanelTab {
    EventLog,
    LiveOcr,
}

fn is_duplicate_material_detection(
    last_material: Option<&str>,
    last_at: Option<Instant>,
    current_material: &str,
    cooldown: Duration,
    now: Instant,
) -> bool {
    last_material == Some(current_material)
        && last_at.is_some_and(|at| now.duration_since(at) < cooldown)
}

#[derive(Debug, Clone)]
struct MonitorInfo {
    id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl From<DisplayInfo> for MonitorInfo {
    fn from(value: DisplayInfo) -> Self {
        Self {
            id: value.id,
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

pub struct MinerScanApp {
    tab: AppTab,
    scanner_active: bool,
    show_region: bool,
    region: CaptureRegion,
    frame_color: Color32,
    calibration_overlay: CalibrationOverlay,
    monitors: Vec<MonitorInfo>,
    selected_monitor: usize,
    search: String,
    manual_signature: String,
    last_detection: Option<DetectionCandidate>,
    definitions: Vec<SignatureDefinition>,
    show_signature_matrix: bool,
    targets_collapsed: bool,
    hotkey_scanner: String,
    hotkey_calibration: String,
    hotkey_move_mode: String,
    hotkeys: Option<HotkeyController>,
    calibration_move_mode: bool,
    last_move_tick: Instant,
    sound_file: Option<PathBuf>,
    sound_volume: f32,
    database_path: Option<PathBuf>,
    use_scunpacked: bool,
    use_uex_fallback: bool,
    uex_base_url: String,
    use_starcitizen_tools: bool,
    starcitizen_tools_api_url: String,
    use_star_citizen_wiki_api: bool,
    star_citizen_wiki_api_url: String,
    analysis_base_scu: f32,
    refining_method_base_efficiency: std::collections::BTreeMap<String, f32>,
    selected_refinery_terminal_id: Option<i64>,
    selected_refinery_label: String,
    refining_strategy: String,
    refinery_options: Vec<RefineryOption>,
    refining_methods: Vec<RefiningMethod>,
    generate_html_reports: bool,
    clear_reports_on_start: bool,
    minimize_to_tray: bool,
    last_report: Option<PathBuf>,
    tray: Option<TrayController>,
    exit_requested: bool,
    logo_texture: Option<egui::TextureHandle>,
    scunpacked_catalog: Option<ScUnpackedCatalog>,
    enrichment_worker: EnrichmentWorker,
    enriched_mineral: Option<EnrichedMineral>,
    enrichment_error: Option<String>,
    enrichment_texture: Option<egui::TextureHandle>,
    show_enrichment_details: bool,
    database_status: String,
    status_message: String,
    event_logger: EventLogger,
    show_log_viewer: bool,
    bottom_panel_tab: BottomPanelTab,
    show_live_ocr_window: bool,
    duplicate_detection_cooldown_secs: u64,
    live_scanner: LiveScanner,
    live_raw_text: String,
    live_numeric_candidates: Vec<u32>,
    live_elapsed_ms: Option<u128>,
    live_preview_texture: Option<egui::TextureHandle>,
    last_live_event_key: Option<String>,
    last_live_event_at: Option<Instant>,
    log_cache: Vec<String>,
    log_cache_at: Option<Instant>,
}

impl MinerScanApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Match StarSyncUniverse typography on Windows without shipping font files.
        let mut fonts = egui::FontDefinitions::default();
        if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
            fonts.font_data.insert(
                "segoe_ui".to_owned(),
                egui::FontData::from_owned(bytes).into(),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "segoe_ui".to_owned());
        }
        if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\consola.ttf") {
            fonts.font_data.insert(
                "consolas".to_owned(),
                egui::FontData::from_owned(bytes).into(),
            );
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "consolas".to_owned());
        }
        cc.egui_ctx.set_fonts(fonts);

        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.window_fill = Color32::from_rgb(14, 14, 14);
        style.visuals.panel_fill = Color32::from_rgb(14, 14, 14);
        style.visuals.extreme_bg_color = Color32::from_rgb(20, 20, 20);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(48, 45, 42);
        style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(31, 30, 29);
        style.visuals.widgets.inactive.bg_stroke =
            Stroke::new(1.0_f32, Color32::from_rgb(118, 82, 52));
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(58, 50, 43);
        style.visuals.widgets.hovered.bg_stroke =
            Stroke::new(1.0_f32, Color32::from_rgb(231, 132, 43));
        style.visuals.widgets.active.bg_fill = Color32::from_rgb(91, 58, 31);
        style.visuals.widgets.active.bg_stroke =
            Stroke::new(1.0_f32, Color32::from_rgb(238, 145, 57));
        style.visuals.widgets.inactive.fg_stroke =
            Stroke::new(1.0_f32, Color32::from_rgb(235, 229, 222));
        style.visuals.selection.bg_fill = Color32::from_rgb(188, 94, 30);
        style.visuals.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(255, 170, 76));
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(11.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(11.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(9.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(9.5, egui::FontFamily::Monospace),
        );
        cc.egui_ctx.set_style(style);

        let mut persisted = load_settings();
        let _ = ensure_database_root();
        if let Some(installed) = installed_dataset() {
            persisted.database_path = Some(installed.display().to_string());
        } else {
            // External SCUnpacked updates are installed below %LOCALAPPDATA%\\StarSync MinerScan\\database.
            // The embedded baseline remains active until an update has been installed.
            persisted.database_path = None;
        }
        for (code, value) in default_refining_method_base_efficiency() {
            persisted
                .refining_method_base_efficiency
                .entry(code)
                .or_insert(value);
        }
        let monitors = detect_monitors();
        let selected_monitor = persisted
            .selected_monitor_id
            .and_then(|id| monitors.iter().position(|m| m.id == id))
            .unwrap_or(0);

        let mut definitions = initial_definitions();
        for definition in &mut definitions {
            if let Some(enabled) = persisted.target_enabled.get(&definition.id) {
                definition.enabled = *enabled;
            }
        }
        definitions.sort_by(|a, b| {
            a.canonical_name
                .to_ascii_lowercase()
                .cmp(&b.canonical_name.to_ascii_lowercase())
        });

        let [r, g, b] = persisted.frame_color_rgb;
        let mut status_message = if monitors.is_empty() {
            "No monitor detected.".to_owned()
        } else {
            format!("{} monitor(s) detected.", monitors.len())
        };
        let scunpacked_catalog = if persisted.use_scunpacked {
            persisted
                .database_path
                .as_deref()
                .map(std::path::Path::new)
                .and_then(|path| ScUnpackedCatalog::load(path).ok())
                .or_else(|| ScUnpackedCatalog::embedded().ok())
        } else {
            None
        };
        let database_status = scunpacked_catalog
            .as_ref()
            .map(|catalog| {
                let source = if catalog
                    .source_path
                    .to_string_lossy()
                    .starts_with("embedded://")
                {
                    "embedded 4.10 baseline".to_owned()
                } else {
                    format!("installed update: {}", catalog.source_path.display())
                };
                format!(
                    "SCUnpacked {source}: {} commodities, {} mineral/raw records.",
                    catalog.commodities.len(),
                    catalog.mineral_count()
                )
            })
            .unwrap_or_else(|| "SCUnpacked baseline disabled or unavailable.".to_owned());

        if let Err(error) = prepare_session_reports(persisted.clear_reports_on_start) {
            status_message = format!("{status_message} Report directory error: {error:#}");
        }
        let logo_texture =
            image::load_from_memory(include_bytes!("../assets/brand/StarSyncMinerScan_logo.png"))
                .ok()
                .map(|image| {
                    let rgba = image.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    cc.egui_ctx.load_texture(
                        "minerscan-brand-logo",
                        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                        egui::TextureOptions::LINEAR,
                    )
                });
        let tray = match TrayController::new(cc.egui_ctx.clone()) {
            Ok(tray) => Some(tray),
            Err(error) => {
                status_message = format!("{status_message} Tray unavailable: {error:#}");
                None
            }
        };
        if let Some(tray) = &tray {
            tray.set_visible(false);
        }
        let hotkeys = match HotkeyController::new(
            cc.egui_ctx.clone(),
            &persisted.hotkey_scanner,
            &persisted.hotkey_calibration,
            &persisted.hotkey_move_mode,
        ) {
            Ok(controller) => Some(controller),
            Err(error) => {
                status_message = format!("{status_message} Hotkeys unavailable: {error:#}");
                None
            }
        };
        let refinery_options = load_refinery_options(
            &persisted.uex_base_url,
            persisted.use_uex_fallback,
            if persisted.use_scunpacked {
                persisted.database_path.as_deref().map(std::path::Path::new)
            } else {
                None
            },
        )
        .unwrap_or_default();
        let refining_methods =
            load_refining_methods(&persisted.uex_base_url, persisted.use_uex_fallback);
        if let Some(id) = persisted.selected_refinery_terminal_id
            && let Some(selected) = refinery_options.iter().find(|r| r.terminal_id == id)
        {
            persisted.selected_refinery_label = selected.display_label();
        } else if !persisted.selected_refinery_label.is_empty()
            && let Some(selected) = refinery_options.iter().find(|r| {
                r.display_label()
                    .eq_ignore_ascii_case(&persisted.selected_refinery_label)
                    || r.location
                        .eq_ignore_ascii_case(&persisted.selected_refinery_label)
            })
        {
            persisted.selected_refinery_terminal_id = Some(selected.terminal_id);
            persisted.selected_refinery_label = selected.display_label();
        }

        Self {
            tab: AppTab::Scanner,
            scanner_active: false,
            show_region: persisted.show_region,
            region: persisted.region,
            frame_color: Color32::from_rgb(r, g, b),
            calibration_overlay: CalibrationOverlay::default(),
            monitors,
            selected_monitor,
            search: String::new(),
            manual_signature: "3385".into(),
            last_detection: None,
            definitions,
            show_signature_matrix: persisted.show_signature_matrix,
            targets_collapsed: persisted.targets_collapsed,
            hotkey_scanner: persisted.hotkey_scanner,
            hotkey_calibration: persisted.hotkey_calibration,
            hotkey_move_mode: persisted.hotkey_move_mode,
            hotkeys,
            calibration_move_mode: false,
            last_move_tick: Instant::now(),
            sound_file: persisted.sound_file.map(PathBuf::from),
            sound_volume: persisted.sound_volume.clamp(0.0, 1.0),
            database_path: persisted.database_path.map(PathBuf::from),
            use_scunpacked: persisted.use_scunpacked,
            use_uex_fallback: persisted.use_uex_fallback,
            uex_base_url: persisted.uex_base_url,
            use_starcitizen_tools: persisted.use_starcitizen_tools,
            starcitizen_tools_api_url: persisted.starcitizen_tools_api_url,
            use_star_citizen_wiki_api: persisted.use_star_citizen_wiki_api,
            star_citizen_wiki_api_url: persisted.star_citizen_wiki_api_url,
            analysis_base_scu: persisted.analysis_base_scu.clamp(0.1, 10000.0),
            refining_method_base_efficiency: persisted.refining_method_base_efficiency,
            selected_refinery_terminal_id: persisted.selected_refinery_terminal_id,
            selected_refinery_label: persisted.selected_refinery_label,
            refining_strategy: persisted.refining_strategy,
            refinery_options,
            refining_methods,
            generate_html_reports: persisted.generate_html_reports,
            clear_reports_on_start: persisted.clear_reports_on_start,
            minimize_to_tray: persisted.minimize_to_tray,
            last_report: latest_report(),
            tray,
            exit_requested: false,
            logo_texture,
            scunpacked_catalog,
            enrichment_worker: EnrichmentWorker::default(),
            enriched_mineral: None,
            enrichment_error: None,
            enrichment_texture: None,
            show_enrichment_details: true,
            database_status,
            status_message,
            event_logger: EventLogger::new(),
            show_log_viewer: true,
            bottom_panel_tab: BottomPanelTab::EventLog,
            show_live_ocr_window: persisted.show_live_ocr_window,
            duplicate_detection_cooldown_secs: persisted.duplicate_detection_cooldown_secs.max(1),
            live_scanner: LiveScanner::default(),
            live_raw_text: String::new(),
            live_numeric_candidates: Vec::new(),
            live_elapsed_ms: None,
            live_preview_texture: None,
            last_live_event_key: None,
            last_live_event_at: None,
            log_cache: Vec::new(),
            log_cache_at: None,
        }
    }

    fn current_settings(&self) -> AppSettings {
        let mut target_enabled = std::collections::BTreeMap::new();
        for definition in &self.definitions {
            target_enabled.insert(definition.id.clone(), definition.enabled);
        }
        AppSettings {
            selected_monitor_id: self.selected_monitor().map(|m| m.id),
            region: self.region,
            show_region: self.show_region,
            frame_color_rgb: [
                self.frame_color.r(),
                self.frame_color.g(),
                self.frame_color.b(),
            ],
            sound_file: self
                .sound_file
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            sound_volume: self.sound_volume,
            database_path: self
                .database_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            use_scunpacked: self.use_scunpacked,
            use_uex_fallback: self.use_uex_fallback,
            uex_base_url: self.uex_base_url.clone(),
            use_starcitizen_tools: self.use_starcitizen_tools,
            starcitizen_tools_api_url: self.starcitizen_tools_api_url.clone(),
            use_star_citizen_wiki_api: self.use_star_citizen_wiki_api,
            star_citizen_wiki_api_url: self.star_citizen_wiki_api_url.clone(),
            analysis_base_scu: self.analysis_base_scu,
            refining_method_base_efficiency: self.refining_method_base_efficiency.clone(),
            selected_refinery_terminal_id: self.selected_refinery_terminal_id,
            selected_refinery_label: self.selected_refinery_label.clone(),
            refining_strategy: self.refining_strategy.clone(),
            generate_html_reports: self.generate_html_reports,
            clear_reports_on_start: self.clear_reports_on_start,
            minimize_to_tray: self.minimize_to_tray,
            show_live_ocr_window: self.show_live_ocr_window,
            duplicate_detection_cooldown_secs: self.duplicate_detection_cooldown_secs,
            show_signature_matrix: self.show_signature_matrix,
            targets_collapsed: self.targets_collapsed,
            hotkey_scanner: self.hotkey_scanner.clone(),
            hotkey_calibration: self.hotkey_calibration.clone(),
            hotkey_move_mode: self.hotkey_move_mode.clone(),
            target_enabled,
        }
    }

    fn persist_settings(&mut self) {
        if let Err(error) = save_settings(&self.current_settings()) {
            self.status_message = format!("Could not save settings: {error}");
        }
    }

    fn enrichment_config(&self) -> EnrichmentConfig {
        EnrichmentConfig {
            use_uex: self.use_uex_fallback,
            uex_base_url: self.uex_base_url.clone(),
            use_starcitizen_tools: self.use_starcitizen_tools,
            starcitizen_tools_api_url: self.starcitizen_tools_api_url.clone(),
            use_star_citizen_wiki_api: self.use_star_citizen_wiki_api,
            star_citizen_wiki_api_url: self.star_citizen_wiki_api_url.clone(),
            analysis_base_scu: self.analysis_base_scu,
            refining_method_base_efficiency: self.refining_method_base_efficiency.clone(),
            selected_refinery_terminal_id: self.selected_refinery_terminal_id,
            refining_strategy: self.refining_strategy.clone(),
            refinery_options: self.refinery_options.clone(),
            refining_methods: self.refining_methods.clone(),
        }
    }

    fn refresh_refinery_options(&mut self) {
        match load_refinery_options(
            &self.uex_base_url,
            self.use_uex_fallback,
            if self.use_scunpacked {
                self.database_path.as_deref()
            } else {
                None
            },
        ) {
            Ok(options) => {
                self.refinery_options = options;
                self.refining_methods =
                    load_refining_methods(&self.uex_base_url, self.use_uex_fallback);
                if let Some(id) = self.selected_refinery_terminal_id
                    && let Some(selected) =
                        self.refinery_options.iter().find(|r| r.terminal_id == id)
                {
                    self.selected_refinery_label = selected.display_label();
                } else if !self.selected_refinery_label.is_empty()
                    && let Some(selected) = self.refinery_options.iter().find(|r| {
                        r.display_label()
                            .eq_ignore_ascii_case(&self.selected_refinery_label)
                            || r.location
                                .eq_ignore_ascii_case(&self.selected_refinery_label)
                    })
                {
                    self.selected_refinery_terminal_id = Some(selected.terminal_id);
                    self.selected_refinery_label = selected.display_label();
                }
                self.status_message = format!(
                    "{} refineries and {} refining methods loaded from local data{}.",
                    self.refinery_options.len(),
                    self.refining_methods.len(),
                    if self.use_uex_fallback {
                        " + UEX cache/update"
                    } else {
                        " + persisted UEX cache when available"
                    }
                );
            }
            Err(error) => {
                self.status_message = format!("Could not load refinery data: {error:#}");
            }
        }
    }

    fn selected_refinery(&self) -> Option<&RefineryOption> {
        let id = self.selected_refinery_terminal_id?;
        self.refinery_options.iter().find(|r| r.terminal_id == id)
    }

    fn request_enrichment(&mut self, name: &str) {
        let profile = if self.use_scunpacked {
            self.scunpacked_catalog
                .as_ref()
                .and_then(|catalog| catalog.profile_for_name(name))
        } else {
            None
        };
        self.enrichment_error = None;
        self.enrichment_texture = None;
        self.enrichment_worker
            .request(name.to_owned(), profile, self.enrichment_config());
    }

    fn process_enrichment_events(&mut self, ctx: &egui::Context) {
        for event in self.enrichment_worker.drain_events() {
            match event {
                EnrichmentEvent::Ready(enriched) => {
                    self.enrichment_texture = enriched
                        .media
                        .as_ref()
                        .and_then(|media| media.image_bytes.as_ref())
                        .and_then(|bytes| image::load_from_memory(bytes).ok())
                        .map(|image| {
                            let rgba = image.to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            let color =
                                egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                            ctx.load_texture(
                                format!("minerscan-media-{}", enriched.name),
                                color,
                                egui::TextureOptions::LINEAR,
                            )
                        });
                    self.status_message = format!(
                        "Enrichment ready for {} via {}.",
                        enriched.name,
                        enriched.providers_used.join(" + ")
                    );
                    self.event_logger.log_enrichment(&enriched);
                    self.log_cache_at = None;
                    if self.generate_html_reports {
                        if enriched.enrichment_complete {
                            match write_enriched_report(&enriched) {
                                Ok(path) => {
                                    self.last_report = Some(path.clone());
                                    self.status_message.push_str(&format!(
                                        " Report cached: {}.",
                                        path.file_name()
                                            .and_then(|name| name.to_str())
                                            .unwrap_or("report.html")
                                    ));
                                }
                                Err(error) => {
                                    self.status_message.push_str(&format!(
                                        " Report generation failed: {error:#}."
                                    ));
                                }
                            }
                        } else {
                            self.status_message.push_str(
                                " Report deferred: critical enrichment data is still incomplete.",
                            );
                        }
                    }
                    self.enriched_mineral = Some(enriched);
                    self.enrichment_error = None;
                }
                EnrichmentEvent::Error { name, message } => {
                    self.enrichment_error = Some(format!("{name}: {message}"));
                    self.status_message =
                        "Provider enrichment failed; scanner remains active.".to_owned();
                }
            }
        }
    }

    fn evaluate_manual_signature(&mut self) {
        if let Some(value) = normalize_numeric(&self.manual_signature) {
            self.last_detection = match_signature(value, &self.definitions).into_iter().next();
            if let Some(detection) = self.last_detection.clone() {
                self.event_logger
                    .log_detection(&detection, "manual-test", None);
                self.log_cache_at = None;
                self.status_message = format!(
                    "Detected {} x{} (RS {}).",
                    detection.name, detection.multiplier, detection.observed_signature
                );
                if self.sound_file.is_some() {
                    self.play_sound(false);
                }
                self.request_enrichment(&detection.name);
            }
        } else {
            self.last_detection = None;
        }
    }

    fn start_live_scanner(&mut self) {
        let Some(monitor) = self.selected_monitor().cloned() else {
            self.status_message = "Cannot start scanner: no monitor selected.".to_owned();
            return;
        };
        let config = LiveScanConfig {
            monitor_x: monitor.x,
            monitor_y: monitor.y,
            region: self.region,
            interval_ms: 50,
            emit_preview: self.show_live_ocr_window,
        };
        match self.live_scanner.start(config) {
            Ok(()) => {
                self.scanner_active = true;
                self.last_live_event_key = None;
                self.last_live_event_at = None;
                self.event_logger.log_status("STARTED");
                self.log_cache_at = None;
                self.status_message =
                    "Live capture and OCR active. Calibration guides hidden during scan."
                        .to_owned();
            }
            Err(error) => {
                self.scanner_active = false;
                self.status_message = format!("Scanner start failed: {error:#}");
            }
        }
    }

    fn stop_live_scanner(&mut self) {
        self.live_scanner.stop();
        if self.scanner_active {
            self.event_logger.log_status("STOPPED");
            self.log_cache_at = None;
        }
        self.scanner_active = false;
        self.status_message = "Scanner stopped.".to_owned();
    }

    fn process_live_scan_events(&mut self, ctx: &egui::Context) {
        for event in self.live_scanner.drain_events() {
            match event {
                LiveScanEvent::Backend(backend) => {
                    self.event_logger.log_capture_backend(&backend);
                    self.log_cache_at = None;
                    self.status_message = format!("Capture backend: {backend}");
                }
                LiveScanEvent::Ocr {
                    raw_text,
                    numeric_candidates,
                    elapsed_ms,
                    preview_width,
                    preview_height,
                    preview_rgba,
                } => {
                    self.live_raw_text = raw_text;
                    self.live_numeric_candidates = numeric_candidates.clone();
                    self.live_elapsed_ms = Some(elapsed_ms);
                    if self.show_live_ocr_window
                        && preview_width > 0
                        && preview_height > 0
                        && preview_rgba.len()
                            == preview_width as usize * preview_height as usize * 4
                    {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [preview_width as usize, preview_height as usize],
                            &preview_rgba,
                        );
                        if let Some(texture) = &mut self.live_preview_texture {
                            texture.set(image, egui::TextureOptions::NEAREST);
                        } else {
                            self.live_preview_texture = Some(ctx.load_texture(
                                "minerscan-live-ocr-preview",
                                image,
                                egui::TextureOptions::NEAREST,
                            ));
                        }
                    }

                    let detection = numeric_candidates
                        .into_iter()
                        .flat_map(|value| match_signature(value, &self.definitions))
                        .next();
                    if let Some(detection) = detection {
                        self.last_detection = Some(detection.clone());
                        let key = detection.target_id.clone();
                        let now = Instant::now();
                        let cooldown = Duration::from_secs(self.duplicate_detection_cooldown_secs.max(1));
                        let duplicate = is_duplicate_material_detection(
                            self.last_live_event_key.as_deref(),
                            self.last_live_event_at,
                            &key,
                            cooldown,
                            now,
                        );
                        if !duplicate {
                            self.last_live_event_key = Some(key);
                            self.last_live_event_at = Some(now);
                            self.event_logger
                                .log_detection(&detection, "live-ocr", None);
                            self.log_cache_at = None;
                            self.status_message = format!(
                                "LIVE: {} x{} detected (RS {}). Duplicate suppression: {} s.",
                                detection.name,
                                detection.multiplier,
                                detection.observed_signature,
                                self.duplicate_detection_cooldown_secs
                            );
                            if self.sound_file.is_some() {
                                self.play_sound(false);
                            }
                            self.request_enrichment(&detection.name);
                        } else {
                            self.status_message = format!(
                                "LIVE: duplicate {} suppressed by {} s detection cooldown.",
                                detection.name, self.duplicate_detection_cooldown_secs
                            );
                        }
                        self.live_scanner.pause_for(Duration::from_secs(10));
                    }
                }
                LiveScanEvent::Error(error) => {
                    self.status_message = format!("Live OCR error: {error}");
                }
            }
        }
    }

    fn selected_monitor(&self) -> Option<&MonitorInfo> {
        self.monitors.get(self.selected_monitor)
    }

    fn refresh_monitors(&mut self) {
        let old_id = self.selected_monitor().map(|m| m.id);
        self.monitors = detect_monitors();
        self.selected_monitor = old_id
            .and_then(|id| self.monitors.iter().position(|m| m.id == id))
            .unwrap_or(0);
        self.status_message = format!("{} monitor(s) detected.", self.monitors.len());
        self.persist_settings();
    }

    fn choose_sound(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Audio files", &["wav", "mp3", "flac", "ogg"])
            .pick_file()
        {
            self.sound_file = Some(path);
            self.persist_settings();
        }
    }

    fn play_sound(&mut self, update_status: bool) {
        let Some(path) = self.sound_file.clone() else {
            if update_status {
                self.status_message = "Select a sound file first.".to_owned();
            }
            return;
        };
        let volume = self.sound_volume;
        if update_status {
            self.status_message = format!("Testing sound at {:.0}% volume.", volume * 100.0);
        }
        thread::spawn(move || {
            let Ok((_stream, handle)) = OutputStream::try_default() else {
                return;
            };
            let Ok(file) = File::open(path) else {
                return;
            };
            let Ok(source) = Decoder::new(BufReader::new(file)) else {
                return;
            };
            let Ok(sink) = Sink::try_new(&handle) else {
                return;
            };
            sink.set_volume(volume);
            sink.append(source);
            sink.sleep_until_end();
        });
    }

    fn probe_provider(&mut self, provider: ProviderCache, url: String) {
        match cached_get_text(provider, &url, Duration::from_secs(3600)) {
            Ok(body) => {
                self.status_message = format!(
                    "{} provider reachable; {} cached bytes.",
                    provider.label(),
                    body.len()
                );
            }
            Err(error) => {
                self.status_message = format!("{} provider test failed: {error}", provider.label());
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(Color32::TRANSPARENT)
            .inner_margin(egui::Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    if let Some(texture) = &self.logo_texture {
                        let available = ui.available_width().min(500.0);
                        let aspect = texture.size()[0] as f32 / texture.size()[1].max(1) as f32;
                        ui.image((texture.id(), egui::vec2(available, available / aspect)));
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let scanner_active = self.tab == AppTab::Scanner;
                    let scanner_button =
                        egui::Button::new(RichText::new("Scanner").size(12.0).color(
                            if scanner_active {
                                Color32::WHITE
                            } else {
                                Color32::from_rgb(224, 214, 203)
                            },
                        ))
                        .fill(if scanner_active {
                            Color32::from_rgb(58, 50, 43)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::new(
                            1.0_f32,
                            if scanner_active {
                                Color32::from_rgb(238, 145, 57)
                            } else {
                                Color32::TRANSPARENT
                            },
                        ));
                    if ui.add(scanner_button).clicked() {
                        self.tab = AppTab::Scanner;
                    }

                    let settings_active = self.tab == AppTab::Settings;
                    let settings_button =
                        egui::Button::new(RichText::new("Settings").size(12.0).color(
                            if settings_active {
                                Color32::WHITE
                            } else {
                                Color32::from_rgb(224, 214, 203)
                            },
                        ))
                        .fill(if settings_active {
                            Color32::from_rgb(58, 50, 43)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::new(
                            1.0_f32,
                            if settings_active {
                                Color32::from_rgb(238, 145, 57)
                            } else {
                                Color32::TRANSPARENT
                            },
                        ));
                    if ui.add(settings_button).clicked() {
                        self.tab = AppTab::Settings;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (status, color) = if self.scanner_active {
                            ("LIVE", Color32::from_rgb(113, 231, 160))
                        } else {
                            ("STANDBY", Color32::from_rgb(132, 128, 124))
                        };
                        ui.label(RichText::new(status).size(10.0).strong().color(color));
                    });
                });
            });
    }

    fn scanner_view(&mut self, ui: &mut egui::Ui) {
        self.targets_panel(ui);
        ui.add_space(8.0);
        self.ocr_panel(ui);
        ui.add_space(8.0);
        ui.label(
            RichText::new(&self.status_message)
                .small()
                .color(Color32::from_rgb(109, 132, 140)),
        );
    }

    fn targets_panel(&mut self, ui: &mut egui::Ui) {
        let selected_refinery_bonuses = self.selected_refinery().map(|r| r.bonuses.clone());
        let active_count = self
            .definitions
            .iter()
            .filter(|definition| definition.enabled)
            .count();
        egui::Frame::new()
            .fill(Color32::from_rgb(25, 24, 23))
            .corner_radius(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_sized(
                            [24.0, 24.0],
                            egui::Button::new(if self.targets_collapsed { ">" } else { "v" }),
                        )
                        .clicked()
                    {
                        self.targets_collapsed = !self.targets_collapsed;
                        self.persist_settings();
                    }
                    ui.label(
                        RichText::new("MONITOR TARGETS")
                            .strong()
                            .color(Color32::from_rgb(225, 132, 45)),
                    );
                    ui.label(
                        RichText::new(format!("{} active", active_count))
                            .small()
                            .color(Color32::from_rgb(158, 154, 150)),
                    );
                    if active_count > 0 {
                        egui::Frame::new()
                            .fill(Color32::from_rgba_unmultiplied(48, 143, 96, 90))
                            .stroke(Stroke::new(
                                1.0_f32,
                                Color32::from_rgba_unmultiplied(91, 214, 145, 150),
                            ))
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("READY")
                                        .small()
                                        .strong()
                                        .color(Color32::from_rgb(149, 238, 188)),
                                );
                            });
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (scan_label, scan_fill, scan_border) = if self.scanner_active {
                            (
                                "STOP SCANNER",
                                Color32::from_rgb(18, 86, 58),
                                Color32::from_rgb(75, 220, 150),
                            )
                        } else {
                            (
                                "START SCANNER",
                                Color32::from_rgb(104, 30, 35),
                                Color32::from_rgb(232, 82, 88),
                            )
                        };
                        let scan_button = egui::Button::new(
                            RichText::new(scan_label).strong().color(Color32::WHITE),
                        )
                        .fill(scan_fill)
                        .stroke(Stroke::new(1.0_f32, scan_border));
                        if ui
                            .add_enabled_ui(active_count > 0 || self.scanner_active, |ui| {
                                ui.add_sized([132.0, 28.0], scan_button)
                            })
                            .inner
                            .clicked()
                        {
                            if self.scanner_active {
                                self.stop_live_scanner();
                            } else {
                                self.start_live_scanner();
                            }
                        }

                        let values_label = if self.show_signature_matrix {
                            "COMPACT"
                        } else {
                            "SHOW VALUES"
                        };
                        if ui
                            .add_sized([132.0, 28.0], egui::Button::new(values_label))
                            .clicked()
                        {
                            self.show_signature_matrix = !self.show_signature_matrix;
                            self.persist_settings();
                        }

                        if self.last_report.as_ref().is_some_and(|path| path.exists())
                            && ui
                                .add_sized([132.0, 28.0], egui::Button::new("ACTUAL REPORTS"))
                                .clicked()
                        {
                            let _ = Command::new("explorer.exe").arg(reports_dir()).spawn();
                        }
                    });
                });

                ui.add_space(5.0);
                ui.add_sized(
                    [360.0, 24.0],
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search name, type or RS value..."),
                );

                let query = self.search.trim().to_owned();
                let mut settings_dirty = false;

                if self.targets_collapsed {
                    if !query.is_empty() {
                        ui.add_space(5.0);
                        let mut shown = 0usize;
                        for definition in &mut self.definitions {
                            if !definition_matches_query(definition, &query) {
                                continue;
                            }
                            if shown >= 8 {
                                break;
                            }
                            shown += 1;
                            ui.horizontal(|ui| {
                                settings_dirty |= themed_toggle(ui, &mut definition.enabled, "");
                                ui.label(RichText::new(&definition.canonical_name).strong());
                                ui.label(
                                    RichText::new(definition.category.label())
                                        .small()
                                        .color(Color32::from_rgb(158, 154, 150)),
                                );
                                if let Some(multiplier) =
                                    definition_match_multiplier(definition, &query)
                                {
                                    ui.label(
                                        RichText::new(format!(
                                            "RS {} = x{}",
                                            definition.base_signature.saturating_mul(multiplier),
                                            multiplier
                                        ))
                                        .small()
                                        .color(Color32::from_rgb(225, 132, 45)),
                                    );
                                }
                            });
                        }
                        if shown == 0 {
                            ui.label(
                                RichText::new("No matching target or RS value.")
                                    .small()
                                    .color(Color32::from_rgb(158, 154, 150)),
                            );
                        }
                    }
                } else if self.show_signature_matrix {
                    let uex_links_enabled = self.use_uex_fallback;
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_sized([42.0, 18.0], egui::Label::new(""));
                        ui.add_sized(
                            [220.0, 18.0],
                            egui::Label::new(RichText::new("Target").strong()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if selected_refinery_bonuses.is_some() {
                                ui.add_sized(
                                    [76.0, 18.0],
                                    egui::Label::new(RichText::new("Refinery").strong()),
                                );
                            }
                            ui.add_sized(
                                [92.0, 18.0],
                                egui::Label::new(RichText::new("Type").strong()),
                            );
                            for label in ["x5", "x4", "x3", "x2", "x1"] {
                                ui.add_sized(
                                    [72.0, 18.0],
                                    egui::Label::new(RichText::new(label).strong()),
                                );
                            }
                        });
                    });
                    egui::ScrollArea::vertical()
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .max_height(520.0)
                        .show(ui, |ui| {
                            for definition in &mut self.definitions {
                                if !definition_matches_query(definition, &query) {
                                    continue;
                                }
                                ui.horizontal(|ui| {
                                    settings_dirty |=
                                        themed_toggle(ui, &mut definition.enabled, "");
                                    ui.add_sized(
                                        [220.0, 18.0],
                                        egui::Label::new(
                                            RichText::new(&definition.canonical_name).strong(),
                                        ),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if let Some(bonuses) = &selected_refinery_bonuses {
                                                let bonus = bonuses
                                                    .get(&refinery_material_key(
                                                        &definition.canonical_name,
                                                    ))
                                                    .copied();
                                                let (text, color) = match bonus {
                                                    Some(value) if value > 0.0 => (
                                                        format!("+{value:.0}%"),
                                                        Color32::from_rgb(104, 220, 150),
                                                    ),
                                                    Some(value) if value < 0.0 => (
                                                        format!("{value:.0}%"),
                                                        Color32::from_rgb(232, 92, 96),
                                                    ),
                                                    Some(_) => (
                                                        "0%".to_owned(),
                                                        Color32::from_rgb(145, 160, 168),
                                                    ),
                                                    None => (
                                                        "-".to_owned(),
                                                        Color32::from_rgb(90, 112, 121),
                                                    ),
                                                };
                                                ui.add_sized(
                                                    [76.0, 18.0],
                                                    egui::Label::new(
                                                        RichText::new(text).strong().color(color),
                                                    ),
                                                );
                                            }
                                            let type_text =
                                                RichText::new(definition.category.label())
                                                    .color(Color32::from_rgb(158, 154, 150));
                                            if uex_links_enabled
                                                && definition.category
                                                    == SignatureCategory::Resource
                                            {
                                                ui.add_sized(
                                                    [92.0, 18.0],
                                                    egui::Hyperlink::from_label_and_url(
                                                        type_text,
                                                        uex_commodity_url(
                                                            &definition.canonical_name,
                                                        ),
                                                    ),
                                                );
                                            } else {
                                                ui.add_sized(
                                                    [92.0, 18.0],
                                                    egui::Label::new(type_text),
                                                );
                                            }
                                            for multiplier in (1..=5).rev() {
                                                let value = if multiplier == 1
                                                    || definition.multiplicative
                                                {
                                                    definition
                                                        .base_signature
                                                        .saturating_mul(multiplier)
                                                        .to_string()
                                                } else {
                                                    "-".to_owned()
                                                };
                                                ui.add_sized(
                                                    [72.0, 18.0],
                                                    egui::Label::new(RichText::new(value)),
                                                );
                                            }
                                        },
                                    );
                                });
                            }
                        });
                } else {
                    egui::ScrollArea::vertical()
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .max_height(280.0)
                        .show(ui, |ui| {
                            for definition in &mut self.definitions {
                                if !definition_matches_query(definition, &query) {
                                    continue;
                                }
                                ui.horizontal(|ui| {
                                    settings_dirty |=
                                        themed_toggle(ui, &mut definition.enabled, "");
                                    ui.label(RichText::new(&definition.canonical_name).strong());
                                    ui.label(
                                        RichText::new(definition.category.label())
                                            .small()
                                            .color(Color32::from_rgb(158, 154, 150)),
                                    );
                                    if let Some(bonuses) = &selected_refinery_bonuses
                                        && definition.category == SignatureCategory::Resource
                                        && let Some(value) = bonuses
                                            .get(&refinery_material_key(&definition.canonical_name))
                                    {
                                        let color = if *value > 0.0 {
                                            Color32::from_rgb(104, 220, 150)
                                        } else if *value < 0.0 {
                                            Color32::from_rgb(232, 92, 96)
                                        } else {
                                            Color32::from_rgb(145, 160, 168)
                                        };
                                        ui.label(
                                            RichText::new(format!("{value:+.0}%"))
                                                .small()
                                                .strong()
                                                .color(color),
                                        );
                                    }
                                    if let Some(multiplier) =
                                        definition_match_multiplier(definition, &query)
                                    {
                                        ui.label(
                                            RichText::new(format!("x{}", multiplier))
                                                .small()
                                                .color(Color32::from_rgb(225, 132, 45)),
                                        );
                                    }
                                });
                            }
                        });
                }

                if settings_dirty {
                    self.persist_settings();
                }
            });
    }
    fn ocr_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(Color32::from_rgb(25, 24, 23))
            .corner_radius(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("LIVE DETECTION:")
                            .strong()
                            .color(Color32::from_rgb(225, 132, 45)),
                    );
                    let potential = self.live_numeric_candidates.iter().find_map(|value| {
                        resolve_signature(*value, &self.definitions)
                            .into_iter()
                            .find(|candidate| {
                                !self
                                    .definitions
                                    .iter()
                                    .find(|definition| definition.id == candidate.target_id)
                                    .is_some_and(|definition| definition.enabled)
                            })
                            .map(|candidate| {
                                format!(
                                    "> Potential Candidate found: {} x{} (RS {}) <",
                                    candidate.name, candidate.multiplier, value
                                )
                            })
                    });
                    if let Some(potential) = potential {
                        ui.label(
                            RichText::new(potential)
                                .strong()
                                .color(Color32::from_rgb(225, 132, 45)),
                        );
                    } else if self.scanner_active {
                        ui.label(
                            RichText::new("Scanning...")
                                .small()
                                .color(Color32::from_rgb(158, 154, 150)),
                        );
                    } else {
                        ui.label(
                            RichText::new("Standby")
                                .small()
                                .color(Color32::from_rgb(132, 128, 124)),
                        );
                    }
                    if let Some(ms) = self.live_elapsed_ms {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format!("{} ms", ms)).small());
                        });
                    }
                });

                if let Some(detection) = &self.last_detection {
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(54, 45, 37))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(219, 124, 40)))
                        .corner_radius(5.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("DETECTED")
                                        .small()
                                        .strong()
                                        .color(Color32::from_rgb(72, 224, 186)),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "{} x{}",
                                        detection.name.to_uppercase(),
                                        detection.multiplier
                                    ))
                                    .size(18.0)
                                    .strong(),
                                );
                                ui.label(
                                    RichText::new(detection.category.label())
                                        .small()
                                        .color(Color32::from_rgb(115, 147, 158)),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.monospace(format!(
                                            "RS {} | BASE {}",
                                            detection.observed_signature, detection.base_signature
                                        ));
                                    },
                                );
                            });
                            if let Some(profile) = self
                                .scunpacked_catalog
                                .as_ref()
                                .and_then(|catalog| catalog.profile_for_name(&detection.name))
                            {
                                if let Some(record) =
                                    profile.raw.as_ref().or(profile.refined.as_ref())
                                {
                                    ui.add_space(4.0);
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "SCUnpacked: {}",
                                                profile.canonical_name
                                            ))
                                            .small()
                                            .strong()
                                            .color(Color32::from_rgb(129, 205, 225)),
                                        );
                                        ui.monospace(format!("UUID {}", record.uuid));
                                        if let Some(tier) = &record.tier {
                                            ui.label(format!("Tier {tier}"));
                                        }
                                    });
                                    if !record.description.is_empty() {
                                        ui.label(
                                            RichText::new(&record.description)
                                                .small()
                                                .color(Color32::from_rgb(158, 185, 194)),
                                        );
                                    }
                                }
                            }
                        });
                }

                if let Some(name) = self.enrichment_worker.in_flight() {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new(format!(
                                "Loading market, refinery and media data for {name}..."
                            ))
                            .small()
                            .color(Color32::from_rgb(158, 154, 150)),
                        );
                    });
                }
                if self.enriched_mineral.is_some() {
                    ui.add_space(8.0);
                    self.enrichment_details_ui(ui);
                }
                if let Some(error) = &self.enrichment_error {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(error)
                            .small()
                            .color(Color32::from_rgb(232, 118, 118)),
                    );
                }

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Manual test");
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.manual_signature).desired_width(100.0),
                    );
                    if response.changed()
                        || ui.button("Evaluate").clicked()
                        || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        self.evaluate_manual_signature();
                    }
                });
            });
    }

    fn enrichment_details_ui(&mut self, ui: &mut egui::Ui) {
        let Some(enriched) = self.enriched_mineral.clone() else {
            return;
        };

        egui::Frame::new()
            .fill(Color32::from_rgb(5, 18, 26))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(22, 91, 112)))
            .corner_radius(5.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("ENRICHED MINING DATA")
                            .strong()
                            .color(Color32::from_rgb(225, 132, 45)),
                    );
                    ui.label(
                        RichText::new(format!("{} SCU basis", enriched.analysis_scu))
                            .small()
                            .color(Color32::from_rgb(158, 154, 150)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if self.show_enrichment_details {
                            "Collapse"
                        } else {
                            "Expand"
                        };
                        if ui.button(label).clicked() {
                            self.show_enrichment_details = !self.show_enrichment_details;
                        }
                    });
                });

                if !self.show_enrichment_details {
                    return;
                }

                ui.add_space(6.0);
                ui.horizontal_top(|ui| {
                    if let Some(texture) = &self.enrichment_texture {
                        ui.add(egui::Image::new(texture).max_width(220.0).max_height(125.0));
                        ui.add_space(8.0);
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&enriched.name).size(17.0).strong());
                        if let Some(uuid) = &enriched.uuid {
                            ui.label(
                                RichText::new(format!("UUID {uuid}"))
                                    .small()
                                    .color(Color32::from_rgb(109, 139, 149)),
                            );
                        }
                        if let Some(tier) = &enriched.tier {
                            ui.label(format!("Tier: {tier}"));
                        }
                        if let Some(density) = enriched.density_g_per_cc {
                            ui.label(format!("Density: {density:.2} g/cc"));
                        }
                        if !enriched.commodity_groups.is_empty() {
                            ui.label(format!("Groups: {}", enriched.commodity_groups.join(", ")));
                        }
                        ui.label(
                            RichText::new(format!(
                                "Sources: {}",
                                if enriched.providers_used.is_empty() {
                                    "local only".to_owned()
                                } else {
                                    enriched.providers_used.join(" + ")
                                }
                            ))
                            .small()
                            .color(Color32::from_rgb(83, 179, 204)),
                        );
                    });
                });

                if let Some(description) = &enriched.description {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(description)
                            .small()
                            .color(Color32::from_rgb(169, 198, 207)),
                    );
                }

                ui.add_space(8.0);
                ui.columns(3, |columns| {
                    columns[0].label(
                        RichText::new("RAW SALE")
                            .strong()
                            .color(Color32::from_rgb(225, 132, 45)),
                    );
                    if let Some(raw) = &enriched.raw_market {
                        columns[0].label(format!("{:.0} aUEC / SCU", raw.price_per_scu));
                        columns[0].label(format!(
                            "{:.0} aUEC @ {:.1} SCU",
                            enriched.raw_value.unwrap_or(0.0),
                            enriched.analysis_scu
                        ));
                        columns[0].label(format!("Best: {}", raw.location));
                        if !raw.terminal.is_empty() && raw.terminal != raw.location {
                            columns[0].label(
                                RichText::new(&raw.terminal)
                                    .small()
                                    .color(Color32::from_rgb(158, 154, 150)),
                            );
                        }
                        if !raw.system.is_empty() {
                            columns[0].label(
                                RichText::new(format!("System: {}", raw.system))
                                    .small()
                                    .color(Color32::from_rgb(158, 154, 150)),
                            );
                        }
                    } else {
                        columns[0].label("No current RAW quote");
                    }

                    columns[1].label(
                        RichText::new("REFINERY")
                            .strong()
                            .color(Color32::from_rgb(225, 132, 45)),
                    );
                    if let Some(refinery) = &enriched.refinery {
                        columns[1].label(format!(
                            "{} ({:+.0}% yield)",
                            refinery.location, refinery.yield_bonus_percent
                        ));
                        if !refinery.terminal.is_empty() && refinery.terminal != refinery.location {
                            columns[1].label(
                                RichText::new(&refinery.terminal)
                                    .small()
                                    .color(Color32::from_rgb(158, 154, 150)),
                            );
                        }
                        if !refinery.system.is_empty() {
                            columns[1].label(
                                RichText::new(format!("System: {}", refinery.system))
                                    .small()
                                    .color(Color32::from_rgb(158, 154, 150)),
                            );
                        }
                        if let Some(method) = &refinery.method_name {
                            columns[1].label(format!("Method: {method}"));
                        }
                        if let (Some(yield_rating), Some(cost_rating), Some(speed_rating)) = (
                            refinery.method_yield_rating,
                            refinery.method_cost_rating,
                            refinery.method_speed_rating,
                        ) {
                            columns[1].label(
                                RichText::new(format!(
                                    "Ratings Y{yield_rating} / C{cost_rating} / S{speed_rating}"
                                ))
                                .small()
                                .color(Color32::from_rgb(158, 154, 150)),
                            );
                        }
                        columns[1].label(format!(
                            "Expected output: {:.2} SCU",
                            refinery.estimated_refined_scu
                        ));
                        if let Some(cost) = refinery.estimated_cost {
                            columns[1].label(format!("Estimated cost: {cost:.0} aUEC"));
                        }
                        if let Some(minutes) = refinery.estimated_minutes {
                            columns[1]
                                .label(format!("Estimated time: {}", format_minutes(minutes)));
                        }
                        columns[1].label(
                            RichText::new(&refinery.estimate_basis)
                                .small()
                                .color(Color32::from_rgb(158, 154, 150)),
                        );
                    } else {
                        columns[1].label("No refinery recommendation");
                    }

                    columns[2].label(
                        RichText::new("REFINED SALE")
                            .strong()
                            .color(Color32::from_rgb(225, 132, 45)),
                    );
                    if let Some(refined) = &enriched.refined_market {
                        columns[2].label(format!("{:.0} aUEC / SCU", refined.price_per_scu));
                        columns[2].label(format!(
                            "Gross: {:.0} aUEC",
                            enriched.refined_gross_value.unwrap_or(0.0)
                        ));
                        if let Some(net) = enriched.estimated_net_value {
                            columns[2].label(
                                RichText::new(format!("Est. net: {net:.0} aUEC"))
                                    .strong()
                                    .color(Color32::from_rgb(113, 231, 160)),
                            );
                            if let Some(raw) = enriched.raw_value {
                                columns[2].label(format!("Delta vs RAW: {:+.0} aUEC", net - raw));
                            }
                        }
                        columns[2].label(format!("Best: {}", refined.location));
                        if !refined.terminal.is_empty() && refined.terminal != refined.location {
                            columns[2].label(
                                RichText::new(&refined.terminal)
                                    .small()
                                    .color(Color32::from_rgb(158, 154, 150)),
                            );
                        }
                    } else {
                        columns[2].label("No current refined quote");
                    }
                });

                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if self.use_uex_fallback {
                        ui.hyperlink_to("Open UEX", uex_commodity_url(&enriched.name));
                    }
                    if let Some(media) = &enriched.media {
                        if let Some(page_url) = &media.page_url {
                            ui.hyperlink_to(format!("Open {}", media.source), page_url);
                        }
                        if let Some(image_url) = &media.image_url {
                            ui.hyperlink_to("Open media", image_url);
                        }
                    }
                    if ui.button("Refresh enrichment").clicked() {
                        self.request_enrichment(&enriched.name);
                    }
                });

                if !enriched.warnings.is_empty() {
                    ui.add_space(6.0);
                    for warning in &enriched.warnings {
                        ui.label(
                            RichText::new(format!("Warning: {warning}"))
                                .small()
                                .color(Color32::from_rgb(218, 177, 93)),
                        );
                    }
                }
            });
    }

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            self.display_settings(&mut columns[0]);
            self.sound_settings(&mut columns[1]);
        });
        ui.add_space(10.0);
        self.hotkey_settings(ui);
        ui.add_space(10.0);
        self.runtime_settings(ui);
        ui.add_space(10.0);
        self.detection_settings(ui);
        ui.add_space(10.0);
        self.refining_calculator_settings(ui);
        ui.add_space(10.0);
        self.database_settings(ui);
    }

    fn display_settings(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(Color32::from_rgb(25, 24, 23))
            .corner_radius(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_min_height(205.0);
                ui.label(
                    RichText::new("DISPLAY AND CAPTURE")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                ui.horizontal(|ui| {
                    ui.label("Monitor");
                    let selected_text = self
                        .selected_monitor()
                        .map(|m| {
                            format!(
                                "Monitor {} | {} x {}",
                                self.selected_monitor + 1,
                                m.width,
                                m.height
                            )
                        })
                        .unwrap_or_else(|| "No monitor detected".to_owned());
                    let before = self.selected_monitor;
                    egui::ComboBox::from_id_salt("monitor_selector")
                        .selected_text(selected_text)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for (index, monitor) in self.monitors.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.selected_monitor,
                                    index,
                                    format!(
                                        "Monitor {} | {} x {}",
                                        index + 1,
                                        monitor.width,
                                        monitor.height
                                    ),
                                );
                            }
                        });
                    if before != self.selected_monitor {
                        self.persist_settings();
                    }
                    if ui.button("Refresh").clicked() {
                        self.refresh_monitors();
                    }
                });

                let monitor = self.selected_monitor().cloned();
                if let Some(monitor) = monitor {
                    ui.label(
                        RichText::new(format!(
                            "Desktop {} x {} | origin {}, {}",
                            monitor.width, monitor.height, monitor.x, monitor.y
                        ))
                        .small()
                        .color(Color32::from_rgb(158, 154, 150)),
                    );
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        ui.add_sized([78.0, 18.0], egui::Label::new("Capture size"));
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.region.width)
                                    .prefix("w ")
                                    .range(20..=monitor.width.max(20)),
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.region.height)
                                    .prefix("h ")
                                    .range(20..=monitor.height.max(20)),
                            )
                            .changed();
                    });
                    self.region.width = self.region.width.min(monitor.width.max(20));
                    self.region.height = self.region.height.min(monitor.height.max(20));
                    let max_x = monitor.width.saturating_sub(self.region.width) as i32;
                    let max_y = monitor.height.saturating_sub(self.region.height) as i32;
                    self.region.x = self.region.x.clamp(0, max_x.max(0));
                    self.region.y = self.region.y.clamp(0, max_y.max(0));

                    ui.horizontal(|ui| {
                        ui.add_sized([78.0, 18.0], egui::Label::new("X position"));
                        changed |= ui
                            .add_sized(
                                [190.0, 18.0],
                                egui::Slider::new(&mut self.region.x, 0..=max_x.max(0))
                                    .show_value(false),
                            )
                            .changed();
                        changed |= ui
                            .add(egui::DragValue::new(&mut self.region.x).prefix("x "))
                            .changed();
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([78.0, 18.0], egui::Label::new("Y position"));
                        changed |= ui
                            .add_sized(
                                [190.0, 18.0],
                                egui::Slider::new(&mut self.region.y, 0..=max_y.max(0))
                                    .show_value(false),
                            )
                            .changed();
                        changed |= ui
                            .add(egui::DragValue::new(&mut self.region.y).prefix("y "))
                            .changed();
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.add_sized([78.0, 18.0], egui::Label::new("Manual ROI"));
                        changed |= ui
                            .add(egui::DragValue::new(&mut self.region.x).prefix("x "))
                            .changed();
                        changed |= ui
                            .add(egui::DragValue::new(&mut self.region.y).prefix("y "))
                            .changed();
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.region.width)
                                    .prefix("w ")
                                    .range(20..=monitor.width.max(20)),
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.region.height)
                                    .prefix("h ")
                                    .range(20..=monitor.height.max(20)),
                            )
                            .changed();
                    });
                    if changed {
                        self.region.x = self
                            .region
                            .x
                            .clamp(0, monitor.width.saturating_sub(self.region.width) as i32);
                        self.region.y = self
                            .region
                            .y
                            .clamp(0, monitor.height.saturating_sub(self.region.height) as i32);
                        self.persist_settings();
                    }
                }

                ui.horizontal(|ui| {
                    let mut show = self.show_region;
                    if themed_toggle(ui, &mut show, "Show calibration overlay") {
                        self.show_region = show;
                        self.persist_settings();
                    }
                    ui.label("Color");
                    if ui.color_edit_button_srgba(&mut self.frame_color).changed() {
                        self.persist_settings();
                    }
                });
                ui.label(
                    RichText::new(if self.calibration_move_mode {
                        "Move mode ACTIVE - use arrow keys (1 px per step)."
                    } else if self.scanner_active {
                        "Calibration overlay is hidden while live OCR is active."
                    } else {
                        "Calibration overlay is a single taskbar-hidden positioning window."
                    })
                    .small()
                    .color(if self.calibration_move_mode {
                        Color32::from_rgb(113, 231, 160)
                    } else {
                        Color32::from_rgb(132, 128, 124)
                    }),
                );
            });
    }
    fn sound_settings(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(Color32::from_rgb(25, 24, 23))
            .corner_radius(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_min_height(205.0);
                ui.label(
                    RichText::new("SIGNAL SOUND")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                ui.horizontal(|ui| {
                    if ui.button("Select sound file").clicked() {
                        self.choose_sound();
                    }
                    let name = self
                        .sound_file
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("No sound selected");
                    ui.label(name);
                    if ui.button("Test").clicked() {
                        self.play_sound(true);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Volume");
                    if ui
                        .add_sized(
                            [220.0, 18.0],
                            egui::Slider::new(&mut self.sound_volume, 0.0..=1.0).show_value(false),
                        )
                        .changed()
                    {
                        self.persist_settings();
                    }
                    ui.monospace(format!("{}%", (self.sound_volume * 100.0).round() as u32));
                });
            });
    }

    fn hotkey_settings(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(Color32::from_rgb(25, 24, 23))
            .corner_radius(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("GLOBAL HOTKEYS")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                ui.label(
                    RichText::new(
                        "Hotkeys work while Star Citizen or another application has focus.",
                    )
                    .small()
                    .color(Color32::from_rgb(158, 154, 150)),
                );
                egui::Grid::new("hotkey_settings_grid")
                    .num_columns(3)
                    .spacing([12.0, 7.0])
                    .show(ui, |ui| {
                        ui.label("Start / stop scanner");
                        ui.add_sized(
                            [220.0, 22.0],
                            egui::TextEdit::singleline(&mut self.hotkey_scanner),
                        );
                        ui.label(
                            RichText::new("Default: NumpadEnter")
                                .small()
                                .color(Color32::from_rgb(132, 128, 124)),
                        );
                        ui.end_row();
                        ui.label("Show / hide calibration");
                        ui.add_sized(
                            [220.0, 22.0],
                            egui::TextEdit::singleline(&mut self.hotkey_calibration),
                        );
                        ui.label(
                            RichText::new("Default: NumpadAdd (+)")
                                .small()
                                .color(Color32::from_rgb(132, 128, 124)),
                        );
                        ui.end_row();
                        ui.label("Calibration move mode");
                        ui.add_sized(
                            [220.0, 22.0],
                            egui::TextEdit::singleline(&mut self.hotkey_move_mode),
                        );
                        ui.label(
                            RichText::new("Default: NumpadSubtract (-)")
                                .small()
                                .color(Color32::from_rgb(132, 128, 124)),
                        );
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    if ui.button("Apply hotkeys").clicked() {
                        self.rebind_hotkeys();
                    }
                    if ui.button("Restore defaults").clicked() {
                        self.hotkey_scanner = "NumpadEnter".to_owned();
                        self.hotkey_calibration = "NumpadAdd".to_owned();
                        self.hotkey_move_mode = "NumpadSubtract".to_owned();
                        self.rebind_hotkeys();
                    }
                    ui.label(
                        RichText::new(if self.calibration_move_mode {
                            "MOVE MODE ACTIVE - arrow keys reposition the capture region"
                        } else {
                            "Move mode off"
                        })
                        .small()
                        .color(if self.calibration_move_mode {
                            Color32::from_rgb(113, 231, 160)
                        } else {
                            Color32::from_rgb(132, 128, 124)
                        }),
                    );
                });
            });
    }

    fn runtime_settings(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_width();
        egui::Frame::new()
            .fill(Color32::from_rgb(25, 24, 23))
            .corner_radius(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_min_width((available - 24.0).max(320.0));
                ui.label(
                    RichText::new("TRAY AND REPORTS")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                let content_width = (ui.available_width() - 170.0).max(300.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(content_width);
                        let mut changed = false;
                        changed |= themed_toggle(
                            ui,
                            &mut self.minimize_to_tray,
                            "Minimize/close application window to system tray",
                        );
                        changed |= themed_toggle(
                            ui,
                            &mut self.generate_html_reports,
                            "Create HTML report after successful enrichment",
                        );
                        changed |= themed_toggle(
                            ui,
                            &mut self.clear_reports_on_start,
                            "Delete cached session reports on next application start",
                        );
                        if changed {
                            self.persist_settings();
                        }
                        ui.add_space(5.0);
                        ui.label("Session reports");
                        ui.label(
                            RichText::new(reports_dir().display().to_string())
                                .small()
                                .color(Color32::from_rgb(158, 154, 150)),
                        );
                        ui.label(
                            RichText::new(
                                "Reports embed mineral/location media inline and remain self-contained.",
                            )
                            .small()
                            .color(Color32::from_rgb(132, 128, 124)),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        ui.vertical(|ui| {
                            if ui.add_sized([145.0, 28.0], egui::Button::new("OPEN FOLDER")).clicked() {
                                let _ = Command::new("explorer.exe").arg(reports_dir()).spawn();
                            }
                            if ui
                                .add_enabled_ui(self.last_report.is_some(), |ui| {
                                    ui.add_sized([145.0, 28.0], egui::Button::new("OPEN LAST REPORT"))
                                })
                                .inner
                                .clicked()
                            {
                                self.open_last_report();
                            }
                        });
                    });
                });
            });
    }

    fn detection_settings(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_width();
        egui::Frame::new()
            .fill(Color32::from_rgb(25, 24, 23))
            .corner_radius(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_min_width((available - 24.0).max(320.0));
                ui.label(
                    RichText::new("DETECTIONS")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                let mut changed = false;
                changed |= themed_toggle(
                    ui,
                    &mut self.show_live_ocr_window,
                    "Enable LIVE OCR preview tab",
                );
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [190.0, 18.0],
                        egui::Label::new("Same-material cooldown"),
                    );
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.duplicate_detection_cooldown_secs)
                                .range(1..=3600)
                                .speed(5.0)
                                .suffix(" s"),
                        )
                        .changed();
                    ui.label(
                        RichText::new(
                            "Suppresses repeated detection, sound, enrichment and report generation for the same material.",
                        )
                        .small()
                        .color(Color32::from_rgb(132, 128, 124)),
                    );
                });
                ui.label(
                    RichText::new(
                        "Default: 120 s. OCR continues scanning; only duplicate events for the immediately repeated material are suppressed.",
                    )
                    .small()
                    .color(Color32::from_rgb(132, 128, 124)),
                );
                if changed {
                    if !self.show_live_ocr_window
                        && self.bottom_panel_tab == BottomPanelTab::LiveOcr
                    {
                        self.bottom_panel_tab = BottomPanelTab::EventLog;
                    }
                    self.persist_settings();
                }
            });
    }

    fn refining_calculator_settings(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_width();
        egui::Frame::new()
            .fill(Color32::from_rgb(25, 24, 23))
            .corner_radius(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_min_width((available - 24.0).max(320.0));
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 7.0);
                ui.label(
                    RichText::new("REFINING CALCULATOR")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                ui.label(
                    RichText::new(
                        "Calculator data is local-first. SCUnpacked supplies the refinery location baseline; an existing UEX cache is reused offline and UEX may update refinery modifiers/method ratings when enabled. Editable method base efficiencies are never overwritten.",
                    )
                    .small()
                    .color(Color32::from_rgb(113, 143, 153)),
                );

                let mut changed = false;
                ui.horizontal(|ui| {
                    ui.add_sized([120.0, 18.0], egui::Label::new("Analysis basis"));
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.analysis_base_scu)
                                .range(0.1..=10000.0)
                                .speed(1.0)
                                .suffix(" SCU"),
                        )
                        .changed();
                    ui.label(
                        RichText::new("Used for output, cost and profit estimates")
                            .small()
                            .color(Color32::from_rgb(132, 128, 124)),
                    );
                });

                let refinery_choices: Vec<(i64, String)> = self
                    .refinery_options
                    .iter()
                    .map(|r| (r.terminal_id, r.display_label()))
                    .collect();
                ui.horizontal(|ui| {
                    ui.add_sized([120.0, 18.0], egui::Label::new("Preferred refinery"));
                    let selected_text = if self.selected_refinery_label.is_empty() {
                        "Select refinery".to_owned()
                    } else {
                        self.selected_refinery_label.clone()
                    };
                    egui::ComboBox::from_id_salt("preferred_refinery_selector")
                        .selected_text(selected_text)
                        .width(320.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(self.selected_refinery_terminal_id.is_none(), "No refinery selected")
                                .clicked()
                            {
                                self.selected_refinery_terminal_id = None;
                                self.selected_refinery_label.clear();
                                changed = true;
                            }
                            for (id, label) in &refinery_choices {
                                if ui
                                    .selectable_label(self.selected_refinery_terminal_id == Some(*id), label)
                                    .clicked()
                                {
                                    self.selected_refinery_terminal_id = Some(*id);
                                    self.selected_refinery_label = label.clone();
                                    changed = true;
                                }
                            }
                        });
                    if ui.button("Refresh refinery data").clicked() {
                        self.refresh_refinery_options();
                    }
                });

                ui.horizontal(|ui| {
                    ui.add_sized([120.0, 18.0], egui::Label::new("Optimization"));
                    let strategy_label = match self.refining_strategy.as_str() {
                        "cost" => "Cost optimized",
                        "time" => "Time optimized",
                        _ => "Efficiency optimized",
                    };
                    egui::ComboBox::from_id_salt("refining_strategy_selector")
                        .selected_text(strategy_label)
                        .width(190.0)
                        .show_ui(ui, |ui| {
                            for (value, label) in [
                                ("efficiency", "Efficiency optimized"),
                                ("cost", "Cost optimized"),
                                ("time", "Time optimized"),
                            ] {
                                if ui
                                    .selectable_label(self.refining_strategy == value, label)
                                    .clicked()
                                {
                                    self.refining_strategy = value.to_owned();
                                    changed = true;
                                }
                            }
                        });
                    ui.label(
                        RichText::new("Method is selected per material according to this strategy.")
                            .small()
                            .color(Color32::from_rgb(132, 128, 124)),
                    );
                });

                if let Some(refinery) = self.selected_refinery() {
                    let source_hint = if refinery.bonuses.is_empty() {
                        "SCUnpacked location baseline; no cached material modifiers yet"
                    } else if self.use_uex_fallback {
                        "local baseline + UEX cache/update"
                    } else {
                        "local baseline + persisted UEX cache"
                    };
                    ui.label(
                        RichText::new(format!(
                            "Selected: {} Ã‚Â· {} material modifiers Ã‚Â· {}",
                            refinery.display_label(),
                            refinery.bonuses.len(),
                            source_hint
                        ))
                        .small()
                        .color(Color32::from_rgb(104, 220, 150)),
                    );
                }

                ui.add_space(5.0);
                let methods = self.refining_methods.clone();
                egui::Grid::new("refining_base_efficiency_grid")
                    .num_columns(6)
                    .spacing([16.0, 5.0])
                    .show(ui, |ui| {
                        for header in ["Method", "Code", "Base efficiency", "Yield", "Cost", "Speed"] {
                            ui.label(RichText::new(header).strong().color(Color32::WHITE));
                        }
                        ui.end_row();
                        for method in methods {
                            ui.label(
                                RichText::new(&method.name)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.label(
                                RichText::new(&method.code)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            if let Some(value) = self.refining_method_base_efficiency.get_mut(&method.code) {
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(value)
                                            .range(0.0..=100.0)
                                            .speed(0.5)
                                            .suffix(" %"),
                                    )
                                    .changed();
                            } else {
                                ui.label("-");
                            }
                            ui.label(RichText::new(method.yield_rating.to_string()).strong().color(Color32::WHITE));
                            ui.label(RichText::new(method.cost_rating.to_string()).strong().color(Color32::WHITE));
                            ui.label(RichText::new(method.speed_rating.to_string()).strong().color(Color32::WHITE));
                            ui.end_row();
                        }
                    });
                if ui.button("Reset refining assumptions to 50/55/60%").clicked() {
                    self.refining_method_base_efficiency =
                        default_refining_method_base_efficiency();
                    changed = true;
                }
                if changed {
                    self.persist_settings();
                }
            });
    }

    fn database_settings(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_width();
        egui::Frame::new()
            .fill(Color32::from_rgb(25, 24, 23))
            .corner_radius(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_min_width((available - 24.0).max(320.0));
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 7.0);
                ui.label(
                    RichText::new("DATA SOURCES")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                ui.label(
                    RichText::new("SCUnpacked updates the local game-data/refinery-location baseline. UEX is optional and updates dynamic market data plus cached refinery modifiers/method ratings.")
                        .small()
                        .color(Color32::from_rgb(113, 143, 153)),
                );
                let previous_uex_enabled = self.use_uex_fallback;
                let previous_scunpacked_enabled = self.use_scunpacked;
                let mut source_settings_changed = false;
                source_settings_changed |= themed_toggle(
                    ui,
                    &mut self.use_scunpacked,
                    "Use SCUnpacked game data",
                );
                source_settings_changed |= themed_toggle(
                    ui,
                    &mut self.use_uex_fallback,
                    "Use UEX API fallback for missing/current price and refinery data",
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_sized([120.0, 18.0], egui::Label::new("UEX endpoint"));
                    source_settings_changed |= ui
                        .add_sized(
                            [420.0, 22.0],
                            egui::TextEdit::singleline(&mut self.uex_base_url),
                        )
                        .changed();
                    if ui.button("Test + cache").clicked() {
                        let url = format!("{}/commodities", self.uex_base_url.trim_end_matches('/'));
                        self.probe_provider(ProviderCache::Uex, url);
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("MEDIA ENRICHMENT")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                source_settings_changed |= themed_toggle(
                    ui,
                    &mut self.use_starcitizen_tools,
                    "Use StarCitizen.Tools for wiki text and page media",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([120.0, 18.0], egui::Label::new("Tools API"));
                    source_settings_changed |= ui
                        .add_sized(
                            [420.0, 22.0],
                            egui::TextEdit::singleline(&mut self.starcitizen_tools_api_url),
                        )
                        .changed();
                    if ui.button("Test + cache").clicked() {
                        let separator = if self.starcitizen_tools_api_url.contains('?') { '&' } else { '?' };
                        let url = format!(
                            "{}{}action=query&format=json&generator=search&gsrsearch=Bexalite&prop=extracts|pageimages&exintro=1&piprop=thumbnail&pithumbsize=320",
                            self.starcitizen_tools_api_url,
                            separator
                        );
                        self.probe_provider(ProviderCache::StarCitizenTools, url);
                    }
                });
                source_settings_changed |= themed_toggle(
                    ui,
                    &mut self.use_star_citizen_wiki_api,
                    "Use api.star-citizen.wiki for structured commodity/game data and media references",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([120.0, 18.0], egui::Label::new("Wiki API"));
                    source_settings_changed |= ui
                        .add_sized(
                            [420.0, 22.0],
                            egui::TextEdit::singleline(&mut self.star_citizen_wiki_api_url),
                        )
                        .changed();
                    if ui.button("Test + cache").clicked() {
                        let url = format!(
                            "{}/commodities?filter[name]=Bexalite",
                            self.star_citizen_wiki_api_url.trim_end_matches('/')
                        );
                        self.probe_provider(ProviderCache::StarCitizenWikiApi, url);
                    }
                });
                if source_settings_changed {
                    self.persist_settings();
                    if previous_uex_enabled != self.use_uex_fallback
                        || previous_scunpacked_enabled != self.use_scunpacked
                    {
                        self.refresh_refinery_options();
                    }
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("PROVIDER CACHE")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                ui.label(
                    RichText::new(format!("Cache root: {}", cache_root().display()))
                        .small()
                        .color(Color32::from_rgb(132, 128, 124)),
                );
                for provider in [
                    ProviderCache::Uex,
                    ProviderCache::StarCitizenTools,
                    ProviderCache::StarCitizenWikiApi,
                ] {
                    let _ = ensure_provider_cache(provider);
                    ui.horizontal(|ui| {
                        ui.add_sized([160.0, 18.0], egui::Label::new(provider.label()));
                        ui.add_sized(
                            [90.0, 18.0],
                            egui::Label::new(format_bytes(provider_cache_size(provider))),
                        );
                        if ui.button("Clear cache").clicked() {
                            match clear_provider_cache(provider) {
                                Ok(()) => {
                                    self.status_message =
                                        format!("{} cache cleared.", provider.label());
                                    if provider == ProviderCache::Uex {
                                        self.refresh_refinery_options();
                                    }
                                }
                                Err(error) => {
                                    self.status_message = format!(
                                        "Could not clear {} cache: {error}",
                                        provider.label()
                                    )
                                }
                            }
                        }
                    });
                }
                if ui.button("Clear all provider caches").clicked() {
                    match clear_all_provider_caches() {
                        Ok(()) => {
                            self.status_message = "All provider caches cleared.".to_owned();
                            self.refresh_refinery_options();
                        }
                        Err(error) => {
                            self.status_message = format!("Could not clear provider caches: {error}")
                        }
                    }
                }
                ui.separator();
                ui.label(
                    RichText::new("SCUNPACKED DATABASE UPDATE")
                        .strong()
                        .color(Color32::from_rgb(225, 132, 45)),
                );
                let inbox = inbox_state();
                ui.label(
                    RichText::new(format!("Database inbox: {}", inbox.root.display()))
                        .small()
                        .color(Color32::from_rgb(137, 133, 129)),
                );
                ui.label(
                    RichText::new(
                        "Place a scunpacked-data ZIP or an extracted scunpacked-data folder here. Install/Update imports commodities, locations and refinery-location changes while the embedded baseline remains available at all times.",
                    )
                    .small()
                    .color(Color32::from_rgb(132, 128, 124)),
                );
                ui.horizontal(|ui| {
                    if ui.button("OPEN DATABASE FOLDER").clicked() {
                        let _ = ensure_database_root();
                        let _ = Command::new("explorer.exe").arg(database_root()).spawn();
                    }
                    let candidate_text = inbox
                        .candidate
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .and_then(|name| name.to_str())
                        .unwrap_or("No update package detected");
                    ui.label(RichText::new(candidate_text).small());
                    let action = inbox.action_label();
                    if ui
                        .add_enabled(inbox.candidate.is_some(), egui::Button::new(action))
                        .clicked()
                    {
                        self.install_database_update();
                    }
                });
                if let Some(installed) = inbox.installed {
                    ui.label(
                        RichText::new(format!("Installed update: {}", installed.display()))
                            .small()
                            .color(Color32::from_rgb(224, 166, 104)),
                    );
                } else {
                    ui.label(
                        RichText::new("Using embedded 4.10 baseline; no external SCUnpacked update installed.")
                            .small()
                            .color(Color32::from_rgb(137, 133, 129)),
                    );
                }
                ui.label(RichText::new(&self.database_status).small());
                ui.label(
                    RichText::new(format!("Settings file: {}", settings_path().display()))
                        .small()
                        .color(Color32::from_rgb(137, 133, 129)),
                );
            });
    }

    fn install_database_update(&mut self) {
        match install_or_update_candidate() {
            Ok(path) => {
                self.database_path = Some(path.clone());
                self.persist_settings();
                self.database_status = format!(
                    "SCUnpacked update installed under {}. Applying game-data updates...",
                    path.display()
                );
                self.read_database_source();
            }
            Err(error) => {
                self.database_status = format!("SCUnpacked install/update failed: {error:#}");
            }
        }
    }

    fn read_database_source(&mut self) {
        let Some(path) = self.database_path.clone() else {
            self.database_status = "No data source selected.".to_owned();
            return;
        };
        if !self.use_scunpacked {
            self.database_status = "SCUnpacked source is disabled in settings.".to_owned();
            self.scunpacked_catalog = None;
            return;
        }

        match ScUnpackedCatalog::load(&path) {
            Ok(catalog) => {
                let mineral_count = catalog.mineral_count();
                let commodity_count = catalog.commodities.len();
                let source = catalog.source_path.display().to_string();
                self.scunpacked_catalog = Some(catalog);
                self.refresh_refinery_options();
                self.database_status = format!(
                    "SCUnpacked loaded: {commodity_count} commodities, {mineral_count} mineral/raw records from {source}. Refinery location baseline refreshed."
                );
            }
            Err(error) => {
                self.scunpacked_catalog = None;
                self.database_status = format!("SCUnpacked import failed: {error:#}");
            }
        }
    }

    fn refresh_log_cache_if_needed(&mut self) {
        let stale = self
            .log_cache_at
            .map(|at| at.elapsed() >= Duration::from_secs(1))
            .unwrap_or(true);
        if stale {
            self.log_cache = self.event_logger.read_recent_lines(100);
            self.log_cache_at = Some(Instant::now());
        }
    }

    fn event_log_panel(&mut self, ctx: &egui::Context) {
        if !self.show_live_ocr_window && self.bottom_panel_tab == BottomPanelTab::LiveOcr {
            self.bottom_panel_tab = BottomPanelTab::EventLog;
        }
        if self.bottom_panel_tab == BottomPanelTab::EventLog && self.show_log_viewer {
            self.refresh_log_cache_if_needed();
        }
        let log_height = match self.bottom_panel_tab {
            BottomPanelTab::LiveOcr => 280.0,
            BottomPanelTab::EventLog if self.show_log_viewer => 180.0,
            BottomPanelTab::EventLog => 42.0,
        };
        let mut clicked_target: Option<String> = None;
        let enriched_snapshot = self.enriched_mineral.clone();
        egui::TopBottomPanel::bottom("minerscan_log_viewer")
            .exact_height(log_height)
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(18, 18, 18))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(58, 54, 50))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let event_label = if self.bottom_panel_tab == BottomPanelTab::EventLog
                        && self.show_log_viewer
                    {
                        "EVENT LOG -"
                    } else {
                        "EVENT LOG +"
                    };
                    if ui.button(event_label).clicked() {
                        if self.bottom_panel_tab == BottomPanelTab::EventLog {
                            self.show_log_viewer = !self.show_log_viewer;
                        } else {
                            self.bottom_panel_tab = BottomPanelTab::EventLog;
                            self.show_log_viewer = true;
                        }
                    }
                    if self.show_live_ocr_window
                        && ui
                            .selectable_label(
                                self.bottom_panel_tab == BottomPanelTab::LiveOcr,
                                "LIVE OCR",
                            )
                            .clicked()
                    {
                        self.bottom_panel_tab = BottomPanelTab::LiveOcr;
                    }
                    if self.bottom_panel_tab == BottomPanelTab::EventLog {
                        if ui.button("Open logfile").clicked() {
                            self.open_logfile();
                        }
                        ui.label(
                            RichText::new(self.event_logger.path().display().to_string())
                                .small()
                                .color(Color32::from_rgb(128, 124, 120)),
                        );
                    } else {
                        ui.label(
                            RichText::new("Live OCR preview - fixed 240 x 240 maximum")
                                .small()
                                .color(Color32::from_rgb(128, 124, 120)),
                        );
                    }
                });
                if self.bottom_panel_tab == BottomPanelTab::LiveOcr {
                    ui.separator();
                    ui.horizontal_top(|ui| {
                        egui::Frame::new()
                            .fill(Color32::from_rgb(10, 10, 10))
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(118, 82, 52)))
                            .inner_margin(4.0)
                            .show(ui, |ui| {
                                ui.set_min_size(egui::vec2(240.0, 240.0));
                                ui.set_max_size(egui::vec2(240.0, 240.0));
                                if let Some(texture) = &self.live_preview_texture {
                                    ui.add(
                                        egui::Image::new(texture)
                                            .max_width(232.0)
                                            .max_height(232.0),
                                    );
                                } else {
                                    ui.centered_and_justified(|ui| {
                                        ui.label(
                                            RichText::new("Waiting for OCR frame...")
                                                .small()
                                                .color(Color32::from_rgb(128, 124, 120)),
                                        );
                                    });
                                }
                            });
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("OCR TEXT")
                                    .strong()
                                    .color(Color32::from_rgb(225, 132, 45)),
                            );
                            ui.monospace(if self.live_raw_text.is_empty() {
                                "<no text>"
                            } else {
                                self.live_raw_text.as_str()
                            });
                            ui.add_space(6.0);
                            let candidates = if self.live_numeric_candidates.is_empty() {
                                "-".to_owned()
                            } else {
                                self.live_numeric_candidates
                                    .iter()
                                    .map(u32::to_string)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            ui.label(format!("Candidates: {candidates}"));
                            if let Some(ms) = self.live_elapsed_ms {
                                ui.label(format!("OCR: {ms} ms"));
                            }
                        });
                    });
                    return;
                }
                if !self.show_log_viewer {
                    return;
                }
                ui.separator();
                let lines = self.log_cache.clone();
                let available = ui.available_width().max(760.0);
                let widths = [
                    165.0,
                    82.0,
                    135.0,
                    68.0,
                    58.0,
                    90.0,
                    92.0,
                    (available - 738.0).max(150.0),
                ];
                ui.horizontal(|ui| {
                    for (index, header) in [
                        "Time",
                        "Event",
                        "Target",
                        "RS",
                        "Count",
                        "Type",
                        "Price",
                        "Best route",
                    ]
                    .iter()
                    .enumerate()
                    {
                        ui.add_sized(
                            [widths[index], 18.0],
                            egui::Label::new(RichText::new(*header).small().strong()),
                        );
                    }
                });
                egui::ScrollArea::vertical()
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in lines {
                            let time = log_timestamp(&line);
                            let mut event = "INFO".to_owned();
                            let mut event_color = Color32::from_rgb(158, 154, 150);
                            let target: String;
                            let mut rs = "-".to_owned();
                            let mut count = "-".to_owned();
                            let mut category = "-".to_owned();
                            let mut price = "-".to_owned();
                            let mut route = "-".to_owned();
                            let mut clickable = false;

                            if line.contains("event=DETECTION") {
                                event = "DETECTION".to_owned();
                                event_color = Color32::from_rgb(72, 224, 186);
                                target = log_field(&line, "name").unwrap_or_else(|| "-".into());
                                rs = log_field(&line, "rs").unwrap_or_else(|| "-".into());
                                count =
                                    log_field(&line, "multiplier").unwrap_or_else(|| "-".into());
                                category =
                                    log_field(&line, "category").unwrap_or_else(|| "-".into());
                                clickable = target != "-";
                                if let Some(enriched) = enriched_snapshot
                                    .as_ref()
                                    .filter(|e| e.name.eq_ignore_ascii_case(&target))
                                {
                                    price = enriched
                                        .refined_market
                                        .as_ref()
                                        .map(|q| format!("{:.0}", q.price_per_scu))
                                        .or_else(|| {
                                            enriched
                                                .raw_market
                                                .as_ref()
                                                .map(|q| format!("{:.0}", q.price_per_scu))
                                        })
                                        .unwrap_or_else(|| "--".to_owned());
                                    route = enriched
                                        .refined_market
                                        .as_ref()
                                        .map(|q| q.location.clone())
                                        .or_else(|| {
                                            enriched.raw_market.as_ref().map(|q| q.location.clone())
                                        })
                                        .unwrap_or_else(|| "--".to_owned());
                                }
                            } else if line.contains("event=ENRICHMENT") {
                                event = "ENRICH".to_owned();
                                event_color = Color32::from_rgb(225, 132, 45);
                                target = log_field(&line, "name").unwrap_or_else(|| "-".into());
                                category = "DATA".to_owned();
                                price =
                                    log_field(&line, "refined_price").unwrap_or_else(|| "-".into());
                                route = log_field(&line, "refined_location")
                                    .unwrap_or_else(|| "-".into());
                                clickable = target != "-";
                            } else if line.contains("event=SCANNER") {
                                event = "SCANNER".to_owned();
                                event_color = Color32::from_rgb(225, 132, 45);
                                target = log_field(&line, "state").unwrap_or_else(|| "-".into());
                            } else {
                                target = line.clone();
                            }

                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    [widths[0], 18.0],
                                    egui::Label::new(RichText::new(time).small()),
                                );
                                ui.add_sized(
                                    [widths[1], 18.0],
                                    egui::Label::new(
                                        RichText::new(event).small().color(event_color),
                                    ),
                                );
                                if clickable {
                                    if ui
                                        .add_sized(
                                            [widths[2], 18.0],
                                            egui::Button::new(
                                                RichText::new(&target)
                                                    .small()
                                                    .color(Color32::from_rgb(232, 168, 105)),
                                            )
                                            .fill(Color32::TRANSPARENT)
                                            .stroke(Stroke::NONE),
                                        )
                                        .on_hover_text("Open enriched details")
                                        .clicked()
                                    {
                                        clicked_target = Some(target.clone());
                                    }
                                } else {
                                    ui.add_sized(
                                        [widths[2], 18.0],
                                        egui::Label::new(RichText::new(target).small()),
                                    );
                                }
                                ui.add_sized(
                                    [widths[3], 18.0],
                                    egui::Label::new(RichText::new(rs).small()),
                                );
                                ui.add_sized(
                                    [widths[4], 18.0],
                                    egui::Label::new(RichText::new(count).small()),
                                );
                                ui.add_sized(
                                    [widths[5], 18.0],
                                    egui::Label::new(RichText::new(category).small()),
                                );
                                ui.add_sized(
                                    [widths[6], 18.0],
                                    egui::Label::new(RichText::new(price).small()),
                                );
                                ui.add_sized(
                                    [widths[7], 18.0],
                                    egui::Label::new(RichText::new(route).small()),
                                );
                            });
                        }
                    });
            });
        if let Some(target) = clicked_target {
            self.tab = AppTab::Scanner;
            self.show_enrichment_details = true;
            self.request_enrichment(&target);
        }
    }
    fn open_logfile(&mut self) {
        let path = self.event_logger.path();
        let result = if cfg!(target_os = "windows") {
            Command::new("notepad.exe").arg(path).spawn()
        } else {
            Command::new("xdg-open").arg(path).spawn()
        };
        if let Err(error) = result {
            self.status_message = format!("Could not open logfile: {error}");
        }
    }

    fn open_last_report(&mut self) {
        let Some(path) = self.last_report.clone() else {
            self.status_message =
                "No enriched mining report has been generated in this session.".to_owned();
            return;
        };
        if !path.is_file() {
            self.status_message = "The last report file is no longer available.".to_owned();
            self.last_report = None;
            return;
        }
        let result = Command::new("cmd.exe")
            .arg("/C")
            .arg("start")
            .arg("")
            .arg(&path)
            .spawn();
        if let Err(error) = result {
            self.status_message = format!("Could not open last report: {error}");
        }
    }

    fn process_hotkeys(&mut self, ctx: &egui::Context) {
        let actions = self
            .hotkeys
            .as_ref()
            .map(HotkeyController::drain_actions)
            .unwrap_or_default();
        for action in actions {
            match action {
                HotkeyAction::ToggleScanner => {
                    if self.scanner_active {
                        self.stop_live_scanner();
                    } else if self.definitions.iter().any(|definition| definition.enabled) {
                        self.start_live_scanner();
                    } else {
                        self.status_message =
                            "Scanner hotkey ignored: select at least one monitor target first."
                                .to_owned();
                    }
                }
                HotkeyAction::ToggleCalibration => {
                    self.show_region = !self.show_region;
                    self.persist_settings();
                    self.status_message = if self.show_region {
                        "Calibration overlay enabled.".to_owned()
                    } else {
                        "Calibration overlay hidden.".to_owned()
                    };
                }
                HotkeyAction::ToggleMoveMode => {
                    self.calibration_move_mode = !self.calibration_move_mode;
                    if self.calibration_move_mode {
                        if self.scanner_active {
                            self.stop_live_scanner();
                        }
                        self.show_region = true;
                        self.status_message =
                            "Calibration move mode active: use arrow keys to move the ROI."
                                .to_owned();
                    } else {
                        self.persist_settings();
                        self.status_message = "Calibration move mode disabled.".to_owned();
                    }
                }
            }
        }

        if !self.calibration_move_mode {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(30));
        if self.last_move_tick.elapsed() < Duration::from_millis(30) {
            return;
        }
        self.last_move_tick = Instant::now();
        let (left, right, up, down) = arrow_state();
        let dx = i32::from(right) - i32::from(left);
        let dy = i32::from(down) - i32::from(up);
        if dx == 0 && dy == 0 {
            return;
        }
        let Some(monitor) = self.selected_monitor().cloned() else {
            return;
        };
        let max_x = monitor.width.saturating_sub(self.region.width) as i32;
        let max_y = monitor.height.saturating_sub(self.region.height) as i32;
        self.region.x = (self.region.x + dx).clamp(0, max_x.max(0));
        self.region.y = (self.region.y + dy).clamp(0, max_y.max(0));
    }

    fn rebind_hotkeys(&mut self) {
        let result = self.hotkeys.as_mut().map(|controller| {
            controller.rebind(
                &self.hotkey_scanner,
                &self.hotkey_calibration,
                &self.hotkey_move_mode,
            )
        });
        match result {
            Some(Ok(())) => {
                self.persist_settings();
                self.status_message = "Global hotkeys updated.".to_owned();
            }
            Some(Err(error)) => {
                self.status_message = format!("Hotkey update failed: {error:#}");
            }
            None => {
                self.status_message = "Global hotkey service is unavailable.".to_owned();
            }
        }
    }

    fn process_tray(&mut self, ctx: &egui::Context) {
        let action = self.tray.as_ref().and_then(TrayController::poll_action);
        if let Some(action) = action {
            match action {
                TrayAction::Show => {
                    let _ = show_main_window();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayAction::Close => {
                    // Tray Close is an explicit process-termination command. Do not route
                    // it through eframe's close/minimize-to-tray path, because a hidden
                    // root viewport can otherwise cancel or defer the close request.
                    self.exit_requested = true;
                    self.stop_live_scanner();
                    self.calibration_overlay.hide();
                    std::process::exit(0);
                }
                TrayAction::StartScanner => {
                    if !self.scanner_active {
                        self.start_live_scanner();
                    }
                }
                TrayAction::StopScanner => {
                    if self.scanner_active {
                        self.stop_live_scanner();
                    }
                }
                TrayAction::LastReport => self.open_last_report(),
            }
        }
        if let Some(tray) = &self.tray {
            tray.set_state(
                self.scanner_active,
                self.last_report.as_ref().is_some_and(|p| p.is_file()),
            );
            tray.set_visible(self.minimize_to_tray && !is_main_window_visible());
        }
    }

    fn update_calibration_overlay(&mut self) {
        let visible = self.show_region && !self.scanner_active;
        let Some(monitor) = self.selected_monitor().cloned() else {
            self.calibration_overlay.hide();
            return;
        };
        self.calibration_overlay.update(
            monitor.x + self.region.x,
            monitor.y + self.region.y,
            self.region.width,
            self.region.height,
            [
                self.frame_color.r(),
                self.frame_color.g(),
                self.frame_color.b(),
            ],
            visible,
        );
    }
}

impl eframe::App for MinerScanApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_tray(ctx);
        self.process_hotkeys(ctx);
        if self.minimize_to_tray && !self.exit_requested {
            let (close_requested, minimized) = ctx.input(|i| {
                (
                    i.viewport().close_requested(),
                    i.viewport().minimized.unwrap_or(false),
                )
            });
            if close_requested {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                let _ = hide_main_window();
                if let Some(tray) = &self.tray {
                    tray.set_visible(true);
                }
            } else if minimized || is_main_window_minimized() {
                // Hide the still-existing native HWND directly. Do not issue an
                // eframe restore command here: it would race the SW_HIDE and make
                // the taskbar window reappear. show_main_window() performs the
                // restore only when the user explicitly requests Show.
                let _ = hide_main_window();
                if let Some(tray) = &self.tray {
                    tray.set_visible(true);
                }
            }
        }
        self.process_live_scan_events(ctx);
        self.process_enrichment_events(ctx);
        if self.scanner_active || self.enrichment_worker.in_flight().is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.tray.is_some() {
            // The tray must remain responsive even while the application window is minimized.
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        self.update_calibration_overlay();

        egui::TopBottomPanel::bottom("minerscan_status_bar")
            .exact_height(24.0)
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(18, 18, 18))
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(17, 56, 73))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if self.scanner_active {
                            "Scanner active"
                        } else {
                            "Scanner standby"
                        })
                        .monospace()
                        .size(9.0)
                        .color(Color32::from_rgb(99, 136, 153)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                .size(9.0)
                                .strong()
                                .color(Color32::from_rgb(96, 201, 138)),
                        );
                        ui.label(
                            RichText::new(if self.scanner_active { "LIVE" } else { "IDLE" })
                                .size(9.0)
                                .color(if self.scanner_active {
                                    Color32::from_rgb(96, 201, 138)
                                } else {
                                    Color32::from_rgb(132, 128, 124)
                                }),
                        );
                    });
                });
            });
        self.event_log_panel(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    self.header(ui);
                    ui.add_space(8.0);
                    match self.tab {
                        AppTab::Scanner => self.scanner_view(ui),
                        AppTab::Settings => self.settings_view(ui),
                    }
                    ui.add_space(16.0);
                });
        });
    }
}

fn uex_commodity_url(name: &str) -> String {
    let slug = name
        .trim()
        .to_lowercase()
        .replace(' ', "-")
        .replace('_', "-")
        .replace('(', "")
        .replace(')', "")
        .replace("--", "-");
    format!("https://uexcorp.space/commodities/info/name/{slug}")
}

fn format_minutes(minutes: f64) -> String {
    let total = minutes.max(0.0).round() as u64;
    let hours = total / 60;
    let mins = total % 60;
    if hours > 0 {
        format!("{hours} h {mins} min")
    } else {
        format!("{mins} min")
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let value = bytes as f64;
    if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

fn log_timestamp(line: &str) -> String {
    line.strip_prefix('[')
        .and_then(|v| v.split_once(']'))
        .map(|(stamp, _)| stamp.to_owned())
        .unwrap_or_else(|| "-".to_owned())
}

fn log_field(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let start = line.find(&needle)? + needle.len();
    let tail = &line[start..];
    if let Some(stripped) = tail.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_owned())
    } else {
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        Some(tail[..end].to_owned())
    }
}

fn themed_toggle(ui: &mut egui::Ui, value: &mut bool, label: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(34.0, 18.0), egui::Sense::click());
        if response.clicked() {
            *value = !*value;
            changed = true;
        }
        let bg = if *value {
            if response.hovered() {
                Color32::from_rgba_unmultiplied(205, 105, 34, 185)
            } else {
                Color32::from_rgba_unmultiplied(175, 82, 24, 160)
            }
        } else if response.hovered() {
            Color32::from_rgb(58, 53, 49)
        } else {
            Color32::from_rgb(38, 37, 36)
        };
        let border = if *value {
            if response.hovered() {
                Color32::from_rgb(255, 169, 76)
            } else {
                Color32::from_rgb(226, 128, 42)
            }
        } else if response.hovered() {
            Color32::from_rgb(137, 116, 98)
        } else {
            Color32::from_rgb(92, 86, 81)
        };
        ui.painter().rect(
            rect,
            9.0,
            bg,
            Stroke::new(1.0_f32, border),
            egui::StrokeKind::Inside,
        );
        let knob_x = if *value {
            rect.right() - 9.0
        } else {
            rect.left() + 9.0
        };
        ui.painter().circle_filled(
            egui::pos2(knob_x, rect.center().y),
            5.5,
            if *value {
                if response.hovered() {
                    Color32::from_rgb(255, 226, 196)
                } else {
                    Color32::from_rgb(244, 214, 184)
                }
            } else if response.hovered() {
                Color32::from_rgb(188, 178, 169)
            } else {
                Color32::from_rgb(139, 133, 128)
            },
        );
        if !label.is_empty() {
            ui.label(label);
        }
    });
    changed
}

fn definition_match_multiplier(definition: &SignatureDefinition, query: &str) -> Option<u32> {
    let digits: String = query.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty()
        || query
            .chars()
            .any(|c| !(c.is_ascii_digit() || c.is_whitespace() || c == ',' || c == '.'))
    {
        return None;
    }
    let value = digits.parse::<u32>().ok()?;
    let max = if definition.multiplicative { 12 } else { 1 };
    (1..=max).find(|multiplier| definition.base_signature.saturating_mul(*multiplier) == value)
}

fn definition_matches_query(definition: &SignatureDefinition, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    let lower = query.trim().to_lowercase();
    definition.canonical_name.to_lowercase().contains(&lower)
        || definition.category.label().to_lowercase().contains(&lower)
        || definition_match_multiplier(definition, query).is_some()
}

fn detect_monitors() -> Vec<MonitorInfo> {
    DisplayInfo::all()
        .map(|items| items.into_iter().map(MonitorInfo::from).collect())
        .unwrap_or_default()
}

fn sig(
    id: &str,
    name: &str,
    category: SignatureCategory,
    base: u32,
    enabled: bool,
) -> SignatureDefinition {
    SignatureDefinition {
        id: id.into(),
        canonical_name: name.into(),
        category,
        base_signature: base,
        multiplicative: true,
        enabled,
        source: "MinerScan seed dataset - verify against extracted LIVE data".into(),
        game_version: "seed/2026-09".into(),
    }
}

fn initial_definitions() -> Vec<SignatureDefinition> {
    vec![
        sig(
            "quantanium",
            "Quantanium",
            SignatureCategory::Resource,
            3170,
            true,
        ),
        sig(
            "stileron",
            "Stileron",
            SignatureCategory::Resource,
            3185,
            false,
        ),
        sig(
            "savrilum",
            "Savrilium",
            SignatureCategory::Resource,
            3200,
            false,
        ),
        sig(
            "ouratite",
            "Ouratite",
            SignatureCategory::Resource,
            3370,
            false,
        ),
        sig(
            "riccite",
            "Riccite",
            SignatureCategory::Resource,
            3385,
            true,
        ),
        sig(
            "lindinium",
            "Lindinium",
            SignatureCategory::Resource,
            3400,
            true,
        ),
        sig("beryl", "Beryl", SignatureCategory::Resource, 3540, false),
        sig(
            "taranite",
            "Taranite",
            SignatureCategory::Resource,
            3555,
            false,
        ),
        sig("borase", "Borase", SignatureCategory::Resource, 3570, false),
        sig("gold", "Gold", SignatureCategory::Resource, 3585, false),
        sig(
            "bexalite",
            "Bexalite",
            SignatureCategory::Resource,
            3600,
            false,
        ),
        sig(
            "laranite",
            "Laranite",
            SignatureCategory::Resource,
            3825,
            false,
        ),
        sig(
            "aslarite",
            "Aslarite",
            SignatureCategory::Resource,
            3840,
            false,
        ),
        sig(
            "titanium",
            "Titanium",
            SignatureCategory::Resource,
            3855,
            false,
        ),
        sig(
            "tungsten",
            "Tungsten",
            SignatureCategory::Resource,
            3870,
            false,
        ),
        sig(
            "agricium",
            "Agricium",
            SignatureCategory::Resource,
            3885,
            false,
        ),
        sig("torite", "Torite", SignatureCategory::Resource, 3900, false),
        sig(
            "hephaestanite",
            "Hephaestanite",
            SignatureCategory::Resource,
            4180,
            false,
        ),
        sig("tin", "Tin", SignatureCategory::Resource, 4195, false),
        sig("quartz", "Quartz", SignatureCategory::Resource, 4210, false),
        sig(
            "corundum",
            "Corundum",
            SignatureCategory::Resource,
            4225,
            false,
        ),
        sig("copper", "Copper", SignatureCategory::Resource, 4240, false),
        sig(
            "silicon",
            "Silicon",
            SignatureCategory::Resource,
            4255,
            false,
        ),
        sig("iron", "Iron", SignatureCategory::Resource, 4270, false),
        sig(
            "aluminium",
            "Aluminium",
            SignatureCategory::Resource,
            4285,
            false,
        ),
        sig("ice", "Ice", SignatureCategory::Resource, 4300, false),
        sig(
            "salvage-panels",
            "Salvage Panels",
            SignatureCategory::Salvage,
            2000,
            true,
        ),
        sig(
            "asteroid-i",
            "I-Type Asteroid",
            SignatureCategory::AsteroidClass,
            4000,
            false,
        ),
        sig(
            "asteroid-s",
            "S-Type Asteroid",
            SignatureCategory::AsteroidClass,
            4720,
            false,
        ),
    ]
}

#[cfg(test)]
mod target_search_tests {
    use super::*;

    #[test]
    fn resolves_rs_value_to_material_multiplier() {
        let definitions = initial_definitions();
        let bexalite = definitions
            .iter()
            .find(|definition| definition.canonical_name == "Bexalite")
            .expect("Bexalite seed");
        assert_eq!(definition_match_multiplier(bexalite, "7200"), Some(2));
        assert!(definition_matches_query(bexalite, "7,200"));
    }

    #[test]
    fn target_search_supports_name_and_type() {
        let definitions = initial_definitions();
        let riccite = definitions
            .iter()
            .find(|definition| definition.canonical_name == "Riccite")
            .expect("Riccite seed");
        assert!(definition_matches_query(riccite, "ric"));
        assert!(definition_matches_query(riccite, "mineral"));
        assert!(!definition_matches_query(riccite, "7200"));
    }

    #[test]
    fn duplicate_cooldown_suppresses_only_immediately_repeated_material() {
        let now = Instant::now();
        let cooldown = Duration::from_secs(120);
        assert!(is_duplicate_material_detection(
            Some("hephaestanite"),
            Some(now - Duration::from_secs(30)),
            "hephaestanite",
            cooldown,
            now,
        ));
        assert!(!is_duplicate_material_detection(
            Some("hephaestanite"),
            Some(now - Duration::from_secs(30)),
            "aluminium",
            cooldown,
            now,
        ));
        assert!(!is_duplicate_material_detection(
            Some("hephaestanite"),
            Some(now - Duration::from_secs(121)),
            "hephaestanite",
            cooldown,
            now,
        ));
    }
}

