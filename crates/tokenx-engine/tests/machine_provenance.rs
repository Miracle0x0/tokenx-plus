use chrono::NaiveDate;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokenx_engine::pricing::{
    custom::CustomPricing, ModelPricing, PricingService, ResolvedPricingSnapshot,
};
use tokenx_engine::scanner::ScannerSettings;
use tokenx_engine::{
    AcquisitionConfig, AcquisitionEngine, CalendarContext, ClientId, ClientSelection,
    ClientUniverse, DateRange, Generation, GroupBy, PricingContext, UsageQuery,
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

fn pi(session: &str, provider: &str, model: &str, tokens: i64) -> Vec<Value> {
    vec![
        json!({"type":"session","id":session}),
        json!({"type":"message","timestamp":"2026-10-18T00:00:00Z",
        "message":{"role":"assistant","model":model,"provider":provider,
        "usage":{"input":tokens,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":tokens}}}),
    ]
}

fn acquire(home: &Path, scanner: ScannerSettings, clients: &[ClientId]) -> Generation {
    let custom = home.join("custom-pricing.json");
    std::fs::write(
        &custom,
        json!({"provider_overrides":[
            {"provider_id":"gateway-a","model_id":"gpt-5","pricing":{"input_cost_per_token":2.0}},
            {"provider_id":"gateway-b","model_id":"gpt-5","pricing":{"input_cost_per_token":0.0}}
        ]})
        .to_string(),
    )
    .unwrap();
    let pricing = Arc::new(ResolvedPricingSnapshot::explicit(
        PricingContext::explicit_with_catalog("custom", "catalog"),
        Some(Arc::new(PricingService::new_with_custom(
            CustomPricing::load_from_path(&custom),
            [(
                "openai/gpt-5".into(),
                ModelPricing {
                    input_cost_per_token: Some(1.0),
                    ..Default::default()
                },
            )]
            .into(),
            Default::default(),
        ))),
        vec![],
    ));
    let config = AcquisitionConfig::new(
        home.to_owned(),
        DateRange::none(),
        ClientUniverse::new(clients.iter().copied()).unwrap(),
        scanner,
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
fn machine_and_price_projections_survive_cache_roundtrip_relabeling_and_missing_inputs() {
    let home = tempfile::tempdir().unwrap();
    let remote = home.path().join("remote");
    let gpu = remote.join("gpu");
    let local_file = home.path().join(".pi/agent/sessions/local.jsonl");
    write(&local_file, &pi("local", "gateway-a", "gpt-5", 10));
    write(
        &home.path().join(".pi/agent/sessions/unpriced.jsonl"),
        &pi("unpriced", "openai", "private-probe", 7),
    );
    write(
        &remote.join("server.jsonl"),
        &pi("server", "gateway-b", "gpt-5", 20),
    );
    write(&gpu.join("gpu.jsonl"), &pi("gpu", "openai", "gpt-5", 30));
    let mut settings = ScannerSettings {
        extra_scan_paths: BTreeMap::from([(ClientId::Pi, vec![remote.clone()])]),
        machine_labels: BTreeMap::from([(remote, "server".into()), (gpu.clone(), "gpu".into())]),
        local_machine_label: "laptop".into(),
        ..Default::default()
    };
    let mut fingerprint = None;
    for iteration in 0..3 {
        if iteration == 2 {
            settings
                .machine_labels
                .insert(gpu.clone(), "accelerator".into());
        }
        let generation = acquire(
            home.path(),
            settings.clone(),
            &[ClientId::Pi, ClientId::Qwen],
        );
        if iteration == 1 {
            assert_eq!(fingerprint, Some(generation.source_fingerprint()));
        }
        if iteration == 2 {
            assert_ne!(fingerprint, Some(generation.source_fingerprint()));
        }
        fingerprint = Some(generation.source_fingerprint());
        let generation: Generation =
            bincode::deserialize(&bincode::serialize(&generation).unwrap()).unwrap();
        let clients = ClientSelection::new([ClientId::Pi]).unwrap();
        let models = generation
            .project_models(&clients, GroupBy::MachineModel)
            .unwrap();
        assert_eq!(models.total_tokens, 67);
        assert_eq!(models.total_cost, 50.0);
        assert!(!models.cost_provenance.complete);
        assert_eq!(models.cost_provenance.sources.len(), 3);
        let labels: Vec<_> = models
            .models
            .iter()
            .map(|row| row.machine.as_deref().unwrap())
            .collect();
        assert!(labels.contains(&"laptop") && labels.contains(&"server"));
        assert!(labels.contains(&if iteration == 2 { "accelerator" } else { "gpu" }));
        let free = models
            .models
            .iter()
            .find(|row| row.machine.as_deref() == Some("server"))
            .unwrap();
        assert_eq!(free.cost, 0.0);
        assert!(free.cost_provenance.complete);
        assert_eq!(
            free.cost_provenance
                .sources
                .first()
                .unwrap()
                .billing_provider
                .as_deref(),
            Some("gateway-b")
        );
        let unpriced = models
            .models
            .iter()
            .find(|row| row.model_id.as_ref() == "private-probe")
            .unwrap();
        assert!(!unpriced.cost_provenance.complete && unpriced.cost_provenance.sources.is_empty());
        for grouping in [
            GroupBy::Model,
            GroupBy::ClientModel,
            GroupBy::ClientProviderModel,
            GroupBy::WorkspaceModel,
            GroupBy::MachineModel,
        ] {
            let view = generation
                .project_usage(&UsageQuery {
                    clients: clients.clone(),
                    group_by: grouping,
                    effective_date: NaiveDate::from_ymd_opt(2026, 10, 18).unwrap(),
                })
                .unwrap();
            assert_eq!((view.total_tokens, view.total_cost), (67, 50.0));
            assert_eq!(view.cost_provenance, models.cost_provenance);
            assert_eq!(view.daily[0].cost_provenance, models.cost_provenance);
            if grouping == GroupBy::MachineModel {
                assert_eq!(view.hourly[0].models.len(), 4);
                assert!(view.hourly[0]
                    .models
                    .iter()
                    .all(|row| row.machine.is_some()));
                let periods = tokenx_engine::build_period_usage(
                    &view,
                    tokenx_engine::projection::PeriodKind::Monthly,
                )
                .unwrap();
                assert_eq!(periods[0].client_breakdown[&ClientId::Pi].models.len(), 4);
            } else {
                assert!(view.models.iter().all(|row| row.machine.is_none()));
            }
        }
        let empty = generation
            .project_models(
                &ClientSelection::new([ClientId::Qwen]).unwrap(),
                GroupBy::MachineModel,
            )
            .unwrap();
        assert!(empty.cost_provenance.complete && empty.cost_provenance.sources.is_empty());
        if iteration == 2 {
            std::fs::remove_file(&local_file).unwrap();
            assert_eq!(
                generation
                    .project_models(&clients, GroupBy::MachineModel)
                    .unwrap()
                    .total_tokens,
                67
            );
        }
    }
}

#[test]
fn codex_archives_and_claude_checkpoints_keep_machine_labels_on_cache_and_append_paths() {
    use std::io::Write;
    let home = tempfile::tempdir().unwrap();
    let codex = home.path().join(".codex");
    let claude = home.path().join(".claude");
    let plain = codex.join("sessions/plain.jsonl");
    let archive = codex.join("archived_sessions/archive.jsonl.zst");
    let transcript = claude.join("projects/project/session.jsonl");
    let codex_rows = |session| {
        vec![
            json!({"type":"session_meta","payload":{"id":session}}),
            json!({"type":"turn_context","payload":{"model":"gpt-5"}}),
            json!({"type":"event_msg","timestamp":"2026-10-18T00:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":0},"last_token_usage":{"input_tokens":10,"output_tokens":0}}}}),
        ]
    };
    write(&plain, &codex_rows("plain"));
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    let text = codex_rows("archive")
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&archive, zstd::encode_all(text.as_bytes(), 0).unwrap()).unwrap();
    let claude_row = |id| {
        json!({"type":"assistant","sessionId":"session","timestamp":"2026-10-18T00:00:00Z",
        "requestId":id,"message":{"id":id,"model":"claude-sonnet-4","usage":{"input_tokens":5,"output_tokens":0}}})
    };
    write(&transcript, &[claude_row("first")]);
    let settings = ScannerSettings {
        machine_labels: BTreeMap::from([
            (codex, "codex-host".into()),
            (claude, "claude-host".into()),
        ]),
        ..Default::default()
    };
    for pass in 0..3 {
        if pass == 2 {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&plain)
                .unwrap();
            writeln!(file,"\n{}",json!({"type":"event_msg","timestamp":"2026-10-18T00:00:01Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":20,"output_tokens":0},"last_token_usage":{"input_tokens":10,"output_tokens":0}}}})).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&transcript)
                .unwrap();
            writeln!(file, "\n{}", claude_row("second")).unwrap();
        }
        let generation = acquire(
            home.path(),
            settings.clone(),
            &[ClientId::Codex, ClientId::Claude],
        );
        let models = generation
            .project_models(
                &ClientSelection::all(generation.universe()),
                GroupBy::MachineModel,
            )
            .unwrap();
        assert_eq!(models.models.len(), 2);
        for row in models.models {
            let (machine, total) = if row.clients == [ClientId::Codex] {
                ("codex-host", if pass == 2 { 30 } else { 20 })
            } else {
                ("claude-host", if pass == 2 { 10 } else { 5 })
            };
            assert_eq!(row.machine.as_deref(), Some(machine));
            assert_eq!(row.tokens.total(), total);
        }
        assert_eq!(generation.health().rejected_records(), 0);
    }
}

