#![cfg(test)]

use super::*;
use std::io::{BufRead, Cursor, Error, ErrorKind, Seek, SeekFrom, Write};
use tempfile::NamedTempFile;

#[test]
fn codex_human_turn_matches_only_known_system_tags() {
    // Real human prompts that happen to start with markup must still count.
    assert!(codex_message_is_human_turn(Some(
        "how do I center a <div>?"
    )));
    assert!(codex_message_is_human_turn(Some("<div>hi</div>")));
    assert!(codex_message_is_human_turn(Some("  plain question")));
    // Known system-injected context blocks are not human turns.
    assert!(!codex_message_is_human_turn(Some(
        "<environment_context>cwd=/tmp</environment_context>"
    )));
    assert!(!codex_message_is_human_turn(Some(
        "  <system-reminder>be concise</system-reminder>"
    )));
    assert!(!codex_message_is_human_turn(Some(
        "<user_instructions>do X</user_instructions>"
    )));
    // A missing body is never a human turn.
    assert!(!codex_message_is_human_turn(None));
}

fn create_test_file(content: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(content.as_bytes()).unwrap();
    file.flush().unwrap();
    file
}

fn parse_codex_file(path: &Path) -> Vec<UsageRecord> {
    super::parse_codex_file(path).expect("test fixture must be valid Codex JSONL")
}

fn parse_codex_file_incremental(
    path: &Path,
    start_offset: u64,
    state: CodexParseState,
) -> ParsedCodexFile {
    super::parse_codex_file_incremental(path, start_offset, state)
        .expect("test fixture must be valid incremental Codex JSONL")
}

fn cache_write_snapshot(total: Value, last: Value) -> String {
    serde_json::json!({
        "timestamp": "2026-09-06T03:19:35Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.6-sol",
                "total_token_usage": total,
                "last_token_usage": last
            }
        }
    })
    .to_string()
}

fn tier_setting(tier: Value) -> String {
    serde_json::json!({"type": "event_msg", "payload": {
        "type": "thread_settings_applied", "thread_settings": {"service_tier": tier}
    }})
    .to_string()
}

fn tier_usage(total: i64) -> String {
    cache_write_snapshot(
        serde_json::json!({"input_tokens": total}),
        serde_json::json!({"input_tokens": 10}),
    )
}

#[test]
fn service_tier_settings_are_prospective_and_survive_incremental_state() {
    let mut file = create_test_file(&format!(
        "{}\n{}\n{}\n",
        tier_usage(10),
        tier_setting("priority".into()),
        tier_usage(20)
    ));
    let first = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());
    assert_eq!(first.messages[0].service_tier, None);
    assert_eq!(first.messages[1].service_tier.as_deref(), Some("priority"));
    let state = bincode::deserialize(&bincode::serialize(&first.state).unwrap()).unwrap();
    writeln!(
        file,
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        tier_usage(20), // Duplicate snapshot remains ignored.
        tier_usage(30),
        tier_setting("default".into()),
        tier_usage(40),
        tier_setting("fast".into()),
        tier_usage(50),
        tier_setting(Value::Null),
        tier_usage(60),
        tier_setting("future-tier".into()),
    )
    .unwrap();
    writeln!(file, "{}", tier_usage(70)).unwrap();
    file.flush().unwrap();
    let next = parse_codex_file_incremental(file.path(), first.consumed_offset, state);
    assert!(next.interrupted.is_none());
    let tiers = next
        .messages
        .iter()
        .map(|r| r.service_tier.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(
        tiers,
        [
            Some("priority"),
            Some("default"),
            Some("fast"),
            None,
            Some("future-tier")
        ]
    );
    let full = parse_codex_file(file.path());
    assert_eq!(full, [first.messages, next.messages].concat());
}

#[test]
fn malformed_service_tier_settings_interrupt_before_later_usage() {
    for settings in [serde_json::json!({"service_tier": 7}), Value::Null] {
        let bad = serde_json::json!({"type":"event_msg", "payload": {
            "type":"thread_settings_applied", "thread_settings":settings
        }});
        let file = create_test_file(&format!("{}\n{bad}\n{}\n", tier_usage(10), tier_usage(20)));
        let parsed = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());
        assert!(parsed.interrupted.is_some());
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.rejections.total(), 1);
    }
}

#[test]
fn inherited_fork_settings_do_not_set_the_child_service_tier() {
    let file = create_test_file(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        r#"{"type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}}"#,
        tier_setting("priority".into()),
        tier_usage(10),
        r#"{"type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
        tier_usage(20),
        tier_setting("flex".into()),
        tier_usage(30)
    ));
    let parsed = parse_codex_file(file.path());
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].service_tier, None);
    assert_eq!(parsed[1].service_tier.as_deref(), Some("flex"));
}

#[test]
fn cache_write_uses_last_usage_and_distinguishes_cumulative_snapshots() {
    let last = serde_json::json!({
        "input_tokens": 100, "cached_input_tokens": 40,
        "cache_write_input_tokens": 20, "output_tokens": 10,
        "reasoning_output_tokens": 5
    });
    let mut total = last.clone();
    total["input_tokens"] = 1000.into();
    let first = cache_write_snapshot(total.clone(), last.clone());
    // A snapshot that differs only in cumulative cache writes is distinct.
    total["cache_write_input_tokens"] = 40.into();
    let second = cache_write_snapshot(total, last);
    let file = create_test_file(&format!("{first}\n{first}\n{second}\n"));

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 2);
    for message in &messages {
        assert_eq!(
            message.tokens,
            TokenBreakdown {
                input: 40,
                output: 10,
                cache_read: 40,
                cache_write: 20,
                cache_write_1h: 0,
                reasoning: 5
            }
        );
    }
    assert_ne!(messages[0].dedup_key, messages[1].dedup_key);
}

#[test]
fn cache_write_missing_field_means_zero() {
    let usage = serde_json::json!({
        "input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 10
    });
    let file = create_test_file(&cache_write_snapshot(usage.clone(), usage));
    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.input, 60);
    assert_eq!(messages[0].tokens.cache_read, 40);
    assert_eq!(messages[0].tokens.cache_write, 0);
}

#[test]
fn cache_write_only_usage_is_not_an_empty_snapshot() {
    let usage = serde_json::json!({
        "input_tokens": 100, "cache_write_input_tokens": 100
    });
    let file = create_test_file(&cache_write_snapshot(usage.clone(), usage));
    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].tokens,
        TokenBreakdown {
            cache_write: 100,
            ..Default::default()
        }
    );
}

#[test]
fn cache_write_invalid_buckets_are_rejected_beside_valid_usage() {
    let good = serde_json::json!({
        "input_tokens": 100, "cached_input_tokens": 40,
        "cache_write_input_tokens": 20, "output_tokens": 10
    });
    let invalid = [
        serde_json::json!({"input_tokens": 100, "cache_write_input_tokens": -1}),
        serde_json::json!({"input_tokens": 100, "cache_write_input_tokens": 101}),
        serde_json::json!({"input_tokens": 100, "cached_input_tokens": 80, "cache_write_input_tokens": 21}),
        serde_json::json!({"input_tokens": 100, "cache_read_input_tokens": 80, "cache_write_input_tokens": 21}),
        serde_json::json!({"input_tokens": i64::MAX, "cached_input_tokens": i64::MAX, "cache_write_input_tokens": 1}),
        serde_json::json!({"input_tokens": i64::MAX, "cache_write_input_tokens": 1}),
    ];
    for invalid_usage in invalid {
        for (total, last) in [
            (invalid_usage.clone(), good.clone()),
            (good.clone(), invalid_usage.clone()),
        ] {
            let bad_row = cache_write_snapshot(total, last);
            let good_row = cache_write_snapshot(good.clone(), good.clone());
            let file = create_test_file(&format!("{bad_row}\n{good_row}\n"));
            let parsed = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());

            assert_eq!(parsed.rejections.total(), 1, "{bad_row}");
            assert!(parsed.interrupted.is_none(), "{bad_row}");
            assert_eq!(parsed.messages.len(), 1, "{bad_row}");
            assert_eq!(parsed.messages[0].tokens.cache_write, 20);
        }
    }
}

