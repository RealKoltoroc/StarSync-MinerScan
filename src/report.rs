use std::{fs, path::PathBuf};

use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Local;

use crate::enrichment::{EnrichedMineral, LocationInfo, MediaInfo};

pub fn reports_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("StarSync MinerScan").join("session-reports")
}

pub fn prepare_session_reports(clear_on_start: bool) -> Result<()> {
    let dir = reports_dir();
    if clear_on_start && dir.exists() {
        fs::remove_dir_all(&dir).context("Could not clear MinerScan session reports")?;
    }
    fs::create_dir_all(&dir).context("Could not create MinerScan report directory")?;
    Ok(())
}

pub fn latest_report() -> Option<PathBuf> {
    let dir = reports_dir();
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_none_or(|e| !e.eq_ignore_ascii_case("html"))
        {
            continue;
        }
        let modified = entry.metadata().ok()?.modified().ok()?;
        if newest.as_ref().is_none_or(|(stamp, _)| modified > *stamp) {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path)
}

pub fn write_enriched_report(enriched: &EnrichedMineral) -> Result<PathBuf> {
    ensure!(
        enriched.enrichment_complete,
        "Refusing to write MinerScan report before enrichment is complete"
    );
    let dir = reports_dir();
    fs::create_dir_all(&dir)?;
    let now = Local::now();
    let unix = now.timestamp();
    let safe_name = sanitize_filename(&enriched.name);
    let filename = format!(
        "{}_{}_Mineral_{}.html",
        safe_name,
        now.format("%H%M%S"),
        unix
    );
    let path = dir.join(filename);
    let html = render_report(enriched, &now.format("%Y-%m-%d %H:%M:%S").to_string());
    fs::write(&path, html).with_context(|| format!("Could not write {}", path.display()))?;
    Ok(path)
}

