#![cfg(test)]

use super::*;
use std::io::Write;
use tempfile::{NamedTempFile, TempDir};

#[test]
fn is_human_turn_counts_html_user_prompt() {
    let line = r#"{"type":"user","message":{"content":"<div>hello</div>"}}"#;
    assert!(is_human_turn(line));
}

#[test]
fn is_human_turn_skips_internal_tool_tags() {
    for tag in CLAUDECODE_INTERNAL_USER_TAGS {
        let line = format!(r#"{{"type":"user","message":{{"content":"{tag}some output</...>"}}}}"#);
        assert!(
            !is_human_turn(&line),
            "expected tag {tag} to be filtered as non-human"
        );
    }
}

#[test]
fn is_human_turn_skips_array_content() {
    let line = r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#;
    assert!(!is_human_turn(line));
}

fn create_test_file(content: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(content.as_bytes()).unwrap();
    file.flush().unwrap();
    file
}

#[test]
fn cancelled_parse_stops_before_decoding_records() {
    let file = create_test_file(
        r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"model":"claude-3-5-sonnet","usage":{"input_tokens":1,"output_tokens":1}}}"#,
    );
    let cancellation = crate::engine::AcquisitionCancellation::default();
    cancellation.cancel();
    let resolver = ClaudeProjectResolver::new(None);

    let (scanned, _) = parse_claude_file_with_project_resolver_and_cancellation(
        file.path(),
        &resolver,
        Some(&cancellation),
    )
    .unwrap();

    assert!(scanned.messages.is_empty());
    let interrupted = scanned.interrupted.unwrap();
    assert_eq!(interrupted.operation, "parse Claude session");
    assert_eq!(interrupted.message, "acquisition cancelled");
}

// Most parser tests assert only the message projection. Health-specific
// cases call `super::parse_claude_file` directly so rejections cannot be
// discarded accidentally in the behavior under test.
fn parse_claude_file(path: &Path) -> SessionParseResult<Vec<UsageRecord>> {
    super::parse_claude_file(path).map(|scanned| scanned.messages)
}

fn create_project_file(
    content: &str,
    project: &str,
    filename: &str,
) -> (TempDir, std::path::PathBuf) {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir
        .path()
        .join(".claude")
        .join("projects")
        .join(project)
        .join(filename);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    (temp_dir, path)
}

fn create_transcript_file(content: &str, filename: &str) -> (TempDir, std::path::PathBuf) {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir
        .path()
        .join(".claude")
        .join("transcripts")
        .join(filename);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    (temp_dir, path)
}

#[test]
fn missing_primary_input_reports_its_path() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("missing.jsonl");

    let error = parse_claude_file_with_home(&path, Some(temp_dir.path())).unwrap_err();

    assert_eq!(error.path(), Some(path.as_path()));
    assert_eq!(error.operation(), "open Claude session");
    assert!(error.to_string().contains("missing.jsonl"));
}

#[test]
fn malformed_primary_jsonl_reports_partial_health() {
    let file = create_test_file("{not-json\n");

    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert!(scanned.messages.is_empty());
    assert_eq!(scanned.rejections.total(), 1);
    let failure = scanned.interrupted.unwrap();
    assert_eq!(failure.operation, "decode Claude session line");
    assert!(failure.message.contains("line 1"));
}

#[test]
fn good_bad_good_assistant_records_keep_confirmed_messages() {
    let file = create_test_file(
        r#"{"type":"assistant","timestamp":"2026-07-14T00:00:00Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":1}}}
{"type":"assistant","timestamp":"2026-07-14T00:00:01Z","message":{"usage":{"input_tokens":99,"output_tokens":9}}}
{"type":"assistant","timestamp":"2026-07-14T00:00:02Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":20,"output_tokens":2}}}"#,
    );

    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert_eq!(scanned.messages.len(), 2);
    assert_eq!(scanned.messages[0].tokens.input, 10);
    assert_eq!(scanned.messages[1].tokens.input, 20);
    assert_eq!(scanned.rejections.total(), 1);
    assert_eq!(
        scanned.rejections.entries().next().unwrap().key,
        "missing-model"
    );
    assert!(scanned.interrupted.is_none());
}

#[test]
fn negative_streaming_duplicate_is_rejected_without_mutating_good_records() {
    let file = create_test_file(
        r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":5}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:10.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":999,"output_tokens":-1}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:02.000Z","requestId":"req_002","message":{"id":"msg_002","model":"claude-sonnet-4.6","usage":{"input_tokens":20,"output_tokens":2}}}"#,
    );

    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert_eq!(scanned.messages.len(), 2);
    assert_eq!(scanned.messages[0].tokens.input, 10);
    assert_eq!(scanned.messages[0].tokens.output, 5);
    assert_eq!(scanned.messages[0].timestamp, 1_733_047_200_000);
    assert_eq!(scanned.messages[1].tokens.input, 20);
    let rejected: Vec<_> = scanned
        .rejections
        .entries()
        .map(|entry| (entry.key, entry.count))
        .collect();
    assert_eq!(rejected, [("malformed-record", 1)]);
    assert!(scanned.interrupted.is_none());
}

#[test]
fn non_usage_records_do_not_emit_or_reject_tokens() {
    for content in [
        r#"{"type":"assistant","message":{"usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
        r#"{"type":"assistant","timestamp":"not-a-timestamp","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":0,"output_tokens":0}}}"#,
        r#"{"type":"assistant","timestamp":"2026-05-27T10:00:00.000Z","message":{"model":"claude-sonnet-4.6"}}
{"type":"user","timestamp":"not-a-timestamp","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_zero","tool_output":{"output":""}}]}}"#,
        r#"{"type":"assistant","timestamp":"2026-05-27T10:00:00.000Z","message":{"model":"claude-sonnet-4.6"}}
{"type":"user","timestamp":"2026-05-27T10:00:01.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_context","tool_output":{"output":"abcdefghijklmnop"}}]}}"#,
    ] {
        let file = create_test_file(content);
        let scanned = super::parse_claude_file(file.path()).unwrap();
        assert!(scanned.messages.is_empty(), "{content}");
        assert!(scanned.rejections.is_empty(), "{content}");
        assert!(scanned.interrupted.is_none(), "{content}");
    }
}

#[test]
fn zero_streaming_chunk_can_seed_a_later_positive_duplicate() {
    let file = create_test_file(
        r#"{"type":"assistant","timestamp":"2026-07-14T00:00:00Z","requestId":"request-1","message":{"id":"message-1","model":"claude-sonnet-4.6","usage":{"input_tokens":0,"output_tokens":0}}}
{"type":"assistant","timestamp":"2026-07-14T00:00:01Z","requestId":"request-1","message":{"id":"message-1","usage":{"input_tokens":10,"output_tokens":2}}}"#,
    );

    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(scanned.messages[0].model_id.as_ref(), "claude-sonnet-4.6");
    assert_eq!(scanned.messages[0].tokens.input, 10);
    assert_eq!(scanned.messages[0].tokens.output, 2);
    assert!(scanned.rejections.is_empty());
    assert!(scanned.interrupted.is_none());
}

#[test]
fn rejected_missing_model_keeps_valid_usage_beside_tool_content() {
    let file = create_test_file(
        r#"{"type":"assistant","timestamp":"2026-07-14T00:00:00Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":1}}}
{"type":"assistant","timestamp":"2026-07-14T00:00:01Z","message":{"usage":{"input_tokens":99,"output_tokens":9}}}
{"type":"user","timestamp":"2026-07-14T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_after_bad_model","tool_output":{"output":"abcdefghijklmnop"}}]}}
{"type":"assistant","timestamp":"2026-07-14T00:00:03Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":20,"output_tokens":2}}}"#,
    );

    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert_eq!(scanned.messages.len(), 2);
    assert_eq!(scanned.messages[0].tokens.input, 10);
    assert_eq!(scanned.messages[1].tokens.input, 20);
    assert_eq!(scanned.rejections.total(), 1);
    assert_eq!(
        scanned.rejections.entries().next().unwrap().key,
        "missing-model"
    );
    assert!(scanned.interrupted.is_none());
}

#[test]
fn blank_assistant_model_is_rejected_without_blocking_later_records() {
    let file = create_test_file(
        r#"{"type":"assistant","timestamp":"2026-07-14T00:00:00Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":1}}}
{"type":"assistant","timestamp":"2026-07-14T00:00:01Z","message":{"model":"   ","usage":{"input_tokens":99,"output_tokens":9}}}
{"type":"assistant","timestamp":"2026-07-14T00:00:02Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":20,"output_tokens":2}}}"#,
    );

    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert_eq!(scanned.messages.len(), 2);
    assert_eq!(scanned.messages[0].tokens.input, 10);
    assert_eq!(scanned.messages[1].tokens.input, 20);
    assert_eq!(scanned.rejections.total(), 1);
    assert_eq!(
        scanned.rejections.entries().next().unwrap().key,
        "missing-model"
    );
    assert!(scanned.interrupted.is_none());
}

#[test]
fn malformed_json_interrupts_when_later_state_cannot_be_trusted() {
    let file = create_test_file(
        r#"{"type":"assistant","timestamp":"2026-07-14T00:00:00Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":1}}}
{"type":"message_start","message":
{"type":"assistant","timestamp":"2026-07-14T00:00:02Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":20,"output_tokens":2}}}"#,
    );

    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(scanned.messages[0].tokens.input, 10);
    assert_eq!(scanned.rejections.total(), 1);
    assert_eq!(
        scanned.rejections.entries().next().unwrap().key,
        "malformed-record"
    );
    let failure = scanned
        .interrupted
        .as_ref()
        .expect("unknown malformed event state must mark the input partial");
    assert_eq!(failure.operation, "decode Claude session line");
    assert!(failure.message.contains("line 2"));
}