#[test]
fn cache_write_regression_keeps_watermark_until_recovery_or_reset() {
    let first = serde_json::json!({
        "input_tokens": 1000, "cache_write_input_tokens": 400
    });
    let stale = serde_json::json!({
        "input_tokens": 1010, "cache_write_input_tokens": 390
    });
    let recovered = serde_json::json!({
        "input_tokens": 1020, "cache_write_input_tokens": 410
    });
    let increment = serde_json::json!({
        "input_tokens": 20, "cache_write_input_tokens": 10
    });
    let reset = serde_json::json!({
        "input_tokens": 100, "cache_write_input_tokens": 20
    });
    let rows = [
        cache_write_snapshot(first.clone(), first),
        cache_write_snapshot(stale, increment.clone()),
        cache_write_snapshot(recovered, increment),
        cache_write_snapshot(reset.clone(), reset),
    ];
    let file = create_test_file(&rows.join("\n"));
    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages
            .iter()
            .map(|row| row.tokens.cache_write)
            .collect::<Vec<_>>(),
        vec![400, 10, 20]
    );
    assert_eq!(
        messages
            .iter()
            .map(|row| row.tokens.input)
            .collect::<Vec<_>>(),
        vec![600, 10, 80]
    );
}

#[test]
fn cache_write_growth_exceeds_fork_inherited_bucket_baseline() {
    let inherited = serde_json::json!({
        "input_tokens": 1000, "cache_write_input_tokens": 400
    });
    let mut own = inherited.clone();
    own["cache_write_input_tokens"] = 420.into();
    let own_last = serde_json::json!({
        "input_tokens": 20, "cache_write_input_tokens": 20
    });
    // Without a reported total, inherited history is compared by buckets.
    let file = create_test_file(&format!(
        "{}\n{}\n{}\n{}\n{}\n",
        r#"{"type":"session_meta","payload":{"id":"child","forked_from_id":"parent","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}}"#,
        cache_write_snapshot(inherited.clone(), inherited.clone()),
        r#"{"type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
        cache_write_snapshot(inherited.clone(), inherited),
        cache_write_snapshot(own, own_last),
    ));
    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.cache_write, 20);
    assert_eq!(messages[0].tokens.input, 0);
    assert!(!messages[0].is_main_session);
}

#[test]
fn cache_write_survives_serialized_incremental_state() {
    let first = serde_json::json!({
        "input_tokens": 100, "cached_input_tokens": 40, "cache_write_input_tokens": 20
    });
    let first_row = cache_write_snapshot(first.clone(), first);
    let file = create_test_file(&format!("{first_row}\n"));
    let initial = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());
    let state = bincode::deserialize(&bincode::serialize(&initial.state).unwrap()).unwrap();
    let total = serde_json::json!({
        "input_tokens": 150, "cached_input_tokens": 50, "cache_write_input_tokens": 50
    });
    let last = serde_json::json!({
        "input_tokens": 50, "cached_input_tokens": 10, "cache_write_input_tokens": 30
    });
    let second_row = cache_write_snapshot(total, last);
    let mut appended = file.reopen().unwrap();
    appended.seek(SeekFrom::End(0)).unwrap();
    writeln!(appended, "{first_row}\n{second_row}").unwrap();
    appended.flush().unwrap();

    let incremental = super::parse_codex_file_incremental_verified(
        file.path(),
        initial.consumed_offset,
        state,
        initial.content_hash.unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(incremental.messages.len(), 1);
    assert_eq!(incremental.messages[0].tokens.cache_write, 30);
    assert_eq!(incremental.messages[0].tokens.input, 10);
    let mut combined = initial.messages;
    combined.extend(incremental.messages);
    assert_eq!(combined, parse_codex_file(file.path()));
}

struct FailAfterFirstLine {
    inner: Cursor<Vec<u8>>,
    fail_next_read: bool,
}

impl FailAfterFirstLine {
    fn new(contents: &str) -> Self {
        Self {
            inner: Cursor::new(contents.as_bytes().to_vec()),
            fail_next_read: false,
        }
    }
}

impl std::io::Read for FailAfterFirstLine {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl BufRead for FailAfterFirstLine {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt);
    }

    fn read_line(&mut self, buf: &mut String) -> std::io::Result<usize> {
        if self.fail_next_read {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "synthetic line decode failure",
            ));
        }
        let bytes_read = self.inner.read_line(buf)?;
        if bytes_read > 0 {
            self.fail_next_read = true;
        }
        Ok(bytes_read)
    }
}

#[test]
fn test_missing_input_returns_open_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("missing.jsonl");

    let error = super::parse_codex_file(&path).expect_err("missing input must fail");

    assert!(error.to_string().contains("open Codex JSONL input"));
}

#[test]
fn test_structurally_malformed_non_state_entry_is_rejected() {
    let file = create_test_file(r#"{"type":7,"payload":{}}"#);

    let parsed =
        super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default()).unwrap();

    assert!(parsed.messages.is_empty());
    assert!(parsed.interrupted.is_none());
    assert_eq!(parsed.rejections.total(), 1);
    assert!(super::parse_codex_file(file.path()).unwrap().is_empty());
}

#[test]
fn cancelled_reader_stops_before_consuming_the_next_record() {
    let cancellation = crate::engine::AcquisitionCancellation::default();
    cancellation.cancel();
    let mut reader = Cursor::new(b"{\"type\":\"event_msg\"}\n".as_slice());

    let parsed = parse_codex_reader(
        &mut reader,
        "session",
        0,
        CodexParseState::default(),
        Some(&cancellation),
    )
    .unwrap();

    assert!(parsed.messages.is_empty());
    assert_eq!(parsed.consumed_offset, 0);
    let interrupted = parsed.interrupted.unwrap();
    assert_eq!(interrupted.operation, "parse Codex JSONL input");
    assert_eq!(interrupted.message, "acquisition cancelled");
}

#[test]
fn test_verified_incremental_prefix_mismatch_is_a_cache_miss() {
    let file = create_test_file(FIRST_CODEX_ENTRY_FOR_PREFIX_TEST);

    let parsed = parse_codex_file_incremental_verified(
        file.path(),
        1,
        CodexParseState::default(),
        [0xff; 32],
    )
    .expect("prefix verification I/O must succeed");

    assert!(parsed.is_none());
}

const FIRST_CODEX_ENTRY_FOR_PREFIX_TEST: &str = concat!(
    r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
    "\n",
);

#[test]
fn structured_stdout_is_not_a_local_session_input() {
    let file = create_test_file(
        r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn.completed","model":"gpt-4o-mini","usage":{"input_tokens":120,"cached_input_tokens":20,"output_tokens":30}}"#,
    );

    let parsed =
        super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default()).unwrap();

    assert!(parsed.messages.is_empty());
    assert!(parsed.rejections.is_empty());
    assert!(parsed.interrupted.is_none());
}

#[test]
fn test_incremental_parse_matches_full_parse_for_appended_lines() {
    let file = create_test_file(concat!(
        r#"{"type":"session_meta","payload":{"source":"chat","model_provider":"openai","agent_nickname":"builder","cwd":"/Users/alice/codex-demo"}}"#,
        "\n",
        r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
        "\n"
    ));

    let initial_size = file.as_file().metadata().unwrap().len();
    let initial = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());
    assert_eq!(initial.messages.len(), 1);
    assert_eq!(initial.consumed_offset, initial_size);
    assert_eq!(
        initial.messages[0].workspace_key.as_deref(),
        Some("/Users/alice/codex-demo")
    );
    assert_eq!(
        initial.messages[0].workspace_label.as_deref(),
        Some("codex-demo")
    );

    let appended = concat!(
        r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15,"cached_input_tokens":3,"output_tokens":5},"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2}}}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":22,"cached_input_tokens":4,"output_tokens":7},"last_token_usage":{"input_tokens":7,"cached_input_tokens":1,"output_tokens":2}}}}"#,
        "\n"
    );

    let mut reopened = file.reopen().unwrap();
    reopened.seek(SeekFrom::End(0)).unwrap();
    reopened.write_all(appended.as_bytes()).unwrap();
    reopened.flush().unwrap();

    let incremental =
        parse_codex_file_incremental(file.path(), initial_size, initial.state.clone());
    let mut combined = initial.messages.clone();
    combined.extend(incremental.messages);
    assert_eq!(
        incremental.consumed_offset,
        file.as_file().metadata().unwrap().len()
    );

    let full = parse_codex_file(file.path());
    assert_eq!(combined, full);
    assert_eq!(
        combined
            .iter()
            .map(|msg| msg.workspace_key.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("/Users/alice/codex-demo"),
            Some("/Users/alice/codex-demo"),
            Some("/Users/alice/codex-demo")
        ]
    );
}

