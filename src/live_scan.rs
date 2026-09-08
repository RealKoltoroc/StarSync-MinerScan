use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use image::imageops::FilterType;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;
use xcap::Monitor;

#[cfg(target_os = "windows")]
use crate::dxgi_capture::DxgiRoiCapture;
use crate::{models::CaptureRegion, ocr::extract_numeric_candidates};

#[derive(Debug, Clone, Copy)]
pub struct LiveScanConfig {
    pub monitor_x: i32,
    pub monitor_y: i32,
    pub region: CaptureRegion,
    pub interval_ms: u64,
    pub emit_preview: bool,
}

#[derive(Debug, Clone)]
pub enum LiveScanEvent {
    Backend(String),
    Ocr {
        raw_text: String,
        numeric_candidates: Vec<u32>,
        elapsed_ms: u128,
        preview_width: u32,
        preview_height: u32,
        preview_rgba: Vec<u8>,
    },
    Error(String),
}

pub struct LiveScanner {
    stop: Option<Arc<AtomicBool>>,
    pause_until: Option<Arc<Mutex<Option<Instant>>>>,
    receiver: Option<Receiver<LiveScanEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Default for LiveScanner {
    fn default() -> Self {
        Self {
            stop: None,
            pause_until: None,
            receiver: None,
            worker: None,
        }
    }
}

impl LiveScanner {
    pub fn start(&mut self, config: LiveScanConfig) -> Result<()> {
        self.stop();

        // Keep only a tiny number of pending OCR results. If the UI is busy we
        // drop stale frames instead of allowing an unbounded RAM queue.
        let (tx, rx) = mpsc::sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let pause_until = Arc::new(Mutex::new(None));
        let worker_stop = Arc::clone(&stop);
        let worker_pause = Arc::clone(&pause_until);

        let worker = thread::spawn(move || {
            let engine = match load_engine() {
                Ok(engine) => engine,
                Err(error) => {
                    let _ = tx.try_send(LiveScanEvent::Error(format!("{error:#}")));
                    return;
                }
            };
            let monitor = match Monitor::from_point(config.monitor_x + 1, config.monitor_y + 1) {
                Ok(monitor) => monitor,
                Err(error) => {
                    let _ = tx.try_send(LiveScanEvent::Error(format!(
                        "Unable to resolve selected monitor for screen capture: {error}"
                    )));
                    return;
                }
            };

            // Prefer a persistent DXGI Desktop Duplication capture that copies only the
            // OCR ROI from the GPU. This sees DirectX/flip-model game output while avoiding
            // both GDI's game-capture limitation and full-monitor CPU readback.
            #[cfg(target_os = "windows")]
            let dxgi = DxgiRoiCapture::new(config.monitor_x, config.monitor_y).ok();
            #[cfg(not(target_os = "windows"))]
            let dxgi: Option<()> = None;

            let backend = if dxgi.is_some() {
                "DXGI ROI Desktop Duplication"
            } else {
                "GDI fallback"
            };
            let _ = tx.try_send(LiveScanEvent::Backend(backend.to_owned()));

            while !worker_stop.load(Ordering::Relaxed) {
                if let Ok(guard) = worker_pause.lock() {
                    if let Some(until) = *guard {
                        let now = Instant::now();
                        if now < until {
                            drop(guard);
                            thread::sleep((until - now).min(Duration::from_millis(100)));
                            continue;
                        }
                    }
                }

                let started = Instant::now();
                #[cfg(target_os = "windows")]
                let result = if let Some(dxgi) = dxgi.as_ref() {
                    dxgi.capture(config.region, 300)
                        .and_then(|rgba| recognize_rgba(rgba, &engine, config.emit_preview))
                        .or_else(|_| {
                            capture_gdi_and_recognize(
                                &monitor,
                                &engine,
                                config.region,
                                config.emit_preview,
                            )
                        })
                } else {
                    capture_gdi_and_recognize(
                        &monitor,
                        &engine,
                        config.region,
                        config.emit_preview,
                    )
                };
                #[cfg(not(target_os = "windows"))]
                let result = capture_gdi_and_recognize(
                    &monitor,
                    &engine,
                    config.region,
                    config.emit_preview,
                );

                match result {
                    Ok((raw_text, numeric_candidates, preview)) => {
                        let (preview_width, preview_height, preview_rgba) = preview
                            .map(|image| (image.width(), image.height(), image.into_raw()))
                            .unwrap_or((0, 0, Vec::new()));
                        let _ = tx.try_send(LiveScanEvent::Ocr {
                            raw_text,
                            numeric_candidates,
                            elapsed_ms: started.elapsed().as_millis(),
                            preview_width,
                            preview_height,
                            preview_rgba,
                        });
                    }
                    Err(error) => {
                        let _ = tx.try_send(LiveScanEvent::Error(format!("{error:#}")));
                    }
                }

                let elapsed = started.elapsed();
                let interval = Duration::from_millis(config.interval_ms.max(20));
                if elapsed < interval {
                    thread::sleep(interval - elapsed);
                }
            }
        });

        self.stop = Some(stop);
        self.pause_until = Some(pause_until);
        self.receiver = Some(rx);
        self.worker = Some(worker);
        Ok(())
    }