#[test]
fn token_bearing_assistant_without_timestamp_is_rejected() {
    let file = create_test_file(
        r#"{"type":"assistant","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":1,"output_tokens":1}}}"#,
    );

    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert!(scanned.messages.is_empty());
    let rejection = scanned.rejections.entries().next().unwrap();
    assert_eq!(rejection.key, "missing-timestamp");
    assert!(scanned.interrupted.is_none());
}

#[test]
fn tool_result_without_timestamp_is_not_api_usage() {
    let file = create_test_file(
        r#"{"type":"assistant","timestamp":"2026-05-27T10:00:00.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":1,"output_tokens":1}}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_missing_timestamp","tool_output":{"output":"abcdefghijklmnop"}}]}}"#,
    );

    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert!(scanned.rejections.is_empty());
    assert!(scanned.interrupted.is_none());
}

#[test]
fn malformed_sidechain_meta_keeps_usage_and_reports_rejection() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir.path().join(".claude/projects/project-a");
    let path = project_dir.join("session/subagents/agent-badmeta.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            r#"{"type":"assistant","isSidechain":true,"sessionId":"session","agentId":"badmeta","timestamp":"2024-12-01T10:00:00.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        )
        .unwrap();
    let meta_path = path.with_file_name("agent-badmeta.meta.json");
    std::fs::write(&meta_path, "{not-json").unwrap();

    let scanned = parse_claude_file_with_home(&path, Some(temp_dir.path())).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(scanned.messages[0].tokens.input, 1);
    assert_eq!(scanned.messages[0].tokens.output, 1);
    assert_eq!(
        scanned.messages[0].agent.as_deref(),
        Some("Claude Subagent")
    );
    assert_eq!(scanned.rejections.total(), 1);
    let rejection = scanned.rejections.entries().next().unwrap();
    assert_eq!(rejection.key, "malformed-record");
    assert!(scanned.interrupted.is_none());
}

#[test]
fn malformed_sidechain_parent_keeps_usage_and_reports_rejection() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir.path().join(".claude/projects/project-a");
    let parent_path = project_dir.join("session.jsonl");
    std::fs::create_dir_all(&project_dir).unwrap();
    std::fs::write(&parent_path, "{not-json").unwrap();
    let path = project_dir.join("session/subagents/agent-badparent.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            r#"{"type":"assistant","isSidechain":true,"sessionId":"session","agentId":"badparent","timestamp":"2024-12-01T10:00:00.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":2,"output_tokens":3}}}"#,
        )
        .unwrap();

    let scanned = parse_claude_file_with_home(&path, Some(temp_dir.path())).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(scanned.messages[0].tokens.input, 2);
    assert_eq!(scanned.messages[0].tokens.output, 3);
    assert_eq!(
        scanned.messages[0].agent.as_deref(),
        Some("Claude Subagent")
    );
    assert_eq!(scanned.rejections.total(), 1);
    let rejection = scanned.rejections.entries().next().unwrap();
    assert_eq!(rejection.key, "malformed-record");
    assert!(scanned.interrupted.is_none());
}

#[test]
fn duplicate_input_prefers_explicit_cache_split_in_either_order() {
    let cases = [
        (
            serde_json::json!({"input_tokens": 42494}),
            serde_json::json!({"input_tokens": 265, "cache_read_input_tokens": 42000, "output_tokens": 120}),
            265,
            true,
        ),
        (
            serde_json::json!({"input_tokens": 42494}),
            serde_json::json!({"input_tokens": 0, "cache_read_input_tokens": 42000}),
            0,
            true,
        ),
        (
            serde_json::json!({"input_tokens": 100}),
            serde_json::json!({"input_tokens": 30, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}),
            30,
            true,
        ),
        (
            serde_json::json!({"input_tokens": 100}),
            serde_json::json!({"cache_read_input_tokens": 42000}),
            100,
            false,
        ),
        (
            serde_json::json!({"input_tokens": 100, "cache_read_input_tokens": 0}),
            serde_json::json!({"input_tokens": 30, "cache_read_input_tokens": 200}),
            100,
            true,
        ),
        (
            serde_json::json!({"input_tokens": 100}),
            serde_json::json!({"input_tokens": 30}),
            100,
            false,
        ),
    ];
    for (left, right, expected, is_split) in cases {
        for usages in [[&left, &right], [&right, &left]] {
            let content = usages
                .map(|usage| {
                    serde_json::json!({
                        "type": "assistant", "timestamp": "2026-09-21T10:00:00Z",
                        "requestId": "request", "message": {
                            "id": "message", "model": "claude-sonnet-4.6", "usage": usage
                        }
                    })
                    .to_string()
                })
                .join("\n");
            let file = create_test_file(&content);
            let messages = parse_claude_file(file.path()).unwrap();
            assert_eq!(messages.len(), 1, "{content}");
            assert_eq!(messages[0].tokens.input, expected, "{content}");
            assert_eq!(messages[0].claude_input_is_split, is_split, "{content}");
        }
    }
}

#[test]
fn missing_input_does_not_claim_split_authority_over_later_snapshots() {
    let content = [
        serde_json::json!({"input_tokens": 10}),
        serde_json::json!({"cache_read_input_tokens": 200}),
        serde_json::json!({"input_tokens": 100}),
        serde_json::json!({"input_tokens": 0, "cache_read_input_tokens": 200}),
        serde_json::json!({"input_tokens": 300}),
    ]
    .map(|usage| {
        serde_json::json!({
            "type": "assistant", "timestamp": "2026-09-21T10:00:00Z",
            "message": {"id": "message", "model": "claude-sonnet-4.6", "usage": usage}
        })
        .to_string()
    })
    .join("\n");
    let file = create_test_file(&content);
    let messages = parse_claude_file(file.path()).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.input, 0);
    assert_eq!(messages[0].tokens.cache_read, 200);
    assert!(messages[0].claude_input_is_split);
}

#[test]
fn test_deduplication_skips_duplicate_entries() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:02.000Z","requestId":"req_002","message":{"id":"msg_002","model":"claude-sonnet-4.6","usage":{"input_tokens":200,"output_tokens":100}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(
        messages.len(),
        2,
        "Should deduplicate to 2 messages (first duplicate skipped)"
    );
    assert_eq!(messages[0].tokens.input, 100);
    assert_eq!(messages[1].tokens.input, 200);
}

#[test]
fn test_deduplication_keeps_max_output_for_streaming_duplicates() {
    // CC streaming writes the same messageId:requestId multiple times.
    // The first entry has a partial output_tokens count; the last has the
    // final (largest) count. We must keep the entry with the highest
    // output_tokens, not the first-seen entry.
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":31}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:00.100Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":31}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:00.200Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":300}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(
        messages.len(),
        1,
        "Streaming duplicates should collapse to one entry"
    );
    assert_eq!(
        messages[0].tokens.output, 300,
        "Should keep the max output_tokens"
    );
    assert_eq!(messages[0].tokens.input, 10);
}

#[test]
fn test_deduplication_per_field_max_not_just_output() {
    // Later entry has same output but higher input - should still update input
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":100,"cache_read_input_tokens":5}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:00.100Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":50,"output_tokens":100,"cache_read_input_tokens":20}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.output, 100);
    assert_eq!(
        messages[0].tokens.input, 50,
        "Should keep max input even if output unchanged"
    );
    assert_eq!(
        messages[0].tokens.cache_read, 20,
        "Should keep max cache_read even if output unchanged"
    );
}

#[test]
fn test_deduplication_higher_first_lower_later() {
    // First entry has higher output than later - should keep first's higher values
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":500}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:00.100Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":100}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].tokens.output, 500,
        "Should keep max output (first entry)"
    );
    assert_eq!(
        messages[0].tokens.input, 100,
        "Should keep max input (first entry)"
    );
}

#[test]
fn test_deduplication_promotes_provider_hint_from_later_duplicate() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:00.100Z","requestId":"req_001","message":{"id":"msg_001","provider":"openrouter/anthropic","model":"claude-sonnet-4.6","usage":{"input_tokens":120,"output_tokens":75}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].provider_id.as_ref(), "openrouter");
    assert_eq!(messages[0].tokens.input, 120);
    assert_eq!(messages[0].tokens.output, 75);
}

#[test]
fn test_deduplication_promotes_provider_hint_without_later_model() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"assistant","provider":"openrouter/anthropic","timestamp":"2024-12-01T10:00:00.100Z","requestId":"req_001","message":{"id":"msg_001","usage":{"input_tokens":120,"output_tokens":75}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].provider_id.as_ref(), "openrouter");
    assert_eq!(messages[0].tokens.input, 120);
    assert_eq!(messages[0].tokens.output, 75);
}

#[test]
fn test_deduplication_preserves_explicit_provider_against_later_inference() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","provider":"openrouter/anthropic","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:00.100Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":120,"output_tokens":75}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].provider_id.as_ref(), "openrouter");
    assert_eq!(messages[0].tokens.input, 120);
    assert_eq!(messages[0].tokens.output, 75);
}

#[test]
fn test_deduplication_rejects_bad_entry_and_keeps_later_good_duplicate() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","usage":{"input_tokens":10,"output_tokens":50}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:00.100Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":100}}}"#;

    let file = create_test_file(content);
    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(scanned.messages[0].tokens.output, 100);
    let rejection = scanned.rejections.entries().next().unwrap();
    assert_eq!(rejection.key, "missing-model");
    assert!(scanned.interrupted.is_none());
}