#[test]
fn test_token_count_before_turn_context_uses_later_model() {
    let file = create_test_file(concat!(
        r#"{"type":"session_meta","payload":{"source":"interactive","model_provider":"openai","agent_nickname":"builder","cwd":"/Users/alice/codex-demo"}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15,"cached_input_tokens":3,"output_tokens":5,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2,"reasoning_output_tokens":0}}}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:04Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":22,"cached_input_tokens":4,"output_tokens":7,"reasoning_output_tokens":2},"last_token_usage":{"input_tokens":7,"cached_input_tokens":1,"output_tokens":2,"reasoning_output_tokens":1}}}}"#,
        "\n"
    ));

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.model_id.as_ref())
            .collect::<Vec<_>>(),
        vec!["gpt-5.5", "gpt-5.5", "gpt-5.5"]
    );
    assert_eq!(
        messages
            .iter()
            .map(|message| message.workspace_key.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("/Users/alice/codex-demo"),
            Some("/Users/alice/codex-demo"),
            Some("/Users/alice/codex-demo")
        ]
    );
    assert_eq!(messages[0].tokens.input, 8);
    assert_eq!(messages[0].tokens.output, 3);
    assert_eq!(messages[0].tokens.cache_read, 2);
    assert_eq!(messages[0].tokens.reasoning, 1);
    assert_eq!(messages[1].tokens.input, 4);
    assert_eq!(messages[1].tokens.output, 2);
    assert_eq!(messages[1].tokens.cache_read, 1);
    assert_eq!(messages[1].tokens.reasoning, 0);
    assert_eq!(messages[2].tokens.input, 6);
    assert_eq!(messages[2].tokens.output, 2);
    assert_eq!(messages[2].tokens.cache_read, 1);
    assert_eq!(messages[2].tokens.reasoning, 1);

    let parsed = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());
    assert_eq!(parsed.messages.len(), messages.len());
}

#[test]
fn test_token_count_without_model_interrupts_incremental_scan() {
    let file = create_test_file(concat!(
        r#"{"type":"session_meta","payload":{"source":"interactive","model_provider":"openai"}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#,
        "\n"
    ));

    let parsed =
        super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default()).unwrap();

    assert!(parsed.messages.is_empty());
    assert_eq!(parsed.rejections.total(), 1);
    assert!(parsed
        .interrupted
        .unwrap()
        .message
        .contains("model was never identified"));
}

#[test]
fn test_parse_reader_returns_interrupted_outcome_on_line_read_error() {
    let mut reader = FailAfterFirstLine::new(concat!(
        r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
        "\n",
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
        "\n"
    ));

    let parsed =
        parse_codex_reader(&mut reader, "session", 0, CodexParseState::default(), None).unwrap();

    assert!(parsed.messages.is_empty());
    assert!(parsed.rejections.is_empty());
    assert_eq!(
        parsed.interrupted.unwrap().operation,
        "read Codex JSONL line"
    );
}

#[test]
fn test_parse_file_returns_error_on_invalid_utf8_line() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(
        concat!(
            r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
            "\n"
        )
        .as_bytes(),
    )
    .unwrap();
    file.write_all(&[0xff, b'\n']).unwrap();
    file.flush().unwrap();

    let full_error =
        super::parse_codex_file(file.path()).expect_err("invalid UTF-8 must fail the full parser");
    assert!(full_error.to_string().contains("read Codex JSONL line"));

    let incremental =
        super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default()).unwrap();
    assert_eq!(
        incremental.interrupted.unwrap().operation,
        "read Codex JSONL line"
    );
}

#[test]
fn test_parse_file_rejects_late_invalid_utf8_line() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(
            concat!(
                r#"{"timestamp":"2026-04-27T09:59:59Z","type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
                "\n",
                r#"{"timestamp":"2026-04-27T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
                "\n"
            )
            .as_bytes(),
        )
        .unwrap();
    file.write_all(&[0xff, b'\n']).unwrap();
    file.flush().unwrap();

    assert!(super::parse_codex_file(file.path()).is_err());
    let incremental =
        super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default()).unwrap();
    assert_eq!(incremental.messages.len(), 1);
    assert!(incremental.interrupted.is_some());
}

#[test]
fn test_session_meta_exec_marks_exec() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"originator":"codex_exec","source":"exec"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5.4","total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#;
    let content = format!("{}\n{}", line1, line2);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].agent.as_deref(), Some("Codex Exec"));
}

#[test]
fn test_token_count_uses_total_deltas_when_totals_repeat() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.input, 80);
    assert_eq!(messages[0].tokens.output, 30);
    assert_eq!(messages[0].tokens.cache_read, 20);
    assert_eq!(messages[0].tokens.reasoning, 5);
}

#[test]
fn test_token_count_uses_last_usage_when_totals_reset() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].tokens.input, 80);
    assert_eq!(messages[0].tokens.output, 30);
    assert_eq!(messages[0].tokens.cache_read, 20);
    assert_eq!(messages[0].tokens.reasoning, 5);
    assert_eq!(messages[1].tokens.input, 8);
    assert_eq!(messages[1].tokens.output, 3);
    assert_eq!(messages[1].tokens.cache_read, 2);
    assert_eq!(messages[1].tokens.reasoning, 1);
}

#[test]
fn test_token_count_rejects_missing_total_usage() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let error = super::parse_codex_file(file.path())
        .expect_err("current Codex token-count rows require total_token_usage");

    assert!(error.to_string().contains("total_token_usage is missing"));
}

#[test]
fn overflowing_token_snapshot_is_rejected_and_later_usage_survives() {
    let content = concat!(
        r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":9223372036854775807,"output_tokens":1},"last_token_usage":{"input_tokens":9223372036854775807,"output_tokens":1}}}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":2},"last_token_usage":{"input_tokens":10,"output_tokens":2}}}}"#,
        "\n",
    );
    let file = create_test_file(content);

    let parsed =
        super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default()).unwrap();

    assert!(parsed.interrupted.is_none());
    assert_eq!(parsed.rejections.total(), 1);
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].tokens.input, 10);
    assert_eq!(parsed.messages[0].tokens.output, 2);
}

#[test]
fn test_token_count_rejects_missing_last_usage() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5}}}}"#;
    let content = format!("{}\n{}", line1, line2);
    let file = create_test_file(&content);

    let error = super::parse_codex_file(file.path())
        .expect_err("current Codex token-count rows require last_token_usage");

    assert!(error.to_string().contains("last_token_usage is missing"));
}

#[test]
fn test_token_count_missing_info_is_ignored_without_blocking_later_usage() {
    for usage_unavailable in [
        r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#,
        r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count"}}"#,
    ] {
        let content = format!(
            "{}\n{}\n{usage_unavailable}\n{}\n",
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.4"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15,"cached_input_tokens":3,"output_tokens":5},"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2}}}}"#,
        );
        let file = create_test_file(&content);

        let parsed =
            super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default())
                .unwrap();

        assert_eq!(parsed.messages.len(), 2, "fixture: {usage_unavailable}");
        assert_eq!(parsed.messages[0].model_id.as_ref(), "gpt-5.4");
        assert_eq!(parsed.messages[1].tokens.input, 4);
        assert_eq!(parsed.messages[1].tokens.cache_read, 1);
        assert_eq!(parsed.messages[1].tokens.output, 2);
        assert_eq!(parsed.rejections.total(), 0);
        assert!(parsed.interrupted.is_none());
    }
}

