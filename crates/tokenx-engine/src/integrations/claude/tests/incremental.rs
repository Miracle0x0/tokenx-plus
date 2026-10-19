use super::*;
use crate::pricing::{ModelPricing, PricingService, ResolvedPricingSnapshot};
use crate::{
    AcquisitionConfig, AcquisitionEngine, CalendarContext, ClientId, ClientSelection,
    ClientUniverse, DateRange, Generation, GroupBy, PricingContext,
};
use serde_json::{json, Value};
use std::{collections::HashMap, io::Write, path::PathBuf, sync::Arc};

struct Fixture {
    home: tempfile::TempDir,
    cache: tempfile::TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new(rows: &[Value]) -> Self {
        let home = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = home
            .path()
            .join(".claude/projects/-work-project/session.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let fixture = Self { home, cache, path };
        fixture.append(&[json!({"type":"system","cwd":"/work/project"})]);
        fixture.append(rows);
        fixture
    }

    fn append(&self, rows: &[Value]) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .unwrap();
        for row in rows {
            writeln!(file, "{row}").unwrap();
        }
    }

    fn acquire(&self, cache: &Path) -> Generation {
        let rates = HashMap::from([(
            "claude-sonnet-4.6".to_string(),
            ModelPricing {
                input_cost_per_token: Some(1.0),
                output_cost_per_token: Some(1.0),
                cache_creation_input_token_cost: Some(1.25),
                cache_creation_input_token_cost_above_1hr: Some(2.0),
                ..Default::default()
            },
        )]);
        let pricing = Arc::new(ResolvedPricingSnapshot::explicit(
            PricingContext::explicit_with_catalog("test", "test"),
            Some(Arc::new(PricingService::new(rates, HashMap::new()))),
            Vec::new(),
        ));
        let config = AcquisitionConfig::new(
            self.home.path().to_owned(),
            DateRange::none(),
            ClientUniverse::new([ClientId::Claude]).unwrap(),
            Default::default(),
            CalendarContext::explicit("UTC").unwrap(),
            pricing.context().clone(),
        )
        .unwrap();
        AcquisitionEngine::new(config, pricing, cache.to_owned())
            .unwrap()
            .acquire()
            .unwrap()
    }

    fn load(&self) -> Generation {
        self.acquire(self.cache.path())
    }

    fn decoded(&self) -> usize {
        *DECODED_LINES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .get(&self.path)
            .unwrap_or(&0)
    }

    fn assert_full_equivalent(&self, incremental: &Generation) {
        let fresh = tempfile::tempdir().unwrap();
        let full = self.acquire(fresh.path());
        let selection = ClientSelection::new([ClientId::Claude]).unwrap();
        assert_eq!(
            incremental
                .project_models(&selection, GroupBy::WorkspaceModel)
                .unwrap(),
            full.project_models(&selection, GroupBy::WorkspaceModel)
                .unwrap()
        );
        assert_eq!(
            incremental.health().rejected_records(),
            full.health().rejected_records()
        );
        assert_eq!(incremental.sessions(), full.sessions());
    }
}

fn assistant(id: &str, input: i64, output: i64, write: i64, hour: Option<i64>) -> Value {
    json!({"type":"assistant","timestamp":"2026-10-18T00:00:00Z","sessionId":"session","requestId":id,
        "message":{"id":id,"model":"claude-sonnet-4.6","usage":{"input_tokens":input,"output_tokens":output,
            "cache_creation_input_tokens":write,"cache_creation":{"ephemeral_1h_input_tokens":hour}}}})
}

fn snapshot(input: i64) -> Value {
    json!({"type":"cost-state","sessionId":"session","startTime":1792281600000_i64,
        "modelUsage":{"claude-sonnet-4.6":{"inputTokens":input,"outputTokens":0,"cacheReadInputTokens":0,"cacheCreationInputTokens":0}}})
}

#[test]
fn claude_append_decodes_only_tail_and_keeps_unclamped_streaming_metadata() {
    let fixture = Fixture::new(&[
        json!({"type":"user","message":{"content":"hello"}}),
        assistant("stream", 10, 5, 1, Some(7)),
    ]);
    let first = fixture.load();
    let before = fixture.decoded();
    let warm = fixture.load();
    assert_eq!(fixture.decoded(), before);
    assert_eq!(first.sessions(), warm.sessions());
    fixture.append(&[assistant("stream", 10, 9, 8, None)]);
    let appended = fixture.load();
    assert_eq!(fixture.decoded() - before, 1);
    fixture.assert_full_equivalent(&appended);
    let total = appended
        .project_models(
            &ClientSelection::new([ClientId::Claude]).unwrap(),
            GroupBy::Model,
        )
        .unwrap();
    assert_eq!(total.total_cost, 34.25);
}

