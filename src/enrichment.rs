use std::{
    cmp::Ordering,
    collections::BTreeMap,
    path::Path,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    data::{MineralProfile, load_scunpacked_refinery_locations},
    provider_cache::{ProviderCache, read_cached_text},
    provider_http::{cached_get_bytes, cached_get_text},
};

const UEX_COMMODITIES_TTL: Duration = Duration::from_secs(60 * 60);
const UEX_PRICES_TTL: Duration = Duration::from_secs(30 * 60);
const UEX_REFINERY_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const WIKI_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MEDIA_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const CURRENT_GAME_VERSION_PREFIX: &str = "4.10";

#[derive(Debug, Clone)]
pub struct EnrichmentConfig {
    pub use_uex: bool,
    pub uex_base_url: String,
    pub use_starcitizen_tools: bool,
    pub starcitizen_tools_api_url: String,
    pub use_star_citizen_wiki_api: bool,
    pub star_citizen_wiki_api_url: String,
    pub analysis_base_scu: f32,
    pub refining_method_base_efficiency: BTreeMap<String, f32>,
    pub selected_refinery_terminal_id: Option<i64>,
    pub refining_strategy: String,
    pub refinery_options: Vec<RefineryOption>,
    pub refining_methods: Vec<RefiningMethod>,
}

#[derive(Debug, Clone, Default)]
pub struct RefineryOption {
    pub terminal_id: i64,
    pub location: String,
    pub terminal: String,
    pub system: String,
    pub bonuses: BTreeMap<String, f64>,
}