#[test]
fn test_cached_tokens_above_input_are_rejected_beside_good_sibling() {
    let content = concat!(
        r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"cached_input_tokens":100,"output_tokens":30},"last_token_usage":{"input_tokens":50,"cached_input_tokens":100,"output_tokens":30}}}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
        "\n",
    );
    let file = create_test_file(content);

    let parsed =
        super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default()).unwrap();

    assert!(parsed.interrupted.is_none());
    assert_eq!(parsed.rejections.total(), 1);
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].tokens.input, 8);
    assert_eq!(parsed.messages[0].tokens.cache_read, 2);
}

#[test]
fn test_negative_total_and_last_usage_do_not_advance_baseline() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":105,"cached_input_tokens":21,"cache_read_input_tokens":-2,"output_tokens":31,"reasoning_output_tokens":5},"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":1,"reasoning_output_tokens":0}}}}"#;
    let line4 = r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":108,"cached_input_tokens":21,"output_tokens":32,"reasoning_output_tokens":5},"last_token_usage":{"input_tokens":8,"cached_input_tokens":2,"cache_read_input_tokens":-1,"output_tokens":2,"reasoning_output_tokens":0}}}}"#;
    let line5 = r#"{"timestamp":"2026-01-01T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":110,"cached_input_tokens":22,"output_tokens":33,"reasoning_output_tokens":6},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{line1}\n{line2}\n{line3}\n{line4}\n{line5}");
    let file = create_test_file(&content);

    let parsed =
        super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default()).unwrap();

    assert_eq!(parsed.messages.len(), 2);
    assert_eq!(parsed.messages[0].tokens.input, 80);
    assert_eq!(parsed.messages[1].tokens.input, 8);
    assert_eq!(parsed.messages[1].tokens.output, 3);
    assert_eq!(parsed.messages[1].tokens.cache_read, 2);
    assert_eq!(parsed.messages[1].tokens.reasoning, 1);
    assert_eq!(parsed.rejections.total(), 2);
    assert!(parsed.interrupted.is_none());
    let totals = parsed.state.previous_totals.unwrap();
    assert_eq!(totals.input, 110);
    assert_eq!(totals.output, 33);
    assert_eq!(totals.cached, 22);
    assert_eq!(totals.reasoning, 6);
}

#[test]
fn invalid_fork_usage_does_not_replace_inherited_baseline() {
    let baseline = CodexTotals {
        input: 7,
        output: 2,
        cached: 1,
        cache_write: 0,
        reasoning: 0,
    };
    let mut state = CodexParseState {
        previous_totals: Some(baseline),
        forked_child_inherited_baseline: Some(baseline),
        forked_child_inherited_reported_total: Some(9),
        ..CodexParseState::default()
    };
    let info = CodexInfo {
        model: None,
        model_name: None,
        total_token_usage: Some(CodexTokenUsage {
            input_tokens: Some(20),
            output_tokens: Some(4),
            cached_input_tokens: Some(2),
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_output_tokens: Some(1),
            total_tokens: Some(25),
        }),
        last_token_usage: Some(CodexTokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            cached_input_tokens: Some(2),
            cache_read_input_tokens: Some(-1),
            cache_write_input_tokens: None,
            reasoning_output_tokens: Some(0),
            total_tokens: Some(12),
        }),
    };

    assert!(remember_forked_child_inherited_baseline(&mut state, &info).is_err());
    assert_eq!(state.previous_totals, Some(baseline));
    assert_eq!(state.forked_child_inherited_baseline, Some(baseline));
    assert_eq!(state.forked_child_inherited_reported_total, Some(9));
}

#[test]
fn test_token_count_avoids_double_counting_stale_cumulative_regressions() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":110,"cached_input_tokens":22,"output_tokens":33,"reasoning_output_tokens":6},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let line4 = r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":109,"cached_input_tokens":21,"output_tokens":32,"reasoning_output_tokens":6},"last_token_usage":{"input_tokens":9,"cached_input_tokens":1,"output_tokens":2,"reasoning_output_tokens":0}}}}"#;
    let line5 = r#"{"timestamp":"2026-01-01T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":119,"cached_input_tokens":23,"output_tokens":35,"reasoning_output_tokens":6},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":0}}}}"#;
    let content = format!("{}\n{}\n{}\n{}\n{}", line1, line2, line3, line4, line5);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].tokens.input, 80);
    assert_eq!(messages[0].tokens.output, 30);
    assert_eq!(messages[0].tokens.cache_read, 20);
    assert_eq!(messages[0].tokens.reasoning, 5);

    assert_eq!(messages[1].tokens.input, 8);
    assert_eq!(messages[1].tokens.output, 3);
    assert_eq!(messages[1].tokens.cache_read, 2);
    assert_eq!(messages[1].tokens.reasoning, 1);

    // Stale snapshot (line4) is now skipped entirely; messages[2]
    // comes from line5's last_token_usage instead.
    assert_eq!(messages[2].tokens.input, 8);
    assert_eq!(messages[2].tokens.output, 3);
    assert_eq!(messages[2].tokens.cache_read, 2);
    assert_eq!(messages[2].tokens.reasoning, 0);
}

#[test]
fn test_token_count_handles_multiple_stale_regressions_before_recovery() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"reasoning_output_tokens":5}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":110,"cached_input_tokens":22,"output_tokens":33,"reasoning_output_tokens":6},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let line4 = r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":109,"cached_input_tokens":21,"output_tokens":32,"reasoning_output_tokens":6},"last_token_usage":{"input_tokens":9,"cached_input_tokens":1,"output_tokens":2,"reasoning_output_tokens":0}}}}"#;
    let line5 = r#"{"timestamp":"2026-01-01T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":118,"cached_input_tokens":22,"output_tokens":34,"reasoning_output_tokens":6},"last_token_usage":{"input_tokens":9,"cached_input_tokens":1,"output_tokens":2,"reasoning_output_tokens":0}}}}"#;
    let line6 = r#"{"timestamp":"2026-01-01T00:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":128,"cached_input_tokens":24,"output_tokens":37,"reasoning_output_tokens":6},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":0}}}}"#;
    let content = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        line1, line2, line3, line4, line5, line6
    );
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    // Stale line4 is skipped; messages come from lines 2, 3, 5, 6.
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].tokens.input, 80);
    assert_eq!(messages[1].tokens.input, 8);
    assert_eq!(messages[2].tokens.input, 8);
    assert_eq!(messages[2].tokens.output, 2);
    assert_eq!(messages[2].tokens.cache_read, 1);
    assert_eq!(messages[2].tokens.reasoning, 0);
    assert_eq!(messages[3].tokens.input, 8);
    assert_eq!(messages[3].tokens.output, 3);
    assert_eq!(messages[3].tokens.cache_read, 2);
    assert_eq!(messages[3].tokens.reasoning, 0);
}

#[test]
fn test_token_count_treats_large_regressions_as_real_resets() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10000,"cached_input_tokens":1000,"output_tokens":400,"reasoning_output_tokens":50},"last_token_usage":{"input_tokens":10000,"cached_input_tokens":1000,"output_tokens":400,"reasoning_output_tokens":50}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":7600,"cached_input_tokens":800,"output_tokens":280,"reasoning_output_tokens":35},"last_token_usage":{"input_tokens":25,"cached_input_tokens":5,"output_tokens":4,"reasoning_output_tokens":1}}}}"#;
    let line4 = r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":7625,"cached_input_tokens":805,"output_tokens":284,"reasoning_output_tokens":36},"last_token_usage":{"input_tokens":25,"cached_input_tokens":5,"output_tokens":4,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}\n{}", line1, line2, line3, line4);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].tokens.input, 9000);
    assert_eq!(messages[0].tokens.output, 400);
    assert_eq!(messages[0].tokens.cache_read, 1000);
    assert_eq!(messages[0].tokens.reasoning, 50);

    assert_eq!(messages[1].tokens.input, 20);
    assert_eq!(messages[1].tokens.output, 4);
    assert_eq!(messages[1].tokens.cache_read, 5);
    assert_eq!(messages[1].tokens.reasoning, 1);

    assert_eq!(messages[2].tokens.input, 20);
    assert_eq!(messages[2].tokens.output, 4);
    assert_eq!(messages[2].tokens.cache_read, 5);
    assert_eq!(messages[2].tokens.reasoning, 1);
}

