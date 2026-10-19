use serde_json::{json, Value};
use std::{collections::HashMap, io::Write, path::Path, sync::Arc};
use tokenx_engine::pricing::{
    custom::CustomPricing, ModelPricing, PricingDiagnosticKind, PricingService,
    ResolvedPricingSnapshot,
};
use tokenx_engine::{
    AcquisitionConfig, AcquisitionEngine, CalendarContext, ClientId, ClientSelection,
    ClientUniverse, DateRange, Generation, GroupBy, PricingContext,
};

fn write_rows(path: &Path, rows: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap();
    }
}

fn rules(a: f64) -> Value {
    let price = |rate| json!({"input_cost_per_token":rate,"output_cost_per_token":rate});
    json!({"models":{"gpt-5":price(10.0)},"provider_overrides":[
        {"provider_id":"gateway-a","model_id":"gpt-5","pricing":price(a)},
        {"provider_id":"gateway-b","model_id":"gpt-5","pricing":price(2.0)},
        {"provider_id":"openai","model_id":"gpt-5","pricing":price(1000.0)}]})
}

fn acquire(home: &Path, cache: &Path, client: ClientId, config: &Value) -> Generation {
    let custom = home.join("custom-pricing.json");
    std::fs::write(&custom, config.to_string()).unwrap();
    let pricing = Arc::new(ResolvedPricingSnapshot::explicit(
        PricingContext::explicit_with_catalog(config.to_string(), "public"),
        Some(Arc::new(PricingService::new_with_custom(
            CustomPricing::load_from_path(&custom),
            HashMap::from([(
                "gpt-5".into(),
                ModelPricing {
                    input_cost_per_token: Some(999.0),
                    output_cost_per_token: Some(999.0),
                    ..Default::default()
                },
            )]),
            HashMap::new(),
        ))),
        Vec::new(),
    ));
    let acquisition = AcquisitionConfig::new(
        home.to_owned(),
        DateRange::none(),
        ClientUniverse::new([client]).unwrap(),
        Default::default(),
        CalendarContext::explicit("UTC").unwrap(),
        pricing.context().clone(),
    )
    .unwrap();
    AcquisitionEngine::new(acquisition, pricing, cache.to_owned())
        .unwrap()
        .acquire()
        .unwrap()
}

fn totals(generation: &Generation, client: ClientId) -> (u64, f64) {
    let models = generation
        .project_models(&ClientSelection::new([client]).unwrap(), GroupBy::Model)
        .unwrap();
    (models.total_tokens, models.total_cost)
}