#[test]
fn test_deduplication_allows_same_message_different_request() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_002","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":150,"output_tokens":75}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(
        messages.len(),
        2,
        "Different requestId should not be deduplicated"
    );
}

#[test]
fn test_deduplication_uses_message_id_without_request_id_and_keeps_final_timestamp() {
    let content = r#"{"type":"user","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Hello"}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","message":{"id":"msg_stream","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":25}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:03.500Z","message":{"id":"msg_stream","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":250}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.output, 250);
    assert_eq!(messages[0].timestamp, 1_733_047_203_500);
    assert_eq!(
        messages[0].dedup_key,
        Some(crate::records::dedup_hash_str("message:msg_stream"))
    );
}

#[test]
fn test_entries_without_dedup_fields_still_processed() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":200,"output_tokens":100}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(
        messages.len(),
        2,
        "Entries without messageId/requestId should still be processed"
    );
}

#[test]
fn test_user_messages_ignored() {
    let content = r#"{"type":"user","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Hello"}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1, "User messages should be ignored");
    assert_eq!(messages[0].tokens.input, 100);
}

#[test]
fn test_turn_start_detection() {
    // Simulate: user asks → assistant responds → tool_result (as user) → assistant responds
    //         → real user asks again → assistant responds
    // Expected: 2 turns (tool_result should NOT count as a turn)
    let content = r#"{"type":"user","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Hello"}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"user","timestamp":"2024-12-01T10:00:02.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"tu_001","content":"file contents here"}]}}
{"type":"assistant","timestamp":"2024-12-01T10:00:03.000Z","requestId":"req_002","message":{"id":"msg_002","model":"claude-sonnet-4.6","usage":{"input_tokens":200,"output_tokens":80}}}
{"type":"user","timestamp":"2024-12-01T10:00:04.000Z","message":{"content":"Thanks, now do X"}}
{"type":"assistant","timestamp":"2024-12-01T10:00:05.000Z","requestId":"req_003","message":{"id":"msg_003","model":"claude-sonnet-4.6","usage":{"input_tokens":300,"output_tokens":120}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 3, "Only assistant usage is counted");

    // First assistant after first human user → turn start
    assert!(
        messages[0].is_turn_start,
        "First response should be turn start"
    );
    // Assistant after tool_result → NOT a new turn
    assert!(
        !messages[1].is_turn_start,
        "Response after tool_result should NOT be turn start"
    );
    // First assistant after second human user → turn start
    assert!(
        messages[2].is_turn_start,
        "Response after real user input should be turn start"
    );

    let turn_count: usize = messages.iter().filter(|m| m.is_turn_start).count();
    assert_eq!(turn_count, 2, "Should detect 2 turns");
}

#[test]
fn test_turn_start_ignores_system_messages() {
    // XML-tagged content like <local-command-stdout> should not count as turns
    let content = r#"{"type":"user","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Do something"}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"user","timestamp":"2024-12-01T10:00:02.000Z","message":{"content":"<local-command-stdout>ok</local-command-stdout>"}}
{"type":"assistant","timestamp":"2024-12-01T10:00:03.000Z","requestId":"req_002","message":{"id":"msg_002","model":"claude-sonnet-4.6","usage":{"input_tokens":200,"output_tokens":80}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 2);
    assert!(
        messages[0].is_turn_start,
        "First response after human input is a turn"
    );
    assert!(
        !messages[1].is_turn_start,
        "Response after local-command should NOT be a turn"
    );

    let turn_count: usize = messages.iter().filter(|m| m.is_turn_start).count();
    assert_eq!(turn_count, 1);
}

#[test]
fn test_turn_start_without_user_message() {
    // No user message → no turn starts (e.g. a partial log)
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":200,"output_tokens":100}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 2);
    assert!(!messages[0].is_turn_start);
    assert!(!messages[1].is_turn_start);
}

#[test]
fn test_token_breakdown_parsing() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":200,"cache_creation_input_tokens":100}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.input, 1000);
    assert_eq!(messages[0].tokens.output, 500);
    assert_eq!(messages[0].tokens.cache_read, 200);
    assert_eq!(messages[0].tokens.cache_write, 100);
    assert_eq!(messages[0].tokens.cache_write_1h, 0);
    assert_eq!(messages[0].tokens.reasoning, 0);
}

#[test]
fn one_hour_cache_creation_is_a_subset_of_total_writes() {
    for (hourly, expected) in [(None, 0), (Some(0), 0), (Some(60), 60), (Some(140), 100)] {
        let mut usage = serde_json::json!({"cache_creation_input_tokens": 100});
        if let Some(hourly) = hourly {
            usage["cache_creation"] = serde_json::json!({
                "ephemeral_5m_input_tokens": 40,
                "ephemeral_1h_input_tokens": hourly
            });
        }
        let entry = serde_json::json!({
            "type": "assistant", "timestamp": "2026-09-21T10:00:00Z",
            "message": {"id": "message", "model": "claude-sonnet-4.6", "usage": usage}
        });
        let file = create_test_file(&entry.to_string());
        let messages = parse_claude_file(file.path()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tokens.cache_write, 100);
        assert_eq!(messages[0].tokens.cache_write_1h, expected);
        assert_eq!(messages[0].tokens.total(), 100);
    }
}

#[test]
fn one_hour_cache_creation_merges_raw_stream_maxima_before_clamping() {
    let first = serde_json::json!({
        "type": "assistant", "timestamp": "2026-09-21T10:00:00Z", "requestId": "request",
        "message": {"id": "message", "model": "claude-sonnet-4.6", "usage": {
            "cache_creation_input_tokens": 50,
            "cache_creation": {"ephemeral_1h_input_tokens": 100}
        }}
    });
    let last = serde_json::json!({
        "type": "assistant", "timestamp": "2026-09-21T10:00:01Z", "requestId": "request",
        "message": {"id": "message", "model": "claude-sonnet-4.6", "usage": {
            "cache_creation_input_tokens": 200, "output_tokens": 10
        }}
    });
    for entries in [[&first, &last], [&last, &first]] {
        let file = create_test_file(&format!("{}\n{}", entries[0], entries[1]));
        let messages = parse_claude_file(file.path()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tokens.cache_write, 200);
        assert_eq!(messages[0].tokens.cache_write_1h, 100);
        assert_eq!(messages[0].tokens.total(), 210);
    }
}

#[test]
fn invalid_one_hour_cache_creation_reports_damage_and_keeps_confirmed_usage() {
    for hourly in [serde_json::json!(-1), serde_json::json!("invalid")] {
        let interrupts = hourly.is_string();
        let bad = serde_json::json!({
            "type": "assistant", "timestamp": "2026-09-21T10:00:00Z",
            "message": {"id": "bad", "model": "claude-sonnet-4.6", "usage": {
                "cache_creation_input_tokens": 100,
                "cache_creation": {"ephemeral_1h_input_tokens": hourly}
            }}
        });
        let good = serde_json::json!({
            "type": "assistant", "timestamp": "2026-09-21T10:00:01Z",
            "message": {"id": "good", "model": "claude-sonnet-4.6", "usage": {
                "cache_creation_input_tokens": 100,
                "cache_creation": {"ephemeral_1h_input_tokens": 60}
            }}
        });
        let file = create_test_file(&format!("{good}\n{bad}"));
        let scanned = super::parse_claude_file(file.path()).unwrap();
        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.messages[0].tokens.cache_write_1h, 60);
        assert_eq!(scanned.rejections.total(), 1);
        assert_eq!(scanned.interrupted.is_some(), interrupts);
    }
}

#[test]
fn test_opus_4_7_usage_is_parsed_when_usage_metadata_exists() {
    let content = r#"{"type":"assistant","timestamp":"2026-04-16T10:00:00.000Z","requestId":"req_opus47","message":{"id":"msg_opus47","model":"claude-opus-4-7","usage":{"input_tokens":321,"output_tokens":654,"cache_read_input_tokens":987,"cache_creation_input_tokens":111}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "claude-opus-4-7");
    assert_eq!(messages[0].provider_id.as_ref(), "anthropic");
    assert_eq!(messages[0].tokens.input, 321);
    assert_eq!(messages[0].tokens.output, 654);
    assert_eq!(messages[0].tokens.cache_read, 987);
    assert_eq!(messages[0].tokens.cache_write, 111);
}

#[test]
fn tool_result_shapes_and_size_metadata_are_not_api_usage() {
    for tool in [
        serde_json::json!({"type": "tool_result", "tool_use_id": "tool", "content": "long tool output"}),
        serde_json::json!({"type": "tool_result", "tool_use_id": "tool", "tool_output": {"output": "long tool output", "input_tokens": 100}}),
        serde_json::json!({"type": "tool_result", "tool_use_id": "tool", "token_count": 100}),
        serde_json::json!({"type": "tool_result", "tool_use_id": "tool", "usage": {"input_tokens": -1}}),
    ] {
        for row in [
            tool.clone(),
            serde_json::json!({"type": "user", "message": {"content": [tool]}}),
        ] {
            let file = create_test_file(&row.to_string());
            let scanned = super::parse_claude_file(file.path()).unwrap();
            assert!(scanned.messages.is_empty(), "{row}");
            assert!(scanned.rejections.is_empty(), "{row}");
            assert!(scanned.interrupted.is_none(), "{row}");
        }
    }
}