#[test]
fn test_first_event_uses_last_not_total_for_resumed_sessions() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5000,"cached_input_tokens":500,"output_tokens":800,"reasoning_output_tokens":100},"last_token_usage":{"input_tokens":12,"cached_input_tokens":2,"output_tokens":5,"reasoning_output_tokens":1}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5012,"cached_input_tokens":502,"output_tokens":805,"reasoning_output_tokens":101},"last_token_usage":{"input_tokens":12,"cached_input_tokens":2,"output_tokens":5,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].tokens.input, 10);
    assert_eq!(messages[0].tokens.output, 5);
    assert_eq!(messages[0].tokens.cache_read, 2);
    assert_eq!(messages[0].tokens.reasoning, 1);
    assert_eq!(messages[1].tokens.input, 10);
    assert_eq!(messages[1].tokens.output, 5);
    assert_eq!(messages[1].tokens.cache_read, 2);
    assert_eq!(messages[1].tokens.reasoning, 1);
}

#[test]
fn test_zero_token_snapshot_does_not_inflate_later_deltas() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"cached_input_tokens":50,"output_tokens":80,"reasoning_output_tokens":10},"last_token_usage":{"input_tokens":500,"cached_input_tokens":50,"output_tokens":80,"reasoning_output_tokens":10}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0}}}}"#;
    let line4 = r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":510,"cached_input_tokens":52,"output_tokens":83,"reasoning_output_tokens":11},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}\n{}", line1, line2, line3, line4);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].tokens.input, 450);
    assert_eq!(messages[0].tokens.output, 80);
    assert_eq!(messages[0].tokens.cache_read, 50);
    assert_eq!(messages[0].tokens.reasoning, 10);
    assert_eq!(messages[1].tokens.input, 8);
    assert_eq!(messages[1].tokens.output, 3);
    assert_eq!(messages[1].tokens.cache_read, 2);
    assert_eq!(messages[1].tokens.reasoning, 1);
}

#[test]
fn test_model_info_slug_from_turn_context() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model_info":{"slug":"o3-pro"}}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}", line1, line2);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "o3-pro");
}

#[test]
fn test_session_meta_provider_and_agent() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"source":"interactive","model_provider":"azure","agent_nickname":"my-agent"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].provider_id.as_ref(), "azure");
    assert_eq!(messages[0].agent.as_deref(), Some("Codex Agent"));
}

#[test]
fn test_session_meta_object_source_keeps_provider_agent_and_workspace() {
    let file = create_test_file(concat!(
        r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"fork-session","forked_from_id":"parent-session","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1}}},"model_provider":"openai","agent_nickname":"worker","cwd":"/Users/alice/codex-fork"}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
        "\n"
    ));

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].provider_id.as_ref(), "openai");
    assert_eq!(messages[0].agent.as_deref(), Some("Codex Subagent"));
    assert_eq!(
        messages[0].workspace_key.as_deref(),
        Some("/Users/alice/codex-fork")
    );
    assert!(messages[0].dedup_key.is_some());
    assert!(!messages[0].is_main_session);
}

#[test]
fn test_human_fork_remains_a_main_session() {
    let file = create_test_file(concat!(
        r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"human-fork","forked_from_id":"parent-session","source":"vscode","model_provider":"openai","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
        "\n",
        r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":2},"last_token_usage":{"input_tokens":10,"output_tokens":2}}}}"#,
        "\n"
    ));

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert!(messages[0].is_main_session);
}

#[test]
fn test_only_thread_spawn_origin_marks_a_child() {
    let thread_spawn = serde_json::json!({
        "subagent": {"thread_spawn": {"parent_thread_id": "thread-parent"}}
    });
    assert!(codex_origin_is_thread_spawn(Some(&thread_spawn)));
    assert!(!codex_origin_is_thread_spawn(Some(&serde_json::json!({
        "subagent": {"review": {}}
    }))));
    assert!(!codex_origin_is_thread_spawn(Some(&serde_json::json!(
        "vscode"
    ))));
}

#[test]
fn test_forked_child_ignores_inherited_records_before_turn_context() {
    let file = create_test_file(concat!(
        r#"{"timestamp":"2026-05-05T21:51:57.991Z","type":"session_meta","payload":{"id":"child-session","forked_from_id":"parent-session","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1}}},"model_provider":"openai","agent_nickname":"worker","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:57.992Z","type":"session_meta","payload":{"id":"parent-session","source":"interactive","model_provider":"azure","agent_nickname":"parent","cwd":"/repo-parent"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:57.993Z","type":"event_msg","payload":{"type":"user_message","message":"parent prompt copied into child log"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:57.994Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":116000,"cached_input_tokens":114000,"output_tokens":1000,"total_tokens":117000},"last_token_usage":{"input_tokens":73000,"cached_input_tokens":72000,"output_tokens":500,"total_tokens":73500}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.947Z","type":"turn_context","payload":{"model":"gpt-5.5","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.948Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":116000,"cached_input_tokens":114000,"output_tokens":1000,"total_tokens":117000},"last_token_usage":{"input_tokens":73000,"cached_input_tokens":72000,"output_tokens":500,"total_tokens":73500}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:59.253Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":117500,"cached_input_tokens":115000,"output_tokens":1200,"reasoning_output_tokens":50,"total_tokens":118700},"last_token_usage":{"input_tokens":1500,"cached_input_tokens":1000,"output_tokens":200,"reasoning_output_tokens":50,"total_tokens":1700}}}}"#,
        "\n"
    ));

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "gpt-5.5");
    assert_eq!(messages[0].provider_id.as_ref(), "openai");
    assert_eq!(messages[0].agent.as_deref(), Some("Codex Subagent"));
    assert_eq!(messages[0].workspace_key.as_deref(), Some("/repo-child"));
    assert_eq!(messages[0].tokens.input, 500);
    assert_eq!(messages[0].tokens.cache_read, 1000);
    assert_eq!(messages[0].tokens.output, 200);
    assert_eq!(messages[0].tokens.reasoning, 50);
}

#[test]
fn test_forked_child_ignores_replayed_parent_rows_after_turn_context() {
    let file = create_test_file(concat!(
        r#"{"timestamp":"2026-05-05T21:51:57.991Z","type":"session_meta","payload":{"id":"child-session","forked_from_id":"parent-session","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1}}},"model_provider":"openai","agent_nickname":"worker","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:57.994Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330},"last_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.947Z","type":"turn_context","payload":{"model":"gpt-5.5","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.948Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"output_tokens":5,"total_tokens":55},"last_token_usage":{"input_tokens":50,"output_tokens":5,"total_tokens":55}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.949Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330},"last_token_usage":{"input_tokens":250,"output_tokens":25,"total_tokens":275}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:59.253Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":310,"output_tokens":32,"total_tokens":342},"last_token_usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}}}}"#,
        "\n"
    ));

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "gpt-5.5");
    assert_eq!(messages[0].tokens.input, 10);
    assert_eq!(messages[0].tokens.output, 2);
}

