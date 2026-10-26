//! Verified provider rules applied within one selected catalog row.

use std::borrow::Cow;

use super::{lookup::LookupResult, ModelPricing, PricingComputationError};
use crate::TokenBreakdown;

const PRE_56_MODELS: &[&str] = &[
    "gpt-5",
    "gpt-5-chat",
    "gpt-5-chat-latest",
    "gpt-5-codex",
    "gpt-5-mini",
    "gpt-5-nano",
    "gpt-5-pro",
    "gpt-5-search-api",
    "gpt-5.1",
    "gpt-5.1-chat-latest",
    "gpt-5.1-codex",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex-mini",
    "gpt-5.2",
    "gpt-5.2-chat-latest",
    "gpt-5.2-codex",
    "gpt-5.2-pro",
    "gpt-5.3-chat-latest",
    "gpt-5.3-codex",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.4-nano",
    "gpt-5.4-pro",
    "gpt-5.5",
    "gpt-5.5-cyber",
    "gpt-5.5-cyber-preview",
    "gpt-5.5-pro",
];

pub(super) fn normalize_openai_cache_writes(result: &mut LookupResult, provider: Option<&str>) {
    if provider != Some("openai")
        || !matches!(result.pricing_source.as_str(), "LiteLLM" | "OpenRouter")
    {
        return;
    }
    let key = result.matched_key.to_ascii_lowercase();
    let model = key.strip_prefix("openai/").unwrap_or(&key);
    let base = model
        .char_indices()
        .rev()
        .nth(10)
        .filter(|(index, separator)| {
            *separator == '-'
                && chrono::NaiveDate::parse_from_str(&model[index + 1..], "%Y-%m-%d").is_ok()
        })
        .map_or(model, |(index, _)| &model[..index]);
    if !PRE_56_MODELS.contains(&base)
        || result
            .pricing
            .litellm_provider
            .as_deref()
            .is_some_and(|provider| provider != "openai")
    {
        return;
    }
    let pricing = &mut result.pricing;
    if pricing.cache_creation_input_token_cost.is_none() {
        pricing.cache_creation_input_token_cost = pricing.input_cost_per_token;
    }
    if pricing
        .cache_creation_input_token_cost_above_272k_tokens
        .is_none()
    {
        pricing.cache_creation_input_token_cost_above_272k_tokens =
            pricing.input_cost_per_token_above_272k_tokens;
    }
    let tiers = &mut pricing.service_tiers;
    for (write, input) in [
        (
            &mut tiers.cache_creation_input_token_cost_priority,
            tiers.input_cost_per_token_priority,
        ),
        (
            &mut tiers.cache_creation_input_token_cost_above_272k_tokens_priority,
            tiers.input_cost_per_token_above_272k_tokens_priority,
        ),
        (
            &mut tiers.cache_creation_input_token_cost_flex,
            tiers.input_cost_per_token_flex,
        ),
        (
            &mut tiers.cache_creation_input_token_cost_above_272k_tokens_flex,
            tiers.input_cost_per_token_above_272k_tokens_flex,
        ),
    ] {
        if write.is_none() {
            *write = input;
        }
    }
}

