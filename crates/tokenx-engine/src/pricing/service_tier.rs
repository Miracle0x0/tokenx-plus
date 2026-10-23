//! Service-tier rates use catalog prices, never an inferred multiplier.

use super::{ModelPricing, PricingComputationError};
use crate::TokenBreakdown;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceTierPricing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token_above_272k_tokens_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token_above_272k_tokens_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost_above_272k_tokens_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_above_272k_tokens_priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token_flex: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token_above_272k_tokens_flex: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token_flex: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token_above_272k_tokens_flex: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost_flex: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost_above_272k_tokens_flex: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_flex: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_above_272k_tokens_flex: Option<f64>,
}

pub(super) fn effective_service_tier<'a>(
    pricing: &'a ModelPricing,
    tier: Option<&str>,
    usage: &TokenBreakdown,
    is_aggregate: bool,
) -> Result<Cow<'a, ModelPricing>, PricingComputationError> {
    let tier = match tier {
        None | Some("default") => "default",
        Some("fast" | "priority") => "priority",
        Some("flex") => "flex",
        Some(tier) => {
            return Err(PricingComputationError::UnsupportedServiceTier {
                tier: tier.to_string(),
            })
        }
    };
    let rates = &pricing.service_tiers;
    let selected = match tier {
        "default" => Cow::Borrowed(pricing),
        "priority" => Cow::Owned(ModelPricing {
            input_cost_per_token: rates.input_cost_per_token_priority,
            input_cost_per_token_above_272k_tokens: rates
                .input_cost_per_token_above_272k_tokens_priority,
            output_cost_per_token: rates.output_cost_per_token_priority,
            output_cost_per_token_above_272k_tokens: rates
                .output_cost_per_token_above_272k_tokens_priority,
            cache_read_input_token_cost: rates.cache_read_input_token_cost_priority,
            cache_read_input_token_cost_above_272k_tokens: rates
                .cache_read_input_token_cost_above_272k_tokens_priority,
            cache_creation_input_token_cost: rates.cache_creation_input_token_cost_priority,
            cache_creation_input_token_cost_above_272k_tokens: rates
                .cache_creation_input_token_cost_above_272k_tokens_priority,
            ..Default::default()
        }),
        "flex" => Cow::Owned(ModelPricing {
            input_cost_per_token: rates.input_cost_per_token_flex,
            input_cost_per_token_above_272k_tokens: rates
                .input_cost_per_token_above_272k_tokens_flex,
            output_cost_per_token: rates.output_cost_per_token_flex,
            output_cost_per_token_above_272k_tokens: rates
                .output_cost_per_token_above_272k_tokens_flex,
            cache_read_input_token_cost: rates.cache_read_input_token_cost_flex,
            cache_read_input_token_cost_above_272k_tokens: rates
                .cache_read_input_token_cost_above_272k_tokens_flex,
            cache_creation_input_token_cost: rates.cache_creation_input_token_cost_flex,
            cache_creation_input_token_cost_above_272k_tokens: rates
                .cache_creation_input_token_cost_above_272k_tokens_flex,
            ..Default::default()
        }),
        _ => unreachable!("tier was normalized above"),
    };
    // OpenAI long-context rates apply to the whole request, using inclusive
    // prompt size (ordinary input plus both cache buckets), not each bucket.
    let prompt_tokens = usage
        .input
        .checked_add(usage.cache_read)
        .and_then(|value| value.checked_add(usage.cache_write))
        .ok_or(PricingComputationError::TokenCountOverflow)?;
    let output_tokens = usage
        .output
        .checked_add(usage.reasoning)
        .ok_or(PricingComputationError::OutputReasoningTokenOverflow)?;
    let has_long_context = pricing.input_cost_per_token_above_272k_tokens.is_some()
        || pricing.output_cost_per_token_above_272k_tokens.is_some()
        || pricing
            .cache_read_input_token_cost_above_272k_tokens
            .is_some()
        || pricing
            .cache_creation_input_token_cost_above_272k_tokens
            .is_some()
        || selected.input_cost_per_token_above_272k_tokens.is_some()
        || selected.output_cost_per_token_above_272k_tokens.is_some()
        || selected
            .cache_read_input_token_cost_above_272k_tokens
            .is_some()
        || selected
            .cache_creation_input_token_cost_above_272k_tokens
            .is_some();
    if tier == "default" && !has_long_context {
        return Ok(Cow::Borrowed(pricing));
    }
    let long_context = prompt_tokens > 272_000 && has_long_context;
    if is_aggregate && long_context {
        return Err(PricingComputationError::AmbiguousRequestTier { threshold: 272_000 });
    }
    if tier == "default" && !long_context {
        let mut ordinary = pricing.clone();
        ordinary.input_cost_per_token_above_272k_tokens = None;
        ordinary.output_cost_per_token_above_272k_tokens = None;
        ordinary.cache_read_input_token_cost_above_272k_tokens = None;
        ordinary.cache_creation_input_token_cost_above_272k_tokens = None;
        return Ok(Cow::Owned(ordinary));
    }
    let rate = |component, tokens, base: Option<f64>, long: Option<f64>| {
        let price = if long_context { long } else { base };
        if tokens > 0 && !price.is_some_and(valid_price) {
            Err(PricingComputationError::MissingServiceTierRate {
                tier: tier.to_string(),
                component,
            })
        } else {
            Ok(price)
        }
    };
    let selected = ModelPricing {
        input_cost_per_token: rate(
            "input",
            usage.input,
            selected.input_cost_per_token,
            selected.input_cost_per_token_above_272k_tokens,
        )?,
        output_cost_per_token: rate(
            "output",
            output_tokens,
            selected.output_cost_per_token,
            selected.output_cost_per_token_above_272k_tokens,
        )?,
        cache_read_input_token_cost: rate(
            "cache-read",
            usage.cache_read,
            selected.cache_read_input_token_cost,
            selected.cache_read_input_token_cost_above_272k_tokens,
        )?,
        cache_creation_input_token_cost: rate(
            "cache-write",
            usage.cache_write,
            selected.cache_creation_input_token_cost,
            selected.cache_creation_input_token_cost_above_272k_tokens,
        )?,
        ..Default::default()
    };
    Ok(Cow::Owned(selected))
}

