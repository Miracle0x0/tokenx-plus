use super::*;
use serde_json::json;
use std::io::Write;

fn entry(kind: &str, payload: Value) -> Value {
    json!({"type":kind,"timestamp":"2026-10-18T00:00:00Z","payload":payload})
}

fn usage(input: i64, output: i64) -> Value {
    json!({"input_tokens":input,"output_tokens":output})
}

fn response(id: &str, input: i64, output: i64) -> Value {
    entry(
        "token_usage_record",
        json!({"response_id":id,"thread_id":"session","turn_id":"turn","usage":usage(input,output)}),
    )
}

fn count(total: Value, last: Value) -> Value {
    entry(
        "event_msg",
        json!({"type":"token_count","turn_id":"turn","info":{"total_token_usage":total,"last_token_usage":last}}),
    )
}

fn terminal(kind: &str) -> Value {
    entry("event_msg", json!({"type":kind,"turn_id":"turn"}))
}

fn file(rows: Vec<Value>) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    for row in [
        entry(
            "session_meta",
            json!({"id":"session","model_provider":"openai"}),
        ),
        entry("turn_context", json!({"model":"gpt-5","turn_id":"turn"})),
    ]
    .into_iter()
    .chain(rows)
    {
        writeln!(file, "{row}").unwrap();
    }
    file
}

fn parse(file: &Path) -> ParsedCodexFile {
    parse_codex_file_incremental(file, 0, CodexParseState::default()).unwrap()
}

fn total(parsed: &ParsedCodexFile) -> i64 {
    assert!(parsed.interrupted.is_none(), "{:?}", parsed.interrupted);
    parsed.messages.iter().map(|m| m.tokens.total()).sum()
}

#[test]
fn compaction_counts_only_the_unconfirmed_response() {
    for confirmed in [false, true] {
        let file = file(vec![
            count(usage(100, 20), usage(100, 20)),
            response("compact", 50, 10),
            entry("compacted", json!({"message":"summary"})),
            if confirmed {
                count(usage(150, 30), usage(50, 10))
            } else {
                count(usage(100, 20), usage(100, 20))
            },
        ]);
        let parsed = parse(file.path());
        assert_eq!(total(&parsed), 180);
        assert_eq!(parsed.messages.len(), 2);
    }
}

#[test]
fn terminal_events_settle_only_the_matching_turn() {
    for kind in ["turn_aborted", "task_complete"] {
        for turn in [Some("turn"), None, Some("other")] {
            let file = file(vec![
                response("response", 100, 20),
                entry("event_msg", json!({"type":kind,"turn_id":turn})),
            ]);
            assert_eq!(
                total(&parse(file.path())),
                if turn == Some("other") { 0 } else { 120 }
            );
        }
    }
}

#[test]
fn confirmed_and_repeated_response_records_count_once() {
    let record = response("response", 100, 20);
    let file = file(vec![
        record.clone(),
        record.clone(),
        count(usage(100, 20), usage(100, 20)),
        record,
        terminal("task_complete"),
    ]);
    assert_eq!(total(&parse(file.path())), 120);
}

#[test]
fn pending_and_settled_responses_survive_serialized_append_state() {
    let mut file = file(vec![response("response", 100, 20)]);
    let pending = parse(file.path());
    assert_eq!(total(&pending), 0);
    let state = bincode::deserialize(&bincode::serialize(&pending.state).unwrap()).unwrap();
    writeln!(file, "{}", terminal("task_complete")).unwrap();
    let settled =
        parse_codex_file_incremental(file.path(), pending.consumed_offset, state).unwrap();
    assert_eq!(total(&settled), 120);
    let state = bincode::deserialize(&bincode::serialize(&settled.state).unwrap()).unwrap();
    writeln!(file, "{}", count(usage(100, 20), usage(100, 20))).unwrap();
    let late = parse_codex_file_incremental(file.path(), settled.consumed_offset, state).unwrap();
    assert_eq!(total(&late), 0);
    assert_eq!(total(&parse(file.path())), 120);
}

#[test]
fn different_following_response_does_not_hide_compaction() {
    let file = file(vec![
        response("compact", 50, 10),
        entry("compacted", json!({})),
        count(usage(100, 20), usage(100, 20)),
    ]);
    assert_eq!(total(&parse(file.path())), 180);
}