fn render_report(enriched: &EnrichedMineral, generated_at: &str) -> String {
    let logo = data_uri(include_bytes!("../assets/brand/StarSyncMinerScan_small.png"));
    let material_image = enriched
        .media
        .as_ref()
        .and_then(media_data_uri)
        .map(|uri| format!("<img class=\"hero-image\" src=\"{uri}\" alt=\"Mineral\">"))
        .unwrap_or_default();

    let raw_price = enriched
        .raw_market
        .as_ref()
        .map(|q| format!("{:.0} aUEC / SCU", q.price_per_scu))
        .unwrap_or_else(|| "No current quote".to_owned());
    let raw_value = enriched
        .raw_value
        .map(|v| format!("{v:.0} aUEC"))
        .unwrap_or_else(|| "--".to_owned());
    let refined_price = enriched
        .refined_market
        .as_ref()
        .map(|q| format!("{:.0} aUEC / SCU", q.price_per_scu))
        .unwrap_or_else(|| "No current quote".to_owned());
    let gross = enriched
        .refined_gross_value
        .map(|v| format!("{v:.0} aUEC"))
        .unwrap_or_else(|| "--".to_owned());
    let net = enriched
        .estimated_net_value
        .map(|v| format!("{v:.0} aUEC"))
        .unwrap_or_else(|| "--".to_owned());

    let refinery_html = if let Some(r) = &enriched.refinery {
        format!(
            "<div class=\"metric\"><span>Refinery</span><strong>{}</strong></div>\
             <div class=\"metric\"><span>Terminal</span><strong>{}</strong></div>\
             <div class=\"metric\"><span>Yield bonus</span><strong>{:+.0}%</strong></div>\
             <div class=\"metric\"><span>Method</span><strong>{}</strong></div>\
             <div class=\"metric\"><span>Output</span><strong>{:.2} SCU</strong></div>\
             <div class=\"metric\"><span>Cost</span><strong>{}</strong></div>\
             <div class=\"metric\"><span>Time</span><strong>{}</strong></div>\
             <p class=\"note\">{}</p>",
            esc(&r.location),
            esc(&r.terminal),
            r.yield_bonus_percent,
            esc(r.method_name.as_deref().unwrap_or("--")),
            r.estimated_refined_scu,
            r.estimated_cost
                .map(|v| format!("{v:.0} aUEC"))
                .unwrap_or_else(|| "--".into()),
            r.estimated_minutes
                .map(format_minutes)
                .unwrap_or_else(|| "--".into()),
            esc(&r.estimate_basis)
        )
    } else {
        "<p>No refinery recommendation available.</p>".to_owned()
    };

    let raw_location = location_card(
        "RAW SALE LOCATION",
        enriched.raw_sale_location.as_ref(),
        enriched.raw_market.as_ref().map(|q| q.location.as_str()),
        enriched.raw_market.as_ref().map(|q| q.system.as_str()),
    );
    let refinery_location = location_card(
        "REFINING LOCATION",
        enriched.refinery_location.as_ref(),
        enriched.refinery.as_ref().map(|q| q.location.as_str()),
        enriched.refinery.as_ref().map(|q| q.system.as_str()),
    );
    let refined_location = location_card(
        "REFINED SALE LOCATION",
        enriched.refined_sale_location.as_ref(),
        enriched
            .refined_market
            .as_ref()
            .map(|q| q.location.as_str()),
        enriched.refined_market.as_ref().map(|q| q.system.as_str()),
    );
    let location_cards = [raw_location, refinery_location, refined_location]
        .into_iter()
        .filter(|card| !card.is_empty())
        .collect::<String>();
    let locations_section = if location_cards.is_empty() {
        String::new()
    } else {
        format!(
            "<section class=\"location-section\"><h2>LOCATIONS</h2><div class=\"locations\">{location_cards}</div></section>"
        )
    };

    let warnings = if enriched.warnings.is_empty() {
        String::new()
    } else {
        format!(
            "<section><h2>Data notes</h2><ul>{}</ul></section>",
            enriched
                .warnings
                .iter()
                .map(|w| format!("<li>{}</li>", esc(w)))
                .collect::<String>()
        )
    };

    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{name} - StarSync MinerScan Report</title>
<style>
:root{{--bg:#0e0e0e;--panel:#191817;--panel2:#2d2925;--line:#795230;--accent:#e1842d;--text:#eee9e3;--muted:#99938d;--good:#71e7a0;--gold:#e3b75d}}
*{{box-sizing:border-box}} body{{margin:0;padding:4px;background:var(--bg);color:var(--text);font-family:"Segoe UI",Arial,sans-serif;font-size:13px}} .wrap{{width:100%;max-width:none;margin:0;padding:4px}}
.brand{{text-align:center;border-bottom:1px solid var(--line);padding:2px 0 5px}} .brand img{{width:min(150px,26vw);max-height:92px;object-fit:contain;height:auto}} h1{{font-size:24px;margin:8px 0 3px}} h2{{font-size:12px;letter-spacing:.08em;color:var(--accent);margin:0 0 8px}} .muted,.note{{color:var(--muted)}}
.hero{{display:flex;flex-wrap:wrap;align-items:stretch;gap:7px;margin-top:7px}} .hero>.card{{flex:1 1 520px}} .hero>div:last-child{{flex:0 1 270px}} .card,section{{background:var(--panel);border:1px solid #4d4035;border-radius:5px;padding:9px;margin-bottom:7px}} .hero-image,.locimg{{width:100%;max-height:190px;object-fit:cover;border-radius:4px;border:1px solid #8b5a32}}
.grid3,.locations{{display:flex;flex-wrap:wrap;align-items:stretch;gap:7px}} .grid3>section,.locations>.card{{flex:1 1 280px;min-width:230px}} .metric{{display:flex;justify-content:space-between;gap:12px;padding:5px 0;border-bottom:1px solid #403831}} .metric span{{color:var(--muted)}} .metric strong{{text-align:right}} .value{{font-size:19px;color:var(--good)}} .loc h3{{margin:0 0 5px;color:var(--accent);font-size:13px}} .loc p{{line-height:1.35;margin:7px 0}} .location-section{{padding:8px}} a{{color:var(--accent)}} .source{{font-size:10px;color:var(--muted)}}
@media(max-width:650px){{body{{padding:2px}} .wrap{{padding:2px}} .hero>div:last-child,.grid3>section,.locations>.card{{flex-basis:100%}}}}
</style></head><body><div class="wrap">
<div class="brand"><img src="{logo}" alt="StarSync MinerScan"></div>
<div class="hero"><div class="card"><h1>{name}</h1><div class="muted">Generated {generated} | Analysis basis {scu:.2} SCU</div>
<p>{description}</p><div class="source">UUID {uuid}<br>Providers: {providers}</div></div><div>{material_image}</div></div>
<div class="grid3">
<section><h2>RAW SALE</h2><div class="value">{raw_price}</div><div class="metric"><span>Basis value</span><strong>{raw_value}</strong></div></section>
<section><h2>REFINING</h2>{refinery_html}</section>
<section><h2>REFINED SALE</h2><div class="value">{refined_price}</div><div class="metric"><span>Gross</span><strong>{gross}</strong></div><div class="metric"><span>Estimated net</span><strong>{net}</strong></div></section>
</div>
{locations_section}
{warnings}
</div></body></html>"#,
        name = esc(&enriched.name),
        generated = esc(generated_at),
        scu = enriched.analysis_scu,
        description = esc(enriched
            .description
            .as_deref()
            .unwrap_or("No description available.")),
        uuid = esc(enriched.uuid.as_deref().unwrap_or("--")),
        providers = esc(&enriched.providers_used.join(" + ")),
    )
}

fn location_card(
    title: &str,
    detail: Option<&LocationInfo>,
    fallback_name: Option<&str>,
    fallback_system: Option<&str>,
) -> String {
    let name = detail
        .map(|d| d.name.as_str())
        .or(fallback_name)
        .unwrap_or("")
        .trim();
    if name.is_empty() || name.eq_ignore_ascii_case("unknown") || name == "--" {
        // A missing RAW quote/location is not a location card. Omitting it lets
        // the flex layout redistribute the available width to the valid cards.
        return String::new();
    }
    let system = detail
        .map(|d| d.system.as_str())
        .or(fallback_system)
        .unwrap_or("");
    let description = detail
        .and_then(|d| d.description.as_deref())
        .unwrap_or("No enriched location description available.");
    let image = detail
        .and_then(|d| d.image_bytes.as_deref())
        .map(|bytes| {
            format!(
                "<img class=\"locimg\" src=\"{}\" alt=\"{}\">",
                data_uri(bytes),
                esc(name)
            )
        })
        .unwrap_or_default();
    let link = detail
        .and_then(|d| d.page_url.as_deref().or(d.image_url.as_deref()))
        .map(|url| format!("<a href=\"{}\">Source page</a>", esc_attr(url)))
        .unwrap_or_default();
    let source = detail
        .and_then(|d| d.source.as_deref())
        .map(|value| format!("Source: {}", esc(value)))
        .unwrap_or_default();
    format!(
        "<div class=\"card loc\"><h3>{}</h3>{}<strong>{}</strong><div class=\"muted\">{}</div><p>{}</p><div class=\"source\">{} {}</div></div>",
        esc(title),
        image,
        esc(name),
        esc(system),
        esc(description),
        source,
        link
    )
}

fn media_data_uri(media: &MediaInfo) -> Option<String> {
    media.image_bytes.as_deref().map(data_uri)
}

fn data_uri(bytes: &[u8]) -> String {
    let mime = match image::guess_format(bytes).ok() {
        Some(image::ImageFormat::Png) => "image/png",
        Some(image::ImageFormat::Jpeg) => "image/jpeg",
        Some(image::ImageFormat::WebP) => "image/webp",
        Some(image::ImageFormat::Gif) => "image/gif",
        _ => "application/octet-stream",
    };
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

fn format_minutes(minutes: f64) -> String {
    let minutes = minutes.max(0.0).round() as u64;
    if minutes >= 60 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

fn sanitize_filename(value: &str) -> String {
    let s: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "Mineral".to_owned()
    } else {
        s
    }
}

fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn esc_attr(value: &str) -> String {
    esc(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_filename_is_safe() {
        assert_eq!(sanitize_filename("Bexalite / Raw"), "Bexalite___Raw");
    }
    #[test]
    fn report_contains_inline_logo() {
        let html = render_report(
            &EnrichedMineral {
                name: "Bexalite".into(),
                ..Default::default()
            },
            "now",
        );
        assert!(html.contains("data:image/"));
        assert!(html.contains(";base64,"));
        assert!(html.contains("Bexalite"));
    }

    #[test]
    #[ignore = "requires live provider network access"]
    fn live_enriched_report_contains_inline_media_and_locations() {
        let catalog = crate::data::ScUnpackedCatalog::embedded().unwrap();
        let profile = catalog.profile_for_name("Bexalite");
        let config = crate::enrichment::EnrichmentConfig {
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
            refinery_options: crate::enrichment::load_refinery_options(
                "https://api.uexcorp.uk/2.0",
                true,
                None,
            )
            .unwrap_or_default(),
            refining_methods: crate::enrichment::load_refining_methods(
                "https://api.uexcorp.uk/2.0",
                true,
            ),
        };
        let enriched = crate::enrichment::enrich_mineral("Bexalite", profile, &config).unwrap();
        let html = render_report(&enriched, "live-test");
        assert!(html.contains("data:image/"));
        assert!(html.contains("RAW SALE LOCATION"));
        assert!(html.contains("REFINING LOCATION"));
        assert!(html.contains("REFINED SALE LOCATION"));
        assert!(html.contains("Bexalite"));
        let path = write_enriched_report(&enriched).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("data:image/"));
        assert!(saved.contains("Bexalite"));
        assert!(saved.contains("LOCATIONS"));
        let _ = fs::remove_file(path);
    }
}