#[test]
fn repricing_cached_usage_replaces_its_origin_with_the_selected_catalog() {
    let home = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".pi/agent/sessions/session.jsonl"),
        &pi("session", "openai", "gpt-5", 10),
    );
    let custom_path = home.path().join("rates.json");
    let mut fingerprint = None;
    for (custom, source, cost) in [(false, "LiteLLM", 10.0), (true, "Custom", 20.0)] {
        let custom_prices = if custom {
            std::fs::write(
                &custom_path,
                json!({"models":{"gpt-5":{"input_cost_per_token":2.0}}}).to_string(),
            )
            .unwrap();
            CustomPricing::load_from_path(&custom_path)
        } else {
            CustomPricing::default()
        };
        let pricing = Arc::new(ResolvedPricingSnapshot::explicit(
            PricingContext::explicit_with_catalog(source, "catalog"),
            Some(Arc::new(PricingService::new_with_custom(
                custom_prices,
                [(
                    "openai/gpt-5".into(),
                    ModelPricing {
                        input_cost_per_token: Some(1.0),
                        ..Default::default()
                    },
                )]
                .into(),
                Default::default(),
            ))),
            vec![],
        ));
        let config = AcquisitionConfig::new(
            home.path().to_owned(),
            DateRange::none(),
            ClientUniverse::new([ClientId::Pi]).unwrap(),
            Default::default(),
            CalendarContext::explicit("UTC").unwrap(),
            pricing.context().clone(),
        )
        .unwrap();
        let generation = AcquisitionEngine::new(config, pricing, home.path().join("cache"))
            .unwrap()
            .acquire()
            .unwrap();
        if let Some(previous) = fingerprint {
            assert_eq!(previous, generation.source_fingerprint());
        }
        fingerprint = Some(generation.source_fingerprint());
        let view = generation
            .project_models(&ClientSelection::all(generation.universe()), GroupBy::Model)
            .unwrap();
        assert_eq!((view.total_tokens, view.total_cost), (10, cost));
        assert!(view.cost_provenance.complete);
        assert_eq!(view.cost_provenance.sources.len(), 1);
        assert_eq!(
            view.cost_provenance
                .sources
                .first()
                .unwrap()
                .source
                .as_ref(),
            source
        );
    }
}
