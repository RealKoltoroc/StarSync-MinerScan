use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ScUnpackedCommodity {
    #[serde(rename = "UUID")]
    pub uuid: String,
    pub key: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "RefinedVersionUUID")]
    pub refined_version_uuid: Option<String>,
    #[serde(rename = "RefinedVersionName")]
    pub refined_version_name: Option<String>,
    pub tier: Option<String>,
    #[serde(rename = "DensityGPerCc")]
    pub density_g_per_cc: Option<f32>,
    pub volatility: Option<f32>,
    pub resistance: Option<f32>,
    pub commodity_groups: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MineralProfile {
    pub canonical_name: String,
    pub refined: Option<ScUnpackedCommodity>,
    pub raw: Option<ScUnpackedCommodity>,
}

#[derive(Debug, Clone, Default)]
pub struct ScUnpackedCatalog {
    pub source_path: PathBuf,
    pub commodities: Vec<ScUnpackedCommodity>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ScUnpackedTradeLocation {
    #[serde(rename = "TradeLocationUUID")]
    uuid: String,
    #[serde(rename = "TradeLocationClassName")]
    class_name: String,
    #[serde(rename = "TradeLocationDisplayName")]
    display_name: Option<String>,
    #[serde(rename = "StarmapObjectUUID")]
    starmap_object_uuid: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ScUnpackedCommodityTradeLocations {
    commodity_key: String,
    commodity_name: String,
    #[serde(default)]
    sold_at: Vec<ScUnpackedTradeLocation>,
    #[serde(default)]
    bought_at: Vec<ScUnpackedTradeLocation>,
}

#[derive(Debug, Clone, Default)]
pub struct ScUnpackedRefineryLocation {
    pub stable_id: i64,
    pub location: String,
}

impl ScUnpackedCatalog {
    pub fn embedded() -> Result<Self> {
        let text = include_str!("../data/scunpacked-4.10/resources/commodities.json");
        let commodities: Vec<ScUnpackedCommodity> =
            serde_json::from_str(text).context("Invalid embedded SCUnpacked commodity snapshot")?;
        Ok(Self {
            source_path: PathBuf::from("embedded://scunpacked-4.10/resources/commodities.json"),
            commodities,
        })
    }

    pub fn load(source: &Path) -> Result<Self> {
        let commodities_path = resolve_commodities_path(source).with_context(|| {
            format!(
                "Could not locate resources/commodities.json under {}",
                source.display()
            )
        })?;
        let text = fs::read_to_string(&commodities_path)
            .with_context(|| format!("Could not read {}", commodities_path.display()))?;
        let commodities: Vec<ScUnpackedCommodity> =
            serde_json::from_str(&text).with_context(|| {
                format!(
                    "Invalid SCUnpacked commodity JSON: {}",
                    commodities_path.display()
                )
            })?;
        Ok(Self {
            source_path: commodities_path,
            commodities,
        })
    }

    pub fn mineral_count(&self) -> usize {
        self.commodities.iter().filter(|c| is_mineral(c)).count()
    }

    pub fn profile_for_name(&self, name: &str) -> Option<MineralProfile> {
        let needle = normalize_name(name);
        let mut refined = None;
        let mut raw = None;

        for commodity in self.commodities.iter().filter(|c| is_mineral(c)) {
            let base_name = commodity
                .refined_version_name
                .as_deref()
                .unwrap_or(&commodity.name);
            if normalize_name(base_name) != needle && normalize_name(&commodity.name) != needle {
                continue;
            }
            if commodity.refined_version_uuid.is_some()
                || commodity.key.starts_with("Raw_")
                || commodity.key.starts_with("Ore_")
                || commodity.name.contains("(Raw)")
                || commodity.name.contains("(Ore)")
            {
                raw = Some(commodity.clone());
            } else {
                refined = Some(commodity.clone());
            }
        }

        if refined.is_none() && raw.is_none() {
            return None;
        }

        if refined.is_none() {
            if let Some(raw_commodity) = &raw {
                if let Some(uuid) = &raw_commodity.refined_version_uuid {
                    refined = self.commodities.iter().find(|c| &c.uuid == uuid).cloned();
                }
            }
        }

        Some(MineralProfile {
            canonical_name: name.to_owned(),
            refined,
            raw,
        })
    }
}

pub fn resolve_commodities_path(source: &Path) -> Option<PathBuf> {
    if source.is_file() {
        return source
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| v.eq_ignore_ascii_case("commodities.json"))
            .map(|_| source.to_path_buf());
    }

    let candidates = [
        source.join("resources").join("commodities.json"),
        source.join("commodities.json"),
    ];
    candidates.into_iter().find(|path| path.is_file())
}

pub fn load_scunpacked_refinery_locations(
    source: Option<&Path>,
) -> Result<Vec<ScUnpackedRefineryLocation>> {
    let text = if let Some(source) = source {
        let path = resolve_trade_locations_path(source).with_context(|| {
            format!(
                "Could not locate resources/commodity_trade_locations.json under {}",
                source.display()
            )
        })?;
        fs::read_to_string(&path).with_context(|| format!("Could not read {}", path.display()))?
    } else {
        include_str!("../data/scunpacked-4.10/resources/commodity_trade_locations.json").to_owned()
    };

    let rows: Vec<ScUnpackedCommodityTradeLocations> =
        serde_json::from_str(&text).context("Invalid SCUnpacked commodity_trade_locations.json")?;
    let mut by_uuid: BTreeMap<String, ScUnpackedRefineryLocation> = BTreeMap::new();
    for row in rows {
        let raw_like = row.commodity_key.starts_with("Raw_")
            || row.commodity_key.starts_with("Ore_")
            || row.commodity_name.contains("(Raw)")
            || row.commodity_name.contains("(Ore)");
        if !raw_like {
            continue;
        }
        for location in row.sold_at.into_iter().chain(row.bought_at) {
            let Some(display_name) = location
                .display_name
                .clone()
                .filter(|v| !v.trim().is_empty())
            else {
                continue;
            };
            let station_like = display_name.contains(" Station")
                || display_name.contains("Gateway")
                || display_name.eq_ignore_ascii_case("Levski")
                || location.class_name.to_ascii_lowercase().contains("refin");
            if !station_like {
                continue;
            }
            by_uuid
                .entry(location.uuid.clone())
                .or_insert_with(|| ScUnpackedRefineryLocation {
                    stable_id: stable_negative_id(&location.uuid),
                    location: display_name,
                });
        }
    }
    let mut values: Vec<_> = by_uuid.into_values().collect();
    values.sort_by(|a, b| {
        a.location
            .to_ascii_lowercase()
            .cmp(&b.location.to_ascii_lowercase())
    });
    Ok(values)
}

fn resolve_trade_locations_path(source: &Path) -> Option<PathBuf> {
    if source.is_file() {
        return source
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| v.eq_ignore_ascii_case("commodity_trade_locations.json"))
            .map(|_| source.to_path_buf());
    }
    [
        source
            .join("resources")
            .join("commodity_trade_locations.json"),
        source.join("commodity_trade_locations.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn stable_negative_id(value: &str) -> i64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    -((hash & 0x3fff_ffff_ffff_ffff) as i64).max(1)
}

fn is_mineral(commodity: &ScUnpackedCommodity) -> bool {
    commodity.commodity_groups.iter().any(|group| {
        matches!(
            group.as_str(),
            "Mineral" | "Metal" | "Raw_Minerals" | "UnrefinedOres"
        )
    })
}

fn normalize_name(value: &str) -> String {
    value
        .replace("(Raw)", "")
        .replace("(Ore)", "")
        .replace("Quantainium", "Quantanium")
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_raw_names() {
        assert_eq!(normalize_name("Bexalite (Raw)"), "bexalite");
        assert_eq!(normalize_name("Riccite (Ore)"), "riccite");
    }

    #[test]
    fn accepts_mineral_groups() {
        let commodity = ScUnpackedCommodity {
            commodity_groups: vec!["Raw_Minerals".to_owned()],
            ..Default::default()
        };
        assert!(is_mineral(&commodity));
    }

    #[test]
    fn embedded_trade_locations_discover_refinery_station_baseline() {
        let refineries = load_scunpacked_refinery_locations(None).unwrap();
        assert!(!refineries.is_empty());
        assert!(
            refineries
                .iter()
                .any(|r| r.location.eq_ignore_ascii_case("ARC-L4 Faint Glen Station"))
        );
    }
}