#[test]
fn test_assistant_usage_with_tool_use_is_not_estimated_from_prompt_text() {
    let content = r#"{"type":"assistant","timestamp":"2026-05-27T10:00:00.000Z","message":{"id":"msg_tool_use","model":"claude-sonnet-4.6","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/tmp/large.txt"}}],"usage":{"input_tokens":100,"output_tokens":50}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.input, 100);
    assert_eq!(messages[0].tokens.output, 50);
}

#[test]
fn test_anthropic_prefixed_claude_model_is_preserved() {
    let content = r#"{"type":"assistant","timestamp":"2026-05-27T10:00:00.000Z","message":{"model":"anthropic/claude-4-6-sonnet","usage":{"input_tokens":100,"output_tokens":50}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "anthropic/claude-4-6-sonnet");
    assert_eq!(messages[0].provider_id.as_ref(), "anthropic");
}

#[test]
fn test_deepseek_ai_prefixed_model_uses_deepseek_provider() {
    let content = r#"{"type":"assistant","timestamp":"2026-06-04T19:32:18.247Z","message":{"model":"deepseek-ai/deepseek-v4-flash","usage":{"input_tokens":1008,"output_tokens":0}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].provider_id.as_ref(), "deepseek");
}

#[test]
fn test_multi_provider_models_infer_provider_from_model() {
    let content = r#"{"type":"assistant","timestamp":"2026-02-18T10:00:00.000Z","message":{"model":"claude-opus-4.6","usage":{"input_tokens":100,"output_tokens":10}}}
{"type":"assistant","timestamp":"2026-02-18T10:00:01.000Z","message":{"model":"gpt-5.3-codex","usage":{"input_tokens":200,"output_tokens":20}}}
{"type":"assistant","timestamp":"2026-02-18T10:00:02.000Z","message":{"model":"gemini-3-flash-preview","usage":{"input_tokens":300,"output_tokens":30}}}
{"type":"assistant","timestamp":"2026-02-18T10:00:03.000Z","message":{"model":"MiniMax-M2.1","usage":{"input_tokens":400,"output_tokens":40}}}
{"type":"assistant","timestamp":"2026-02-18T10:00:04.000Z","message":{"model":"glm-5.1","usage":{"input_tokens":500,"output_tokens":50}}}
{"type":"assistant","timestamp":"2026-02-18T10:00:05.000Z","message":{"model":"mimo-v2.5-pro","usage":{"input_tokens":600,"output_tokens":60}}}
{"type":"assistant","timestamp":"2026-02-18T10:00:06.000Z","message":{"model":"kimi-for-coding","usage":{"input_tokens":700,"output_tokens":70}}}
{"type":"assistant","timestamp":"2026-02-18T10:00:07.000Z","message":{"model":"longcat-flash-thinking","usage":{"input_tokens":800,"output_tokens":80}}}
{"type":"assistant","timestamp":"2026-02-18T10:00:08.000Z","message":{"model":"<synthetic>","usage":{"input_tokens":900,"output_tokens":90}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 8);
    assert_eq!(messages[0].provider_id.as_ref(), "anthropic");
    assert_eq!(messages[1].provider_id.as_ref(), "openai");
    assert_eq!(messages[2].provider_id.as_ref(), "google");
    assert_eq!(messages[3].provider_id.as_ref(), "minimax");
    assert_eq!(messages[4].provider_id.as_ref(), "zai");
    assert_eq!(messages[5].provider_id.as_ref(), "xiaomi");
    assert_eq!(messages[6].provider_id.as_ref(), "kimi");
    assert_eq!(messages[7].provider_id.as_ref(), "meituan");
    assert!(!messages
        .iter()
        .any(|msg| msg.model_id.as_ref() == "<synthetic>"));
}

#[test]
fn test_synthetic_placeholder_and_tool_content_are_not_usage() {
    let content = r#"{"type":"assistant","timestamp":"2026-02-18T10:00:00.000Z","message":{"model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}},"isApiErrorMessage":true,"error":"unknown"}
{"type":"user","timestamp":"2026-02-18T10:00:01.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"tool output that should not be estimated under a placeholder model"}]}}
{"type":"assistant","timestamp":"2026-02-18T10:00:02.000Z","message":{"model":"glm-5.1","usage":{"input_tokens":123,"output_tokens":45}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "glm-5.1");
    assert_eq!(messages[0].tokens.input, 123);
    assert_eq!(messages[0].tokens.output, 45);
}

#[test]
fn test_multi_provider_models_prefer_specific_model_over_default_anthropic_hint() {
    let content = r#"{"type":"assistant","provider":"anthropic","timestamp":"2026-02-18T10:00:00.000Z","message":{"model":"gpt-5.3-codex","usage":{"input_tokens":200,"output_tokens":20}}}
{"type":"assistant","provider":"anthropic","timestamp":"2026-02-18T10:00:01.000Z","message":{"model":"glm-5.1","usage":{"input_tokens":300,"output_tokens":30}}}
{"type":"assistant","provider":"anthropic","timestamp":"2026-02-18T10:00:02.000Z","message":{"model":"mimo-v2.5-pro","usage":{"input_tokens":400,"output_tokens":40}}}
{"type":"assistant","provider":"anthropic","timestamp":"2026-02-18T10:00:03.000Z","message":{"model":"kimi-for-coding","usage":{"input_tokens":500,"output_tokens":50}}}
{"type":"assistant","provider":"anthropic","timestamp":"2026-02-18T10:00:04.000Z","message":{"model":"longcat-flash-thinking","usage":{"input_tokens":600,"output_tokens":60}}}
{"type":"assistant","provider":"anthropic","timestamp":"2026-02-18T10:00:05.000Z","message":{"model":"model1","usage":{"input_tokens":700,"output_tokens":70}}}
{"type":"assistant","provider":"anthropic","timestamp":"2026-02-18T10:00:06.000Z","message":{"model":"model2","usage":{"input_tokens":800,"output_tokens":80}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 7);
    assert_eq!(messages[0].model_id.as_ref(), "gpt-5.3-codex");
    assert_eq!(messages[0].provider_id.as_ref(), "openai");
    assert_eq!(messages[1].provider_id.as_ref(), "zai");
    assert_eq!(messages[2].provider_id.as_ref(), "xiaomi");
    assert_eq!(messages[3].provider_id.as_ref(), "kimi");
    assert_eq!(messages[4].provider_id.as_ref(), "meituan");
    assert_eq!(messages[5].provider_id.as_ref(), "deepseek");
    assert_eq!(messages[6].provider_id.as_ref(), "deepseek");
    assert!(
        messages
            .iter()
            .filter(|message| !provider_identity::is_anthropic_model(&message.model_id))
            .all(|message| message.provider_id.as_ref() != "anthropic"),
        "non-Claude models must never be attributed to Anthropic"
    );
}

#[test]
fn test_historical_model_aliases_override_provider_hint() {
    let content = r#"{"type":"assistant","provider":"pandora-deepseek","timestamp":"2026-02-18T10:00:00.000Z","message":{"model":"model1","usage":{"input_tokens":700,"output_tokens":70}}}
{"type":"assistant","provider":"some-reseller","timestamp":"2026-02-18T10:00:01.000Z","message":{"model":"model2","usage":{"input_tokens":800,"output_tokens":80}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].provider_id.as_ref(), "deepseek");
    assert_eq!(messages[1].provider_id.as_ref(), "deepseek");
}

#[test]
fn test_anthropic_hint_without_later_model_does_not_override_non_claude_duplicate() {
    let content = r#"{"type":"assistant","timestamp":"2026-02-18T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"glm-5.1","usage":{"input_tokens":100,"output_tokens":10}}}
{"type":"assistant","provider":"anthropic","timestamp":"2026-02-18T10:00:00.100Z","requestId":"req_001","message":{"id":"msg_001","usage":{"input_tokens":120,"output_tokens":15}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].provider_id.as_ref(), "zai");
    assert_eq!(messages[0].tokens.input, 120);
    assert_eq!(messages[0].tokens.output, 15);
}

#[test]
fn test_multi_provider_models_preserve_reseller_provider_hint() {
    let content = r#"{"type":"assistant","timestamp":"2026-02-18T10:00:00.000Z","message":{"provider":"openrouter/anthropic","model":"claude-opus-4.6","usage":{"input_tokens":100,"output_tokens":10}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].model_id.as_ref(), "claude-opus-4.6");
    assert_eq!(messages[0].provider_id.as_ref(), "openrouter");
}

#[test]
fn test_workspace_metadata_from_explicit_project_path() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","projectPath":"/home/tester/sample-project","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    let (_dir, path) = create_project_file(content, "-home-tester-sample-project", "session.jsonl");

    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].workspace_key.as_deref(),
        Some("/home/tester/sample-project")
    );
    assert_eq!(
        messages[0].workspace_label.as_deref(),
        Some("sample-project")
    );
}

#[test]
fn test_workspace_metadata_prefers_entry_cwd_over_claude_project_path() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","cwd":"/home/tester/01-workspace/tokenx","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    let (_dir, path) =
        create_project_file(content, "-home-tester-01-workspace-tokenx", "session.jsonl");

    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].workspace_key,
        Some("/home/tester/01-workspace/tokenx".into())
    );
    assert_eq!(messages[0].workspace_label.as_deref(), Some("tokenx"));
}

