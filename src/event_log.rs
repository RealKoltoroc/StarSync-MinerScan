use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use chrono::Local;

use crate::{enrichment::EnrichedMineral, models::DetectionCandidate};

#[derive(Debug, Clone)]
pub struct EventLogger {
    path: PathBuf,
}

impl EventLogger {
    pub fn new() -> Self {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join("StarSync MinerScan");
        let _ = fs::create_dir_all(&dir);
        Self {
            path: dir.join("minerscan-events.log"),
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn log_status(&self, state: &str) {
        self.write_line(&format!("event=SCANNER state={state}"));
    }

    pub fn log_capture_backend(&self, backend: &str) {
        self.write_line(&format!(
            "event=CAPTURE backend=\"{}\"",
            sanitize_log_value(backend)
        ));
    }

    pub fn read_recent_lines(&self, max_lines: usize) -> Vec<String> {
        let Ok(content) = fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(max_lines);
        lines[start..]
            .iter()
            .map(|line| (*line).to_owned())
            .collect()
    }

    pub fn log_detection(
        &self,
        detection: &DetectionCandidate,
        source: &str,
        confidence: Option<f32>,
    ) {
        let confidence_text = confidence
            .map(|v| format!("{v:.3}"))
            .unwrap_or_else(|| "n/a".to_owned());
        self.write_line(&format!(
            "event=DETECTION source={source} target_id={} name=\"{}\" category={} rs={} base_rs={} multiplier={} confidence={confidence_text}",
            detection.target_id,
            detection.name,
            detection.category.label(),
            detection.observed_signature,
            detection.base_signature,
            detection.multiplier,
        ));
    }

    pub fn log_enrichment(&self, enriched: &EnrichedMineral) {
        let raw_price = enriched
            .raw_market
            .as_ref()
            .map(|q| format!("{:.0}", q.price_per_scu))
            .unwrap_or_else(|| "n/a".to_owned());
        let refined_price = enriched
            .refined_market
            .as_ref()
            .map(|q| format!("{:.0}", q.price_per_scu))
            .unwrap_or_else(|| "n/a".to_owned());
        let raw_location = enriched
            .raw_market
            .as_ref()
            .map(|q| q.location.as_str())
            .unwrap_or("n/a");
        let refined_location = enriched
            .refined_market
            .as_ref()
            .map(|q| q.location.as_str())
            .unwrap_or("n/a");
        let refinery = enriched
            .refinery
            .as_ref()
            .map(|r| r.location.as_str())
            .unwrap_or("n/a");
        let method = enriched
            .refinery
            .as_ref()
            .and_then(|r| r.method_name.as_deref())
            .unwrap_or("n/a");
        let net = enriched
            .estimated_net_value
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "n/a".to_owned());
        self.write_line(&format!(
            "event=ENRICHMENT name=\"{}\" scu={:.2} raw_price={} refined_price={} raw_location=\"{}\" refined_location=\"{}\" refinery=\"{}\" method=\"{}\" net={} providers=\"{}\"",
            enriched.name,
            enriched.analysis_scu,
            raw_price,
            refined_price,
            sanitize_log_value(raw_location),
            sanitize_log_value(refined_location),
            sanitize_log_value(refinery),
            sanitize_log_value(method),
            net,
            sanitize_log_value(&enriched.providers_used.join("+")),
        ));
    }

    fn write_line(&self, message: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f%:z");
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "[{timestamp}] {message}");
        }
    }
}

fn sanitize_log_value(value: &str) -> String {
    value.replace('"', "'").replace(['\r', '\n'], " ")
}
