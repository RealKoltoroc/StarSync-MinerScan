use std::{collections::BTreeMap, fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::CaptureRegion;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub selected_monitor_id: Option<u32>,
    pub region: CaptureRegion,
    pub show_region: bool,
    pub frame_color_rgb: [u8; 3],
    pub sound_file: Option<String>,
    pub sound_volume: f32,
    pub database_path: Option<String>,
    pub use_scunpacked: bool,
    pub use_uex_fallback: bool,
    pub uex_base_url: String,
    pub use_starcitizen_tools: bool,
    pub starcitizen_tools_api_url: String,
    pub use_star_citizen_wiki_api: bool,
    pub star_citizen_wiki_api_url: String,
    pub analysis_base_scu: f32,
    pub refining_method_base_efficiency: BTreeMap<String, f32>,
    pub selected_refinery_terminal_id: Option<i64>,
    pub selected_refinery_label: String,
    pub refining_strategy: String,
    pub generate_html_reports: bool,
    pub clear_reports_on_start: bool,
    pub minimize_to_tray: bool,
    pub show_live_ocr_window: bool,
    pub duplicate_detection_cooldown_secs: u64,
    pub show_signature_matrix: bool,
    pub targets_collapsed: bool,
    pub hotkey_scanner: String,
    pub hotkey_calibration: String,
    pub hotkey_move_mode: String,
    pub target_enabled: BTreeMap<String, bool>,
}

pub fn default_refining_method_base_efficiency() -> BTreeMap<String, f32> {
    // Temporary 4.8+/4.10 assumptions until enough current in-game work orders exist.
    // Preserve the UEX yield tiers as relative guidance only: high=60%, medium=55%, low=50%.
    [
        ("COR", 50.0),
        ("DIN", 60.0),
        ("EST", 55.0),
        ("GAS", 55.0),
        ("PYR", 60.0),
        ("KZW", 50.0),
        ("TND", 55.0),
        ("FRX", 60.0),
        ("XCR", 50.0),
    ]
    .into_iter()
    .map(|(code, value)| (code.to_owned(), value))
    .collect()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            selected_monitor_id: None,
            region: CaptureRegion::default(),
            show_region: true,
            frame_color_rgb: [69, 221, 255],
            sound_file: None,
            sound_volume: 0.70,
            database_path: None,
            use_scunpacked: true,
            use_uex_fallback: false,
            uex_base_url: "https://api.uexcorp.uk/2.0".to_owned(),
            use_starcitizen_tools: false,
            starcitizen_tools_api_url: "https://starcitizen.tools/api.php".to_owned(),
            use_star_citizen_wiki_api: false,
            star_citizen_wiki_api_url: "https://api.star-citizen.wiki/api".to_owned(),
            analysis_base_scu: 10.0,
            refining_method_base_efficiency: default_refining_method_base_efficiency(),
            selected_refinery_terminal_id: None,
            selected_refinery_label: String::new(),
            refining_strategy: "efficiency".to_owned(),
            generate_html_reports: true,
            clear_reports_on_start: true,
            minimize_to_tray: true,
            show_live_ocr_window: false,
            duplicate_detection_cooldown_secs: 120,
            show_signature_matrix: false,
            targets_collapsed: false,
            hotkey_scanner: "NumpadEnter".to_owned(),
            hotkey_calibration: "NumpadAdd".to_owned(),
            hotkey_move_mode: "NumpadSubtract".to_owned(),
            target_enabled: BTreeMap::new(),
        }
    }
}

pub fn data_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("StarSync MinerScan")
}

pub fn settings_path() -> PathBuf {
    data_root().join("settings.json")
}

pub fn ensure_runtime_layout() -> std::io::Result<()> {
    let root = data_root();
    fs::create_dir_all(root.join("cache"))?;
    fs::create_dir_all(root.join("session-reports"))?;
    fs::create_dir_all(root.join("database"))?;
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("minerscan-events.log"))?;
    if !settings_path().is_file() {
        save_settings(&AppSettings::default())?;
    }
    Ok(())
}

pub fn load_settings() -> AppSettings {
    let path = settings_path();
    let Ok(text) = fs::read_to_string(path) else {
        return AppSettings::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_settings(settings: &AppSettings) -> std::io::Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
    fs::write(path, text)
}
