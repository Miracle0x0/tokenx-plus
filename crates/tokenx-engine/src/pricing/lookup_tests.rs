use super::*;
use chrono::{TimeZone, Utc};

fn pricing(input: f64, output: f64) -> ModelPricing {
    ModelPricing {
        input_cost_per_token: Some(input),
        output_cost_per_token: Some(output),
        ..Default::default()
    }
}

fn lookup_with_all_catalogs(
    litellm: HashMap<String, ModelPricing>,
    openrouter: HashMap<String, ModelPricing>,
    models_dev: HashMap<String, ModelPricing>,
) -> PricingLookup {
    PricingLookup::new_with_models_dev(litellm, openrouter, models_dev)
}

fn timestamp_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
        .single()
        .unwrap()
        .timestamp_millis()
}

#[test]
fn exact_lookup_is_case_insensitive() {
    let lookup = PricingLookup::new(
        HashMap::from([("GPT-5.3-Codex".into(), pricing(1.0, 2.0))]),
        HashMap::new(),
    );

    let result = lookup.lookup("gpt-5.3-codex").unwrap();

    assert_eq!(result.pricing_source, "LiteLLM");
    assert_eq!(result.matched_key, "GPT-5.3-Codex");
}

#[test]
fn inferred_provider_enables_provider_scoped_exact_lookup() {
    let lookup = PricingLookup::new(
        HashMap::new(),
        HashMap::from([("openai/gpt-5.3-codex".into(), pricing(1.0, 2.0))]),
    );

    let result = lookup.lookup("gpt-5.3-codex").unwrap();

    assert_eq!(result.pricing_source, "OpenRouter");
    assert_eq!(result.matched_key, "openai/gpt-5.3-codex");
}

#[test]
fn opus_5_5_spellings_use_their_own_price() {
    let lookup = PricingLookup::new(
        HashMap::from([
            ("claude-opus-5".into(), pricing(1.0, 2.0)),
            ("claude-opus-5.5".into(), pricing(3.0, 4.0)),
        ]),
        HashMap::new(),
    );
    let without_minor = PricingLookup::new(
        HashMap::from([("claude-opus-5".into(), pricing(1.0, 2.0))]),
        HashMap::new(),
    );
    for observed in ["claude-opus-5-5", "claude-opus-5.5", "opus-5-5", "opus-5.5"] {
        let result = lookup.lookup(observed).unwrap();
        assert_eq!(result.matched_key, "claude-opus-5.5", "{observed}");
        assert_eq!(result.pricing.input_cost_per_token, Some(3.0));
        assert!(without_minor.lookup(observed).is_none(), "{observed}");
    }
}

#[test]
fn observed_provider_takes_precedence_over_family_inference() {
    let lookup = PricingLookup::new(
        HashMap::from([
            ("openai/gpt-5.3-codex".into(), pricing(1.0, 2.0)),
            ("gpt-5.3-codex".into(), pricing(3.0, 4.0)),
        ]),
        HashMap::new(),
    );

    let result = lookup
        .lookup_with_provider("gpt-5.3-codex", Some("owl"))
        .unwrap();

    assert_eq!(result.matched_key, "gpt-5.3-codex");
    assert_eq!(result.pricing.input_cost_per_token, Some(3.0));
}

#[test]
fn provider_scoped_rows_across_all_catalogs_beat_unscoped_rows() {
    let lookup = lookup_with_all_catalogs(
        HashMap::from([("gpt-5.3-codex".into(), pricing(1.0, 1.0))]),
        HashMap::new(),
        HashMap::from([("openai/gpt-5.3-codex".into(), pricing(3.0, 3.0))]),
    );

    let result = lookup
        .lookup_with_provider("gpt-5.3-codex", Some("openai"))
        .unwrap();

    assert_eq!(result.pricing_source, "Models.dev");
    assert_eq!(result.matched_key, "openai/gpt-5.3-codex");
}

#[test]
fn catalog_order_breaks_ties_inside_provider_scoped_class() {
    let lookup = lookup_with_all_catalogs(
        HashMap::from([("openai/gpt-5.3-codex".into(), pricing(1.0, 1.0))]),
        HashMap::from([("openai/gpt-5.3-codex".into(), pricing(2.0, 2.0))]),
        HashMap::from([("openai/gpt-5.3-codex".into(), pricing(3.0, 3.0))]),
    );

    let result = lookup
        .lookup_with_provider("gpt-5.3-codex", Some("openai"))
        .unwrap();

    assert_eq!(result.pricing_source, "LiteLLM");
    assert_eq!(result.pricing.input_cost_per_token, Some(1.0));
}