#[test]
fn test_workspace_metadata_prefers_later_entry_cwd_on_dedupe() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_late_cwd","message":{"id":"msg_late_cwd","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_late_cwd","cwd":"/home/tester/01-workspace/tokenx","message":{"id":"msg_late_cwd","model":"claude-sonnet-4.6","usage":{"input_tokens":120,"output_tokens":70}}}"#;
    let (_dir, path) =
        create_project_file(content, "-home-tester-01-workspace-tokenx", "session.jsonl");

    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tokens.input, 120);
    assert_eq!(messages[0].tokens.output, 70);
    assert_eq!(
        messages[0].workspace_key,
        Some("/home/tester/01-workspace/tokenx".into())
    );
    assert_eq!(messages[0].workspace_label.as_deref(), Some("tokenx"));
}

#[test]
fn test_workspace_metadata_normalizes_entry_windows_cwd() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","cwd":"C:\\Users\\TestUser\\Desktop","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    let (_dir, path) = create_project_file(content, "C--Users-TestUser-Desktop", "session.jsonl");

    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].workspace_key,
        Some("C:/Users/TestUser/Desktop".into())
    );
    assert_eq!(messages[0].workspace_label.as_deref(), Some("Desktop"));
}

#[test]
fn test_workspace_metadata_matches_lowercase_drive_to_uppercase_project_slug() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","cwd":"x:\\WorkSapce\\fish-claude","message":{"model":"claude-haiku-4.5","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    let (_dir, path) = create_project_file(content, "X--WorkSapce-fish-claude", "session.jsonl");

    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].workspace_key.as_deref(),
        Some("X:/WorkSapce/fish-claude")
    );
    assert_eq!(messages[0].workspace_label.as_deref(), Some("fish-claude"));
}

#[test]
fn test_workspace_metadata_resolves_project_from_cwd_ancestor() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","cwd":"/home/tester/01-workspace/tokenx/crates/tokenx-engine","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    let (_dir, path) =
        create_project_file(content, "-home-tester-01-workspace-tokenx", "session.jsonl");

    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].workspace_key.as_deref(),
        Some("/home/tester/01-workspace/tokenx")
    );
    assert_eq!(messages[0].workspace_label.as_deref(), Some("tokenx"));
}

#[test]
fn test_workspace_metadata_uses_history_only_after_local_candidates_miss() {
    let home = tempfile::tempdir().unwrap();
    let history_path = home.path().join(".claude/history.jsonl");
    std::fs::create_dir_all(history_path.parent().unwrap()).unwrap();
    std::fs::write(
        &history_path,
        r#"{"project":"/home/tester/history-project","sessionId":"history-session"}"#,
    )
    .unwrap();
    let path = home
        .path()
        .join(".claude/projects/-home-tester-history-project/session.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#,
        )
        .unwrap();

    let resolver = ClaudeProjectResolver::new(Some(home.path()));
    let (scanned, dependency) =
        parse_claude_file_with_project_resolver(&path, &resolver, None).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(
        scanned.messages[0].workspace_key.as_deref(),
        Some("/home/tester/history-project")
    );
    assert_eq!(dependency, ClaudeProjectDependency::ExternalMetadata);
    assert_eq!(resolver.external_loads.load(Ordering::Relaxed), 1);
}

#[test]
fn test_workspace_metadata_keeps_usage_when_project_path_is_unresolved() {
    let home = tempfile::tempdir().unwrap();
    let path = home
        .path()
        .join(".claude/projects/-home-tester-missing/session.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#,
        )
        .unwrap();

    let scanned = super::parse_claude_file_with_home(&path, Some(home.path())).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(scanned.messages[0].tokens.input, 100);
    assert_eq!(
        scanned.messages[0].workspace_key.as_deref(),
        Some("-home-tester-missing")
    );
    assert_eq!(
        scanned.messages[0].workspace_label.as_deref(),
        Some("-home-tester-missing")
    );
    assert!(scanned.interrupted.is_none());
}

#[test]
fn test_workspace_metadata_keeps_usage_when_project_path_is_ambiguous() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".claude.json"),
        r#"{"projects":{"/home/tester/a-b":{},"/home/tester/a/b":{}}}"#,
    )
    .unwrap();
    let path = home
        .path()
        .join(".claude/projects/-home-tester-a-b/session.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#,
        )
        .unwrap();

    let scanned = super::parse_claude_file_with_home(&path, Some(home.path())).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(
        scanned.messages[0].workspace_key.as_deref(),
        Some("-home-tester-a-b")
    );
    assert_eq!(
        scanned.messages[0].workspace_label.as_deref(),
        Some("-home-tester-a-b")
    );
    assert!(scanned.interrupted.is_none());
}

#[test]
fn test_workspace_metadata_uses_project_slug_when_explicit_cwd_does_not_match() {
    let content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","cwd":"/home/tester/other","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    let (_dir, path) = create_project_file(content, "-home-tester-expected", "session.jsonl");

    let scanned = super::parse_claude_file_with_home(&path, None).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(
        scanned.messages[0].workspace_key.as_deref(),
        Some("-home-tester-expected")
    );
    assert_eq!(
        scanned.messages[0].workspace_label.as_deref(),
        Some("-home-tester-expected")
    );
}

#[test]
fn test_blank_entry_type_cannot_supply_project_path() {
    let content = r#"{"type":"","projectPath":"/home/tester/invalid-candidate"}
{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    let (_dir, path) =
        create_project_file(content, "-home-tester-invalid-candidate", "session.jsonl");

    let scanned = super::parse_claude_file_with_home(&path, None).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(scanned.rejections.total(), 1);
    assert_eq!(
        scanned.messages[0].workspace_key.as_deref(),
        Some("-home-tester-invalid-candidate")
    );
}

#[test]
fn test_claude_project_key_matches_long_path_hashing() {
    let path = format!("/{}", "a".repeat(205));
    let expected = format!("-{}-bn8w8e", "a".repeat(199));

    assert_eq!(claude_project_key(&path), expected);
}

#[test]
fn test_project_resolution_does_not_reuse_local_candidate_between_files() {
    let resolver = ClaudeProjectResolver::new(None);
    let input = Path::new("/home/tester/.claude/projects/-home-tester-a-b/session.jsonl");
    let mut hyphenated = ClaudeProjectCandidates::default();
    hyphenated.record(Some("/home/tester/a-b"), None);
    let mut nested = ClaudeProjectCandidates::default();
    nested.record(Some("/home/tester/a/b"), None);

    let first = resolver.resolve("-home-tester-a-b", &hyphenated, input, None);
    let second = resolver.resolve("-home-tester-a-b", &nested, input, None);
    let metadata_less = resolver.resolve(
        "-home-tester-a-b",
        &ClaudeProjectCandidates::default(),
        input,
        None,
    );

    assert_eq!(
        first,
        (
            resolved_workspace("/home/tester/a-b"),
            ClaudeProjectDependency::None
        )
    );
    assert_eq!(
        second,
        (
            resolved_workspace("/home/tester/a/b"),
            ClaudeProjectDependency::None
        )
    );
    assert_eq!(
        metadata_less,
        (
            ClaudeProjectResolution::Unresolved,
            ClaudeProjectDependency::ExternalMetadata
        )
    );
    assert_eq!(resolver.external_loads.load(Ordering::Relaxed), 1);
}

#[test]
fn test_wrapper_transcript_with_usage_is_parsed() {
    let content = r#"{"type":"user","timestamp":"2026-04-01T10:00:00.000Z","message":{"content":"Wrapped prompt"}}
{"type":"assistant","timestamp":"2026-04-01T10:00:01.000Z","requestId":"req_wrapper","message":{"id":"msg_wrapper","model":"claude-sonnet-4","usage":{"input_tokens":123,"output_tokens":45,"cache_read_input_tokens":67,"cache_creation_input_tokens":8}}}"#;
    let (_dir, path) = create_transcript_file(content, "ses_123456789012345678901234567.jsonl");

    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].session_id.as_ref(),
        "ses_123456789012345678901234567"
    );
    assert_eq!(messages[0].model_id.as_ref(), "claude-sonnet-4");
    assert_eq!(messages[0].tokens.input, 123);
    assert_eq!(messages[0].tokens.output, 45);
    assert_eq!(messages[0].tokens.cache_read, 67);
    assert_eq!(messages[0].tokens.cache_write, 8);
    assert_eq!(messages[0].workspace_key, None);
    assert_eq!(messages[0].workspace_label, None);
}

#[test]
fn test_wrapper_transcript_without_usage_is_skipped() {
    let content = r#"{"type":"user","timestamp":"2026-04-01T10:00:00.000Z","message":{"content":"Wrapped prompt"}}
{"type":"tool_use","timestamp":"2026-04-01T10:00:01.000Z","message":{"content":"Run tool"}}
{"type":"tool_result","timestamp":"2026-04-01T10:00:02.000Z","tool_use_id":"toolu_wrapper","content":"Tool result with root-level output text"}"#;
    let (_dir, path) = create_transcript_file(content, "ses_765432109876543210987654321.jsonl");

    let messages = parse_claude_file(&path).unwrap();

    assert!(
        messages.is_empty(),
        "wrapper transcripts without usage metadata must not be estimated"
    );
}

// --- Sidechain / Agent tracking tests ---

/// Helper: create a sidechain JSONL file and optional meta sidecar in a nested layout.
fn create_sidechain_files(
    project: &str,
    parent_session: &str,
    agent_file_stem: &str,
    jsonl_content: &str,
    meta_content: Option<&str>,
) -> (TempDir, std::path::PathBuf) {
    let temp_dir = tempfile::tempdir().unwrap();
    let subagents_dir = temp_dir
        .path()
        .join(".claude")
        .join("projects")
        .join(project)
        .join(parent_session)
        .join("subagents");
    std::fs::create_dir_all(&subagents_dir).unwrap();

    let jsonl_path = subagents_dir.join(format!("{}.jsonl", agent_file_stem));
    std::fs::write(&jsonl_path, jsonl_content).unwrap();

    if let Some(meta) = meta_content {
        let meta_path = subagents_dir.join(format!("{}.meta.json", agent_file_stem));
        std::fs::write(&meta_path, meta).unwrap();
    }

    (temp_dir, jsonl_path)
}