#[test]
fn claude_append_preserves_zero_records_pending_turns_and_replaces_cost_snapshots() {
    let fixture = Fixture::new(&[
        json!({"type":"user","message":{"content":"hello"}}),
        assistant("zero", 0, 0, 0, None),
        snapshot(100),
    ]);
    fixture.load();
    let before = fixture.decoded();
    fixture.append(&[assistant("zero",10,2,0,None),json!({"type":"cost-state","sessionId":"session","startTime":1792281600000_i64,"modelUsage":{}})]);
    let appended = fixture.load();
    assert_eq!(fixture.decoded() - before, 2);
    fixture.assert_full_equivalent(&appended);
    assert_eq!(appended.sessions()[0].turn_count, 1);
}

#[test]
fn claude_prefix_rewrite_and_related_input_change_require_full_decode() {
    let fixture = Fixture::new(&[assistant("a", 10, 1, 0, None)]);
    fixture.load();
    let original = std::fs::read_to_string(&fixture.path).unwrap();
    let before = fixture.decoded();
    std::fs::write(
        &fixture.path,
        original.replace("\"input_tokens\":10", "\"input_tokens\":20"),
    )
    .unwrap();
    fixture.append(&[assistant("b", 30, 2, 0, None)]);
    let rewritten = fixture.load();
    assert_eq!(fixture.decoded() - before, 3);
    fixture.assert_full_equivalent(&rewritten);
    let before = fixture.decoded();
    std::fs::write(
        fixture.path.with_extension("meta.json"),
        "{\"agentType\":\"plan\"}",
    )
    .unwrap();
    fixture.append(&[assistant("c", 40, 3, 0, None)]);
    let changed = fixture.load();
    assert_eq!(fixture.decoded() - before, 4);
    fixture.assert_full_equivalent(&changed);
}

#[test]
fn claude_malformed_snapshot_rejection_is_replaced_by_later_snapshot() {
    let fixture = Fixture::new(&[
        assistant("a", 10, 1, 0, None),
        json!({"type":"cost-state","modelUsage":null}),
    ]);
    assert_eq!(fixture.load().health().rejected_records(), 1);
    fixture.append(&[snapshot(30)]);
    let appended = fixture.load();
    assert_eq!(appended.health().rejected_records(), 0);
    fixture.assert_full_equivalent(&appended);
}

#[test]
fn claude_incomplete_line_and_corrupt_shard_reparse_authoritative_input() {
    let fixture = Fixture::new(&[assistant("a", 10, 1, 0, None)]);
    fixture.load();
    let shard = walkdir::WalkDir::new(fixture.cache.path())
        .into_iter()
        .map(Result::unwrap)
        .find(|entry| entry.path().extension().is_some_and(|e| e == "bin"))
        .unwrap()
        .into_path();
    let mut bytes = std::fs::read(&shard).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(shard, bytes).unwrap();
    let before = fixture.decoded();
    let recovered = fixture.load();
    assert_eq!(fixture.decoded() - before, 2);
    assert!(!recovered.health().complete());
    fixture.assert_full_equivalent(&recovered);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.path)
        .unwrap();
    write!(file, "{{\"type\":\"assistant\"").unwrap();
    let partial = fixture.load();
    assert!(!partial.health().complete());
    write!(file,",\"timestamp\":\"2026-10-18T00:00:00Z\",\"message\":{{\"model\":\"claude-sonnet-4.6\",\"usage\":{{\"input_tokens\":5,\"output_tokens\":1}}}}}}\n").unwrap();
    let repaired = fixture.load();
    fixture.assert_full_equivalent(&repaired);
    assert!(repaired.health().complete());
}

#[test]
fn claude_append_carries_pending_user_turn_and_reprojects_existing_workspace() {
    let fixture = Fixture::new(&[
        assistant("first", 10, 1, 0, None),
        json!({"type":"user","message":{"content":"continue"}}),
    ]);
    let original = fixture.load();
    let before = fixture.decoded();
    fixture.append(&[
        json!({"type":"system","cwd":"/work-project"}),
        assistant("second", 20, 2, 0, None),
    ]);
    let appended = fixture.load();
    assert_eq!(fixture.decoded() - before, 2);
    assert_eq!(appended.sessions()[0].turn_count, 1);
    assert_ne!(
        original.sessions()[0].workspace_key,
        appended.sessions()[0].workspace_key
    );
    fixture.assert_full_equivalent(&appended);
}

#[test]
fn claude_unterminated_valid_line_uses_whole_file_reparse_on_growth() {
    let fixture = Fixture::new(&[assistant("first", 10, 1, 0, None)]);
    let mut text = std::fs::read_to_string(&fixture.path).unwrap();
    assert_eq!(text.pop(), Some('\n'));
    std::fs::write(&fixture.path, text).unwrap();
    fixture.load();
    let before = fixture.decoded();
    let mut writer = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.path)
        .unwrap();
    writeln!(writer).unwrap();
    fixture.append(&[assistant("second", 20, 2, 0, None)]);
    let appended = fixture.load();
    assert_eq!(fixture.decoded() - before, 3);
    fixture.assert_full_equivalent(&appended);
}
