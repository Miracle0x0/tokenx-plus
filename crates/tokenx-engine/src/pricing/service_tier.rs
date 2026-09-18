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
) -> Result<Cow<'a, ModelPricing>, PricingComputationError> {
    let tier = match tier {
        None | Some("default") => return Ok(Cow::Borrowed(pricing)),
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
        "priority" => ModelPricing {
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
        },
        "flex" => ModelPricing {
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
        },
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
    let long_context = prompt_tokens > 272_000
        && (pricing.input_cost_per_token_above_272k_tokens.is_some()
            || selected.input_cost_per_token_above_272k_tokens.is_some());
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
        let selected = effective_service_tier(pricing, tier, usage).unwrap();
        compute_cost(
            &selected,
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
            usage.reasoning,
        )
        .unwrap()
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
                effective_service_tier(&row, Some("priority"), &usage),
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
                effective_service_tier(&row, Some(tier), &usage),
                Err(PricingComputationError::UnsupportedServiceTier { .. })
            ));
        }
        let long = TokenBreakdown {
            input: 272_001,
            ..Default::default()
        };
        row.service_tiers
            .input_cost_per_token_above_272k_tokens_priority = None;
        assert!(effective_service_tier(&row, Some("priority"), &long).is_err());
    }

    #[test]
    fn old_pricing_cache_without_tier_schema_is_not_reused() {
        let bytes = br#"{"timestamp":1,"data":{}}"#;
        assert!(
            cache::parse_cache::<std::collections::HashMap<String, ModelPricing>>(bytes).is_err()
        );
    }
}
