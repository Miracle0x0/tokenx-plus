//! DeepSeek Harness (DSH) session decoder.
//!
//! DSH writes one append-only JSONL transcript per session. The physical
//! encoding is selected independently of the file name, so the decoder sniffs
//! the zstd frame magic and otherwise parses the payload as plain JSONL.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::input_health::{InputFailure, RecordRejectionReason, ScannedInput};
use crate::records::error::{SessionParseError, SessionParseResult};
use crate::records::{
    dedup_hash_str, normalize_workspace_key, workspace_label_from_key, UsageRecord,
};
use crate::{provider_identity, TokenBreakdown};

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const ZSTD_CHUNK_BYTES: usize = 128 * 1024;

struct DecodedSession {
    bytes: Vec<u8>,
    interrupted: Option<InputFailure>,
}

pub(crate) fn parse_dsh_file(path: &Path) -> SessionParseResult<ScannedInput> {
    let decoded = read_session_bytes(path)?;
    let mut scanned = ScannedInput {
        interrupted: decoded.interrupted,
        ..Default::default()
    };

    let session_id_from_path = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);

    let mut session_id = None;
    let mut workspace_key = None;
    let mut seed_length = 0_i64;
    let mut request_provider = None;
    let mut request_model = None;
    let mut seen = HashSet::new();
    let mut started_turns = HashSet::new();
    let mut pending_user_turn = false;

    let parse_len = complete_prefix_len(&decoded.bytes, scanned.interrupted.is_some());
    for (line_index, raw_line) in decoded.bytes[..parse_len]
        .split(|byte| *byte == b'\n')
        .enumerate()
    {
        let raw_line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        let raw_line = if line_index == 0 {
            raw_line
                .strip_prefix("\u{feff}".as_bytes())
                .unwrap_or(raw_line)
        } else {
            raw_line
        };
        let line = String::from_utf8_lossy(raw_line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(_) => {
                scanned
                    .rejections
                    .record(RecordRejectionReason::MalformedRecord);
                continue;
            }
        };
        let Some(event_type) = non_empty_str(value.get("type")) else {
            continue;
        };

        match event_type {
            "session" => {
                session_id = non_empty_str(value.get("id")).map(str::to_string);
                workspace_key = non_empty_str(value.get("cwd")).and_then(normalize_workspace_key);
                seed_length = value
                    .get("seedLength")
                    .and_then(Value::as_i64)
                    .filter(|length| *length > 0)
                    .unwrap_or(0);
            }
            "request/header" => {
                let config = value.pointer("/data/header/config");
                request_provider = config
                    .and_then(|config| non_empty_str(config.get("provider")))
                    .map(str::to_string);
                request_model = config
                    .and_then(|config| non_empty_str(config.get("model")))
                    .map(str::to_string);
            }
            "user/message" => pending_user_turn = true,
            "assistant/message" => {
                if seed_length > 0
                    && value
                        .get("seq")
                        .and_then(Value::as_i64)
                        .is_some_and(|seq| seq < seed_length)
                {
                    continue;
                }

                let Some(usage) = value.pointer("/data/usage") else {
                    continue;
                };
                let tokens = match tokens_from_usage(usage) {
                    Ok(Some(tokens)) => tokens,
                    Ok(None) => continue,
                    Err(()) => {
                        scanned
                            .rejections
                            .record(RecordRejectionReason::MalformedRecord);
                        continue;
                    }
                };

                let source = value.pointer("/data/message/source");
                let model_id = source
                    .and_then(|source| non_empty_str(source.get("model")))
                    .map(str::to_string)
                    .or_else(|| request_model.clone());
                let Some(model_id) = model_id else {
                    scanned
                        .rejections
                        .record(RecordRejectionReason::MissingModel);
                    continue;
                };

                let Some(timestamp) = value
                    .get("time")
                    .and_then(Value::as_i64)
                    .filter(|timestamp| *timestamp > 0)
                else {
                    scanned
                        .rejections
                        .record(RecordRejectionReason::MissingTimestamp);
                    continue;
                };

                let sid = session_id.clone().or_else(|| session_id_from_path.clone());
                let Some(sid) = sid else {
                    scanned
                        .rejections
                        .record(RecordRejectionReason::MissingSession);
                    continue;
                };

                let source_provider = source
                    .and_then(|source| non_empty_str(source.get("provider")))
                    .filter(|provider| is_observed_provider(provider));
                let header_provider = request_provider
                    .as_deref()
                    .filter(|provider| is_observed_provider(provider));
                let provider_id = provider_identity::observed_provider_id(
                    source_provider.or(header_provider).unwrap_or_default(),
                    &model_id,
                );

                let identity = value
                    .pointer("/data/message/id")
                    .and_then(|id| non_empty_str(Some(id)))
                    .map_or_else(|| format!("sid:{sid}"), |id| format!("msg:{id}"));
                let dedup_key = dedup_hash_str(&format!(
                    "dsh:{identity}:{timestamp}:{provider_id}:{model_id}:{}:{}:{}:{}:{}",
                    tokens.input,
                    tokens.output,
                    tokens.cache_read,
                    tokens.cache_write,
                    tokens.reasoning
                ));
                if !seen.insert(dedup_key) {
                    continue;
                }

                let is_turn_start = match value.pointer("/data/turn").and_then(Value::as_i64) {
                    Some(turn) => started_turns.insert(turn),
                    None => std::mem::take(&mut pending_user_turn),
                };

                let mut message = UsageRecord::new_with_dedup(
                    model_id,
                    provider_id,
                    sid,
                    timestamp,
                    tokens,
                    0.0,
                    Some(dedup_key),
                );
                message.is_turn_start = is_turn_start;
                if let Some(key) = workspace_key.clone() {
                    let label = workspace_label_from_key(&key);
                    message.set_workspace(Some(key), label);
                }
                scanned.messages.push(message);
            }
            _ => {}
        }
    }

    Ok(scanned)
}

