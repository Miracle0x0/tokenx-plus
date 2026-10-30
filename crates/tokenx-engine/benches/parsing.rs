use codspeed_criterion_compat::{
    black_box, criterion_group, criterion_main, BatchSize, Bencher, BenchmarkId, Criterion,
    Throughput,
};
use serde_json::json;
use std::io::Write;
use std::sync::Arc;
use tempfile::TempDir;
use tokenx_engine::{
    pricing::ResolvedPricingSnapshot, scanner::ScannerSettings, AcquisitionConfig,
    AcquisitionEngine, CalendarContext, ClientId, ClientUniverse, DateRange, PricingContext,
};

fn engine(home: &TempDir, cache: &TempDir, client: ClientId) -> AcquisitionEngine {
    let pricing = Arc::new(ResolvedPricingSnapshot::explicit(
        PricingContext::explicit_with_catalog("benchmark", "benchmark"),
        None,
        Vec::new(),
    ));
    let config = AcquisitionConfig::new(
        home.path().to_owned(),
        DateRange::none(),
        ClientUniverse::new([client]).unwrap(),
        ScannerSettings::default(),
        CalendarContext::explicit("UTC").unwrap(),
        pricing.context().clone(),
    )
    .unwrap();
    AcquisitionEngine::new(config, pricing, cache.path().to_owned()).unwrap()
}

fn codex_home(responses: usize) -> TempDir {
    let home = TempDir::new().unwrap();
    let sessions = home.path().join(".codex/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let mut file = std::fs::File::create(sessions.join("long-session.jsonl")).unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"session_meta","payload":{"id":"session"}})
    )
    .unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"turn_context","payload":{"model":"gpt-5","turn_id":"turn"}})
    )
    .unwrap();
    for index in 0..responses {
        writeln!(
            file,
            "{}",
            json!({
                "type":"token_usage_record", "timestamp":"2026-10-18T00:00:00Z",
                "payload":{"response_id":format!("response-{index}"),"turn_id":"turn",
                    "usage":{"input_tokens":100,"output_tokens":20}}
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn"}})
        )
        .unwrap();
    }
    home
}

fn dsh_home() -> TempDir {
    let home = TempDir::new().unwrap();
    let session = home.path().join(".dsh/sessions/session");
    std::fs::create_dir_all(&session).unwrap();
    let mut file = std::fs::File::create(session.join("session.jsonl")).unwrap();
    let body = json!({"text":"tool output ".repeat(1024),"metadata":[0,1,2,3]});
    for seq in 0..256 {
        writeln!(
            file,
            "{}",
            json!({
                "type":"assistant/message", "time":1786669450000_i64 + seq, "seq":seq,
                "data":{"message":{"source":{"model":"deepseek-v4-flash"},
                    "content":[body,body,body,body]},"usage":{"inputTokens":100,"outputTokens":20}}
            })
        )
        .unwrap();
    }
    home
}

fn validate_fixture(home: &TempDir, client: ClientId, expected_tokens: u64) {
    let cache = TempDir::new().unwrap();
    let generation = engine(home, &cache, client).acquire().unwrap();
    assert!(!generation.health().requires_input_retry());
    assert_eq!(generation.health().rejected_records(), 0);
    assert_eq!(
        generation
            .sessions()
            .iter()
            .map(|session| session.tokens.checked_total().unwrap())
            .sum::<u64>(),
        expected_tokens
    );
}

fn bench_cold_build(b: &mut Bencher<'_>, home: &TempDir, client: ClientId) {
    b.iter_batched(
        || {
            let cache = TempDir::new().unwrap();
            let engine = engine(home, &cache, client);
            let prepared = engine.prepare().unwrap();
            (cache, engine, prepared)
        },
        |(cache, engine, prepared)| {
            let generation = engine.build(prepared).unwrap();
            // Criterion drops outputs after stopping the timer. Keep the cache
            // and worker pool alive so cleanup is excluded from acquisition.
            black_box((generation, engine, cache))
        },
        BatchSize::PerIteration,
    );
}

fn parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_parsing");
    for responses in [1_000, 4_000, 8_000] {
        let home = codex_home(responses);
        validate_fixture(&home, ClientId::Codex, responses as u64 * 120);
        group.throughput(Throughput::Elements(responses as u64));
        group.bench_with_input(
            BenchmarkId::new("codex_responses", responses),
            &home,
            |b, home| bench_cold_build(b, home, ClientId::Codex),
        );
    }
    let home = dsh_home();
    validate_fixture(&home, ClientId::Dsh, 256 * 120);
    group.throughput(Throughput::Elements(256));
    group.bench_function("dsh_large_bodies", |b| {
        bench_cold_build(b, &home, ClientId::Dsh);
    });
    group.finish();
}

criterion_group!(benches, parsing);
criterion_main!(benches);
