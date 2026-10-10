use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use tokenx_engine::pricing::{ModelPricing, PricingService, ResolvedPricingSnapshot};
use tokenx_engine::scanner::ScannerSettings;
use tokenx_engine::{
    AcquisitionConfig, AcquisitionEngine, CalendarContext, ClientId, ClientSelection,
    ClientUniverse, DateRange, Generation, GroupBy, ModelMappings, PricingContext,
};

fn write(path: &Path, rows: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        rows.iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
}

fn model_usage(input: i64, output: i64, read: i64, write: i64) -> Value {
    json!({"inputTokens": input, "outputTokens": output,
        "thinkingTokens": output, "cacheReadInputTokens": read,
        "cacheCreationInputTokens": write, "costUSD": 12345})
}

fn snapshot(models: Value) -> Value {
    json!({"type": "cost-state", "sessionId": "session",
        "startTime": 1791532800000_i64, "modelUsage": models})
}

fn acquire(home: &Path, cache: &Path, rate: f64, mappings: ModelMappings) -> Generation {
    let service = PricingService::new(
        HashMap::from([(
            "claude-haiku-5.5".into(),
            ModelPricing {
                input_cost_per_token: Some(rate),
                output_cost_per_token: Some(2.0 * rate),
                cache_read_input_token_cost: Some(0.1 * rate),
                cache_creation_input_token_cost: Some(1.25 * rate),
                cache_creation_input_token_cost_above_1hr: Some(2.0 * rate),
                ..Default::default()
            },
        )]),
        HashMap::new(),
    );
    let pricing = Arc::new(ResolvedPricingSnapshot::explicit(
        PricingContext::explicit_with_catalog("test", format!("rate-{rate}")),
        Some(Arc::new(service)),
        Vec::new(),
    ));
    let config = AcquisitionConfig::new(
        home.to_owned(),
        DateRange::none(),
        ClientUniverse::new([ClientId::Claude]).unwrap(),
        ScannerSettings::default(),
        CalendarContext::explicit("UTC").unwrap(),
        pricing.context().clone(),
    )
    .unwrap()
    .with_model_mappings(mappings);
    AcquisitionEngine::new(config, pricing, cache.to_owned())
        .unwrap()
        .acquire()
        .unwrap()
}

fn models(generation: &Generation) -> tokenx_engine::ModelProjection {
    generation
        .project_models(
            &ClientSelection::new([ClientId::Claude]).unwrap(),
            GroupBy::Model,
        )
        .unwrap()
}

#[test]
fn cost_state_reconciles_subagents_mirrors_cached_inputs_and_repricing() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let project = home.path().join(".claude/projects/-project");
    let state = snapshot(json!({"claude-haiku-5-5[1m]": model_usage(100, 20, 100, 40)}));
    write(
        &project.join("session.jsonl"),
        &[
            json!({"type": "system", "cwd": "/project"}),
            state.clone(),
            state.clone(),
        ],
    );
    write(
        &home.path().join(".claude/transcripts/session.jsonl"),
        &[state],
    );
    let run = |rate| acquire(home.path(), cache.path(), rate, ModelMappings::default());
    let cold = run(1.0);
    assert_eq!(models(&cold).models.len(), 1);
    assert_eq!(models(&cold).total_tokens, 260);
    assert_eq!(models(&cold).total_cost, 200.0);
    assert_eq!(cold.sessions()[0].message_count, 0);
    assert_eq!(cold.sessions()[0].turn_count, 0);
    assert_eq!(models(&run(1.0)), models(&cold));

    let child = json!({"type": "assistant", "cwd": "/project",
        "isSidechain": true, "sessionId": "session", "agentId": "worker",
        "timestamp": "2026-10-09T10:00:00Z", "requestId": "request",
        "message": {"id": "message", "model": "claude-haiku-5.5", "usage": {
            "input_tokens": 20, "output_tokens": 5, "cache_read_input_tokens": 30,
            "cache_creation_input_tokens": 10,
            "cache_creation": {"ephemeral_1h_input_tokens": 10}
        }}
    });
    let child_path = project.join("session/subagents/agent-worker.jsonl");
    write(&child_path, &[child.clone(), child.clone()]);
    std::fs::write(
        child_path.with_extension("meta.json"),
        r#"{"agentType":"explore"}"#,
    )
    .unwrap();
    let mirror = home.path().join(".claude/transcripts/agent-worker.jsonl");
    write(&mirror, &[child]);
    std::fs::write(
        mirror.with_extension("meta.json"),
        r#"{"agentType":"explore"}"#,
    )
    .unwrap();

    // Existing snapshot shards combine with new detailed inputs; their tokens
    // remain unchanged, while the newly observed one-hour split refines cost.
    let mixed = run(1.0);
    assert_eq!(mixed.health().rejected_records(), 0);
    assert_eq!(models(&mixed).total_tokens, 260);
    assert_eq!(models(&mixed).total_cost, 207.5);
    assert_eq!(mixed.sessions()[0].message_count, 1);
    assert_eq!(mixed.sessions()[0].turn_count, 0);
    assert_eq!(models(&run(1.0)), models(&mixed));
    assert_eq!(models(&run(2.0)).total_cost, 415.0);
    let rebuilt_cache = tempfile::tempdir().unwrap();
    let rebuilt = acquire(
        home.path(),
        rebuilt_cache.path(),
        1.0,
        ModelMappings::default(),
    );
    assert_eq!(models(&rebuilt), models(&mixed));
}