#[test]
fn catalog_order_breaks_ties_inside_unscoped_class() {
    let lookup = lookup_with_all_catalogs(
        HashMap::from([("mystery-model".into(), pricing(1.0, 1.0))]),
        HashMap::from([("mystery-model".into(), pricing(2.0, 2.0))]),
        HashMap::from([("mystery-model".into(), pricing(3.0, 3.0))]),
    );

    let result = lookup.lookup("mystery-model").unwrap();

    assert_eq!(result.pricing_source, "LiteLLM");
    assert_eq!(result.pricing.input_cost_per_token, Some(1.0));
}

#[test]
fn deepseek_v4_time_pricing_precedes_other_public_catalogs() {
    let lookup = lookup_with_all_catalogs(
        HashMap::from([("deepseek/deepseek-v4-flash".into(), pricing(1.0, 1.0))]),
        HashMap::from([(
            "deepseek/deepseek-v4-flash".into(),
            ModelPricing {
                input_cost_per_token: Some(2.0),
                output_cost_per_token: Some(2.0),
                time_period_prices: Some(vec![TimePeriodPrice {
                    utc_days: Some(vec!["monday".into()]),
                    input_cost_per_token: Some(0.5),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        )]),
        HashMap::new(),
    );

    let result = lookup.lookup("deepseek-v4-flash").unwrap();

    assert_eq!(result.pricing_source, "OpenRouter");
    assert_eq!(result.pricing.input_cost_per_token, Some(2.0));
}

#[test]
fn non_deepseek_models_keep_existing_catalog_priority() {
    let lookup = lookup_with_all_catalogs(
        HashMap::from([("openai/gpt-5".into(), pricing(1.0, 1.0))]),
        HashMap::from([(
            "openai/gpt-5".into(),
            ModelPricing {
                input_cost_per_token: Some(2.0),
                output_cost_per_token: Some(2.0),
                time_period_prices: Some(vec![TimePeriodPrice {
                    utc_days: Some(vec!["monday".into()]),
                    input_cost_per_token: Some(0.5),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        )]),
        HashMap::new(),
    );

    let result = lookup.lookup("gpt-5").unwrap();

    assert_eq!(result.pricing_source, "LiteLLM");
    assert_eq!(result.pricing.input_cost_per_token, Some(1.0));
}

#[test]
fn unknown_observation_uses_model_family_inference() {
    let lookup = lookup_with_all_catalogs(
        HashMap::from([("openai/gpt-5.3-codex".into(), pricing(1.0, 1.0))]),
        HashMap::from([("gpt-5.3-codex".into(), pricing(2.0, 2.0))]),
        HashMap::new(),
    );

    let result = lookup
        .lookup_with_provider("gpt-5.3-codex", Some("unknown"))
        .unwrap();

    assert_eq!(result.matched_key, "openai/gpt-5.3-codex");
    assert_eq!(result.pricing.input_cost_per_token, Some(1.0));
}

#[test]
fn unknown_scope_after_inference_uses_unscoped_exact_rows_only() {
    let lookup = lookup_with_all_catalogs(
        HashMap::from([("private-route/mystery-model".into(), pricing(1.0, 1.0))]),
        HashMap::from([("mystery-model".into(), pricing(2.0, 2.0))]),
        HashMap::new(),
    );

    let result = lookup
        .lookup_with_provider("mystery-model", Some("unknown"))
        .unwrap();

    assert_eq!(result.matched_key, "mystery-model");
    assert_eq!(result.pricing.input_cost_per_token, Some(2.0));
}

#[test]
fn forced_source_is_an_exact_catalog_boundary() {
    let lookup = lookup_with_all_catalogs(
        HashMap::from([("openai/gpt-5.3-codex".into(), pricing(1.0, 1.0))]),
        HashMap::from([("openai/gpt-5.3-codex".into(), pricing(2.0, 2.0))]),
        HashMap::from([("openai/gpt-5.3-codex".into(), pricing(3.0, 3.0))]),
    );

    let openrouter = lookup
        .lookup_with_pricing_source_and_provider(
            "gpt-5.3-codex",
            Some("OpenRouter"),
            Some("openai"),
        )
        .unwrap();
    let models_dev = lookup
        .lookup_with_pricing_source_and_provider(
            "gpt-5.3-codex",
            Some("models.dev"),
            Some("openai"),
        )
        .unwrap();

    assert_eq!(openrouter.pricing_source, "OpenRouter");
    assert_eq!(openrouter.pricing.input_cost_per_token, Some(2.0));
    assert_eq!(models_dev.pricing_source, "Models.dev");
    assert_eq!(models_dev.pricing.input_cost_per_token, Some(3.0));
    assert!(lookup
        .lookup_with_pricing_source("gpt-5.3-codex", Some("modelsdev"))
        .is_none());
}

#[test]
fn non_exact_model_ids_are_ordinary_misses() {
    let lookup = PricingLookup::new(
        HashMap::from([
            ("openai/gpt-5-preview".into(), pricing(1.0, 1.0)),
            ("some-special-model".into(), pricing(1.0, 1.0)),
            ("foo-1.2".into(), pricing(1.0, 1.0)),
            ("azure/route-model".into(), pricing(1.0, 1.0)),
        ]),
        HashMap::new(),
    );

    assert!(lookup.lookup("gpt-5").is_none());
    assert!(lookup.lookup("special-model").is_none());
    assert!(lookup.lookup("foo-1-2").is_none());
    assert!(lookup
        .lookup_with_provider("route-model", Some("openai"))
        .is_none());
}

#[test]
fn catalog_model_component_must_be_the_exact_terminal_segment() {
    let lookup = PricingLookup::new(
        HashMap::from([(
            "fireworks/accounts/openai/models/gpt-5.3-codex".into(),
            pricing(1.0, 1.0),
        )]),
        HashMap::new(),
    );

    let result = lookup
        .lookup_with_provider("gpt-5.3-codex", Some("fireworks"))
        .unwrap();
    assert_eq!(
        result.matched_key,
        "fireworks/accounts/openai/models/gpt-5.3-codex"
    );

    assert!(lookup
        .lookup_with_provider("models/gpt-5.3-codex", Some("fireworks"))
        .is_some());
}

#[test]
fn unusable_rows_are_skipped_and_explicit_zero_is_valid() {
    let lookup = PricingLookup::new(
        HashMap::from([("openai/gpt-5.3-codex".into(), ModelPricing::default())]),
        HashMap::from([(
            "openai/gpt-5.3-codex".into(),
            ModelPricing {
                input_cost_per_token: Some(0.0),
                ..Default::default()
            },
        )]),
    );

    let result = lookup.lookup("gpt-5.3-codex").unwrap();

    assert_eq!(result.pricing_source, "OpenRouter");
    assert_eq!(result.pricing.input_cost_per_token, Some(0.0));
}

#[test]
fn standalone_lookup_uses_shared_model_canonicalizer() {
    let lookup = PricingLookup::new(
        HashMap::from([("gpt-5.5".into(), pricing(1.0, 2.0))]),
        HashMap::new(),
    );

    let result = lookup.lookup("openai/GPT-5.5 (high)").unwrap();

    assert_eq!(result.matched_key, "gpt-5.5");
}

#[test]
fn lookup_cache_keeps_provider_scopes_separate() {
    let lookup = PricingLookup::new(
        HashMap::from([
            ("openai/shared-model".into(), pricing(1.0, 1.0)),
            ("anthropic/shared-model".into(), pricing(2.0, 2.0)),
        ]),
        HashMap::new(),
    );

    let openai = lookup
        .lookup_with_provider("shared-model", Some("openai"))
        .unwrap();
    let anthropic = lookup
        .lookup_with_provider("shared-model", Some("anthropic"))
        .unwrap();

    assert_eq!(openai.pricing.input_cost_per_token, Some(1.0));
    assert_eq!(anthropic.pricing.input_cost_per_token, Some(2.0));
}

#[test]
fn calculate_cost_combines_reasoning_with_output_and_clamps_negative_tokens() {
    let lookup = PricingLookup::new(
        HashMap::from([("mystery-model".into(), pricing(0.5, 2.0))]),
        HashMap::new(),
    );

    let cost = lookup
        .calculate_cost("mystery-model", -10, 3, -20, -30, 2)
        .unwrap();

    assert_eq!(cost, 10.0);
}

#[test]
fn deepseek_v4_cost_selects_utc_hhmm_periods_at_boundaries() {
    let lookup = PricingLookup::new(
        HashMap::new(),
        HashMap::from([(
            "deepseek/deepseek-v4-flash".into(),
            ModelPricing {
                input_cost_per_token: Some(9.0),
                time_period_prices: Some(vec![
                    TimePeriodPrice {
                        utc_days: Some(vec!["monday".into()]),
                        utc_start: Some(0),
                        utc_end: Some(100),
                        input_cost_per_token: Some(1.0),
                        ..Default::default()
                    },
                    TimePeriodPrice {
                        utc_days: Some(vec!["monday".into()]),
                        utc_start: Some(100),
                        utc_end: Some(400),
                        input_cost_per_token: Some(2.0),
                        ..Default::default()
                    },
                    TimePeriodPrice {
                        utc_days: Some(vec!["monday".into()]),
                        utc_start: Some(1000),
                        utc_end: Some(0),
                        input_cost_per_token: Some(3.0),
                        ..Default::default()
                    },
                    TimePeriodPrice {
                        utc_days: Some(
                            vec!["saturday", "sunday"]
                                .into_iter()
                                .map(str::to_string)
                                .collect(),
                        ),
                        input_cost_per_token: Some(4.0),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
        )]),
    );
    let usage = TokenBreakdown {
        input: 1,
        ..Default::default()
    };
    let cost_at = |timestamp_ms| {
        lookup
            .calculate_cost_with_provider_and_time(
                "deepseek-v4-flash",
                Some("deepseek"),
                &usage,
                timestamp_ms,
            )
            .unwrap()
    };

    assert_eq!(cost_at(Some(timestamp_ms(2026, 8, 31, 0, 59))), 1.0);
    assert_eq!(cost_at(Some(timestamp_ms(2026, 8, 31, 1, 0))), 2.0);
    assert_eq!(cost_at(Some(timestamp_ms(2026, 8, 31, 4, 0))), 9.0);
    assert_eq!(cost_at(Some(timestamp_ms(2026, 8, 31, 10, 0))), 3.0);
    assert_eq!(cost_at(Some(timestamp_ms(2026, 8, 30, 12, 0))), 4.0);
    assert_eq!(cost_at(None), 9.0);
}

#[test]
fn matching_time_periods_merge_in_source_order() {
    let lookup = PricingLookup::new(
        HashMap::new(),
        HashMap::from([(
            "deepseek/deepseek-v4-pro".into(),
            ModelPricing {
                input_cost_per_token: Some(10.0),
                output_cost_per_token: Some(20.0),
                time_period_prices: Some(vec![
                    TimePeriodPrice {
                        utc_days: Some(vec!["monday".into()]),
                        input_cost_per_token: Some(1.0),
                        output_cost_per_token: Some(2.0),
                        ..Default::default()
                    },
                    TimePeriodPrice {
                        utc_days: Some(vec!["monday".into()]),
                        input_cost_per_token: Some(3.0),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
        )]),
    );
    let usage = TokenBreakdown {
        input: 1,
        output: 1,
        ..Default::default()
    };

    let cost = lookup
        .calculate_cost_with_provider_and_time(
            "deepseek-v4-pro",
            Some("deepseek"),
            &usage,
            Some(timestamp_ms(2026, 8, 31, 12, 0)),
        )
        .unwrap();

    assert_eq!(cost, 5.0);
}

#[test]
fn invalid_explicit_pricing_timestamp_is_an_error() {
    let lookup = PricingLookup::new(
        HashMap::new(),
        HashMap::from([(
            "deepseek/deepseek-v4-pro".into(),
            ModelPricing {
                input_cost_per_token: Some(1.0),
                time_period_prices: Some(vec![TimePeriodPrice {
                    utc_days: Some(vec!["monday".into()]),
                    input_cost_per_token: Some(0.5),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        )]),
    );

    let error = lookup
        .calculate_cost_with_provider_and_time(
            "deepseek-v4-pro",
            Some("deepseek"),
            &TokenBreakdown {
                input: 1,
                ..Default::default()
            },
            Some(i64::MAX),
        )
        .unwrap_err();

    assert_eq!(
        error,
        PricingComputationError::InvalidTimestamp {
            timestamp_ms: i64::MAX
        }
    );
}

#[test]
fn compute_cost_applies_multiple_tiers_in_order() {
    let model_pricing = ModelPricing {
        input_cost_per_token: Some(1.0),
        input_cost_per_token_above_128k_tokens: Some(2.0),
        input_cost_per_token_above_200k_tokens: Some(3.0),
        input_cost_per_token_above_256k_tokens: Some(4.0),
        input_cost_per_token_above_272k_tokens: Some(5.0),
        ..Default::default()
    };

    let cost = compute_cost(
        &model_pricing,
        &TokenBreakdown {
            input: 300_000,
            ..Default::default()
        },
    )
    .unwrap();
    let expected = 128_000.0 + 72_000.0 * 2.0 + 56_000.0 * 3.0 + 16_000.0 * 4.0 + 28_000.0 * 5.0;

    assert_eq!(cost, expected);
}

#[test]
fn compute_cost_applies_cache_tiers_per_bucket() {
    let model_pricing = ModelPricing {
        cache_read_input_token_cost: Some(1.0),
        cache_read_input_token_cost_above_200k_tokens: Some(2.0),
        cache_read_input_token_cost_above_272k_tokens: Some(3.0),
        cache_creation_input_token_cost: Some(4.0),
        cache_creation_input_token_cost_above_200k_tokens: Some(5.0),
        ..Default::default()
    };

    let cost = compute_cost(
        &model_pricing,
        &TokenBreakdown {
            cache_read: 300_000,
            cache_write: 300_000,
            ..Default::default()
        },
    )
    .unwrap();
    let expected_cache_read = 200_000.0 + 72_000.0 * 2.0 + 28_000.0 * 3.0;
    let expected_cache_write = 200_000.0 * 4.0 + 100_000.0 * 5.0;

    assert_eq!(cost, expected_cache_read + expected_cache_write);
}

#[test]
fn compute_cost_ignores_invalid_prices() {
    let model_pricing = ModelPricing {
        input_cost_per_token: Some(f64::NAN),
        output_cost_per_token: Some(-1.0),
        cache_read_input_token_cost: Some(f64::INFINITY),
        ..Default::default()
    };

    assert_eq!(
        compute_cost(
            &model_pricing,
            &TokenBreakdown {
                input: 10,
                output: 10,
                cache_read: 10,
                cache_write: 10,
                reasoning: 10,
                ..Default::default()
            }
        )
        .unwrap(),
        0.0
    );
}

#[test]
fn compute_cost_rejects_output_reasoning_token_overflow() {
    let model_pricing = pricing(0.0, 1.0);
    let error = compute_cost(
        &model_pricing,
        &TokenBreakdown {
            output: i64::MAX,
            reasoning: 1,
            ..Default::default()
        },
    )
    .unwrap_err();

    assert_eq!(error, PricingComputationError::OutputReasoningTokenOverflow);
}

#[test]
fn compute_cost_rejects_non_finite_component_cost() {
    let model_pricing = pricing(f64::MAX, 0.0);
    let error = compute_cost(
        &model_pricing,
        &TokenBreakdown {
            input: i64::MAX,
            ..Default::default()
        },
    )
    .unwrap_err();

    assert_eq!(
        error,
        PricingComputationError::NonFiniteCost { component: "input" }
    );
}

#[test]
fn compute_cost_prices_mixed_cache_durations_at_their_published_rates() {
    let row = ModelPricing {
        cache_creation_input_token_cost: Some(1.25e-5),
        cache_creation_input_token_cost_above_1hr: Some(2e-5),
        ..Default::default()
    };
    let usage = TokenBreakdown {
        cache_write: 200_000,
        cache_write_1h: 100_000,
        ..Default::default()
    };
    assert!((compute_cost(&row, &usage).unwrap() - 3.25).abs() < 1e-12);
    assert_eq!(usage.total(), 200_000);
}

#[test]
fn compute_cost_prices_one_hour_cache_writes_across_the_200k_tier() {
    let row = ModelPricing {
        cache_creation_input_token_cost: Some(1.0),
        cache_creation_input_token_cost_above_200k_tokens: Some(2.0),
        cache_creation_input_token_cost_above_1hr: Some(3.0),
        cache_creation_input_token_cost_above_1hr_above_200k_tokens: Some(4.0),
        ..Default::default()
    };
    for hourly in [200_000, 200_001] {
        let usage = TokenBreakdown {
            cache_write: 200_001 + hourly,
            cache_write_1h: hourly,
            ..Default::default()
        };
        assert_eq!(
            compute_cost(&row, &usage).unwrap(),
            200_000.0 + 2.0 + 200_000.0 * 3.0 + (hourly - 200_000) as f64 * 4.0
        );
    }
}

#[test]
fn one_hour_only_pricing_rows_are_usable_and_accept_explicit_zero_rates() {
    for rate in [0.0, 2e-5] {
        let lookup = PricingLookup::new(
            HashMap::from([(
                "claude-sonnet-4.6".into(),
                ModelPricing {
                    cache_creation_input_token_cost_above_1hr: Some(rate),
                    ..Default::default()
                },
            )]),
            HashMap::new(),
        );
        assert!(lookup.lookup("claude-sonnet-4.6").is_some());
        let usage = TokenBreakdown {
            cache_write: 100_000,
            cache_write_1h: 100_000,
            ..Default::default()
        };
        assert_eq!(
            lookup
                .calculate_cost_with_provider("claude-sonnet-4.6", Some("anthropic"), &usage)
                .unwrap(),
            100_000.0 * rate
        );
    }
}

#[test]
fn missing_or_invalid_one_hour_rates_are_explicit_pricing_errors() {
    let usage = TokenBreakdown {
        cache_write: 300_000,
        cache_write_1h: 100_000,
        ..Default::default()
    };
    for rate in [None, Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
        let row = ModelPricing {
            cache_creation_input_token_cost: Some(1.0),
            cache_creation_input_token_cost_above_200k_tokens: Some(2.0),
            cache_creation_input_token_cost_above_1hr: rate,
            ..Default::default()
        };
        assert_eq!(
            compute_cost(&row, &usage),
            Err(PricingComputationError::MissingCacheWrite1hRate)
        );
        let ordinary = TokenBreakdown {
            cache_write_1h: 0,
            ..usage.clone()
        };
        assert_eq!(compute_cost(&row, &ordinary).unwrap(), 400_000.0);
    }
}

#[test]
fn one_hour_pricing_keeps_selected_provider_and_catalog_authority() {
    let complete = ModelPricing {
        cache_creation_input_token_cost: Some(1.25e-5),
        cache_creation_input_token_cost_above_1hr: Some(2e-5),
        ..Default::default()
    };
    let incomplete = ModelPricing {
        cache_creation_input_token_cost_above_1hr: None,
        ..complete.clone()
    };
    let usage = TokenBreakdown {
        cache_write: 100,
        cache_write_1h: 100,
        ..Default::default()
    };
    let scoped = PricingLookup::new(
        HashMap::from([("claude-sonnet-4.6".into(), complete.clone())]),
        HashMap::from([("anthropic/claude-sonnet-4.6".into(), incomplete.clone())]),
    );
    assert_eq!(
        scoped.calculate_cost_with_provider("claude-sonnet-4.6", Some("anthropic"), &usage),
        Err(PricingComputationError::MissingCacheWrite1hRate)
    );
    let ordered = PricingLookup::new_with_models_dev_and_order(
        HashMap::from([("claude-sonnet-4.6".into(), complete)]),
        HashMap::from([("claude-sonnet-4.6".into(), incomplete)]),
        HashMap::new(),
        serde_json::from_str(r#"["openrouter","litellm","models.dev"]"#).unwrap(),
    );
    assert_eq!(
        ordered.calculate_cost_with_provider("claude-sonnet-4.6", Some("anthropic"), &usage),
        Err(PricingComputationError::MissingCacheWrite1hRate)
    );
    let empty = PricingLookup::new(HashMap::new(), HashMap::new());
    assert_eq!(
        empty.calculate_cost_with_provider("claude-sonnet-4.6", Some("anthropic"), &usage),
        Err(PricingComputationError::MissingCacheWrite1hRate)
    );
}
