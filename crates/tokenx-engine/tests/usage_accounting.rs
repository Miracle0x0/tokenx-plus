use std::{collections::HashMap, path::Path, sync::Arc};

use serde_json::json;
use tokenx_engine::pricing::{
    ModelPricing, PricingDiagnosticKind, PricingService, ResolvedPricingSnapshot,
};
use tokenx_engine::{
    AcquisitionConfig, AcquisitionEngine, CalendarContext, ClientId, ClientSelection,
    ClientUniverse, DateRange, Generation, GroupBy, PricingContext,
};

fn acquire(home: &Path, client: ClientId, key: &str, row: ModelPricing) -> Generation {
    let pricing = Arc::new(ResolvedPricingSnapshot::explicit(
        PricingContext::explicit_with_catalog("none", "accounting-test"),
        Some(Arc::new(PricingService::new(
            HashMap::from([(key.into(), row)]),
            HashMap::new(),
        ))),
        vec![],
    ));
    let config = AcquisitionConfig::new(
        home.to_owned(),
        DateRange::none(),
        ClientUniverse::new([client]).unwrap(),
        Default::default(),
        CalendarContext::explicit("UTC").unwrap(),
        pricing.context().clone(),
    )
    .unwrap();
    AcquisitionEngine::new(config, pricing, home.join("cache"))
        .unwrap()
        .acquire()
        .unwrap()
}

#[test]
fn qwen_subset_accounting_survives_cached_inputs_and_generation_roundtrip() {
    let home = tempfile::tempdir().unwrap();
    let input = home.path().join(".qwen/projects/probe/chats/session.jsonl");
    std::fs::create_dir_all(input.parent().unwrap()).unwrap();
    std::fs::write(
        input,
        json!({"type":"assistant","model":"qwen3.5-plus",
        "timestamp":"2026-10-18T00:00:00Z","sessionId":"probe","usageMetadata":{
        "promptTokenCount":10000,"candidatesTokenCount":300,"totalTokenCount":10300,
        "cachedContentTokenCount":9000,"thoughtsTokenCount":120}})
        .to_string(),
    )
    .unwrap();
    for _ in 0..2 {
        let generation = acquire(
            home.path(),
            ClientId::Qwen,
            "qwen/qwen3.5-plus",
            ModelPricing {
                input_cost_per_token: Some(1e-6),
                output_cost_per_token: Some(2e-6),
                cache_read_input_token_cost: Some(1e-7),
                ..Default::default()
            },
        );
        let generation: Generation =
            bincode::deserialize(&bincode::serialize(&generation).unwrap()).unwrap();
        let models = generation
            .project_models(
                &ClientSelection::new([ClientId::Qwen]).unwrap(),
                GroupBy::Model,
            )
            .unwrap();
        assert_eq!(models.total_tokens, 10300);
        assert!((models.total_cost - 0.0025).abs() < 1e-12);
        assert_eq!(generation.health().rejected_records(), 0);
    }
}

#[test]
fn grok_multi_call_tier_ambiguity_keeps_tokens_and_diagnostics_across_caches() {
    let home = tempfile::tempdir().unwrap();
    let input = home
        .path()
        .join(".grok/sessions/project/session/updates.jsonl");
    std::fs::create_dir_all(input.parent().unwrap()).unwrap();
    std::fs::write(
        input,
        json!({"params":{"sessionId":"probe","modelId":"grok-4.5",
        "_meta":{"agentTimestampMs":1790900001000i64},"update":{"sessionUpdate":"turn_completed",
        "usage":{"inputTokens":372794,"outputTokens":8021,"cachedReadTokens":314112,
        "reasoningTokens":4144,"totalTokens":380815,"modelCalls":8}}}})
        .to_string(),
    )
    .unwrap();
    for _ in 0..2 {
        let generation = acquire(
            home.path(),
            ClientId::Grok,
            "xai/grok-4.5",
            ModelPricing {
                input_cost_per_token: Some(1e-6),
                output_cost_per_token: Some(2e-6),
                cache_read_input_token_cost: Some(1e-7),
                input_cost_per_token_above_200k_tokens: Some(2e-6),
                output_cost_per_token_above_200k_tokens: Some(4e-6),
                cache_read_input_token_cost_above_200k_tokens: Some(2e-7),
                ..Default::default()
            },
        );
        let generation: Generation =
            bincode::deserialize(&bincode::serialize(&generation).unwrap()).unwrap();
        let generation = generation.with_pricing_diagnostics(vec![]);
        let models = generation
            .project_models(
                &ClientSelection::new([ClientId::Grok]).unwrap(),
                GroupBy::Model,
            )
            .unwrap();
        assert_eq!(models.total_tokens, 380815);
        assert_eq!(models.total_cost, 0.0);
        assert_eq!(generation.health().rejected_records(), 0);
        assert!(generation
            .pricing_diagnostics()
            .iter()
            .any(|issue| issue.kind() == PricingDiagnosticKind::RequestTierUnavailable));
    }
}