    pub fn pause_for(&self, duration: Duration) {
        let Some(pause) = &self.pause_until else {
            return;
        };
        if let Ok(mut guard) = pause.lock() {
            *guard = Some(Instant::now() + duration);
        }
    }

    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.pause_until.take();
        self.worker.take();
        self.receiver.take();
    }

    pub fn drain_events(&mut self) -> Vec<LiveScanEvent> {
        let mut events = Vec::new();
        let Some(receiver) = &self.receiver else {
            return events;
        };
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        events
    }
}

impl Drop for LiveScanner {
    fn drop(&mut self) {
        self.stop();
    }
}

const OCR_DETECTION_MODEL: &[u8] = include_bytes!("../assets/ocr/text-detection.rten");
const OCR_RECOGNITION_MODEL: &[u8] = include_bytes!("../assets/ocr/text-recognition.rten");

fn load_engine() -> Result<OcrEngine> {
    let detection_model = Model::load_static_slice(OCR_DETECTION_MODEL)
        .context("Unable to load embedded OCR detection model")?;
    let recognition_model = Model::load_static_slice(OCR_RECOGNITION_MODEL)
        .context("Unable to load embedded OCR recognition model")?;

    OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection_model),
        recognition_model: Some(recognition_model),
        ..Default::default()
    })
    .context("Unable to initialize embedded OCR engine")
}

fn capture_gdi_and_recognize(
    monitor: &Monitor,
    engine: &OcrEngine,
    region: CaptureRegion,
    emit_preview: bool,
) -> Result<(String, Vec<u32>, Option<image::RgbaImage>)> {
    let x = region.x.max(0) as u32;
    let y = region.y.max(0) as u32;
    let width = region.width.max(20);
    let height = region.height.max(20);

    let rgba = monitor
        .capture_region(x, y, width, height)
        .context("Screen capture of OCR region failed")?;
    recognize_rgba(rgba, engine, emit_preview)
}

fn recognize_rgba(
    rgba: image::RgbaImage,
    engine: &OcrEngine,
    emit_preview: bool,
) -> Result<(String, Vec<u32>, Option<image::RgbaImage>)> {
    let width = rgba.width();
    let height = rgba.height();
    let preview = if emit_preview {
        Some(if width <= 240 && height <= 240 {
            rgba.clone()
        } else {
            image::DynamicImage::ImageRgba8(rgba.clone())
                .thumbnail(240, 240)
                .to_rgba8()
        })
    } else {
        None
    };

    // Keep preprocessing proportional to ROI size. Tiny HUD regions benefit from
    // 2x scaling; large regions are kept native to avoid needless CPU/RAM cost.
    let dynamic = image::DynamicImage::ImageRgba8(rgba);
    let rgb = if height <= 120 && width <= 640 {
        dynamic
            .resize_exact(width * 2, height * 2, FilterType::Triangle)
            .to_rgb8()
    } else {
        dynamic.to_rgb8()
    };

    let source = ImageSource::from_bytes(rgb.as_raw(), rgb.dimensions())
        .context("Unable to prepare captured ROI for OCR")?;
    let input = engine
        .prepare_input(source)
        .context("OCR preprocessing failed")?;
    let text = engine.get_text(&input).context("OCR recognition failed")?;
    let candidates = extract_numeric_candidates(&text);

    Ok((text.trim().to_owned(), candidates, preview))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_ocr_models_initialize_without_runtime_files() {
        let engine = load_engine();
        assert!(
            engine.is_ok(),
            "embedded OCR engine failed to initialize: {:?}",
            engine.err()
        );
    }
}