#[test]
fn test_forked_child_submit_cap_regression_skips_large_inherited_cache_replays() {
    let file = create_test_file(concat!(
        r#"{"timestamp":"2026-05-05T21:51:57.991Z","type":"session_meta","payload":{"id":"child-session","forked_from_id":"parent-session","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1,"agent_role":"architect"}}},"model_provider":"openai","agent_nickname":"architect","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:57.994Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1200000000,"cached_input_tokens":1180000000,"output_tokens":1000000,"reasoning_output_tokens":100000,"total_tokens":1201100000},"last_token_usage":{"input_tokens":750000000,"cached_input_tokens":740000000,"output_tokens":500000,"reasoning_output_tokens":50000,"total_tokens":750550000}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.947Z","type":"turn_context","payload":{"model":"gpt-5.5","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.948Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1180000000,"cached_input_tokens":1160000000,"output_tokens":900000,"reasoning_output_tokens":90000,"total_tokens":1180990000},"last_token_usage":{"input_tokens":20000000,"cached_input_tokens":20000000,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":20000000}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.949Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1200000000,"cached_input_tokens":1180000000,"output_tokens":1000000,"reasoning_output_tokens":100000,"total_tokens":1201100000},"last_token_usage":{"input_tokens":20000000,"cached_input_tokens":20000000,"output_tokens":100000,"reasoning_output_tokens":10000,"total_tokens":20110000}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:59.253Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1200001500,"cached_input_tokens":1180001000,"output_tokens":1000200,"reasoning_output_tokens":100050,"total_tokens":1201101750},"last_token_usage":{"input_tokens":1500,"cached_input_tokens":1000,"output_tokens":200,"reasoning_output_tokens":50,"total_tokens":1750}}}}"#,
        "\n"
    ));

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "gpt-5.5");
    assert_eq!(messages[0].tokens.input, 500);
    assert_eq!(messages[0].tokens.cache_read, 1000);
    assert_eq!(messages[0].tokens.output, 200);
    assert_eq!(messages[0].tokens.reasoning, 50);
}

#[test]
fn test_forked_child_detects_thread_spawn_origin_without_top_level_fork_id() {
    let file = create_test_file(concat!(
        r#"{"timestamp":"2026-05-05T21:51:57.991Z","type":"session_meta","payload":{"id":"child-session","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1}}},"model_provider":"openai","agent_nickname":"worker","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:57.994Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330},"last_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.947Z","type":"turn_context","payload":{"model":"gpt-5.5","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.948Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"output_tokens":5,"total_tokens":55},"last_token_usage":{"input_tokens":50,"output_tokens":5,"total_tokens":55}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:59.253Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":310,"output_tokens":32,"total_tokens":342},"last_token_usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}}}}"#,
        "\n"
    ));

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "gpt-5.5");
    assert_eq!(messages[0].tokens.input, 10);
    assert_eq!(messages[0].tokens.output, 2);
    assert!(!messages[0].is_main_session);
}

#[test]
fn test_forked_child_skips_nested_parent_replay_until_own_turn() {
    let parent = create_test_file(concat!(
        r#"{"timestamp":"2026-05-05T21:51:57.991Z","type":"session_meta","payload":{"id":"019e5b00-0000-7000-8000-000000000001","source":"vscode","model_provider":"openai","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.000Z","type":"turn_context","payload":{"turn_id":"019e5b00-0001-7000-8000-000000000001","model":"gpt-5.5","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.100Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330},"last_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330}}}}"#,
        "\n"
    ));
    let child = create_test_file(concat!(
        r#"{"timestamp":"2026-05-05T21:52:10.000Z","type":"session_meta","payload":{"id":"019e5c03-1e99-7000-8000-000000000001","forked_from_id":"019e5b00-0000-7000-8000-000000000001","source":{"subagent":{"thread_spawn":{"parent_thread_id":"019e5b00-0000-7000-8000-000000000001","depth":1}}},"model_provider":"openai","agent_nickname":"worker","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:10.000Z","type":"session_meta","payload":{"id":"019e5b00-0000-7000-8000-000000000001","source":"vscode","model_provider":"openai","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:10.100Z","type":"turn_context","payload":{"turn_id":"019e5b00-0001-7000-8000-000000000001","model":"gpt-5.5","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:10.200Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330},"last_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:20.000Z","type":"event_msg","payload":{"type":"task_started","turn_id":"019e5c03-6425-7000-8000-000000000001"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:20.100Z","type":"turn_context","payload":{"turn_id":"019e5c03-6425-7000-8000-000000000001","model":"gpt-5.5","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:20.200Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":320,"output_tokens":32,"total_tokens":352},"last_token_usage":{"input_tokens":20,"output_tokens":2,"total_tokens":22}}}}"#,
        "\n"
    ));

    let parent_messages = parse_codex_file(parent.path());
    let child_messages = parse_codex_file(child.path());

    assert_eq!(parent_messages.len(), 1);
    assert_eq!(child_messages.len(), 1);
    assert_ne!(parent_messages[0].dedup_key, child_messages[0].dedup_key);
    assert_eq!(child_messages[0].tokens.input, 20);
    assert_eq!(child_messages[0].tokens.output, 2);
}

#[test]
fn test_forked_child_same_millisecond_turn_starts_own_session() {
    let child = create_test_file(concat!(
        r#"{"timestamp":"2026-05-05T21:52:10.000Z","type":"session_meta","payload":{"id":"019e5c03-1e99-7000-8000-0000000000ff","forked_from_id":"019e5b00-0000-7000-8000-000000000001","source":{"subagent":{"thread_spawn":{"parent_thread_id":"019e5b00-0000-7000-8000-000000000001","depth":1}}},"model_provider":"openai","agent_nickname":"worker","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:10.000Z","type":"session_meta","payload":{"id":"019e5b00-0000-7000-8000-000000000001","source":"vscode","model_provider":"openai","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:10.100Z","type":"turn_context","payload":{"turn_id":"019e5b00-0001-7000-8000-000000000001","model":"gpt-5.5","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:10.200Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330},"last_token_usage":{"input_tokens":300,"output_tokens":30,"total_tokens":330}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:20.100Z","type":"turn_context","payload":{"turn_id":"019e5c03-1e99-7000-8000-000000000001","model":"gpt-5.5","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:52:20.200Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":320,"output_tokens":32,"total_tokens":352},"last_token_usage":{"input_tokens":20,"output_tokens":2,"total_tokens":22}}}}"#,
        "\n"
    ));

    let child_messages = parse_codex_file(child.path());

    assert_eq!(child_messages.len(), 1);
    assert_eq!(child_messages[0].tokens.input, 20);
    assert_eq!(child_messages[0].tokens.output, 2);
}

#[test]
fn test_forked_child_incremental_state_skips_inherited_prefix() {
    let file = create_test_file(concat!(
        r#"{"timestamp":"2026-05-05T21:51:57.991Z","type":"session_meta","payload":{"id":"child-session","forked_from_id":"parent-session","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1}}},"model_provider":"openai","agent_nickname":"worker","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:57.992Z","type":"session_meta","payload":{"id":"parent-session","source":"interactive","model_provider":"azure","agent_nickname":"parent","cwd":"/repo-parent"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:57.994Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":116000,"cached_input_tokens":114000,"output_tokens":1000,"total_tokens":117000},"last_token_usage":{"input_tokens":73000,"cached_input_tokens":72000,"output_tokens":500,"total_tokens":73500}}}}"#,
        "\n"
    ));
    let prefix_size = file.as_file().metadata().unwrap().len();
    let prefix = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());

    assert!(prefix.messages.is_empty());

    let appended = concat!(
        r#"{"timestamp":"2026-05-05T21:51:58.947Z","type":"turn_context","payload":{"model":"gpt-5.5","cwd":"/repo-child"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:58.948Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":116000,"cached_input_tokens":114000,"output_tokens":1000,"total_tokens":117000},"last_token_usage":{"input_tokens":73000,"cached_input_tokens":72000,"output_tokens":500,"total_tokens":73500}}}}"#,
        "\n",
        r#"{"timestamp":"2026-05-05T21:51:59.253Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":117500,"cached_input_tokens":115000,"output_tokens":1200,"reasoning_output_tokens":50,"total_tokens":118700},"last_token_usage":{"input_tokens":1500,"cached_input_tokens":1000,"output_tokens":200,"reasoning_output_tokens":50,"total_tokens":1700}}}}"#,
        "\n"
    );
    let mut reopened = file.reopen().unwrap();
    reopened.seek(SeekFrom::End(0)).unwrap();
    reopened.write_all(appended.as_bytes()).unwrap();
    reopened.flush().unwrap();

    let incremental = parse_codex_file_incremental(file.path(), prefix_size, prefix.state.clone());
    let full = parse_codex_file(file.path());

    assert_eq!(incremental.messages, full);
    assert_eq!(incremental.messages.len(), 1);
    assert_eq!(incremental.messages[0].tokens.input, 500);
    assert_eq!(incremental.messages[0].tokens.cache_read, 1000);
    assert_eq!(incremental.messages[0].tokens.output, 200);
    assert_eq!(incremental.messages[0].tokens.reasoning, 50);
}

