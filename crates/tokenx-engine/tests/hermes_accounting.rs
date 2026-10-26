use std::{collections::HashMap, path::Path, sync::Arc};
use tokenx_engine::pricing::{ModelPricing, PricingService, ResolvedPricingSnapshot};
use tokenx_engine::{
    AcquisitionConfig, AcquisitionEngine, CalendarContext, ClientId, ClientSelection,
    ClientUniverse, DateRange, Generation, GroupBy, PricingContext,
};

fn database(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    let db = rusqlite::Connection::open(root.join("state.db")).unwrap();
    db.execute_batch("CREATE TABLE sessions (id TEXT, model TEXT, billing_provider TEXT, started_at REAL,
        message_count INTEGER,input_tokens INTEGER,output_tokens INTEGER,cache_read_tokens INTEGER,
        cache_write_tokens INTEGER,reasoning_tokens INTEGER);
        CREATE TABLE session_model_usage (session_id TEXT,model TEXT,billing_provider TEXT,
        input_tokens INTEGER,output_tokens INTEGER,cache_read_tokens INTEGER,cache_write_tokens INTEGER,reasoning_tokens INTEGER);
        INSERT INTO sessions VALUES ('shared','gpt-5','openai',1791676800,9,9999,9999,0,0,0);
        INSERT INTO session_model_usage VALUES ('shared','gpt-5','openai',40,10,40,10,5);
        INSERT INTO session_model_usage VALUES ('shared','gpt-5','openai',60,20,0,0,10);
        INSERT INTO session_model_usage VALUES ('shared','claude-sonnet-4.6','anthropic',7,3,0,0,1);").unwrap();
}

fn logs(root: &Path, full: bool) {
    std::fs::create_dir_all(root.join("logs")).unwrap();
    let first = "2026-10-12 12:00:00,000 INFO [shared] agent.conversation_loop: API call #1: model=gpt-5 provider=openai in=90 out=10 cache=40/90 write=10\n";
    let second = "2026-10-13 12:00:00,000 INFO [shared] agent.conversation_loop: API call #1: model=gpt-5 provider=openai in=60 out=20 cache=0/60\n";
    std::fs::write(
        root.join("logs/agent.log"),
        if full {
            format!("{first}{second}")
        } else {
            first.to_string()
        },
    )
    .unwrap();
    // Rotation can contain the same call without introducing new usage.
    std::fs::write(root.join("logs/agent.log.1"), first).unwrap();
}

fn acquire(home: &Path, extra: Option<&Path>) -> Generation {
    let row = ModelPricing {
        input_cost_per_token: Some(1.0),
        output_cost_per_token: Some(1.0),
        cache_read_input_token_cost: Some(1.0),
        cache_creation_input_token_cost: Some(1.0),
        ..Default::default()
    };
    let service = Arc::new(ResolvedPricingSnapshot::explicit(
        PricingContext::explicit_with_catalog("test", "test"),
        Some(Arc::new(PricingService::new(
            HashMap::from([
                ("gpt-5".into(), row.clone()),
                ("claude-sonnet-4.6".into(), row),
            ]),
            HashMap::new(),
        ))),
        Vec::new(),
    ));
    let mut settings = tokenx_engine::scanner::ScannerSettings::default();
    if let Some(extra) = extra {
        settings
            .extra_scan_paths
            .insert(ClientId::Hermes, vec![extra.to_owned()]);
    }
    let config = AcquisitionConfig::new(
        home.to_owned(),
        DateRange::none(),
        ClientUniverse::new([ClientId::Hermes]).unwrap(),
        settings,
        CalendarContext::explicit("UTC").unwrap(),
        service.context().clone(),
    )
    .unwrap();
    AcquisitionEngine::new(config, service, home.join("cache"))
        .unwrap()
        .acquire()
        .unwrap()
}

fn totals(generation: &Generation) -> (u64, f64) {
    let report = generation
        .project_models(
            &ClientSelection::new([ClientId::Hermes]).unwrap(),
            GroupBy::Model,
        )
        .unwrap();
    (report.total_tokens, report.total_cost)
}

#[test]
fn hermes_per_model_totals_and_call_log_changes_preserve_exact_usage() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".hermes");
    database(&root);
    logs(&root, false);
    let first = acquire(home.path(), None);
    assert_eq!(totals(&first), (190, 190.0));
    assert_eq!(first.sessions()[0].message_count, 9);
    logs(&root, true);
    let second = acquire(home.path(), None);
    assert_eq!(totals(&second), (190, 190.0));
    assert_ne!(first.source_fingerprint(), second.source_fingerprint());
    let selected = ClientSelection::new([ClientId::Hermes]).unwrap();
    let query = tokenx_engine::UsageQuery {
        clients: selected,
        group_by: GroupBy::Model,
        effective_date: chrono::NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(),
    };
    let daily = second.project_usage(&query).unwrap().daily;
    assert_eq!(
        daily
            .iter()
            .map(|day| (day.date.to_string(), day.tokens.total()))
            .collect::<Vec<_>>(),
        vec![
            ("2026-10-13".into(), 80),
            ("2026-10-12".into(), 100),
            ("2026-10-11".into(), 10)
        ]
    );
}

#[test]
fn hermes_duplicate_database_prefers_wider_logs_only_for_equal_totals() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".hermes");
    let extra = home.path().join("import/profile");
    database(&root);
    logs(&root, false);
    database(&extra);
    logs(&extra, true);
    let expected = acquire(home.path(), Some(&extra));
    assert_eq!(totals(&expected), (190, 190.0));
    assert_eq!(expected.sessions()[0].message_count, 9);
    logs(&root, true);
    let same = acquire(home.path(), None);
    assert_eq!(
        expected
            .project_models(
                &ClientSelection::new([ClientId::Hermes]).unwrap(),
                GroupBy::Model
            )
            .unwrap(),
        same.project_models(
            &ClientSelection::new([ClientId::Hermes]).unwrap(),
            GroupBy::Model
        )
        .unwrap()
    );
    let db = rusqlite::Connection::open(extra.join("state.db")).unwrap();
    db.execute(
        "UPDATE session_model_usage SET input_tokens=input_tokens+1",
        [],
    )
    .unwrap();
    assert_eq!(totals(&acquire(home.path(), Some(&extra))), (190, 190.0));
}

#[test]
fn hermes_bad_call_evidence_keeps_database_usage_visible() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".hermes");
    database(&root);
    logs(&root, true);
    std::fs::write(root.join("logs/agent.log"),"2026-10-12 12:00:00,000 INFO [shared] agent.conversation_loop: API call #1: model=gpt-5 provider=openai in=999999 out=10\n").unwrap();
    let generation = acquire(home.path(), None);
    assert_eq!(totals(&generation), (190, 190.0));
    assert_eq!(generation.health().rejected_records(), 1);
}

#[test]
fn hermes_log_failures_preserve_the_database_but_wal_failures_do_not() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".hermes");
    database(&root);
    std::fs::create_dir_all(root.join("logs/agent.log")).unwrap();
    let partial = acquire(home.path(), None);
    assert_eq!(totals(&partial), (190, 190.0));
    assert_eq!(partial.health().partial_inputs(), 1);
    std::fs::create_dir(root.join("state.db-wal")).unwrap();
    let failed = acquire(home.path(), None);
    assert_eq!(totals(&failed).0, 0);
    assert_eq!(failed.health().failed_inputs(), 1);
}
