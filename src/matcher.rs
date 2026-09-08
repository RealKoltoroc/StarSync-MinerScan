use crate::models::{DetectionCandidate, SignatureDefinition};

pub fn match_signature(
    observed: u32,
    definitions: &[SignatureDefinition],
) -> Vec<DetectionCandidate> {
    match_signature_filtered(observed, definitions, true)
}

pub fn resolve_signature(
    observed: u32,
    definitions: &[SignatureDefinition],
) -> Vec<DetectionCandidate> {
    match_signature_filtered(observed, definitions, false)
}

fn match_signature_filtered(
    observed: u32,
    definitions: &[SignatureDefinition],
    enabled_only: bool,
) -> Vec<DetectionCandidate> {
    let mut matches = Vec::new();

    for definition in definitions
        .iter()
        .filter(|d| (!enabled_only || d.enabled) && d.base_signature > 0)
    {
        if observed == definition.base_signature {
            matches.push(DetectionCandidate {
                target_id: definition.id.clone(),
                name: definition.canonical_name.clone(),
                category: definition.category,
                observed_signature: observed,
                base_signature: definition.base_signature,
                multiplier: 1,
            });
            continue;
        }

        if definition.multiplicative && observed % definition.base_signature == 0 {
            let multiplier = observed / definition.base_signature;
            let max_multiplier = match definition.category {
                crate::models::SignatureCategory::Resource => 12,
                _ => 64,
            };
            if multiplier > 1 && multiplier <= max_multiplier {
                matches.push(DetectionCandidate {
                    target_id: definition.id.clone(),
                    name: definition.canonical_name.clone(),
                    category: definition.category,
                    observed_signature: observed,
                    base_signature: definition.base_signature,
                    multiplier,
                });
            }
        }
    }

    matches.sort_by_key(|m| (m.multiplier, m.base_signature));
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SignatureCategory;

    fn def(name: &str, base: u32) -> SignatureDefinition {
        SignatureDefinition {
            id: name.to_lowercase(),
            canonical_name: name.to_owned(),
            category: SignatureCategory::Resource,
            base_signature: base,
            multiplicative: true,
            enabled: true,
            source: "test".into(),
            game_version: "test".into(),
        }
    }

    #[test]
    fn detects_cluster_multiplier() {
        let matches = match_signature(13_540, &[def("Riccite", 3385)]);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].multiplier, 4);
    }

    #[test]
    fn ignores_non_match() {
        assert!(match_signature(9999, &[def("Riccite", 3385)]).is_empty());
    }

    #[test]
    fn resolves_disabled_signature_without_alerting() {
        let mut bexalite = def("Bexalite", 3600);
        bexalite.enabled = false;
        assert!(match_signature(7200, &[bexalite.clone()]).is_empty());
        let resolved = resolve_signature(7200, &[bexalite]);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "Bexalite");
        assert_eq!(resolved[0].multiplier, 2);
    }
}