#[test]
fn test_session_meta_cwd_sets_workspace_metadata() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"source":"interactive","cwd":"/Users/alice/demo-repo"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].workspace_key.as_deref(),
        Some("/Users/alice/demo-repo")
    );
    assert_eq!(messages[0].workspace_label.as_deref(), Some("demo-repo"));
}

#[test]
fn test_inaccessible_cwd_still_parses_token_usage() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"source":"interactive","cwd":"/path/that/does/not/exist/demo-repo"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.input, 8);
    assert_eq!(messages[0].tokens.output, 3);
    assert_eq!(messages[0].tokens.cache_read, 2);
    assert_eq!(
        messages[0].workspace_key.as_deref(),
        Some("/path/that/does/not/exist/demo-repo")
    );
    assert_eq!(messages[0].workspace_label.as_deref(), Some("demo-repo"));
}

#[test]
fn test_session_meta_empty_cwd_clears_workspace_metadata() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"source":"interactive","cwd":"   "}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].workspace_key, None);
    assert_eq!(messages[0].workspace_label, None);
    assert_eq!(messages[0].tokens.input, 8);
}

#[test]
fn test_session_meta_malformed_cwd_clears_workspace_metadata() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"source":"interactive","cwd":"file:///Users/alice/demo-repo"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].workspace_key, None);
    assert_eq!(messages[0].workspace_label, None);
    assert_eq!(messages[0].tokens.input, 8);
}

#[test]
fn test_session_meta_path_like_noncanonical_cwd_normalizes_consistently() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"source":"interactive","cwd":"//server//share///demo-repo/"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].workspace_key.as_deref(),
        Some("//server/share/demo-repo")
    );
    assert_eq!(messages[0].workspace_label.as_deref(), Some("demo-repo"));
    assert_eq!(messages[0].tokens.input, 8);
}

#[test]
fn test_cached_tokens_takes_max_of_both_fields() {
    let usage = CodexTokenUsage {
        input_tokens: Some(100),
        output_tokens: Some(30),
        cached_input_tokens: Some(10),
        cache_read_input_tokens: Some(20),
        cache_write_input_tokens: None,
        reasoning_output_tokens: Some(5),
        total_tokens: None,
    };
    let totals = CodexTotals::from_usage(&usage);
    assert_eq!(totals.cached, 20);
}

#[test]
fn test_compaction_total_drop_uses_last_as_increment() {
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150000,"cached_input_tokens":10000,"output_tokens":20000,"reasoning_output_tokens":5000},"last_token_usage":{"input_tokens":150000,"cached_input_tokens":10000,"output_tokens":20000,"reasoning_output_tokens":5000}}}}"#;
    let line3 = r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200000,"cached_input_tokens":15000,"output_tokens":25000,"reasoning_output_tokens":6000},"last_token_usage":{"input_tokens":50,"cached_input_tokens":5,"output_tokens":10,"reasoning_output_tokens":2}}}}"#;
    let content = format!("{}\n{}\n{}", line1, line2, line3);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].tokens.input, 45);
    assert_eq!(messages[1].tokens.output, 10);
    assert_eq!(messages[1].tokens.cache_read, 5);
    assert_eq!(messages[1].tokens.reasoning, 2);
}

#[test]
fn test_extract_model_skips_empty_slug_falls_through_to_model() {
    // model_info.slug is empty string, but payload.model has a valid value.
    // extract_model should skip the empty slug and return payload.model.
    let line1 = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"model_info":{"slug":""},"model":"gpt-4o"}}"#;
    let line2 = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":5},"last_token_usage":{"input_tokens":10,"output_tokens":5}}}}"#;
    let content = format!("{}\n{}", line1, line2);
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "gpt-4o");
}

#[test]
fn test_pending_model_messages_do_not_bind_across_unrelated_turns() {
    let file = create_test_file(concat!(
        r#"{"type":"session_meta","payload":{"source":"interactive","model_provider":"openai"}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:02Z","type":"assistant_message"}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:04Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":15,"cached_input_tokens":3,"output_tokens":5},"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2}}}}"#,
        "\n"
    ));

    let parsed =
        super::parse_codex_file_incremental(file.path(), 0, CodexParseState::default()).unwrap();

    assert!(parsed
        .interrupted
        .unwrap()
        .message
        .contains("resolve Codex token-count model"));
}

#[test]
fn test_token_count_ignores_empty_info_model_until_later_valid_model() {
    let file = create_test_file(concat!(
        r#"{"type":"session_meta","payload":{"source":"interactive","model_provider":"openai"}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"","model_name":"","total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
        "\n",
        r#"{"timestamp":"2026-04-27T10:00:04Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
        "\n"
    ));

    let parsed = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());

    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].model_id.as_ref(), "gpt-5.5");
}

fn completed_user_item(content: Value) -> Value {
    serde_json::json!({
        "type": "item_completed", "turn_id": "turn-1",
        "item": {"type": "UserMessage", "id": "prompt-1", "content": content}
    })
}

fn turn_usage_event(sequence: i64) -> Value {
    serde_json::json!({
        "type": "token_count",
        "info": {
            "last_token_usage": {"input_tokens": 10, "output_tokens": 3},
            "total_token_usage": {"input_tokens": 10 * sequence, "output_tokens": 3 * sequence}
        }
    })
}

fn parse_turn_events(events: &[Value]) -> ParsedCodexFile {
    let mut content = String::from(
        "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.2\",\"turn_id\":\"turn-1\"}}\n",
    );
    for event in events {
        content.push_str(
            &serde_json::json!({
                "timestamp": "2026-09-15T00:00:00Z",
                "type": "event_msg", "payload": event
            })
            .to_string(),
        );
        content.push('\n');
    }
    let file = create_test_file(&content);
    parse_codex_file_incremental(file.path(), 0, CodexParseState::default())
}

#[test]
fn completed_user_item_and_legacy_event_share_one_pending_turn() {
    let modern = completed_user_item(serde_json::json!([
        {"type": "text", "text": "hello", "text_elements": []}
    ]));
    let legacy = serde_json::json!({"type": "user_message", "message": "hello"});
    for mut events in [
        vec![legacy.clone()],
        vec![modern.clone()],
        vec![modern.clone(), legacy.clone()],
        vec![legacy, modern],
    ] {
        events.extend([
                serde_json::json!({"type": "item_completed", "item": {"type": "AgentMessage", "text": "hi"}}),
                turn_usage_event(1),
                turn_usage_event(2),
            ]);
        let parsed = parse_turn_events(&events);
        assert!(parsed.interrupted.is_none());
        assert!(parsed.rejections.is_empty());
        assert_eq!(parsed.messages.len(), 2);
        assert!(parsed.messages[0].is_turn_start, "{events:?}");
        assert!(!parsed.messages[1].is_turn_start, "{events:?}");
        assert_eq!(parsed.messages[0].tokens.total(), 13);
        assert_eq!(parsed.messages[1].tokens.total(), 13);
    }
}