pub(super) fn effective_xai_request_rates<'a>(
    pricing: &'a ModelPricing,
    provider: Option<&str>,
    usage: &TokenBreakdown,
    is_aggregate: bool,
) -> Result<Cow<'a, ModelPricing>, PricingComputationError> {
    let has_tier = pricing.input_cost_per_token_above_200k_tokens.is_some()
        || pricing.output_cost_per_token_above_200k_tokens.is_some()
        || pricing
            .cache_read_input_token_cost_above_200k_tokens
            .is_some();
    if provider != Some("xai") || !has_tier {
        return Ok(Cow::Borrowed(pricing));
    }
    let prompt = usage
        .input
        .checked_add(usage.cache_read)
        .ok_or(PricingComputationError::TokenCountOverflow)?;
    let high = prompt >= 200_000;
    if high && is_aggregate {
        return Err(PricingComputationError::AmbiguousRequestTier { threshold: 200_000 });
    }
    let mut effective = pricing.clone();
    if high {
        for (used, base, long, component) in [
            (
                usage.input > 0,
                &mut effective.input_cost_per_token,
                pricing.input_cost_per_token_above_200k_tokens,
                "input",
            ),
            (
                usage.output > 0 || usage.reasoning > 0,
                &mut effective.output_cost_per_token,
                pricing.output_cost_per_token_above_200k_tokens,
                "output",
            ),
            (
                usage.cache_read > 0,
                &mut effective.cache_read_input_token_cost,
                pricing.cache_read_input_token_cost_above_200k_tokens,
                "cache-read",
            ),
        ] {
            if used && !long.is_some_and(valid_price) {
                return Err(PricingComputationError::MissingServiceTierRate {
                    tier: "default".into(),
                    component,
                });
            }
            *base = long;
        }
    }
    effective.input_cost_per_token_above_200k_tokens = None;
    effective.output_cost_per_token_above_200k_tokens = None;
    effective.cache_read_input_token_cost_above_200k_tokens = None;
    Ok(Cow::Owned(effective))
}