impl RefineryOption {
    pub fn display_label(&self) -> String {
        if self.system.is_empty() {
            self.location.clone()
        } else {
            format!("{} · {}", self.location, self.system)
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RefiningMethod {
    pub code: String,
    pub name: String,
    pub yield_rating: i64,
    pub cost_rating: i64,
    pub speed_rating: i64,
}

pub fn default_refining_methods() -> Vec<RefiningMethod> {
    [
        ("COR", "Cormack", 1, 2, 3),
        ("DIN", "Dinyx Solventation", 3, 1, 1),
        ("EST", "Electrostarolysis", 2, 2, 2),
        ("GAS", "Gaskin Process", 2, 3, 3),
        ("PYR", "Pyrometric Chromalysis", 3, 3, 1),
        ("KZW", "Kazen Winnowing", 1, 2, 2),
        ("TND", "Thermonatic Deposition", 2, 2, 1),
        ("FRX", "Ferron Exchange", 3, 2, 1),
        ("XCR", "XCR Reaction", 1, 3, 3),
    ]
    .into_iter()
    .map(
        |(code, name, yield_rating, cost_rating, speed_rating)| RefiningMethod {
            code: code.to_owned(),
            name: name.to_owned(),
            yield_rating,
            cost_rating,
            speed_rating,
        },
    )
    .collect()
}

#[derive(Debug, Clone, Default)]
pub struct MarketQuote {
    pub price_per_scu: f64,
    pub location: String,
    pub terminal: String,
    pub system: String,
    pub game_version: Option<String>,
    pub updated_unix: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct RefineryRecommendation {
    pub location: String,
    pub terminal: String,
    pub system: String,
    pub yield_bonus_percent: f64,
    pub method_name: Option<String>,
    pub method_code: Option<String>,
    pub method_yield_rating: Option<i64>,
    pub method_cost_rating: Option<i64>,
    pub method_speed_rating: Option<i64>,
    pub estimated_cost: Option<f64>,
    pub estimated_minutes: Option<f64>,
    pub estimated_refined_scu: f64,
    pub estimate_basis: String,
}

#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub source: String,
    pub page_url: Option<String>,
    pub image_url: Option<String>,
    pub image_bytes: Option<Vec<u8>>,
    pub extract: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LocationInfo {
    pub name: String,
    pub system: String,
    pub description: Option<String>,
    pub page_url: Option<String>,
    pub image_url: Option<String>,
    pub image_bytes: Option<Vec<u8>>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EnrichedMineral {
    pub name: String,
    pub uuid: Option<String>,
    pub raw_uuid: Option<String>,
    pub description: Option<String>,
    pub tier: Option<String>,
    pub density_g_per_cc: Option<f32>,
    pub commodity_groups: Vec<String>,
    pub analysis_scu: f32,
    pub raw_market: Option<MarketQuote>,
    pub refined_market: Option<MarketQuote>,
    pub refinery: Option<RefineryRecommendation>,
    pub raw_sale_location: Option<LocationInfo>,
    pub refined_sale_location: Option<LocationInfo>,
    pub refinery_location: Option<LocationInfo>,
    pub media: Option<MediaInfo>,
    pub raw_value: Option<f64>,
    pub refined_gross_value: Option<f64>,
    pub estimated_net_value: Option<f64>,
    pub providers_used: Vec<String>,
    pub warnings: Vec<String>,
    /// True only after all enabled provider, market, refinery, location and media lookups finished.
    pub enrichment_complete: bool,
}

#[derive(Debug, Clone)]
pub enum EnrichmentEvent {
    Ready(EnrichedMineral),
    Error { name: String, message: String },
}

#[derive(Default)]
pub struct EnrichmentWorker {
    receiver: Option<Receiver<EnrichmentEvent>>,
    in_flight: Option<String>,
}

impl EnrichmentWorker {
    pub fn request(
        &mut self,
        name: String,
        profile: Option<MineralProfile>,
        config: EnrichmentConfig,
    ) {
        if self.in_flight.as_deref() == Some(name.as_str()) {
            return;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.receiver = Some(rx);
        self.in_flight = Some(name.clone());
        thread::spawn(move || {
            let event = match enrich_mineral(&name, profile, &config) {
                Ok(value) => EnrichmentEvent::Ready(value),
                Err(error) => EnrichmentEvent::Error {
                    name,
                    message: format!("{error:#}"),
                },
            };
            let _ = tx.send(event);
        });
    }

    pub fn drain_events(&mut self) -> Vec<EnrichmentEvent> {
        let mut events = Vec::new();
        let Some(rx) = &self.receiver else {
            return events;
        };
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        if !events.is_empty() {
            self.receiver = None;
            self.in_flight = None;
        }
        events
    }

    pub fn in_flight(&self) -> Option<&str> {
        self.in_flight.as_deref()
    }
}

pub fn enrich_mineral(
    name: &str,
    profile: Option<MineralProfile>,
    config: &EnrichmentConfig,
) -> Result<EnrichedMineral> {
    let mut result = EnrichedMineral {
        name: name.to_owned(),
        analysis_scu: config.analysis_base_scu.max(0.1),
        ..Default::default()
    };

    if let Some(profile) = profile {
        let base = profile.refined.as_ref().or(profile.raw.as_ref());
        result.uuid = profile.refined.as_ref().map(|v| v.uuid.clone());
        result.raw_uuid = profile.raw.as_ref().map(|v| v.uuid.clone());
        if let Some(base) = base {
            result.description = non_empty(&base.description);
            result.tier = base.tier.clone();
            result.density_g_per_cc = base.density_g_per_cc;
            result.commodity_groups = base.commodity_groups.clone();
        }
        result.providers_used.push("SCUnpacked".to_owned());
    }

    let mut uex_refined: Option<UexCommodity> = None;
    let mut uex_raw: Option<UexCommodity> = None;
    let mut uex_lookup_succeeded = !config.use_uex;
    let mut refined_market_complete = !config.use_uex;
    if config.use_uex {
        match resolve_uex(name, config) {
            Ok(uex) => {
                uex_lookup_succeeded = true;
                let refined_expected = uex.refined.is_some();
                refined_market_complete = !refined_expected || uex.refined_market.is_some();
                if refined_expected && !refined_market_complete {
                    result.warnings.push(
                        "UEX mapped a refined commodity but no refined market quote was resolved; report generation deferred."
                            .to_owned(),
                    );
                }
                result.raw_market = uex.raw_market;
                result.refined_market = uex.refined_market;
                result.refinery = uex.refinery;
                uex_refined = uex.refined;
                uex_raw = uex.raw;
                result.providers_used.push("UEX".to_owned());
                result.warnings.extend(uex.warnings);
            }
            Err(error) => {
                result
                    .warnings
                    .push(format!("UEX enrichment failed: {error:#}"));
            }
        }
    }

    // Refining calculation is intentionally independent from external providers.
    // The configured refinery/method dataset is composed by the app from the local
    // SCUnpacked baseline plus optional persisted UEX cache/network updates.
    if result.refinery.is_none() {
        result.refinery = resolve_configured_refinery(name, config);
    }

    if result.uuid.is_none() {
        result.uuid = uex_refined.as_ref().and_then(|v| v.uuid.clone());
    }
    if result.raw_uuid.is_none() {
        result.raw_uuid = uex_raw.as_ref().and_then(|v| v.uuid.clone());
    }

    if config.use_star_citizen_wiki_api {
        match resolve_wiki_api(&result, config) {
            Ok(Some(wiki)) => {
                if result.description.is_none() {
                    result.description = wiki.description.clone();
                }
                if result.uuid.is_none() {
                    result.uuid = wiki.uuid.clone();
                }
                if wiki.media.is_some() {
                    result.media = wiki.media;
                }
                result
                    .providers_used
                    .push("api.star-citizen.wiki".to_owned());
            }
            Ok(None) => {}
            Err(error) => result
                .warnings
                .push(format!("star-citizen.wiki API failed: {error:#}")),
        }
    }

    if config.use_starcitizen_tools {
        match resolve_starcitizen_tools(name, config) {
            Ok(Some(media)) => {
                if result.description.is_none() {
                    result.description = media.extract.clone();
                }
                if result.media.is_none() {
                    result.media = Some(media);
                }
                result.providers_used.push("StarCitizen.Tools".to_owned());
            }
            Ok(None) => {}
            Err(error) => result
                .warnings
                .push(format!("StarCitizen.Tools failed: {error:#}")),
        }

        result.raw_sale_location = result.raw_market.as_ref().and_then(|quote| {
            resolve_location_info(&quote.location, &quote.system, config)
                .ok()
                .flatten()
        });
        result.refined_sale_location = result.refined_market.as_ref().and_then(|quote| {
            resolve_location_info(&quote.location, &quote.system, config)
                .ok()
                .flatten()
        });
        result.refinery_location = result.refinery.as_ref().and_then(|refinery| {
            resolve_location_info(&refinery.location, &refinery.system, config)
                .ok()
                .flatten()
        });
    }

    calculate_values(&mut result);
    // Report generation is evaluated only after the local refining calculator has run.
    // UEX is responsible for dynamic market data when enabled, but a refinery
    // recommendation is a local/calculator concern and must not be gated by UEX.
    let refinery_required = uex_raw.is_some()
        || result.raw_uuid.is_some()
        || result.commodity_groups.iter().any(|group| {
            matches!(
                group.as_str(),
                "Mineral" | "Metal" | "Raw_Minerals" | "UnrefinedOres"
            )
        });
    let refinery_complete = !refinery_required || result.refinery.is_some();
    if refinery_required && !refinery_complete {
        result.warnings.push(
            "No local/configured refinery recommendation could be resolved; report generation deferred."
                .to_owned(),
        );
    }
    result.enrichment_complete =
        uex_lookup_succeeded && refined_market_complete && refinery_complete;
    Ok(result)
}

fn calculate_values(result: &mut EnrichedMineral) {
    let scu = result.analysis_scu as f64;
    result.raw_value = result.raw_market.as_ref().map(|q| q.price_per_scu * scu);

    if let Some(refined) = &result.refined_market {
        let refined_scu = result
            .refinery
            .as_ref()
            .map(|r| r.estimated_refined_scu)
            .unwrap_or(scu);
        result.refined_gross_value = Some(refined.price_per_scu * refined_scu);
        result.estimated_net_value = result.refined_gross_value.map(|gross| {
            gross
                - result
                    .refinery
                    .as_ref()
                    .and_then(|r| r.estimated_cost)
                    .unwrap_or(0.0)
        });
    }
}

#[derive(Debug, Deserialize, Clone)]
struct UexCommodity {
    id: i64,
    #[serde(default)]
    id_parent: Option<i64>,
    #[serde(default)]
    uuid: Option<String>,
    name: String,
    #[serde(default)]
    is_raw: i64,
    #[serde(default)]
    is_refined: i64,
}

#[derive(Default)]
struct UexResolved {
    refined: Option<UexCommodity>,
    raw: Option<UexCommodity>,
    raw_market: Option<MarketQuote>,
    refined_market: Option<MarketQuote>,
    refinery: Option<RefineryRecommendation>,
    warnings: Vec<String>,
}

fn resolve_uex(name: &str, config: &EnrichmentConfig) -> Result<UexResolved> {
    let base = config.uex_base_url.trim_end_matches('/');
    let commodities_url = format!("{base}/commodities");
    let text = cached_get_text(ProviderCache::Uex, &commodities_url, UEX_COMMODITIES_TTL)?;
    let root: Value = serde_json::from_str(&text).context("Invalid UEX commodities JSON")?;
    let commodities: Vec<UexCommodity> =
        serde_json::from_value(root_data(root)).context("Invalid UEX commodities data")?;
    let needle = normalize_name(name);

    let matching: Vec<UexCommodity> = commodities
        .into_iter()
        .filter(|c| normalize_name(&c.name) == needle)
        .collect();
    if matching.is_empty() {
        return Ok(UexResolved {
            warnings: vec![format!("UEX has no commodity mapping for {name}.")],
            ..Default::default()
        });
    }

    let refined = matching
        .iter()
        .find(|c| c.is_refined == 1 || c.is_raw == 0 && !is_raw_name(&c.name))
        .cloned();
    let raw = matching
        .iter()
        .find(|c| c.is_raw == 1 || is_raw_name(&c.name))
        .cloned()
        .or_else(|| {
            let refined_id = refined.as_ref().map(|c| c.id)?;
            matching
                .iter()
                .find(|c| c.id_parent == Some(refined_id))
                .cloned()
        });

    let mut resolved = UexResolved {
        refined: refined.clone(),
        raw: raw.clone(),
        ..Default::default()
    };

    if let Some(refined) = &refined {
        let url = format!("{base}/commodities_prices?id_commodity={}", refined.id);
        match load_market_quotes(&url, ProviderCache::Uex, UEX_PRICES_TTL) {
            Ok(quotes) => resolved.refined_market = best_market_quote(quotes),
            Err(error) => resolved
                .warnings
                .push(format!("Refined UEX prices unavailable: {error:#}")),
        }
    }
    if let Some(raw) = &raw {
        let url = format!("{base}/commodities_raw_prices?id_commodity={}", raw.id);
        match load_market_quotes(&url, ProviderCache::Uex, UEX_PRICES_TTL) {
            Ok(quotes) => resolved.raw_market = best_market_quote(quotes),
            Err(error) => resolved
                .warnings
                .push(format!("RAW UEX prices unavailable: {error:#}")),
        }
    }
    resolved.refinery = resolve_configured_refinery(name, config);

    Ok(resolved)
}

fn load_market_quotes(
    url: &str,
    provider: ProviderCache,
    ttl: Duration,
) -> Result<Vec<MarketQuote>> {
    let text = cached_get_text(provider, url, ttl)?;
    let root: Value = serde_json::from_str(&text)?;
    let data = root_data(root);
    let rows = data.as_array().cloned().unwrap_or_default();
    let mut quotes = Vec::new();
    for row in rows {
        let price = number(&row, "price_sell").unwrap_or(0.0);
        if price <= 0.0 {
            continue;
        }
        let terminal = string(&row, "terminal_name").unwrap_or_default();
        let station = first_non_empty(&[
            string(&row, "space_station_name"),
            string(&row, "city_name"),
            string(&row, "outpost_name"),
            string(&row, "poi_name"),
            string(&row, "orbit_name"),
            string(&row, "planet_name"),
        ])
        .unwrap_or_else(|| terminal.clone());
        quotes.push(MarketQuote {
            price_per_scu: price,
            location: station,
            terminal,
            system: string(&row, "star_system_name").unwrap_or_default(),
            game_version: string(&row, "game_version"),
            updated_unix: integer(&row, "date_modified").or_else(|| integer(&row, "date_added")),
        });
    }
    Ok(quotes)
}

fn best_market_quote(quotes: Vec<MarketQuote>) -> Option<MarketQuote> {
    let current: Vec<MarketQuote> = quotes
        .iter()
        .filter(|q| {
            q.game_version
                .as_deref()
                .map(|v| v.starts_with(CURRENT_GAME_VERSION_PREFIX))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    let candidates = if current.is_empty() { quotes } else { current };
    candidates.into_iter().max_by(|a, b| {
        a.price_per_scu
            .partial_cmp(&b.price_per_scu)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.updated_unix.cmp(&b.updated_unix))
    })
}

pub fn load_refinery_options(
    base: &str,
    allow_uex_network: bool,
    scunpacked_source: Option<&Path>,
) -> Result<Vec<RefineryOption>> {
    let mut by_location: BTreeMap<String, RefineryOption> = BTreeMap::new();

    // Local baseline: derive refinery-capable station candidates from the bundled or
    // user-selected SCUnpacked commodity trade-location dataset. This keeps the selector
    // available without any external provider and lets new locations appear after a
    // SCUnpacked update even before UEX has metadata for them.
    if let Ok(locations) = load_scunpacked_refinery_locations(scunpacked_source) {
        for location in locations {
            let key = normalize_name(&location.location);
            by_location.entry(key).or_insert_with(|| RefineryOption {
                terminal_id: location.stable_id,
                location: location.location,
                terminal: String::new(),
                system: String::new(),
                bonuses: BTreeMap::new(),
            });
        }
    }

    // Ship the current known refinery modifiers as a local seed so a fresh install can
    // calculate bonuses immediately. SCUnpacked remains authoritative for discovering
    // location additions/removals; UEX cache/network may then update these modifiers.
    merge_refinery_yields_text(
        &mut by_location,
        include_str!("../data/scunpacked-4.10/resources/refinery_yields_seed.json"),
    )?;

    // UEX is an optional enrichment/update layer. When disabled we still consume its
    // existing cache until the user explicitly deletes that cache.
    let yields_url = format!("{}/refineries_yields", base.trim_end_matches('/'));
    let uex_text = if allow_uex_network {
        cached_get_text(ProviderCache::Uex, &yields_url, UEX_REFINERY_TTL).ok()
    } else {
        read_cached_text(ProviderCache::Uex, &yields_url)
            .ok()
            .flatten()
    };

    if let Some(text) = uex_text {
        merge_refinery_yields_text(&mut by_location, &text)?;
    }

    let mut values: Vec<_> = by_location.into_values().collect();
    values.sort_by(|a, b| {
        a.display_label()
            .to_ascii_lowercase()
            .cmp(&b.display_label().to_ascii_lowercase())
    });
    Ok(values)
}

fn merge_refinery_yields_text(
    by_location: &mut BTreeMap<String, RefineryOption>,
    text: &str,
) -> Result<()> {
    let root: Value = serde_json::from_str(text)?;
    let rows = root_data(root).as_array().cloned().unwrap_or_default();
    for row in rows {
        let Some(terminal_id) = integer(&row, "id_terminal") else {
            continue;
        };
        if terminal_id <= 0 {
            continue;
        }
        let location = first_non_empty(&[
            string(&row, "space_station_name"),
            string(&row, "city_name"),
            string(&row, "outpost_name"),
            string(&row, "orbit_name"),
        ])
        .unwrap_or_else(|| "Unknown refinery".to_owned());
        let key = normalize_name(&location);
        let entry = by_location.entry(key).or_insert_with(|| RefineryOption {
            terminal_id,
            location: location.clone(),
            terminal: String::new(),
            system: String::new(),
            bonuses: BTreeMap::new(),
        });
        entry.terminal_id = terminal_id;
        entry.location = location;
        if let Some(terminal) = string(&row, "terminal_name") {
            entry.terminal = terminal;
        }
        if let Some(system) = string(&row, "star_system_name") {
            entry.system = system;
        }
        if let Some(name) = string(&row, "commodity_name")
            && let Some(value) = number(&row, "value")
        {
            entry.bonuses.insert(refinery_material_key(&name), value);
        }
    }
    Ok(())
}

pub fn load_refining_methods(base: &str, allow_uex_network: bool) -> Vec<RefiningMethod> {
    let mut methods = default_refining_methods();
    merge_refining_methods_text(
        &mut methods,
        include_str!("../data/scunpacked-4.10/resources/refinery_methods_seed.json"),
    );
    let url = format!("{}/refineries_methods", base.trim_end_matches('/'));
    let text = if allow_uex_network {
        cached_get_text(ProviderCache::Uex, &url, UEX_REFINERY_TTL).ok()
    } else {
        read_cached_text(ProviderCache::Uex, &url).ok().flatten()
    };
    if let Some(text) = text {
        merge_refining_methods_text(&mut methods, &text);
    }
    methods.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    methods
}

fn merge_refining_methods_text(methods: &mut Vec<RefiningMethod>, text: &str) {
    let Ok(root) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let rows = root_data(root).as_array().cloned().unwrap_or_default();
    for row in rows {
        let Some(code) = string(&row, "code") else {
            continue;
        };
        if let Some(method) = methods
            .iter_mut()
            .find(|method| method.code.eq_ignore_ascii_case(&code))
        {
            if let Some(name) = string(&row, "name") {
                method.name = name;
            }
            method.yield_rating = integer(&row, "rating_yield").unwrap_or(method.yield_rating);
            method.cost_rating = integer(&row, "rating_cost").unwrap_or(method.cost_rating);
            method.speed_rating = integer(&row, "rating_speed").unwrap_or(method.speed_rating);
        } else {
            methods.push(RefiningMethod {
                code,
                name: string(&row, "name").unwrap_or_else(|| "Unknown method".to_owned()),
                yield_rating: integer(&row, "rating_yield").unwrap_or(0),
                cost_rating: integer(&row, "rating_cost").unwrap_or(99),
                speed_rating: integer(&row, "rating_speed").unwrap_or(0),
            });
        }
    }
}

fn resolve_configured_refinery(
    material_name: &str,
    config: &EnrichmentConfig,
) -> Option<RefineryRecommendation> {
    let material_key = refinery_material_key(material_name);
    let refinery = if let Some(selected_id) = config.selected_refinery_terminal_id {
        config
            .refinery_options
            .iter()
            .find(|option| option.terminal_id == selected_id)?
    } else {
        // Automatic mode is local-first: choose the refinery with the best known
        // material modifier. If no modifier is known yet, use the first available
        // local refinery instead of returning no recommendation at all.
        config.refinery_options.iter().max_by(|a, b| {
            let a_bonus = a.bonuses.get(&material_key).copied();
            let b_bonus = b.bonuses.get(&material_key).copied();
            match (a_bonus, b_bonus) {
                (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Greater,
                (None, Some(_)) => Ordering::Less,
                (None, None) => Ordering::Equal,
            }
        })?
    };
    let bonus = refinery.bonuses.get(&material_key).copied().unwrap_or(0.0);
    let method = select_refining_method(
        &config.refining_methods,
        &config.refining_strategy,
        &config.refining_method_base_efficiency,
    );

    let mut recommendation = RefineryRecommendation {
        location: refinery.location.clone(),
        terminal: refinery.terminal.clone(),
        system: refinery.system.clone(),
        yield_bonus_percent: bonus,
        estimated_refined_scu: 0.0,
        estimate_basis:
            "Configured method base efficiency plus selected refinery material modifier".to_owned(),
        ..Default::default()
    };

    if let Some(method) = method {
        recommendation.method_name = Some(method.name.clone());
        recommendation.method_code = Some(method.code.clone());
        recommendation.method_yield_rating = Some(method.yield_rating);
        recommendation.method_cost_rating = Some(method.cost_rating);
        recommendation.method_speed_rating = Some(method.speed_rating);
        let configured_base = config
            .refining_method_base_efficiency
            .get(&method.code)
            .copied()
            .unwrap_or(55.0)
            .clamp(0.0, 100.0) as f64;
        let effective_percent = (configured_base + bonus).clamp(0.0, 100.0);
        recommendation.estimated_refined_scu =
            config.analysis_base_scu.max(0.1) as f64 * effective_percent / 100.0;
        recommendation.estimate_basis = format!(
            "Configured method base {:.1}% + selected refinery modifier {:+.1} pp = {:.1}% effective yield",
            configured_base, bonus, effective_percent
        );
    } else {
        recommendation.estimated_refined_scu = config.analysis_base_scu.max(0.1) as f64 * 0.55;
        recommendation.estimate_basis =
            "No refining method metadata available; temporary 55% fallback".to_owned();
    }
    Some(recommendation)
}

fn method_efficiency(
    method: &RefiningMethod,
    method_base_efficiency: &BTreeMap<String, f32>,
) -> f32 {
    method_base_efficiency
        .get(&method.code)
        .copied()
        .unwrap_or(55.0)
}

fn select_refining_method<'a>(
    methods: &'a [RefiningMethod],
    strategy: &str,
    method_base_efficiency: &BTreeMap<String, f32>,
) -> Option<&'a RefiningMethod> {
    methods.iter().max_by(|a, b| {
        let eff_a = method_efficiency(a, method_base_efficiency);
        let eff_b = method_efficiency(b, method_base_efficiency);
        match strategy {
            "cost" => b
                .cost_rating
                .cmp(&a.cost_rating)
                .then_with(|| eff_a.partial_cmp(&eff_b).unwrap_or(Ordering::Equal))
                .then_with(|| a.yield_rating.cmp(&b.yield_rating)),
            "time" => a
                .speed_rating
                .cmp(&b.speed_rating)
                .then_with(|| eff_a.partial_cmp(&eff_b).unwrap_or(Ordering::Equal))
                .then_with(|| a.yield_rating.cmp(&b.yield_rating)),
            _ => eff_a
                .partial_cmp(&eff_b)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.yield_rating.cmp(&b.yield_rating))
                .then_with(|| b.cost_rating.cmp(&a.cost_rating)),
        }
    })
}

#[derive(Default)]
struct WikiApiResolved {
    uuid: Option<String>,
    description: Option<String>,
    media: Option<MediaInfo>,
}

fn resolve_wiki_api(
    result: &EnrichedMineral,
    config: &EnrichmentConfig,
) -> Result<Option<WikiApiResolved>> {
    let Some(uuid) = result.uuid.as_deref() else {
        return Ok(None);
    };
    let base = config.star_citizen_wiki_api_url.trim_end_matches('/');
    let url = format!("{base}/commodities/{uuid}");
    let text = cached_get_text(ProviderCache::StarCitizenWikiApi, &url, WIKI_TTL)?;
    let root: Value = serde_json::from_str(&text).context("Invalid star-citizen.wiki JSON")?;
    let data = root.get("data").cloned().unwrap_or(root);
    if !data.is_object() {
        return Ok(None);
    }
    let image_url = data
        .get("images")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|image| string(image, "thumbnail_url").or_else(|| string(image, "original_url")));
    let mut media = image_url.as_ref().map(|image_url| MediaInfo {
        source: "api.star-citizen.wiki".to_owned(),
        page_url: string(&data, "web_url"),
        image_url: Some(image_url.clone()),
        image_bytes: None,
        extract: string(&data, "description"),
    });
    if let Some(media_info) = &mut media
        && let Some(image_url) = media_info.image_url.as_deref()
    {
        if let Ok(bytes) = cached_get_bytes(ProviderCache::StarCitizenWikiApi, image_url, MEDIA_TTL)
        {
            media_info.image_bytes = Some(bytes);
        }
    }
    Ok(Some(WikiApiResolved {
        uuid: string(&data, "uuid"),
        description: string(&data, "description"),
        media,
    }))
}

fn resolve_starcitizen_tools(name: &str, config: &EnrichmentConfig) -> Result<Option<MediaInfo>> {
    let base = config.starcitizen_tools_api_url.trim_end_matches('/');
    let title = percent_encode_query(name);
    let url = format!(
        "{base}?action=query&format=json&prop=extracts%7Cpageimages&exintro=1&explaintext=1&pithumbsize=640&titles={title}"
    );
    let text = cached_get_text(ProviderCache::StarCitizenTools, &url, WIKI_TTL)?;
    let root: Value = serde_json::from_str(&text).context("Invalid StarCitizen.Tools JSON")?;
    let page = root
        .pointer("/query/pages")
        .and_then(Value::as_object)
        .and_then(|pages| pages.values().next());
    let Some(page) = page else {
        return Ok(None);
    };
    if page.get("missing").is_some() {
        return Ok(None);
    }
    let image_url = page.get("thumbnail").and_then(|v| string(v, "source"));
    let mut media = MediaInfo {
        source: "StarCitizen.Tools".to_owned(),
        page_url: Some(format!(
            "https://starcitizen.tools/{}",
            name.replace(' ', "_")
        )),
        image_url: image_url.clone(),
        image_bytes: None,
        extract: string(page, "extract"),
    };
    if let Some(image_url) = image_url {
        if let Ok(bytes) = cached_get_bytes(ProviderCache::StarCitizenTools, &image_url, MEDIA_TTL)
        {
            media.image_bytes = Some(bytes);
        }
    }
    Ok(Some(media))
}

fn resolve_location_info(
    name: &str,
    system: &str,
    config: &EnrichmentConfig,
) -> Result<Option<LocationInfo>> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let Some(media) = resolve_starcitizen_tools(name, config)? else {
        return Ok(Some(LocationInfo {
            name: name.to_owned(),
            system: system.to_owned(),
            ..Default::default()
        }));
    };
    Ok(Some(LocationInfo {
        name: name.to_owned(),
        system: system.to_owned(),
        description: media.extract,
        page_url: media.page_url,
        image_url: media.image_url,
        image_bytes: media.image_bytes,
        source: Some(media.source),
    }))
}

fn root_data(root: Value) -> Value {
    root.get("data").cloned().unwrap_or(root)
}

pub fn refinery_material_key(value: &str) -> String {
    normalize_name(value)
}

fn normalize_name(value: &str) -> String {
    value
        .replace("(Raw)", "")
        .replace("(Ore)", "")
        .replace("Quantainium", "Quantanium")
        .trim()
        .to_ascii_lowercase()
}

fn is_raw_name(value: &str) -> bool {
    value.contains("(Raw)") || value.contains("(Ore)")
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn string(row: &Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn number(row: &Value, key: &str) -> Option<f64> {
    row.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_i64().map(|n| n as f64))
            .or_else(|| v.as_u64().map(|n| n as f64))
    })
}

fn integer(row: &Value, key: &str) -> Option<i64> {
    row.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
    })
}