fn read_session_bytes(path: &Path) -> SessionParseResult<DecodedSession> {
    let raw = std::fs::read(path)
        .map_err(|error| SessionParseError::at_path(path, "read DSH session", error))?;
    if !raw.starts_with(&ZSTD_MAGIC) {
        return Ok(DecodedSession {
            bytes: raw,
            interrupted: None,
        });
    }

    let mut decoder = zstd::stream::read::Decoder::new(raw.as_slice())
        .map_err(|error| SessionParseError::at_path(path, "decode DSH zstd stream", error))?;
    let mut bytes = Vec::new();
    let mut chunk = vec![0_u8; ZSTD_CHUNK_BYTES];
    loop {
        match decoder.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            Err(error) => {
                let error = SessionParseError::at_path(path, "decode DSH zstd stream", error);
                if bytes.is_empty() {
                    return Err(error);
                }
                return Ok(DecodedSession {
                    bytes,
                    interrupted: Some(InputFailure::from(&error)),
                });
            }
        }
    }

    Ok(DecodedSession {
        bytes,
        interrupted: None,
    })
}

fn complete_prefix_len(bytes: &[u8], interrupted: bool) -> usize {
    if !interrupted || bytes.ends_with(b"\n") {
        return bytes.len();
    }
    bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1)
}