fn valid_price(price: f64) -> bool {
    price.is_finite() && price >= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{cache, lookup::compute_cost};

    fn catalog_row() -> ModelPricing {
        serde_json::from_value(serde_json::json!({
            "input_cost_per_token": 1.0, "output_cost_per_token": 2.0,
            "input_cost_per_token_above_272k_tokens": 3.0,
            "input_cost_per_token_priority": 4.0,
            "output_cost_per_token_priority": 7.0,
            "cache_read_input_token_cost_priority": 0.5,
            "cache_creation_input_token_cost_priority": 5.0,
            "input_cost_per_token_above_272k_tokens_priority": 8.0,
            "output_cost_per_token_above_272k_tokens_priority": 9.0,
            "cache_read_input_token_cost_above_272k_tokens_priority": 1.0,
            "cache_creation_input_token_cost_above_272k_tokens_priority": 10.0,
            "input_cost_per_token_flex": 0.5,
            "output_cost_per_token_flex": 1.0,
            "cache_read_input_token_cost_flex": 0.1,
            "cache_creation_input_token_cost_flex": 0.6
        }))
        .unwrap()
    }

    fn cost(pricing: &ModelPricing, tier: Option<&str>, usage: &TokenBreakdown) -> f64 {
        let selected = effective_service_tier(pricing, tier, usage, false).unwrap();
        compute_cost(&selected, usage).unwrap()
    }

    #[test]
    fn ordinary_and_priority_share_request_threshold_semantics() {
        let row: ModelPricing = serde_json::from_value(serde_json::json!({
            "input_cost_per_token":1.0,"output_cost_per_token":2.0,
            "input_cost_per_token_above_272k_tokens":3.0,"output_cost_per_token_above_272k_tokens":4.0,
            "cache_read_input_token_cost":0.1,"cache_read_input_token_cost_above_272k_tokens":0.3,
            "input_cost_per_token_priority":1.0,"output_cost_per_token_priority":2.0,
            "input_cost_per_token_above_272k_tokens_priority":3.0,"output_cost_per_token_above_272k_tokens_priority":4.0,
            "cache_read_input_token_cost_priority":0.1,"cache_read_input_token_cost_above_272k_tokens_priority":0.3
        })).unwrap();
        for (input, cached, output, expected) in [
            (300000, 0, 10000, 940000.0),
            (2000, 270000, 10, 29020.0),
            (2001, 270000, 10, 87043.0),
            (1, 0, 300000, 600001.0),
        ] {
            let usage = TokenBreakdown {
                input,
                cache_read: cached,
                output,
                ..Default::default()
            };
            for tier in [None, Some("default"), Some("priority")] {
                assert!((cost(&row, tier, &usage) - expected).abs() < 1e-8);
            }
        }
        let usage = TokenBreakdown {
            input: 300000,
            ..Default::default()
        };
        assert!(matches!(
            effective_service_tier(&row, None, &usage, true),
            Err(PricingComputationError::AmbiguousRequestTier { threshold: 272000 })
        ));
    }

    #[test]
    fn xai_threshold_uses_inclusive_prompt_and_requires_request_evidence() {
        let row = ModelPricing {
            input_cost_per_token: Some(1.0),
            output_cost_per_token: Some(2.0),
            cache_read_input_token_cost: Some(0.1),
            input_cost_per_token_above_200k_tokens: Some(3.0),
            output_cost_per_token_above_200k_tokens: Some(4.0),
            cache_read_input_token_cost_above_200k_tokens: Some(0.3),
            ..Default::default()
        };
        let usage = TokenBreakdown {
            input: 1000,
            cache_read: 199000,
            output: 10,
            ..Default::default()
        };
        let effective = effective_xai_request_rates(&row, Some("xai"), &usage, false).unwrap();
        assert_eq!(compute_cost(&effective, &usage).unwrap(), 62740.0);
        assert!(matches!(
            effective_xai_request_rates(&row, Some("xai"), &usage, true),
            Err(PricingComputationError::AmbiguousRequestTier { threshold: 200000 })
        ));
    }

    #[test]
    fn service_tier_catalog_rates_survive_disk_cache_and_price_every_bucket() {
        let dir = tempfile::tempdir().unwrap();
        cache::save_cache(dir.path(), "rates.json", &catalog_row()).unwrap();
        let row: ModelPricing = cache::load_cache(dir.path(), "rates.json").unwrap();
        let usage = TokenBreakdown {
            input: 10,
            output: 2,
            cache_read: 4,
            cache_write: 3,
            cache_write_1h: 0,
            reasoning: 1,
        };
        assert_eq!(cost(&row, Some("priority"), &usage), 78.0);
        assert_eq!(cost(&row, Some("fast"), &usage), 78.0);
        assert!((cost(&row, Some("flex"), &usage) - 10.2).abs() < 1e-9);
        assert_eq!(
            cost(&row, None, &usage),
            cost(&row, Some("default"), &usage)
        );
    }

    #[test]
    fn service_tier_long_context_uses_inclusive_prompt_and_prices_the_whole_request() {
        let row = catalog_row();
        let mut usage = TokenBreakdown {
            input: 1,
            output: 2,
            cache_read: 271_998,
            cache_write: 1,
            cache_write_1h: 0,
            reasoning: 1,
        };
        assert_eq!(
            cost(&row, Some("priority"), &usage),
            4.0 + 21.0 + 135_999.0 + 5.0
        );
        usage.cache_read += 1;
        assert_eq!(
            cost(&row, Some("priority"), &usage),
            8.0 + 27.0 + 271_999.0 + 10.0
        );
    }

    #[test]
    fn service_tier_never_substitutes_standard_rates_for_missing_or_invalid_rates() {
        let usage = TokenBreakdown {
            input: 1,
            ..Default::default()
        };
        let mut row = catalog_row();
        for price in [None, Some(-1.0), Some(f64::NAN)] {
            row.service_tiers.input_cost_per_token_priority = price;
            assert!(matches!(
                effective_service_tier(&row, Some("priority"), &usage, false),
                Err(PricingComputationError::MissingServiceTierRate {
                    component: "input",
                    ..
                })
            ));
        }
        row.service_tiers.input_cost_per_token_priority = Some(0.0);
        assert_eq!(cost(&row, Some("priority"), &usage), 0.0);
        for tier in ["auto", "unknown", ""] {
            assert!(matches!(
                effective_service_tier(&row, Some(tier), &usage, false),
                Err(PricingComputationError::UnsupportedServiceTier { .. })
            ));
        }
        let long = TokenBreakdown {
            input: 272_001,
            ..Default::default()
        };
        row.service_tiers
            .input_cost_per_token_above_272k_tokens_priority = None;
        assert!(effective_service_tier(&row, Some("priority"), &long, false).is_err());
    }

    #[test]
    fn old_pricing_cache_without_tier_schema_is_not_reused() {
        let bytes = br#"{"timestamp":1,"data":{}}"#;
        assert!(
            cache::parse_cache::<std::collections::HashMap<String, ModelPricing>>(
                bytes,
                std::time::SystemTime::now()
            )
            .is_err()
        );
    }
}