fn first_non_empty(values: &[Option<String>]) -> Option<String> {
    values.iter().find_map(Clone::clone)
}

fn percent_encode_query(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_uex_raw_names() {
        assert_eq!(normalize_name("Bexalite (Raw)"), "bexalite");
        assert_eq!(normalize_name("Riccite (Ore)"), "riccite");
    }

    #[test]
    fn local_method_baseline_is_complete() {
        let methods = default_refining_methods();
        assert_eq!(methods.len(), 9);
        assert!(methods.iter().any(|m| m.code == "DIN"));
        assert!(methods.iter().any(|m| m.code == "GAS"));
    }

    #[test]
    fn offline_refinery_seed_contains_mic_l5_iron_modifier() {
        let options = load_refinery_options("https://invalid.local", false, None).unwrap();
        let mic_l5 = options
            .iter()
            .find(|r| {
                r.location
                    .eq_ignore_ascii_case("MIC-L5 Modern Icarus Station")
            })
            .expect("MIC-L5 in bundled refinery baseline");
        assert_eq!(mic_l5.bonuses.get("iron").copied(), Some(8.0));

        let methods = load_refining_methods("https://invalid.local", false);
        let din = methods.iter().find(|m| m.code == "DIN").expect("Dinyx");
        assert_eq!(
            (din.yield_rating, din.cost_rating, din.speed_rating),
            (3, 1, 1)
        );
    }

    #[test]
    fn method_strategy_selects_expected_methods() {
        let methods = default_refining_methods();
        let efficiencies = crate::settings::default_refining_method_base_efficiency();

        let efficiency = select_refining_method(&methods, "efficiency", &efficiencies).unwrap();
        assert_eq!(efficiency.code, "DIN");

        let cost = select_refining_method(&methods, "cost", &efficiencies).unwrap();
        assert_eq!(cost.code, "DIN");

        let time = select_refining_method(&methods, "time", &efficiencies).unwrap();
        assert_eq!(time.code, "GAS");
    }

    #[test]
    fn automatic_refinery_mode_resolves_local_recommendation() {
        let mut best = RefineryOption {
            terminal_id: 1,
            location: "Best Test Refinery".to_owned(),
            ..Default::default()
        };
        best.bonuses.insert("savrilium".to_owned(), 7.0);
        let mut worse = RefineryOption {
            terminal_id: 2,
            location: "Worse Test Refinery".to_owned(),
            ..Default::default()
        };
        worse.bonuses.insert("savrilium".to_owned(), -2.0);
        let config = EnrichmentConfig {
            use_uex: false,
            uex_base_url: String::new(),
            use_starcitizen_tools: false,
            starcitizen_tools_api_url: String::new(),
            use_star_citizen_wiki_api: false,
            star_citizen_wiki_api_url: String::new(),
            analysis_base_scu: 10.0,
            refining_method_base_efficiency:
                crate::settings::default_refining_method_base_efficiency(),
            selected_refinery_terminal_id: None,
            refining_strategy: "efficiency".to_owned(),
            refinery_options: vec![worse, best],
            refining_methods: default_refining_methods(),
        };
        let refinery = resolve_configured_refinery("Savrilium", &config)
            .expect("automatic refinery recommendation");
        assert_eq!(refinery.location, "Best Test Refinery");
        assert_eq!(refinery.yield_bonus_percent, 7.0);
        assert!(refinery.estimated_refined_scu > 0.0);
    }

    #[test]
    fn url_encoding_handles_spaces() {
        assert_eq!(percent_encode_query("Bexalite Raw"), "Bexalite%20Raw");
    }

    #[test]
    #[ignore = "requires live provider network access"]
    fn live_savrilium_automatic_refinery_completes_report_data() {
        let catalog = crate::data::ScUnpackedCatalog::embedded().unwrap();
        let profile = catalog.profile_for_name("Savrilium");
        let config = EnrichmentConfig {
            use_uex: true,
            uex_base_url: "https://api.uexcorp.uk/2.0".to_owned(),
            use_starcitizen_tools: false,
            starcitizen_tools_api_url: "https://starcitizen.tools/api.php".to_owned(),
            use_star_citizen_wiki_api: false,
            star_citizen_wiki_api_url: "https://api.star-citizen.wiki/api".to_owned(),
            analysis_base_scu: 10.0,
            refining_method_base_efficiency:
                crate::settings::default_refining_method_base_efficiency(),
            selected_refinery_terminal_id: None,
            refining_strategy: "efficiency".to_owned(),
            refinery_options: load_refinery_options("https://api.uexcorp.uk/2.0", true, None)
                .unwrap_or_default(),
            refining_methods: load_refining_methods("https://api.uexcorp.uk/2.0", true),
        };
        let result = enrich_mineral("Savrilium", profile, &config).unwrap();
        eprintln!("Savrilium enrichment: {result:#?}");
        assert!(
            result.refined_market.is_some(),
            "Savrilium refined market missing"
        );
        assert!(
            result.refinery.is_some(),
            "Savrilium local refinery recommendation missing"
        );
        assert!(
            result.enrichment_complete,
            "Savrilium enrichment should be report-ready"
        );
    }

    #[test]
    #[ignore = "requires live provider network access"]
    fn live_selected_refinery_iron_uses_mic_l5_bonus() {
        let catalog = crate::data::ScUnpackedCatalog::embedded().unwrap();
        let profile = catalog.profile_for_name("Iron");
        let config = EnrichmentConfig {
            use_uex: true,
            uex_base_url: "https://api.uexcorp.uk/2.0".to_owned(),
            use_starcitizen_tools: false,
            starcitizen_tools_api_url: "https://starcitizen.tools/api.php".to_owned(),
            use_star_citizen_wiki_api: false,
            star_citizen_wiki_api_url: "https://api.star-citizen.wiki/api".to_owned(),
            analysis_base_scu: 10.0,
            refining_method_base_efficiency:
                crate::settings::default_refining_method_base_efficiency(),
            selected_refinery_terminal_id: Some(244),
            refining_strategy: "efficiency".to_owned(),
            refinery_options: load_refinery_options("https://api.uexcorp.uk/2.0", true, None)
                .unwrap_or_default(),
            refining_methods: load_refining_methods("https://api.uexcorp.uk/2.0", true),
        };
        let result = enrich_mineral("Iron", profile, &config).unwrap();
        let refinery = result.refinery.expect("Iron refinery recommendation");
        assert_eq!(refinery.location, "MIC-L5 Modern Icarus Station");
        assert!((refinery.yield_bonus_percent - 8.0).abs() < f64::EPSILON);
        assert_eq!(refinery.method_code.as_deref(), Some("DIN"));
        if refinery
            .estimate_basis
            .starts_with("Configured method base")
        {
            assert!((refinery.estimated_refined_scu - 6.8).abs() < 0.001);
        }
    }

    #[test]
    #[ignore = "requires live provider network access"]
    fn live_provider_smoke_lindinium() {
        let catalog = crate::data::ScUnpackedCatalog::embedded().unwrap();
        let profile = catalog.profile_for_name("Lindinium");
        let config = EnrichmentConfig {
            use_uex: true,
            uex_base_url: "https://api.uexcorp.uk/2.0".to_owned(),
            use_starcitizen_tools: true,
            starcitizen_tools_api_url: "https://starcitizen.tools/api.php".to_owned(),
            use_star_citizen_wiki_api: true,
            star_citizen_wiki_api_url: "https://api.star-citizen.wiki/api".to_owned(),
            analysis_base_scu: 10.0,
            refining_method_base_efficiency:
                crate::settings::default_refining_method_base_efficiency(),
            selected_refinery_terminal_id: Some(244),
            refining_strategy: "efficiency".to_owned(),
            refinery_options: load_refinery_options("https://api.uexcorp.uk/2.0", true, None)
                .unwrap_or_default(),
            refining_methods: load_refining_methods("https://api.uexcorp.uk/2.0", true),
        };
        let result = enrich_mineral("Lindinium", profile, &config).unwrap();
        eprintln!("Lindinium enrichment: {result:#?}");
        assert!(
            result.refined_market.is_some(),
            "Lindinium refined market missing"
        );
        assert!(result.refinery.is_some(), "Lindinium refinery missing");
        assert!(result.enrichment_complete);
    }

    #[test]
    #[ignore = "requires live provider network access"]
    fn live_provider_smoke_bexalite() {
        let catalog = crate::data::ScUnpackedCatalog::embedded().unwrap();
        let profile = catalog.profile_for_name("Bexalite");
        let config = EnrichmentConfig {
            use_uex: true,
            uex_base_url: "https://api.uexcorp.uk/2.0".to_owned(),
            use_starcitizen_tools: true,
            starcitizen_tools_api_url: "https://starcitizen.tools/api.php".to_owned(),
            use_star_citizen_wiki_api: true,
            star_citizen_wiki_api_url: "https://api.star-citizen.wiki/api".to_owned(),
            analysis_base_scu: 10.0,
            refining_method_base_efficiency:
                crate::settings::default_refining_method_base_efficiency(),
            selected_refinery_terminal_id: Some(244),
            refining_strategy: "efficiency".to_owned(),
            refinery_options: load_refinery_options("https://api.uexcorp.uk/2.0", true, None)
                .unwrap_or_default(),
            refining_methods: load_refining_methods("https://api.uexcorp.uk/2.0", true),
        };
        let result = enrich_mineral("Bexalite", profile, &config).unwrap();
        assert_eq!(result.name, "Bexalite");
        assert!(result.uuid.is_some());
        assert!(result.raw_market.is_some());
        assert!(result.refined_market.is_some());
        assert!(result.refinery.is_some());
        assert!(result.providers_used.iter().any(|v| v == "UEX"));
        assert!(
            result
                .providers_used
                .iter()
                .any(|v| v == "api.star-citizen.wiki")
        );
        assert!(
            result
                .providers_used
                .iter()
                .any(|v| v == "StarCitizen.Tools")
        );
        let image_bytes = result
            .media
            .as_ref()
            .and_then(|m| m.image_bytes.as_ref())
            .expect("provider media bytes");
        assert!(image::load_from_memory(image_bytes).is_ok());
    }
}
