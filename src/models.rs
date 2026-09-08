use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureCategory {
    Resource,
    AsteroidClass,
    GroundDeposit,
    Salvage,
    Ship,
    Unknown,
}

impl SignatureCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Resource => "Mineral",
            Self::AsteroidClass => "Asteroid",
            Self::GroundDeposit => "Ground",
            Self::Salvage => "Salvage",
            Self::Ship => "Ship",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureDefinition {
    pub id: String,
    pub canonical_name: String,
    pub category: SignatureCategory,
    pub base_signature: u32,
    pub multiplicative: bool,
    pub enabled: bool,
    pub source: String,
    pub game_version: String,
}

#[derive(Debug, Clone)]
pub struct DetectionCandidate {
    pub target_id: String,
    pub name: String,
    pub category: SignatureCategory,
    pub observed_signature: u32,
    pub base_signature: u32,
    pub multiplier: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CaptureRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Default for CaptureRegion {
    fn default() -> Self {
        Self {
            x: 1240,
            y: 462,
            width: 230,
            height: 74,
        }
    }
}