#[test]
fn completed_user_item_filters_context_and_counts_structured_input() {
    for (content, expected) in [
        (serde_json::json!([]), false),
        (
            serde_json::json!([
                {"type": "text", "text": "\n<environment_context>cwd=/tmp</environment_context>"},
                {"type": "text", "text": "<system-reminder>context</system-reminder>"},
                {"type": "text", "text": "<user_instructions>context</user_instructions>"}
            ]),
            false,
        ),
        (
            serde_json::json!([
                {"type": "text", "text": "<environment_context>context</environment_context>"},
                {"type": "text", "text": "<div>explain this markup</div>"}
            ]),
            true,
        ),
        (
            serde_json::json!([{"type": "image", "image_url": "data:image/png;base64,test"}]),
            true,
        ),
        (
            serde_json::json!([{"type": "local_image", "path": "/tmp/test.png"}]),
            true,
        ),
        (
            serde_json::json!([{"type": "skill", "name": "test", "path": "/tmp/SKILL.md"}]),
            true,
        ),
        (
            serde_json::json!([{"type": "mention", "name": "test", "path": "app://test"}]),
            true,
        ),
    ] {
        let parsed = parse_turn_events(&[
            completed_user_item(content.clone()),
            turn_usage_event(1),
            turn_usage_event(2),
        ]);
        assert!(parsed.interrupted.is_none(), "{content}");
        assert!(parsed.rejections.is_empty(), "{content}");
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].is_turn_start, expected, "{content}");
        assert!(!parsed.messages[1].is_turn_start);
    }
}

#[test]
fn completed_user_item_steering_and_mixed_formats_keep_input_based_turns() {
    let mut steering = completed_user_item(serde_json::json!([
        {"type": "text", "text": "focus on the tests"}
    ]));
    steering["item"]["id"] = serde_json::json!("prompt-2");
    let parsed = parse_turn_events(&[
        serde_json::json!({"type": "user_message", "message": "start"}),
        turn_usage_event(1),
        steering,
        turn_usage_event(2),
        serde_json::json!({"type": "task_started", "turn_id": "turn-2"}),
        serde_json::json!({"type": "item_started", "item": {"type": "UserMessage", "content": [{"type": "text", "text": "not completed"}]}}),
        turn_usage_event(3),
    ]);
    assert!(parsed.interrupted.is_none());
    assert!(parsed.rejections.is_empty());
    assert_eq!(parsed.messages.len(), 3);
    assert!(parsed.messages[0].is_turn_start);
    assert!(parsed.messages[1].is_turn_start);
    assert!(!parsed.messages[2].is_turn_start);
}

#[test]
fn completed_user_item_survives_duplicate_and_zero_usage_snapshots() {
    let mut zero = turn_usage_event(0);
    zero["info"]["last_token_usage"] = serde_json::json!({"input_tokens": 0, "output_tokens": 0});
    let parsed = parse_turn_events(&[
        turn_usage_event(1),
        completed_user_item(serde_json::json!([{"type": "text", "text": "continue"}])),
        turn_usage_event(1),
        zero,
        turn_usage_event(2),
        turn_usage_event(3),
    ]);
    assert!(parsed.interrupted.is_none());
    assert!(parsed.rejections.is_empty());
    assert_eq!(parsed.messages.len(), 3);
    assert!(!parsed.messages[0].is_turn_start);
    assert!(parsed.messages[1].is_turn_start);
    assert!(!parsed.messages[2].is_turn_start);
}

#[test]
fn malformed_completed_user_item_interrupts_without_inventing_turns() {
    for event in [
        serde_json::json!({"type": "item_completed"}),
        serde_json::json!({"type": "item_completed", "item": {"type": "UserMessage"}}),
        completed_user_item(serde_json::json!([{"type": "text", "text": 42}])),
        completed_user_item(serde_json::json!([{"type": "unknown_input"}])),
    ] {
        let parsed = parse_turn_events(&[turn_usage_event(1), event.clone(), turn_usage_event(2)]);
        assert!(parsed.interrupted.is_some(), "{event}");
        assert_eq!(parsed.rejections.total(), 1);
        assert_eq!(parsed.messages.len(), 1);
        assert!(!parsed.messages[0].is_turn_start);
        assert!(!parsed.state.pending_turn_start);
    }
}

#[test]
fn completed_user_item_inherited_from_fork_does_not_mark_child_usage() {
    let content = [
            r#"{"type":"session_meta","payload":{"id":"child-session","forked_from_id":"parent-session","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session"}}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"parent prompt"}]}}}"#,
            r#"{"type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
            r#"{"timestamp":"2026-09-15T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":3},"last_token_usage":{"input_tokens":10,"output_tokens":3}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"child prompt"}]}}}"#,
            r#"{"timestamp":"2026-09-15T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":20,"output_tokens":6},"last_token_usage":{"input_tokens":10,"output_tokens":3}}}}"#,
        ].join("\n");
    let file = create_test_file(&content);
    let parsed = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());
    assert!(parsed.interrupted.is_none());
    assert!(parsed.rejections.is_empty());
    assert_eq!(parsed.messages.len(), 2);
    assert!(!parsed.messages[0].is_turn_start);
    assert!(parsed.messages[1].is_turn_start);
    assert!(parsed
        .messages
        .iter()
        .all(|message| !message.is_main_session));
}

#[test]
fn test_user_message_marks_next_token_count_as_turn_start() {
    let content = [
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"user_message","message":"continue please"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
            r#"{"timestamp":"2026-01-01T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":20,"cached_input_tokens":4,"output_tokens":6},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
        ]
        .join("\n");
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 2);
    assert!(
        messages[0].is_turn_start,
        "first reply after a human user_message is a turn start"
    );
    assert!(
        !messages[1].is_turn_start,
        "a later reply with no new user_message is not a turn start"
    );
}

#[test]
fn test_xml_user_message_does_not_mark_turn_start() {
    let content = [
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"user_message","message":"\n<environment_context>\n  <cwd>/tmp</cwd>\n</environment_context>"}}"#,
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
        ]
        .join("\n");
    let file = create_test_file(&content);

    let messages = parse_codex_file(file.path());

    assert_eq!(messages.len(), 1);
    assert!(
        !messages[0].is_turn_start,
        "a system-injected <...> message is not a human turn"
    );
}

#[test]
fn test_exec_user_message_still_marks_turn_start() {
    // A `codex exec` one-shot is non-interactive but still carries a real human
    // prompt, so it counts as exactly one turn (verified against a real
    // `codex exec` session: 1 user_message -> turn_count 1).
    for user_event in [
        r#"{"type":"event_msg","payload":{"type":"user_message","message":"hello"}}"#,
        r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"hello"}]}}}"#,
    ] {
        let content = [
                r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"source":"exec"}}"#,
                r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
                user_event,
                // A real `codex exec` interleaves an agent_message between the user
                // prompt and the token_count; the deferred turn flag must survive it.
                r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"agent_message","message":"hi"}}"#,
                r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#,
            ]
            .join("\n");
        let file = create_test_file(&content);

        let messages = parse_codex_file(file.path());

        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].is_turn_start,
            "an exec one-shot with a human prompt counts as one turn"
        );
        assert_eq!(messages[0].agent.as_deref(), Some("Codex Exec"));
    }
}

#[test]
fn test_incremental_parse_preserves_pending_turn_start() {
    for user_event in [
        r#"{"type":"event_msg","payload":{"type":"user_message","message":"hello"}}"#,
        r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"hello"}]}}}"#,
    ] {
        let content = [
                r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.2"}}"#,
                user_event,
                "",
            ]
            .join("\n");
        let file = create_test_file(&content);
        let initial_size = file.as_file().metadata().unwrap().len();

        let initial = parse_codex_file_incremental(file.path(), 0, CodexParseState::default());
        assert!(
            initial.messages.is_empty(),
            "no token_count yet, so no message"
        );
        assert!(
            initial.state.pending_turn_start,
            "a pending turn survives a chunk that ends before the token_count"
        );
        let appended = format!(
            "{}\n",
            r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3}}}}"#
        );
        let mut reopened = file.reopen().unwrap();
        reopened.seek(SeekFrom::End(0)).unwrap();
        reopened.write_all(appended.as_bytes()).unwrap();
        reopened.flush().unwrap();

        let cached_state = bincode::serialize(&initial.state).unwrap();
        let restored_state = bincode::deserialize(&cached_state).unwrap();
        let incremental = parse_codex_file_incremental(file.path(), initial_size, restored_state);

        assert_eq!(incremental.messages.len(), 1);
        assert!(
            incremental.messages[0].is_turn_start,
            "the deferred turn applies to the message parsed in the next chunk"
        );
        assert!(
            !incremental.state.pending_turn_start,
            "the pending flag is consumed once applied"
        );
    }
}
