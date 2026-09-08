use crate::models::CaptureRegion;

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub region: CaptureRegion,
    pub target_fps: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            region: CaptureRegion::default(),
            target_fps: 30,
        }
    }
}

pub trait CaptureBackend {
    fn start(&mut self, config: CaptureConfig) -> anyhow::Result<()>;
    fn stop(&mut self) -> anyhow::Result<()>;
}

#[derive(Default)]
pub struct NullCaptureBackend;

impl CaptureBackend for NullCaptureBackend {
    fn start(&mut self, _config: CaptureConfig) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