fn pi(provider: Option<&str>) -> Value {
    json!({"type":"message","timestamp":"2026-10-18T00:00:00Z","message":{
        "role":"assistant","model":"openai/gpt-5","provider":provider,
        "usage":{"input":10,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":11,"cost":{"total":999}}}})
}

#[test]
fn billing_scopes_price_canonical_models_and_reprice_cost_free_cache_paid_free_paid() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_rows(
        &home.path().join(".pi/agent/sessions/session.jsonl"),
        &[
            json!({"type":"session","id":"pi"}),
            pi(Some(" Gateway-A ")),
            pi(Some("gateway-b")),
            pi(None),
        ],
    );
    let mut fingerprint = None;
    for (rate, cost) in [(1.0, 143.0), (0.0, 132.0), (4.0, 176.0)] {
        let generation = acquire(home.path(), cache.path(), ClientId::Pi, &rules(rate));
        assert_eq!(totals(&generation, ClientId::Pi), (33, cost));
        if let Some(previous) = fingerprint {
            assert_eq!(generation.source_fingerprint(), previous);
        }
        fingerprint = Some(generation.source_fingerprint());
    }
}

#[test]
fn a_selected_scope_never_borrows_missing_rates_from_global_or_public_rows() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_rows(
        &home.path().join(".pi/agent/sessions/session.jsonl"),
        &[json!({"type":"session","id":"pi"}), pi(Some("gateway-a"))],
    );
    let mut config = rules(1.0);
    config["provider_overrides"][0]["pricing"]
        .as_object_mut()
        .unwrap()
        .remove("output_cost_per_token");
    for _ in 0..2 {
        let generation = acquire(home.path(), cache.path(), ClientId::Pi, &config)
            .with_pricing_diagnostics(Vec::new());
        assert_eq!(totals(&generation, ClientId::Pi), (11, 0.0));
        assert!(generation
            .pricing_diagnostics()
            .iter()
            .any(|issue| issue.kind() == PricingDiagnosticKind::BillingRateUnavailable));
        assert_eq!(generation.health().rejected_records(), 0);
    }
}

fn claude(provider: Option<&str>, input: i64) -> Value {
    json!({"type":"assistant","cwd":"/work/project","providerId":provider,"sessionId":"session",
        "requestId":"request","timestamp":"2026-10-18T00:00:00Z","message":{"id":"response","model":"gpt-5","usage":{"input_tokens":input,"output_tokens":1}}})
}

#[test]
fn conflicting_claude_billing_evidence_stays_unknown_across_files_cache_and_append() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let path = home
        .path()
        .join(".claude/projects/-work-project/session.jsonl");
    let mirror = home.path().join(".claude/transcripts/session.jsonl");
    write_rows(&path, &[claude(Some("gateway-a"), 10)]);
    write_rows(&mirror, &[claude(Some("gateway-b"), 10)]);
    for _ in 0..2 {
        assert_eq!(
            totals(
                &acquire(home.path(), cache.path(), ClientId::Claude, &rules(1.0)),
                ClientId::Claude
            ),
            (11, 110.0)
        );
    }
    write_rows(
        &path,
        &[claude(Some("gateway-b"), 20), claude(Some("gateway-a"), 30)],
    );
    assert_eq!(
        totals(
            &acquire(home.path(), cache.path(), ClientId::Claude, &rules(1.0)),
            ClientId::Claude
        ),
        (31, 310.0)
    );
}

#[test]
fn codex_pending_response_retains_billing_scope_across_append_checkpoints() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let path = home.path().join(".codex/sessions/session.jsonl");
    let event =
        |kind, payload| json!({"type":kind,"timestamp":"2026-10-18T00:00:00Z","payload":payload});
    write_rows(
        &path,
        &[
            event(
                "session_meta",
                json!({"id":"session","model_provider":"gateway-a"}),
            ),
            event("turn_context", json!({"model":"gpt-5","turn_id":"turn"})),
            event(
                "token_usage_record",
                json!({"response_id":"response","turn_id":"turn","usage":{"input_tokens":10,"output_tokens":1}}),
            ),
        ],
    );
    assert_eq!(
        totals(
            &acquire(home.path(), cache.path(), ClientId::Codex, &rules(1.0)),
            ClientId::Codex
        ),
        (0, 0.0)
    );
    write_rows(
        &path,
        &[event(
            "event_msg",
            json!({"type":"task_complete","turn_id":"turn"}),
        )],
    );
    for _ in 0..2 {
        assert_eq!(
            totals(
                &acquire(home.path(), cache.path(), ClientId::Codex, &rules(1.0)),
                ClientId::Codex
            ),
            (11, 11.0)
        );
    }
}

#[test]
fn forced_public_lookup_bypasses_billing_and_global_overrides() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("custom-pricing.json");
    std::fs::write(&path, rules(1.0).to_string()).unwrap();
    let service = PricingService::new_with_custom(
        CustomPricing::load_from_path(&path),
        HashMap::from([(
            "gpt-5".into(),
            ModelPricing {
                input_cost_per_token: Some(77.0),
                ..Default::default()
            },
        )]),
        HashMap::new(),
    );
    let result = service
        .lookup_canonical_with_billing_provider(
            "gpt-5",
            Some("litellm"),
            Some("openai"),
            Some("gateway-a"),
        )
        .unwrap();
    assert_eq!(result.pricing.input_cost_per_token, Some(77.0));
    assert!(result.billing_provider.is_none());
}

#[test]
fn copilot_provider_attributes_are_attribution_not_billing_channels() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_rows(
        &home.path().join(".copilot/otel/trace.jsonl"),
        &[
            json!({"type":"span","traceId":"trace","spanId":"span","name":"chat gpt-5","endTime":[1792281601,0],"attributes":{
                "gen_ai.operation.name":"chat","gen_ai.response.model":"gpt-5","gen_ai.provider.name":"openai",
                "gen_ai.usage.input_tokens":10,"gen_ai.usage.output_tokens":1
            }}),
        ],
    );
    assert_eq!(
        totals(
            &acquire(home.path(), cache.path(), ClientId::Copilot, &rules(1.0)),
            ClientId::Copilot
        ),
        (11, 110.0)
    );
}