pub(super) fn haiku_rates<'a>(
    pricing: &'a ModelPricing,
    model: &str,
    source: &str,
    usage: &TokenBreakdown,
    aggregate: bool,
) -> Result<Cow<'a, ModelPricing>, PricingComputationError> {
    if crate::model_aliases::canonicalize_model_id(model) != "claude-haiku-5.5" {
        return Ok(Cow::Borrowed(pricing));
    }
    let declared = [
        pricing.input_cost_per_token_above_100k_tokens,
        pricing.output_cost_per_token_above_100k_tokens,
        pricing.cache_read_input_token_cost_above_100k_tokens,
        pricing.cache_creation_input_token_cost_above_100k_tokens,
        pricing.cache_creation_input_token_cost_above_1hr_above_100k_tokens,
    ]
    .iter()
    .any(Option::is_some);
    if source == "Custom" && !declared {
        return Ok(Cow::Borrowed(pricing));
    }
    let prompt = usage
        .input
        .checked_add(usage.cache_read)
        .and_then(|n| n.checked_add(usage.cache_write))
        .ok_or(PricingComputationError::TokenCountOverflow)?;
    if prompt <= 100_000 {
        return Ok(Cow::Borrowed(pricing));
    }
    if aggregate {
        return Err(PricingComputationError::AmbiguousRequestTier { threshold: 100_000 });
    }
    let rate = |used, price: Option<f64>, component| {
        if used && !price.is_some_and(|value| value.is_finite() && value >= 0.0) {
            Err(PricingComputationError::MissingServiceTierRate {
                tier: "haiku-100k".into(),
                component,
            })
        } else {
            Ok(price)
        }
    };
    Ok(Cow::Owned(ModelPricing {
        input_cost_per_token: rate(
            usage.input > 0,
            pricing.input_cost_per_token_above_100k_tokens,
            "input",
        )?,
        output_cost_per_token: rate(
            usage.output > 0 || usage.reasoning > 0,
            pricing.output_cost_per_token_above_100k_tokens,
            "output",
        )?,
        cache_read_input_token_cost: rate(
            usage.cache_read > 0,
            pricing.cache_read_input_token_cost_above_100k_tokens,
            "cache-read",
        )?,
        cache_creation_input_token_cost: rate(
            usage.cache_write > usage.cache_write_1h,
            pricing.cache_creation_input_token_cost_above_100k_tokens,
            "cache-write",
        )?,
        cache_creation_input_token_cost_above_1hr: rate(
            usage.cache_write_1h > 0,
            pricing.cache_creation_input_token_cost_above_1hr_above_100k_tokens,
            "cache-write-1h",
        )?,
        ..Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{lookup::compute_cost, PricingService, UsagePricingContext};
    use std::collections::HashMap;

    fn haiku() -> ModelPricing {
        serde_json::from_value(serde_json::json!({
            "input_cost_per_token":1.0,"output_cost_per_token":2.0,
            "cache_read_input_token_cost":0.1,"cache_creation_input_token_cost":1.25,
            "cache_creation_input_token_cost_above_1hr":2.0,
            "input_cost_per_token_above_100k_tokens":5.0,
            "output_cost_per_token_above_100k_tokens":10.0,
            "cache_read_input_token_cost_above_100k_tokens":0.5,
            "cache_creation_input_token_cost_above_100k_tokens":6.25,
            "cache_creation_input_token_cost_above_1hr_above_100k_tokens":10.0
        }))
        .unwrap()
    }

    #[test]
    fn haiku_request_boundary_counts_all_prompt_buckets_once() {
        let pricing = haiku();
        let mut usage = TokenBreakdown {
            input: 90_000,
            cache_read: 9_000,
            cache_write: 1_000,
            cache_write_1h: 400,
            output: 20,
            reasoning: 10,
        };
        let base = haiku_rates(&pricing, "claude-haiku-5.5", "LiteLLM", &usage, false).unwrap();
        assert_eq!(compute_cost(&base, &usage).unwrap(), 92_510.0);
        usage.cache_read += 1;
        let high = haiku_rates(&pricing, "claude-haiku-5.5", "LiteLLM", &usage, false).unwrap();
        assert_eq!(compute_cost(&high, &usage).unwrap(), 462_550.5);
        assert!(matches!(
            haiku_rates(&pricing, "claude-haiku-5.5", "LiteLLM", &usage, true),
            Err(PricingComputationError::AmbiguousRequestTier { threshold: 100_000 })
        ));
        assert_eq!(
            compute_cost(
                &haiku_rates(&pricing, "claude-sonnet-5.5", "LiteLLM", &usage, false).unwrap(),
                &usage
            )
            .unwrap(),
            92_510.1
        );
    }

    #[test]
    fn haiku_missing_tier_is_visible_without_borrowing_from_another_catalog() {
        let mut incomplete = haiku();
        incomplete.cache_creation_input_token_cost_above_1hr_above_100k_tokens = None;
        let service = PricingService::new(
            HashMap::from([("claude-haiku-5-5".into(), incomplete)]),
            HashMap::from([("claude-haiku-5.5".into(), haiku())]),
        );
        let usage = TokenBreakdown {
            input: 100_000,
            cache_write: 1,
            cache_write_1h: 1,
            ..Default::default()
        };
        assert!(matches!(
            service.estimate_canonical_usage(
                "claude-haiku-5.5",
                Some("anthropic"),
                &usage,
                UsagePricingContext::default()
            ),
            Err(PricingComputationError::MissingServiceTierRate {
                component: "cache-write-1h",
                ..
            })
        ));
        let flat = ModelPricing {
            input_cost_per_token: Some(3.0),
            ..Default::default()
        };
        assert_eq!(
            haiku_rates(&flat, "claude-haiku-5.5", "Custom", &usage, false)
                .unwrap()
                .input_cost_per_token,
            Some(3.0)
        );
    }

    #[test]
    fn native_openai_cache_write_rates_do_not_escape_the_selected_route() {
        for (key, provider, source, inherited) in [
            ("gpt-5.5", Some("openai"), "LiteLLM", true),
            ("gpt-5.4-2026-03-05", Some("openai"), "LiteLLM", true),
            ("openai/gpt-5.5-pro", Some("openai"), "OpenRouter", true),
            ("gpt-5.6-sol", Some("openai"), "LiteLLM", false),
            ("azure/gpt-5.5", Some("openai"), "LiteLLM", false),
            ("gpt-5.5", Some("openrouter"), "LiteLLM", false),
            ("gpt-5.5", Some("openai"), "Custom", false),
            ("gpt-5.5-batch", Some("openai"), "LiteLLM", false),
        ] {
            let mut row = LookupResult {
                pricing: ModelPricing {
                    input_cost_per_token: Some(2.0),
                    input_cost_per_token_above_272k_tokens: Some(4.0),
                    ..Default::default()
                },
                matched_key: key.into(),
                pricing_source: source.into(),
                billing_provider: None,
            };
            normalize_openai_cache_writes(&mut row, provider);
            assert_eq!(
                row.pricing.cache_creation_input_token_cost,
                inherited.then_some(2.0),
                "{key} {provider:?}"
            );
            assert_eq!(
                row.pricing
                    .cache_creation_input_token_cost_above_272k_tokens,
                inherited.then_some(4.0)
            );
            row.pricing.cache_creation_input_token_cost = Some(0.0);
            normalize_openai_cache_writes(&mut row, provider);
            assert_eq!(row.pricing.cache_creation_input_token_cost, Some(0.0));
        }
    }
}