#[test]
fn test_sidechain_nested_with_meta_sidecar() {
    let jsonl = r#"{"type":"user","isSidechain":true,"sessionId":"parent-uuid-001","agentId":"abc123","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Find files"}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-uuid-001","agentId":"abc123","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_s01","message":{"id":"msg_s01","model":"claude-sonnet-4.6","usage":{"input_tokens":200,"output_tokens":80,"cache_read_input_tokens":50}}}"#;
    let meta = r#"{"agentType":"explore","description":"Find session creation UI"}"#;

    let (_dir, path) = create_sidechain_files(
        "myproject",
        "parent-uuid-001",
        "agent-abc123",
        jsonl,
        Some(meta),
    );
    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].agent,
        Some("Claude Explore".into()),
        "Should resolve agent name from meta sidecar and normalize"
    );
    assert_eq!(
        messages[0].session_id.as_ref(),
        "parent-uuid-001",
        "Should use parent session ID from transcript, not filename"
    );
    assert_eq!(messages[0].tokens.input, 200);
    assert_eq!(messages[0].tokens.output, 80);
    assert_eq!(messages[0].tokens.cache_read, 50);
    assert!(!messages[0].is_main_session);
}

#[test]
fn test_fork_context_sidechain_is_child_and_keeps_legacy_session_id() {
    let jsonl = r#"{"type":"fork-context-ref","uuid":"fork-ref-001"}
{"type":"user","isSidechain":true,"sessionId":"parent-fork-001","agentId":"fork1","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Continue delegated work"}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-fork-001","agentId":"fork1","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_fork_01","message":{"id":"msg_fork_01","model":"claude-sonnet-4.6","usage":{"input_tokens":120,"output_tokens":40}}}"#;
    let (_dir, path) =
        create_sidechain_files("myproject", "parent-fork-001", "agent-fork1", jsonl, None);

    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].session_id.as_ref(), "agent-fork1");
    assert!(!messages[0].is_main_session);
}

#[test]
fn test_sidechain_temporary_meta_agent_type_uses_generic_identity() {
    let jsonl = r#"{"type":"user","isSidechain":true,"sessionId":"parent-temp-001","agentId":"temp1","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Scan auth"}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-temp-001","agentId":"temp1","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_temp","message":{"id":"msg_temp","model":"claude-sonnet-4.6","usage":{"input_tokens":200,"output_tokens":80}}}"#;
    let meta = r#"{"agentType":"auth-scanner"}"#;

    let (_dir, path) = create_sidechain_files(
        "myproject",
        "parent-temp-001",
        "agent-temp1",
        jsonl,
        Some(meta),
    );
    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].agent.as_deref(), Some("Claude Subagent"));
}

#[test]
fn test_sidechain_nested_without_meta_falls_back() {
    let jsonl = r#"{"type":"user","isSidechain":true,"sessionId":"parent-uuid-002","agentId":"def456","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Do something"}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-uuid-002","agentId":"def456","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_s02","message":{"id":"msg_s02","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":40}}}"#;

    let (_dir, path) =
        create_sidechain_files("myproject", "parent-uuid-002", "agent-def456", jsonl, None);
    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].agent,
        Some("Claude Subagent".into()),
        "Without meta sidecar, should fall back to generic label"
    );
    assert_eq!(messages[0].session_id.as_ref(), "parent-uuid-002");
}

#[test]
fn test_sidechain_flat_legacy_layout() {
    // Flat layout: agent file lives directly under the project dir, no meta sidecar
    let jsonl = r#"{"type":"user","isSidechain":true,"sessionId":"legacy-session-001","agentId":"ac0c74c","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Warmup"}}
{"type":"assistant","isSidechain":true,"sessionId":"legacy-session-001","agentId":"ac0c74c","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_l01","message":{"id":"msg_l01","model":"claude-sonnet-4.6","usage":{"input_tokens":150,"output_tokens":60}}}"#;

    let (_dir, path) = create_project_file(jsonl, "myproject", "agent-ac0c74c.jsonl");
    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].agent,
        Some("Claude Subagent".into()),
        "Legacy flat layout has no meta → Tier 3 fallback"
    );
    assert_eq!(
        messages[0].session_id.as_ref(),
        "legacy-session-001",
        "Should use parent session ID from transcript body"
    );
}

#[test]
fn test_sidechain_session_id_correction() {
    // Multiple sidechain files from the same parent should share the parent's session_id
    let make_jsonl = |agent_id: &str, req: &str, msg: &str| {
        format!(
            r#"{{"type":"user","isSidechain":true,"sessionId":"shared-parent-uuid","agentId":"{agent_id}","timestamp":"2024-12-01T10:00:00.000Z","message":{{"content":"task"}}}}
{{"type":"assistant","isSidechain":true,"sessionId":"shared-parent-uuid","agentId":"{agent_id}","timestamp":"2024-12-01T10:00:01.000Z","requestId":"{req}","message":{{"id":"{msg}","model":"claude-sonnet-4.6","usage":{{"input_tokens":100,"output_tokens":50}}}}}}"#
        )
    };

    let (_dir1, path1) = create_sidechain_files(
        "myproject",
        "shared-parent-uuid",
        "agent-aaa",
        &make_jsonl("aaa", "req_a", "msg_a"),
        Some(r#"{"agentType":"explore"}"#),
    );
    let (_dir2, path2) = create_sidechain_files(
        "myproject",
        "shared-parent-uuid",
        "agent-bbb",
        &make_jsonl("bbb", "req_b", "msg_b"),
        Some(r#"{"agentType":"general-purpose"}"#),
    );
    let (_dir3, path3) = create_sidechain_files(
        "myproject",
        "shared-parent-uuid",
        "agent-ccc",
        &make_jsonl("ccc", "req_c", "msg_c"),
        None,
    );

    let msgs1 = parse_claude_file(&path1).unwrap();
    let msgs2 = parse_claude_file(&path2).unwrap();
    let msgs3 = parse_claude_file(&path3).unwrap();

    // All three should share the parent session ID
    assert_eq!(msgs1[0].session_id.as_ref(), "shared-parent-uuid");
    assert_eq!(msgs2[0].session_id.as_ref(), "shared-parent-uuid");
    assert_eq!(msgs3[0].session_id.as_ref(), "shared-parent-uuid");

    // Agent names should differ
    assert_eq!(msgs1[0].agent.as_deref(), Some("Claude Explore"));
    assert_eq!(msgs2[0].agent.as_deref(), Some("Claude General Purpose"));
    assert_eq!(msgs3[0].agent.as_deref(), Some("Claude Subagent"));
}

#[test]
fn test_sidechain_token_totals_preserved() {
    // Verify that sidechain parsing doesn't change token accounting
    let sidechain_jsonl = r#"{"type":"user","isSidechain":true,"sessionId":"parent-001","agentId":"xyz","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"task"}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-001","agentId":"xyz","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_t1","message":{"id":"msg_t1","model":"claude-sonnet-4.6","usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":200,"cache_creation_input_tokens":100}}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-001","agentId":"xyz","timestamp":"2024-12-01T10:00:02.000Z","requestId":"req_t2","message":{"id":"msg_t2","model":"claude-sonnet-4.6","usage":{"input_tokens":800,"output_tokens":300,"cache_read_input_tokens":150,"cache_creation_input_tokens":50}}}"#;

    let (_dir, path) = create_sidechain_files(
        "myproject",
        "parent-001",
        "agent-xyz",
        sidechain_jsonl,
        Some(r#"{"agentType":"code-reviewer"}"#),
    );
    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 2);

    let total_input: i64 = messages.iter().map(|m| m.tokens.input).sum();
    let total_output: i64 = messages.iter().map(|m| m.tokens.output).sum();
    let total_cache_read: i64 = messages.iter().map(|m| m.tokens.cache_read).sum();
    let total_cache_write: i64 = messages.iter().map(|m| m.tokens.cache_write).sum();

    assert_eq!(total_input, 1800, "input: 1000 + 800");
    assert_eq!(total_output, 800, "output: 500 + 300");
    assert_eq!(total_cache_read, 350, "cache_read: 200 + 150");
    assert_eq!(total_cache_write, 150, "cache_write: 100 + 50");

    // Both messages should have the same agent
    assert_eq!(messages[0].agent.as_deref(), Some("Claude Subagent"));
    assert_eq!(messages[1].agent.as_deref(), Some("Claude Subagent"));
}

#[test]
fn test_main_session_no_agent_regression() {
    // Non-sidechain (main session) files must produce agent: None
    let content = r#"{"type":"user","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"Hello"}}
{"type":"assistant","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_m01","message":{"id":"msg_m01","model":"claude-sonnet-4.6","usage":{"input_tokens":500,"output_tokens":200}}}
{"type":"assistant","timestamp":"2024-12-01T10:00:02.000Z","requestId":"req_m02","message":{"id":"msg_m02","model":"claude-sonnet-4.6","usage":{"input_tokens":600,"output_tokens":250}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0].agent, None,
        "Main session messages must not have an agent"
    );
    assert_eq!(messages[1].agent, None);
}

#[test]
fn test_main_session_with_is_sidechain_false() {
    // Explicit isSidechain: false should be treated as main session
    let content = r#"{"type":"assistant","isSidechain":false,"timestamp":"2024-12-01T10:00:00.000Z","requestId":"req_001","message":{"id":"msg_001","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;

    let file = create_test_file(content);
    let messages = parse_claude_file(file.path()).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].agent, None,
        "isSidechain=false should not set agent"
    );
}