#[test]
fn foreign_thread_and_invalid_response_records_do_not_change_pending_state() {
    let mut foreign = response("foreign", 900, 10);
    foreign["payload"]["thread_id"] = json!("other-thread");
    let mut invalid = response("invalid", -100, 20);
    invalid["payload"]["model"] = json!("wrong-model");
    let file = file(vec![
        response("response", 100, 20),
        foreign,
        invalid,
        terminal("task_complete"),
    ]);
    let parsed = parse(file.path());
    assert_eq!(total(&parsed), 120);
    assert_eq!(parsed.messages[0].model_id.as_ref(), "gpt-5");
    assert_eq!(parsed.rejections.total(), 1);
}

#[test]
fn request_tier_and_turn_marker_survive_confirmation() {
    let mut record = response("response", 100, 20);
    record["payload"]["service_tier"] = json!("priority");
    for confirmed in [false, true] {
        let file = file(vec![
            entry(
                "event_msg",
                json!({"type":"user_message","message":"hello"}),
            ),
            record.clone(),
            if confirmed {
                count(usage(100, 20), usage(100, 20))
            } else {
                terminal("task_complete")
            },
        ]);
        let parsed = parse(file.path());
        assert_eq!(total(&parsed), 120);
        assert_eq!(parsed.messages[0].service_tier.as_deref(), Some("priority"));
        assert!(parsed.messages[0].is_turn_start);
    }
}

#[test]
fn next_turn_settles_with_original_model() {
    let file = file(vec![
        response("response", 100, 20),
        entry("turn_context", json!({"model":"gpt-6","turn_id":"next"})),
    ]);
    let parsed = parse(file.path());
    assert_eq!(total(&parsed), 120);
    assert_eq!(parsed.messages[0].model_id.as_ref(), "gpt-5");
}

#[test]
fn response_model_is_used_when_its_counter_does_not_repeat_it() {
    let mut record = response("own-model", 100, 20);
    record["payload"]["model"] = json!("gpt-6");
    let file = file(vec![record, count(usage(100, 20), usage(100, 20))]);
    let parsed = parse(file.path());
    assert_eq!(total(&parsed), 120);
    assert_eq!(parsed.messages[0].model_id.as_ref(), "gpt-6");
}

#[test]
fn compressed_and_plain_response_accounting_agree() {
    let plain = file(vec![
        response("compact", 50, 10),
        entry("compacted", json!({})),
        terminal("task_complete"),
    ]);
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("rollout.jsonl.zst");
    let encoded = zstd::stream::encode_all(std::fs::File::open(plain.path()).unwrap(), 1).unwrap();
    std::fs::write(&archive, encoded).unwrap();
    let decoded = parse_codex_archive(&archive, None).unwrap();
    assert_eq!(
        decoded
            .messages
            .iter()
            .map(|m| m.tokens.total())
            .sum::<i64>(),
        total(&parse(plain.path()))
    );
    assert_eq!(
        decoded.messages[0].dedup_key,
        parse(plain.path()).messages[0].dedup_key
    );
}

#[test]
fn replay_gate_excludes_parent_response_records() {
    let file = file(vec![
        entry(
            "session_meta",
            json!({"id":"child","forked_from_id":"parent"}),
        ),
        response("parent-response", 100, 20),
        terminal("task_complete"),
    ]);
    let parsed = parse(file.path());
    assert_eq!(total(&parsed), 0);
    assert!(parsed.state.pending_response.is_none());
}

#[test]
fn contradictory_counter_identity_does_not_discard_pending_usage() {
    for field in ["turn_id", "model"] {
        let mut counter = count(usage(200, 40), usage(200, 40));
        counter["payload"][field] = json!(if field == "model" { "gpt-6" } else { "other" });
        let file = file(vec![response("original", 100, 20), counter]);
        assert_eq!(total(&parse(file.path())), 360);
    }
}

#[test]
fn response_timestamp_errors_are_visible_and_do_not_settle_previous_usage() {
    let mut invalid = response("missing-time", 900, 10);
    invalid.as_object_mut().unwrap().remove("timestamp");
    let file = file(vec![
        response("original", 100, 20),
        invalid,
        terminal("task_complete"),
    ]);
    let parsed = parse(file.path());
    assert_eq!(total(&parsed), 120);
    assert_eq!(parsed.rejections.total(), 1);
}

#[test]
fn late_confirmation_remains_deduplicated_after_many_settlements() {
    let mut rows = Vec::new();
    for index in 0..24 {
        rows.push(response(&format!("response-{index}"), 100 + index, 20));
        rows.push(terminal("task_complete"));
    }
    rows.push(count(usage(100, 20), usage(100, 20)));
    let file = file(rows);
    assert_eq!(
        total(&parse(file.path())),
        (0..24).map(|i| 120 + i).sum::<i64>()
    );
}
