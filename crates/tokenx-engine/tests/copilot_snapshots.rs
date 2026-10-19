use std::{collections::HashMap, path::Path, sync::Arc};

use serde_json::{json, Value};
use tokenx_engine::pricing::{ModelPricing, PricingService, ResolvedPricingSnapshot};
use tokenx_engine::{
    AcquisitionConfig, AcquisitionEngine, CalendarContext, ClientId, ClientSelection,
    ClientUniverse, DateRange, GroupBy, PricingContext,
};

fn span(trace: Option<&str>, session: Option<&str>, input: i64, output: i64, read: i64) -> Value {
    json!({"type":"span", "traceId":trace, "spanId":"span", "name":"chat gpt-5",
    "endTime":[1792281601,0], "attributes":{
        "gen_ai.operation.name":"chat", "gen_ai.response.model":"gpt-5",
        "gen_ai.conversation.id":session, "gen_ai.usage.input_tokens":input,
        "gen_ai.usage.output_tokens":output, "gen_ai.usage.cache_read.input_tokens":read
    }})
}

fn write(home: &Path, name: &str, row: Value) {
    let root = home.join(".copilot/otel");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(name), format!("{row}\n")).unwrap();
}

fn engine(home: &Path) -> AcquisitionEngine {
    let service = PricingService::new(
        HashMap::from([(
            "gpt-5".into(),
            ModelPricing {
                input_cost_per_token: Some(1.0),
                output_cost_per_token: Some(1.0),
                cache_read_input_token_cost: Some(1.0),
                ..Default::default()
            },
        )]),
        HashMap::new(),
    );
    let pricing = Arc::new(ResolvedPricingSnapshot::explicit(
        PricingContext::explicit_with_catalog("test", "test"),
        Some(Arc::new(service)),
        Vec::new(),
    ));
    let config = AcquisitionConfig::new(
        home.to_owned(),
        DateRange::none(),
        ClientUniverse::new([ClientId::Copilot, ClientId::Pi]).unwrap(),
        Default::default(),
        CalendarContext::explicit("UTC").unwrap(),
        pricing.context().clone(),
    )
    .unwrap();
    AcquisitionEngine::new(config, pricing, home.join("cache")).unwrap()
}

#[test]
fn cross_file_snapshots_merge_before_pricing_on_cold_warm_and_changed_inputs() {
    let home = tempfile::tempdir().unwrap();
    // More files than the acquisition batch width; all describe the same call.
    for index in 0..7 {
        write(
            home.path(),
            &format!("{index}.jsonl"),
            span(Some("trace"), Some("session"), 100, 10, 0),
        );
    }
    write(
        home.path(),
        "rich.jsonl",
        span(Some("trace"), Some("session"), 100, 20, 30),
    );
    let engine = engine(home.path());
    for output in [20, 20, 40] {
        if output == 40 {
            write(
                home.path(),
                "rich.jsonl",
                span(Some("trace"), Some("session"), 100, output, 30),
            );
        }
        let generation = engine.acquire().unwrap();
        let models = generation
            .project_models(
                &ClientSelection::new([ClientId::Copilot]).unwrap(),
                GroupBy::Model,
            )
            .unwrap();
        assert_eq!(models.total_tokens, (100 + output) as u64);
        assert_eq!(models.total_cost, (100 + output) as f64);
        assert_eq!(generation.health().rejected_records(), 0);
    }
}

#[test]
fn sparse_file_identity_and_other_clients_are_never_cross_merged() {
    let home = tempfile::tempdir().unwrap();
    for name in ["a.jsonl", "b.jsonl"] {
        write(home.path(), name, span(None, Some("session"), 100, 20, 0));
    }
    let pi = home.path().join(".pi/agent/sessions");
    std::fs::create_dir_all(&pi).unwrap();
    std::fs::write(pi.join("session.jsonl"), concat!(
        "{\"type\":\"session\",\"id\":\"session\"}\n",
        "{\"type\":\"message\",\"timestamp\":\"2026-10-18T00:00:00Z\",\"message\":{\"role\":\"assistant\",\"model\":\"gpt-5\",\"usage\":{\"input\":100,\"output\":20,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":120}}}\n"
    )).unwrap();
    let generation = engine(home.path()).acquire().unwrap();
    for (client, expected) in [(ClientId::Copilot, 240), (ClientId::Pi, 120)] {
        let models = generation
            .project_models(&ClientSelection::new([client]).unwrap(), GroupBy::Model)
            .unwrap();
        assert_eq!(models.total_tokens, expected);
    }
}

#[test]
fn merged_overflow_is_reported_without_erasing_unrelated_usage() {
    let home = tempfile::tempdir().unwrap();
    for (name, trace, input, output) in [
        ("input.jsonl", "overflow", i64::MAX, 0),
        ("output.jsonl", "overflow", 0, 1),
        ("healthy.jsonl", "healthy", 100, 20),
    ] {
        write(
            home.path(),
            name,
            span(Some(trace), Some("session"), input, output, 0),
        );
    }
    for _ in 0..2 {
        let generation = engine(home.path()).acquire().unwrap();
        let models = generation
            .project_models(
                &ClientSelection::new([ClientId::Copilot]).unwrap(),
                GroupBy::Model,
            )
            .unwrap();
        assert_eq!(models.total_tokens, 120);
        assert_eq!(generation.health().rejected_records(), 1);
    }
}