#[test]
fn test_sidechain_dedup_preserves_agent() {
    // Streaming duplicates within a sidechain file should still carry the agent
    let jsonl = r#"{"type":"user","isSidechain":true,"sessionId":"parent-dedup","agentId":"dd1","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"task"}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-dedup","agentId":"dd1","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_d1","message":{"id":"msg_d1","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":30}}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-dedup","agentId":"dd1","timestamp":"2024-12-01T10:00:01.100Z","requestId":"req_d1","message":{"id":"msg_d1","model":"claude-sonnet-4.6","usage":{"input_tokens":10,"output_tokens":300}}}"#;

    let (_dir, path) = create_sidechain_files(
        "myproject",
        "parent-dedup",
        "agent-dd1",
        jsonl,
        Some(r#"{"agentType":"architect"}"#),
    );
    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(
        messages.len(),
        1,
        "Streaming duplicates should collapse to one"
    );
    assert_eq!(
        messages[0].tokens.output, 300,
        "Should keep max output_tokens"
    );
    assert_eq!(
        messages[0].agent,
        Some("Claude Subagent".into()),
        "Deduped message should retain the generic agent identity"
    );
    assert_eq!(messages[0].session_id.as_ref(), "parent-dedup");
}

#[test]
fn test_sidechain_meta_with_omc_prefix_agent() {
    // Meta file might contain oh-my-claudecode: prefixed stable agent types.
    let jsonl = r#"{"type":"user","isSidechain":true,"sessionId":"parent-omc","agentId":"omc1","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"task"}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-omc","agentId":"omc1","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_omc","message":{"id":"msg_omc","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;

    let (_dir, path) = create_sidechain_files(
        "myproject",
        "parent-omc",
        "agent-omc1",
        jsonl,
        Some(r#"{"agentType":"oh-my-claudecode:general-purpose"}"#),
    );
    let messages = parse_claude_file(&path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].agent,
        Some("Claude General Purpose".into()),
        "Should strip oh-my-claudecode: prefix and normalize"
    );
}

#[test]
fn test_sidechain_without_session_id_does_not_block_later_records() {
    let jsonl = r#"{"type":"user","isSidechain":true,"agentId":"noid","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"task"}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-valid","agentId":"noid","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_no","message":{"id":"msg_no","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;

    let file = create_test_file(jsonl);
    let scanned = super::parse_claude_file(file.path()).unwrap();

    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(scanned.messages[0].session_id.as_ref(), "parent-valid");
    assert_eq!(scanned.rejections.total(), 1);
    assert_eq!(
        scanned.rejections.entries().next().unwrap().key,
        "malformed-record"
    );
    assert!(scanned.interrupted.is_none());
}

// --- Tier 2: parent session tool_use inference tests ---

#[test]
fn test_tier2_recovers_agent_from_parent_tool_use() {
    // Nested layout: sidechain without meta, but parent session has matching tool_use
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir
        .path()
        .join(".claude")
        .join("projects")
        .join("myproject");
    std::fs::create_dir_all(&project_dir).unwrap();

    // Create parent session file with tool_use (Agent) and tool_result (agentId)
    let parent_session_id = "parent-tier2-uuid";
    let parent_content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"id":"msg_p1","model":"claude-sonnet-4.6","role":"assistant","content":[{"type":"tool_use","id":"toolu_abc","name":"Agent","input":{"subagent_type":"document-specialist","prompt":"Research something"}}],"usage":{"input_tokens":100,"output_tokens":50}}}
{"type":"user","timestamp":"2024-12-01T10:00:01.000Z","message":{"role":"user","content":[{"tool_use_id":"toolu_abc","type":"tool_result","content":[{"type":"text","text":"Found the docs"},{"type":"text","text":"agentId: t2agent1 (use SendMessage with to: 't2agent1' to continue this agent)\n<usage>total_tokens: 5000</usage>"}]}]}}"#;
    let parent_path = project_dir.join(format!("{}.jsonl", parent_session_id));
    std::fs::write(&parent_path, parent_content).unwrap();

    // Create sidechain file (nested layout, no meta sidecar)
    let subagents_dir = project_dir.join(parent_session_id).join("subagents");
    std::fs::create_dir_all(&subagents_dir).unwrap();
    let sidechain_content = r#"{"type":"user","isSidechain":true,"sessionId":"parent-tier2-uuid","agentId":"t2agent1","timestamp":"2024-12-01T10:00:00.500Z","message":{"content":"Research something"}}
{"type":"assistant","isSidechain":true,"sessionId":"parent-tier2-uuid","agentId":"t2agent1","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_t2","message":{"id":"msg_t2","model":"claude-sonnet-4.6","usage":{"input_tokens":300,"output_tokens":120}}}"#;
    let sidechain_path = subagents_dir.join("agent-t2agent1.jsonl");
    std::fs::write(&sidechain_path, sidechain_content).unwrap();

    let messages = parse_claude_file(&sidechain_path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].agent,
        Some("Claude Subagent".into()),
        "Tier 2 should collapse custom parent subagent identities"
    );
    assert_eq!(messages[0].session_id.as_ref(), parent_session_id);
}

#[test]
fn test_tier2_flat_layout_recovers_agent() {
    // Flat layout: sidechain file in same dir as parent, no meta sidecar
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir
        .path()
        .join(".claude")
        .join("projects")
        .join("myproject");
    std::fs::create_dir_all(&project_dir).unwrap();

    let parent_session_id = "flat-parent-uuid";
    let parent_content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"id":"msg_fp","model":"claude-sonnet-4.6","role":"assistant","content":[{"type":"tool_use","id":"toolu_flat","name":"Agent","input":{"subagent_type":"explore","prompt":"Find files"}}],"usage":{"input_tokens":50,"output_tokens":30}}}
{"type":"user","timestamp":"2024-12-01T10:00:01.000Z","message":{"role":"user","content":[{"tool_use_id":"toolu_flat","type":"tool_result","content":[{"type":"text","text":"agentId: flatagent1 (use SendMessage)"}]}]}}"#;
    std::fs::write(
        project_dir.join(format!("{}.jsonl", parent_session_id)),
        parent_content,
    )
    .unwrap();

    let sidechain_content = r#"{"type":"user","isSidechain":true,"sessionId":"flat-parent-uuid","agentId":"flatagent1","timestamp":"2024-12-01T10:00:00.500Z","message":{"content":"task"}}
{"type":"assistant","isSidechain":true,"sessionId":"flat-parent-uuid","agentId":"flatagent1","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_flat","message":{"id":"msg_flat","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    std::fs::write(
        project_dir.join("agent-flatagent1.jsonl"),
        sidechain_content,
    )
    .unwrap();

    let messages = parse_claude_file(&project_dir.join("agent-flatagent1.jsonl")).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].agent,
        Some("Claude Explore".into()),
        "Tier 2 should work for flat layout too"
    );
}

#[test]
fn test_tier1_takes_precedence_over_tier2() {
    // When meta sidecar exists, Tier 1 wins even if parent has a different subagent_type
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir
        .path()
        .join(".claude")
        .join("projects")
        .join("myproject");
    std::fs::create_dir_all(&project_dir).unwrap();

    let parent_session_id = "precedence-parent";
    let parent_content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"id":"msg_prec","model":"claude-sonnet-4.6","role":"assistant","content":[{"type":"tool_use","id":"toolu_prec","name":"Agent","input":{"subagent_type":"explore","prompt":"task"}}],"usage":{"input_tokens":50,"output_tokens":30}}}
{"type":"user","timestamp":"2024-12-01T10:00:01.000Z","message":{"role":"user","content":[{"tool_use_id":"toolu_prec","type":"tool_result","content":[{"type":"text","text":"agentId: precagent1 done"}]}]}}"#;
    std::fs::write(
        project_dir.join(format!("{}.jsonl", parent_session_id)),
        parent_content,
    )
    .unwrap();

    let subagents_dir = project_dir.join(parent_session_id).join("subagents");
    std::fs::create_dir_all(&subagents_dir).unwrap();

    let sidechain_content = r#"{"type":"user","isSidechain":true,"sessionId":"precedence-parent","agentId":"precagent1","timestamp":"2024-12-01T10:00:00.500Z","message":{"content":"task"}}
{"type":"assistant","isSidechain":true,"sessionId":"precedence-parent","agentId":"precagent1","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_prec","message":{"id":"msg_prec2","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    std::fs::write(
        subagents_dir.join("agent-precagent1.jsonl"),
        sidechain_content,
    )
    .unwrap();
    std::fs::write(
        subagents_dir.join("agent-precagent1.meta.json"),
        r#"{"agentType":"plan"}"#,
    )
    .unwrap();

    let messages = parse_claude_file(&subagents_dir.join("agent-precagent1.jsonl")).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].agent,
        Some("Claude Plan".into()),
        "Tier 1 (meta sidecar) should take precedence over Tier 2 (parent lookup)"
    );
}

#[test]
fn test_extract_agent_id_from_text() {
    assert_eq!(
        extract_agent_id_from_text(
            "agentId: a8f80f8f33163def2 (use SendMessage with to: 'a8f80f8f33163def2')"
        ),
        Some("a8f80f8f33163def2".to_string())
    );
    assert_eq!(
        extract_agent_id_from_text("agentId: abc123\n<usage>total_tokens: 5000</usage>"),
        Some("abc123".to_string())
    );
    assert_eq!(extract_agent_id_from_text("no agent id here"), None);
    assert_eq!(
        extract_agent_id_from_text("agentId: "),
        None,
        "Empty agent id should return None"
    );
}