#[test]
fn cost_state_reconciles_source_models_before_user_mappings() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".claude/transcripts/session.jsonl"),
        &[
            json!({"type": "assistant", "timestamp": "2026-10-09T10:00:00Z",
            "message": {"model": "claude-haiku-4-5-20251001", "usage": {
                "input_tokens": 500, "output_tokens": 100
            }}}),
            snapshot(json!({
                "claude-haiku-4-5-20251001": model_usage(500, 100, 0, 0),
                "claude-haiku-5-5": model_usage(100, 20, 0, 0),
                "claude-haiku-5-5[1m]": model_usage(200, 40, 0, 0)
            })),
        ],
    );
    let normal = acquire(home.path(), cache.path(), 1.0, ModelMappings::default());
    let projected = models(&normal);
    assert_eq!(projected.models.len(), 2);
    assert_eq!(projected.total_tokens, 960);
    let haiku = projected
        .models
        .iter()
        .find(|row| row.model_id.as_ref() == "claude-haiku-5.5")
        .unwrap();
    assert_eq!(haiku.tokens.input, 300);
    assert_eq!(haiku.tokens.output, 60);
    let merged = acquire(
        home.path(),
        cache.path(),
        1.0,
        ModelMappings::from_toml("[[rules]]\npattern = 'claude-haiku-*'\nmodel = 'merged-haiku'")
            .unwrap(),
    );
    assert_eq!(models(&merged).models.len(), 1);
    assert_eq!(models(&merged).total_tokens, projected.total_tokens);
}

#[test]
fn invalid_cost_state_model_keeps_healthy_siblings_and_reports_damage_on_cache_hits() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".claude/transcripts/session.jsonl"),
        &[snapshot(
            json!({"claude-haiku-5-5": model_usage(100, 20, 0, 0),
                "negative": model_usage(-1, 5, 0, 0),
                "overflow": model_usage(i64::MAX, 5, 0, 0),
                "malformed": {"inputTokens": "bad"},
                "": model_usage(1, 5, 0, 0)
            }),
        )],
    );
    for _ in 0..2 {
        let generation = acquire(home.path(), cache.path(), 1.0, ModelMappings::default());
        assert_eq!(generation.health().rejected_records(), 4);
        assert_eq!(models(&generation).total_tokens, 120);
        assert_eq!(models(&generation).models.len(), 1);
    }
}

#[test]
fn invalid_last_cost_state_does_not_restore_an_older_snapshot() {
    for (field, value) in [
        ("startTime", json!(0)),
        ("startTime", json!("bad")),
        ("sessionId", json!("")),
        ("sessionId", json!(123)),
        ("modelUsage", Value::Null),
    ] {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let state = snapshot(json!({"claude-haiku-5-5": model_usage(100, 20, 0, 0)}));
        let mut invalid = state.clone();
        invalid[field] = value;
        write(
            &home.path().join(".claude/transcripts/session.jsonl"),
            &[
                state,
                invalid,
                json!({"type":"assistant", "timestamp":"2026-10-09T10:00:00Z",
                "message":{"model":"claude-haiku-4.5", "usage":{"input_tokens":10}}}),
            ],
        );
        let generation = acquire(home.path(), cache.path(), 1.0, ModelMappings::default());
        assert_eq!(generation.health().rejected_records(), 1, "{field}");
        assert_eq!(models(&generation).total_tokens, 10);
        assert_eq!(
            models(&generation).models[0].model_id.as_ref(),
            "claude-haiku-4.5"
        );
    }
}

#[test]
fn cost_state_matches_source_session_id_in_renamed_transcripts() {
    let home = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".claude/transcripts/exported-copy.jsonl"),
        &[
            json!({"type":"assistant", "sessionId":"session",
                "timestamp":"2026-10-09T10:00:00Z",
                "message":{"id":"message", "model":"claude-haiku-5-5",
                    "usage":{"input_tokens":100, "output_tokens":20}}}),
            snapshot(json!({"claude-haiku-5-5": model_usage(120, 25, 0, 0)})),
        ],
    );
    for _ in 0..2 {
        let generation = acquire(home.path(), cache.path(), 1.0, ModelMappings::default());
        assert_eq!(generation.health().rejected_records(), 0);
        assert_eq!(models(&generation).total_tokens, 145);
    }
}