fn non_empty_str(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_observed_provider(provider: &str) -> bool {
    !provider.eq_ignore_ascii_case("unknown")
        && !(provider.starts_with('<') && provider.ends_with('>'))
}

fn tokens_from_usage(usage: &Value) -> Result<Option<TokenBreakdown>, ()> {
    if !usage.is_object() {
        return Err(());
    }

    let input = token_field(usage, "inputTokens")?;
    let raw_output = token_field(usage, "outputTokens")?;
    let cache_read = token_field(usage, "cacheReadTokens")?;
    let cache_write = token_field(usage, "cacheWriteTokens")?;
    let reasoning = token_field(usage, "reasoningTokens")?;
    if [input, raw_output, cache_read, cache_write, reasoning]
        .into_iter()
        .any(|tokens| tokens < 0)
    {
        return Err(());
    }

    let tokens = TokenBreakdown {
        input,
        output: raw_output.saturating_sub(reasoning).max(0),
        cache_read,
        cache_write,
        reasoning,
    };
    match tokens.checked_total() {
        Some(0) => Ok(None),
        Some(_) => Ok(Some(tokens)),
        None => Err(()),
    }
}

fn token_field(usage: &Value, field: &str) -> Result<i64, ()> {
    match usage.get(field) {
        None | Some(Value::Null) => Ok(0),
        Some(value) => value.as_i64().ok_or(()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use super::*;

    fn session_path(root: &Path, session_id: &str, compressed: bool) -> PathBuf {
        root.join(session_id).join(if compressed {
            "session.jsonl.zstd"
        } else {
            "session.jsonl"
        })
    }

    fn write_plain_session(root: &Path, session_id: &str, lines: &[&str]) -> PathBuf {
        let path = session_path(root, session_id, false);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        path
    }

    fn write_zstd_session(root: &Path, session_id: &str, lines: &[&str]) -> PathBuf {
        let path = session_path(root, session_id, true);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = format!("{}\n", lines.join("\n"));
        std::fs::write(&path, zstd::encode_all(payload.as_bytes(), 3).unwrap()).unwrap();
        path
    }

    fn rejection_count(scanned: &ScannedInput, key: &str) -> u64 {
        scanned
            .rejections
            .entries()
            .find(|entry| entry.key == key)
            .map_or(0, |entry| entry.count)
    }

    #[test]
    fn zstd_session_maps_buckets_workspace_and_turns() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_zstd_session(
            dir.path(),
            "folder-session",
            &[
                r#"{"type":"session","id":"header-session","cwd":"e:\\repo\\proj"}"#,
                r#"{"type":"user/message","seq":1,"time":1786669450000,"data":{"turn":1}}"#,
                r#"{"type":"assistant/message","seq":2,"time":1786669454772,"data":{"turn":1,"message":{"id":"message-1","source":{"provider":"irix","model":"deepseek-v4-flash"}},"usage":{"inputTokens":10,"outputTokens":60,"cacheReadTokens":30,"cacheWriteTokens":40,"reasoningTokens":50}}}"#,
                r#"{"type":"assistant/message","seq":3,"time":1786669455000,"data":{"turn":1,"message":{"id":"message-2","source":{"provider":"irix","model":"deepseek-v4-flash"}},"usage":{"inputTokens":11,"outputTokens":21}}}"#,
                r#"{"type":"assistant/message","seq":4,"time":1786669456000,"data":{"turn":2,"message":{"id":"message-3","source":{"provider":"irix","model":"deepseek-v4-flash"}},"usage":{"inputTokens":12,"outputTokens":22}}}"#,
            ],
        );

        let scanned = parse_dsh_file(&path).unwrap();

        assert!(scanned.interrupted.is_none());
        assert!(scanned.rejections.is_empty());
        assert_eq!(scanned.messages.len(), 3);
        let first = &scanned.messages[0];
        assert_eq!(first.session_id.as_ref(), "header-session");
        assert_eq!(first.provider_id.as_ref(), "irix");
        assert_eq!(first.model_id.as_ref(), "deepseek-v4-flash");
        assert_eq!(first.tokens.input, 10);
        assert_eq!(first.tokens.output, 10);
        assert_eq!(first.tokens.cache_read, 30);
        assert_eq!(first.tokens.cache_write, 40);
        assert_eq!(first.tokens.reasoning, 50);
        assert_eq!(first.workspace_key.as_deref(), Some("E:/repo/proj"));
        assert_eq!(first.workspace_label.as_deref(), Some("proj"));
        assert!(first.is_turn_start);
        assert!(!scanned.messages[1].is_turn_start);
        assert!(scanned.messages[2].is_turn_start);
    }

    #[test]
    fn reasoning_overlap_uses_saturating_output_subtraction() {
        let usage = serde_json::json!({
            "outputTokens": 20,
            "reasoningTokens": 31
        });

        let tokens = tokens_from_usage(&usage).unwrap().unwrap();

        assert_eq!(tokens.output, 0);
        assert_eq!(tokens.reasoning, 31);
        assert_eq!(tokens.total(), 31);
    }

    #[test]
    fn plain_session_uses_request_routing_and_parent_folder() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_plain_session(
            dir.path(),
            "folder-session",
            &[
                r#"{"type":"request/header","data":{"header":{"config":{"provider":"header-route","model":"header-model"}}}}"#,
                r#"{"type":"assistant/message","time":1786669454772,"data":{"turn":1,"message":{"id":"message-1"},"usage":{"inputTokens":5,"outputTokens":7}}}"#,
                r#"{"type":"assistant/message","time":1786669455000,"data":{"turn":2,"message":{"id":"message-2","source":{"provider":"source-route","model":"source-model"}},"usage":{"inputTokens":6,"outputTokens":8}}}"#,
            ],
        );

        let scanned = parse_dsh_file(&path).unwrap();

        assert_eq!(scanned.messages.len(), 2);
        assert_eq!(scanned.messages[0].session_id.as_ref(), "folder-session");
        assert_eq!(scanned.messages[0].model_id.as_ref(), "header-model");
        assert_eq!(scanned.messages[0].provider_id.as_ref(), "header-route");
        assert_eq!(scanned.messages[1].model_id.as_ref(), "source-model");
        assert_eq!(scanned.messages[1].provider_id.as_ref(), "source-route");
    }

    #[test]
    fn missing_provider_uses_central_model_inference() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_plain_session(
            dir.path(),
            "session-inferred",
            &[
                r#"{"type":"assistant/message","time":1786669454772,"data":{"message":{"id":"message-1","source":{"model":"deepseek-reasoner"}},"usage":{"inputTokens":5}}}"#,
            ],
        );

        let scanned = parse_dsh_file(&path).unwrap();

        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.messages[0].provider_id.as_ref(), "deepseek");
    }

    #[test]
    fn seed_length_excludes_the_inherited_prefix() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_plain_session(
            dir.path(),
            "session-child",
            &[
                r#"{"type":"session","id":"session-child","cwd":"/work","seedLength":42}"#,
                r#"{"type":"assistant/message","seq":39,"time":1785730448979,"data":{"turn":1,"message":{"id":"parent-message","source":{"provider":"deepseek","model":"deepseek-reasoner"}},"usage":{"inputTokens":2885,"outputTokens":25,"reasoningTokens":23}}}"#,
                r#"{"type":"assistant/message","seq":42,"time":1786358035361,"data":{"turn":2,"message":{"id":"child-message","source":{"provider":"deepseek","model":"deepseek-reasoner"}},"usage":{"inputTokens":97,"outputTokens":39,"cacheReadTokens":2816,"reasoningTokens":34}}}"#,
            ],
        );

        let scanned = parse_dsh_file(&path).unwrap();

        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.messages[0].timestamp, 1786358035361);
        assert_eq!(scanned.messages[0].tokens.input, 97);
    }

    #[test]
    fn message_identity_produces_the_same_dedup_key_across_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let row = r#"{"type":"assistant/message","time":1785730448979,"data":{"turn":1,"message":{"id":"shared-message","source":{"provider":"deepseek","model":"deepseek-reasoner"}},"usage":{"inputTokens":2885,"outputTokens":25,"reasoningTokens":23}}}"#;
        let parent = write_plain_session(
            dir.path(),
            "parent",
            &[r#"{"type":"session","id":"parent"}"#, row],
        );
        let child = write_plain_session(
            dir.path(),
            "child",
            &[r#"{"type":"session","id":"child"}"#, row],
        );

        let parent = parse_dsh_file(&parent).unwrap();
        let child = parse_dsh_file(&child).unwrap();

        assert_ne!(parent.messages[0].session_id, child.messages[0].session_id);
        assert_eq!(parent.messages[0].dedup_key, child.messages[0].dedup_key);
    }

    #[test]
    fn corrupt_zstd_without_a_decodable_prefix_is_an_input_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = session_path(dir.path(), "corrupt", true);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut payload = ZSTD_MAGIC.to_vec();
        payload.extend_from_slice(b"not-a-zstd-frame");
        std::fs::write(&path, payload).unwrap();

        let error = parse_dsh_file(&path).unwrap_err();

        assert_eq!(error.operation(), "decode DSH zstd stream");
        assert_eq!(error.path(), Some(path.as_path()));
    }

    #[test]
    fn torn_zstd_frame_keeps_the_committed_prefix_and_marks_partial() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = session_path(dir.path(), "session-torn", true);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let header = zstd::encode_all(
            &b"{\"type\":\"session\",\"id\":\"session-torn\",\"cwd\":\"/work\"}\n"[..],
            3,
        )
        .unwrap();
        let committed = zstd::encode_all(
            &b"{\"type\":\"assistant/message\",\"time\":1786669454772,\"data\":{\"turn\":1,\"message\":{\"id\":\"committed\",\"source\":{\"provider\":\"p\",\"model\":\"m\"}},\"usage\":{\"inputTokens\":10,\"outputTokens\":20}}}\n"[..],
            3,
        )
        .unwrap();
        let torn = zstd::encode_all(
            &b"{\"type\":\"assistant/message\",\"time\":1786669455000,\"data\":{\"turn\":2,\"message\":{\"id\":\"torn\",\"source\":{\"provider\":\"p\",\"model\":\"m\"}},\"usage\":{\"inputTokens\":11,\"outputTokens\":21}}}\n"[..],
            3,
        )
        .unwrap();
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&header).unwrap();
        file.write_all(&committed).unwrap();
        file.write_all(&torn[..torn.len() / 2]).unwrap();
        drop(file);

        let scanned = parse_dsh_file(&path).unwrap();

        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.messages[0].tokens.input, 10);
        assert_eq!(
            scanned.interrupted.as_ref().unwrap().operation,
            "decode DSH zstd stream"
        );
    }

    #[test]
    fn rejects_negative_overflowing_missing_model_and_missing_timestamp_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_plain_session(
            dir.path(),
            "session-invalid",
            &[
                r#"{"type":"session","id":"session-invalid"}"#,
                r#"{"type":"assistant/message","time":1786669454772,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":-1}}}"#,
                r#"{"type":"assistant/message","time":1786669454773,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":9223372036854775807,"outputTokens":1}}}"#,
                r#"{"type":"assistant/message","time":1786669454774,"data":{"message":{},"usage":{"inputTokens":1}}}"#,
                r#"{"type":"assistant/message","data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":1}}}"#,
                r#"{"type":"assistant/message","data":{"message":{},"usage":{"inputTokens":0,"outputTokens":0}}}"#,
            ],
        );

        let scanned = parse_dsh_file(&path).unwrap();

        assert!(scanned.messages.is_empty());
        assert_eq!(scanned.rejections.total(), 4);
        assert_eq!(rejection_count(&scanned, "malformed-record"), 2);
        assert_eq!(rejection_count(&scanned, "missing-model"), 1);
        assert_eq!(rejection_count(&scanned, "missing-timestamp"), 1);
    }
}