#[test]
fn test_sidechain_agent_id_from_stem_extracts_aside_question_suffix() {
    assert_eq!(
        sidechain_agent_id_from_stem("agent-aside_question-0320a3d71bc1d01e"),
        Some("0320a3d71bc1d01e".to_string())
    );
    assert_eq!(
        sidechain_agent_id_from_stem("agent-flatagent1"),
        Some("flatagent1".to_string())
    );
}

#[test]
fn test_tier2_uses_entry_agent_id_when_filename_prefix_differs() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir
        .path()
        .join(".claude")
        .join("projects")
        .join("myproject");
    std::fs::create_dir_all(&project_dir).unwrap();

    let parent_session_id = "aside-parent-uuid";
    let parent_content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"id":"msg_aside_parent","model":"claude-sonnet-4.6","role":"assistant","content":[{"type":"tool_use","id":"toolu_aside","name":"Agent","input":{"subagent_type":"plan","prompt":"Summarize findings"}}],"usage":{"input_tokens":50,"output_tokens":30}}}
{"type":"user","timestamp":"2024-12-01T10:00:01.000Z","message":{"role":"user","content":[{"tool_use_id":"toolu_aside","type":"tool_result","content":[{"type":"text","text":"agentId: 0320a3d71bc1d01e (use SendMessage)"}]}]}}"#;
    std::fs::write(
        project_dir.join(format!("{}.jsonl", parent_session_id)),
        parent_content,
    )
    .unwrap();

    let subagents_dir = project_dir.join(parent_session_id).join("subagents");
    std::fs::create_dir_all(&subagents_dir).unwrap();
    let sidechain_content = r#"{"type":"user","isSidechain":true,"sessionId":"aside-parent-uuid","agentId":"0320a3d71bc1d01e","timestamp":"2024-12-01T10:00:00.500Z","message":{"content":"task"}}
{"type":"assistant","isSidechain":true,"sessionId":"aside-parent-uuid","agentId":"0320a3d71bc1d01e","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_aside","message":{"id":"msg_aside","model":"claude-sonnet-4.6","usage":{"input_tokens":100,"output_tokens":50}}}"#;
    let sidechain_path = subagents_dir.join("agent-aside_question-0320a3d71bc1d01e.jsonl");
    std::fs::write(&sidechain_path, sidechain_content).unwrap();

    let messages = parse_claude_file(&sidechain_path).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].agent.as_deref(), Some("Claude Plan"));
}

#[test]
fn test_parent_metadata_reuses_parsed_parent_results() {
    let temp_dir = tempfile::tempdir().unwrap();
    let parent_path = temp_dir.path().join("parent.jsonl");
    let initial_parent = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_a","name":"Agent","input":{"subagent_type":"explore"}},{"type":"tool_use","id":"toolu_b","name":"Agent","input":{"subagent_type":"executor"}}]}}
{"type":"user","message":{"content":[{"tool_use_id":"toolu_a","type":"tool_result","content":[{"type":"text","text":"agentId: cacheA"}]},{"tool_use_id":"toolu_b","type":"tool_result","content":[{"type":"text","text":"agentId: cacheB"}]}]}}"#;
    std::fs::write(&parent_path, initial_parent).unwrap();
    let resolver = ClaudeProjectResolver::new(None);

    assert_eq!(
        resolver
            .lookup_subagent_type(&parent_path, "cacheA")
            .unwrap(),
        Some("explore".to_string())
    );

    std::fs::write(
            &parent_path,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_b","name":"Agent","input":{"subagent_type":"writer"}}]}}"#,
        )
        .unwrap();

    assert_eq!(
        resolver
            .lookup_subagent_type(&parent_path, "cacheB")
            .unwrap(),
        Some("executor".to_string())
    );
    assert_eq!(resolver.parent_metadata_load_count(), 1);
}

#[test]
fn test_tier2_multiple_agents_in_same_parent() {
    // Parent spawns multiple agents; each sidechain should get the correct type
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir
        .path()
        .join(".claude")
        .join("projects")
        .join("myproject");
    std::fs::create_dir_all(&project_dir).unwrap();

    let parent_session_id = "multi-agent-parent";
    let parent_content = r#"{"type":"assistant","timestamp":"2024-12-01T10:00:00.000Z","message":{"id":"msg_ma1","model":"claude-sonnet-4.6","role":"assistant","content":[{"type":"tool_use","id":"toolu_m1","name":"Agent","input":{"subagent_type":"explore","prompt":"find files"}}],"usage":{"input_tokens":50,"output_tokens":30}}}
{"type":"user","timestamp":"2024-12-01T10:00:01.000Z","message":{"role":"user","content":[{"tool_use_id":"toolu_m1","type":"tool_result","content":[{"type":"text","text":"agentId: multiA1 done"}]}]}}
{"type":"assistant","timestamp":"2024-12-01T10:00:02.000Z","message":{"id":"msg_ma2","model":"claude-sonnet-4.6","role":"assistant","content":[{"type":"tool_use","id":"toolu_m2","name":"Agent","input":{"subagent_type":"plan","prompt":"implement feature"}}],"usage":{"input_tokens":60,"output_tokens":40}}}
{"type":"user","timestamp":"2024-12-01T10:00:03.000Z","message":{"role":"user","content":[{"tool_use_id":"toolu_m2","type":"tool_result","content":[{"type":"text","text":"agentId: multiB2 done"}]}]}}"#;
    std::fs::write(
        project_dir.join(format!("{}.jsonl", parent_session_id)),
        parent_content,
    )
    .unwrap();

    let subagents_dir = project_dir.join(parent_session_id).join("subagents");
    std::fs::create_dir_all(&subagents_dir).unwrap();

    let make_sidechain = |agent_id: &str| {
        format!(
            r#"{{"type":"user","isSidechain":true,"sessionId":"{parent_session_id}","agentId":"{agent_id}","timestamp":"2024-12-01T10:00:00.500Z","message":{{"content":"task"}}}}
{{"type":"assistant","isSidechain":true,"sessionId":"{parent_session_id}","agentId":"{agent_id}","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req_{agent_id}","message":{{"id":"msg_{agent_id}","model":"claude-sonnet-4.6","usage":{{"input_tokens":100,"output_tokens":50}}}}}}"#
        )
    };

    std::fs::write(
        subagents_dir.join("agent-multiA1.jsonl"),
        make_sidechain("multiA1"),
    )
    .unwrap();
    std::fs::write(
        subagents_dir.join("agent-multiB2.jsonl"),
        make_sidechain("multiB2"),
    )
    .unwrap();

    let msgs_a = parse_claude_file(&subagents_dir.join("agent-multiA1.jsonl")).unwrap();
    let msgs_b = parse_claude_file(&subagents_dir.join("agent-multiB2.jsonl")).unwrap();

    assert_eq!(
        msgs_a[0].agent,
        Some("Claude Explore".into()),
        "First agent should be explore"
    );
    assert_eq!(
        msgs_b[0].agent,
        Some("Claude Plan".into()),
        "Second agent should be plan"
    );
}

#[test]
fn workflow_journal_is_never_ingested() {
    let temp_dir = tempfile::tempdir().unwrap();
    let journal_path = temp_dir
        .path()
        .join("project/session/subagents/workflows/wf/journal.jsonl");
    std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
    std::fs::write(
            &journal_path,
            r#"{"type":"assistant","sessionId":"parent","message":{"id":"fake","model":"claude-sonnet-4.6","usage":{"input_tokens":9999,"output_tokens":9999}}}"#,
        )
        .unwrap();

    assert!(parse_claude_file(&journal_path).unwrap().is_empty());
}

#[test]
fn deep_workflow_transcript_resolves_parent_agent_and_counts_tokens() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_dir = temp_dir.path().join(".claude/projects/project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let parent_session_id = "deep-parent";
    std::fs::write(
            project_dir.join(format!("{parent_session_id}.jsonl")),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_deep","name":"Agent","input":{"subagent_type":"plan"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_deep","content":[{"type":"text","text":"agentId: deepagent (use SendMessage)"}]}]}}"#,
        )
        .unwrap();

    let workflow_dir = project_dir
        .join(parent_session_id)
        .join("subagents/workflows/wf-deep");
    std::fs::create_dir_all(&workflow_dir).unwrap();
    let transcript_path = workflow_dir.join("agent-deepagent.jsonl");
    std::fs::write(
            &transcript_path,
            r#"{"type":"user","isSidechain":true,"sessionId":"deep-parent","agentId":"deepagent","timestamp":"2024-12-01T10:00:00.000Z","message":{"content":"task"}}
{"type":"assistant","isSidechain":true,"sessionId":"deep-parent","agentId":"deepagent","timestamp":"2024-12-01T10:00:01.000Z","requestId":"req-deep","message":{"id":"msg-deep","model":"claude-sonnet-4.6","usage":{"input_tokens":300,"output_tokens":120,"cache_read_input_tokens":40}}}"#,
        )
        .unwrap();

    let messages = parse_claude_file(&transcript_path).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].session_id.as_ref(), parent_session_id);
    assert_eq!(messages[0].agent.as_deref(), Some("Claude Plan"));
    assert_eq!(messages[0].tokens.input, 300);
    assert_eq!(messages[0].tokens.output, 120);
    assert_eq!(messages[0].tokens.cache_read, 40);
}
