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

/// Bound the amount of one transcript that can be retained from disk.
///
/// DSH transcripts are parsed in parallel.  A file-controlled allocation here
/// would therefore be multiplied by the number of acquisition workers before
/// any record-level validation can run.
const MAX_TRANSCRIPT_FILE_BYTES: usize = 64 * 1024 * 1024;

/// Bound the expansion of a compressed transcript before it is materialized.
///
/// Zstandard permits very high expansion ratios, so limiting only the bytes
/// read from disk does not bound memory use.
const MAX_DECODED_TRANSCRIPT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
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
            "assistant/message" | "compaction/summary" => {
                let is_summary = event_type == "compaction/summary";
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
                let model_id = served_model(source)
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
                    .map(|id| format!("msg:{id}"))
                    .or_else(|| {
                        value
                            .get("seq")
                            .and_then(Value::as_i64)
                            .map(|seq| format!("seq:{seq}"))
                    })
                    .unwrap_or_else(|| format!("sid:{sid}"));
                let kind = if is_summary { "summary:" } else { "" };
                let dedup_key = dedup_hash_str(&format!(
                    "dsh:{kind}{identity}:{timestamp}:{provider_id}:{model_id}:{}:{}:{}:{}:{}",
                    tokens.input,
                    tokens.output,
                    tokens.cache_read,
                    tokens.cache_write,
                    tokens.reasoning
                ));
                if !seen.insert(dedup_key) {
                    continue;
                }

                let is_turn_start = if is_summary {
                    false
                } else {
                    match value.pointer("/data/turn").and_then(Value::as_i64) {
                        Some(turn) => started_turns.insert(turn),
                        None => std::mem::take(&mut pending_user_turn),
                    }
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
    read_session_bytes_bounded(
        path,
        MAX_TRANSCRIPT_FILE_BYTES,
        MAX_DECODED_TRANSCRIPT_BYTES,
    )
}

fn read_session_bytes_bounded(
    path: &Path,
    max_file_bytes: usize,
    max_decoded_bytes: usize,
) -> SessionParseResult<DecodedSession> {
    let raw = read_bounded_file(path, max_file_bytes)?;
    if !raw.starts_with(&ZSTD_MAGIC) {
        if raw.len() > max_decoded_bytes {
            return Err(size_limit_error(
                path,
                "read DSH session",
                "decoded",
                max_decoded_bytes,
                raw.len(),
            ));
        }
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
        let remaining = max_decoded_bytes.saturating_sub(bytes.len());
        let want = remaining.saturating_add(1).min(chunk.len());
        if want == 0 {
            return Err(size_limit_error(
                path,
                "decode DSH zstd stream",
                "decoded",
                max_decoded_bytes,
                bytes.len(),
            ));
        }
        match decoder.read(&mut chunk[..want]) {
            Ok(0) => break,
            Ok(read) if bytes.len().saturating_add(read) > max_decoded_bytes => {
                return Err(size_limit_error(
                    path,
                    "decode DSH zstd stream",
                    "decoded",
                    max_decoded_bytes,
                    max_decoded_bytes.saturating_add(1),
                ));
            }
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

fn read_bounded_file(path: &Path, max_bytes: usize) -> SessionParseResult<Vec<u8>> {
    let file = std::fs::File::open(path)
        .map_err(|error| SessionParseError::at_path(path, "read DSH session", error))?;
    let mut raw = Vec::new();
    let limit = u64::try_from(max_bytes)
        .expect("DSH transcript byte limit must fit in u64")
        .saturating_add(1);
    file.take(limit)
        .read_to_end(&mut raw)
        .map_err(|error| SessionParseError::at_path(path, "read DSH session", error))?;
    if raw.len() > max_bytes {
        return Err(size_limit_error(
            path,
            "read DSH session",
            "read",
            max_bytes,
            raw.len(),
        ));
    }
    Ok(raw)
}

fn size_limit_error(
    path: &Path,
    operation: &'static str,
    kind: &str,
    limit: usize,
    observed: usize,
) -> SessionParseError {
    SessionParseError::at_path(
        path,
        operation,
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("DSH {kind} transcript exceeds {limit} bytes (observed {observed})"),
        ),
    )
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

fn served_model(source: Option<&Value>) -> Option<&str> {
    source
        .and_then(|source| source.pointer("/replayState/response/responseModel"))
        .and_then(|value| non_empty_str(Some(value)))
        .or_else(|| source.and_then(|source| non_empty_str(source.get("model"))))
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
    fn compaction_summary_is_counted_without_consuming_the_pending_turn() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_plain_session(
            dir.path(),
            "summary-session",
            &[
                r#"{"type":"session","id":"summary-session","cwd":"/work"}"#,
                r#"{"type":"user/message","time":1786669450001,"data":{}}"#,
                r#"{"type":"compaction/summary","seq":8,"time":1786669450002,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":10,"outputTokens":20}}}"#,
                r#"{"type":"assistant/message","seq":9,"time":1786669450003,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":30,"outputTokens":40}}}"#,
            ],
        );

        let scanned = parse_dsh_file(&path).unwrap();

        assert_eq!(scanned.messages.len(), 2);
        assert_eq!(scanned.messages[0].tokens.total(), 30);
        assert!(!scanned.messages[0].is_turn_start);
        assert!(scanned.messages[1].is_turn_start);
        assert_ne!(scanned.messages[0].dedup_key, scanned.messages[1].dedup_key);
    }

    #[test]
    fn compaction_summary_obeys_seed_length_and_skips_rows_without_usage() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_plain_session(
            dir.path(),
            "summary-seed",
            &[
                r#"{"type":"session","id":"summary-seed","seedLength":8}"#,
                r#"{"type":"compaction/summary","seq":7,"time":1786669450001,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":10}}}"#,
                r#"{"type":"compaction/summary","seq":8,"time":1786669450002,"data":{"message":{"source":{"provider":"p","model":"m"}}}}"#,
                r#"{"type":"compaction/summary","seq":9,"time":1786669450003,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":0,"outputTokens":0}}}"#,
                r#"{"type":"compaction/summary","seq":10,"time":1786669450004,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":11,"outputTokens":12}}}"#,
            ],
        );

        let scanned = parse_dsh_file(&path).unwrap();

        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.messages[0].timestamp, 1786669450004);
        assert_eq!(scanned.messages[0].tokens.total(), 23);
    }

    #[test]
    fn response_model_takes_precedence_for_attribution_and_deduplication() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_plain_session(
            dir.path(),
            "served-model",
            &[
                r#"{"type":"assistant/message","seq":1,"time":1786669450002,"data":{"message":{"source":{"provider":"p","model":"requested-model","replayState":{"response":{"responseModel":" served-model "}}}},"usage":{"inputTokens":10,"outputTokens":20}}}"#,
            ],
        );

        let scanned = parse_dsh_file(&path).unwrap();

        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.messages[0].model_id.as_ref(), "served-model");
    }

    #[test]
    fn invalid_response_model_falls_back_to_source_then_request_header() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_plain_session(
            dir.path(),
            "served-model-fallback",
            &[
                r#"{"type":"request/header","data":{"header":{"config":{"provider":"p","model":"header-model"}}}}"#,
                r#"{"type":"assistant/message","seq":1,"time":1786669450001,"data":{"message":{"source":{"provider":"p","model":"source-model","replayState":{"response":{"responseModel":"   "}}}},"usage":{"inputTokens":10}}}"#,
                r#"{"type":"compaction/summary","seq":2,"time":1786669450002,"data":{"message":{"source":{"provider":"p","replayState":{"response":{"responseModel":42}}}},"usage":{"inputTokens":20}}}"#,
            ],
        );

        let scanned = parse_dsh_file(&path).unwrap();

        assert_eq!(scanned.messages.len(), 2);
        assert_eq!(scanned.messages[0].model_id.as_ref(), "source-model");
        assert_eq!(scanned.messages[1].model_id.as_ref(), "header-model");
    }

    #[test]
    fn idless_rows_use_seq_for_cross_file_deduplication() {
        let dir = tempfile::TempDir::new().unwrap();
        let row = r#"{"type":"assistant/message","seq":7,"time":1786669450002,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":10,"outputTokens":20}}}"#;
        let parent = write_plain_session(dir.path(), "parent", &[row]);
        let child = write_plain_session(dir.path(), "child", &[row]);

        let parent = parse_dsh_file(&parent).unwrap();
        let child = parse_dsh_file(&child).unwrap();

        assert_eq!(parent.messages.len(), 1);
        assert_eq!(child.messages.len(), 1);
        assert_eq!(parent.messages[0].dedup_key, child.messages[0].dedup_key);
    }

    #[test]
    fn idless_summary_uses_seq_across_files_without_colliding_with_a_reply() {
        let dir = tempfile::TempDir::new().unwrap();
        let summary = r#"{"type":"compaction/summary","seq":7,"time":1786669450002,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":10,"outputTokens":20}}}"#;
        let reply = r#"{"type":"assistant/message","seq":7,"time":1786669450002,"data":{"message":{"source":{"provider":"p","model":"m"}},"usage":{"inputTokens":10,"outputTokens":20}}}"#;
        let parent = write_plain_session(dir.path(), "parent-summary", &[summary]);
        let child = write_plain_session(dir.path(), "child-summary", &[summary]);
        let reply = write_plain_session(dir.path(), "reply", &[reply]);

        let parent = parse_dsh_file(&parent).unwrap();
        let child = parse_dsh_file(&child).unwrap();
        let reply = parse_dsh_file(&reply).unwrap();

        assert_eq!(parent.messages[0].dedup_key, child.messages[0].dedup_key);
        assert_ne!(parent.messages[0].dedup_key, reply.messages[0].dedup_key);
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

    #[test]
    fn bounded_plain_reads_fail_before_buffering_past_the_limit() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = session_path(dir.path(), "oversized", false);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"0123456789").unwrap();

        let error = read_session_bytes_bounded(&path, 9, 64).unwrap_err();

        assert_eq!(error.operation(), "read DSH session");
        assert!(error.to_string().contains("exceeds 9 bytes"));
    }

    #[test]
    fn bounded_zstd_reads_fail_before_materializing_expansion_past_the_limit() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = session_path(dir.path(), "expanded", true);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let compressed = zstd::encode_all(vec![b'a'; 4096].as_slice(), 3).unwrap();
        assert!(compressed.len() < 128);
        std::fs::write(&path, compressed).unwrap();

        let error = read_session_bytes_bounded(&path, 128, 1024).unwrap_err();

        assert_eq!(error.operation(), "decode DSH zstd stream");
        assert!(error.to_string().contains("exceeds 1024 bytes"));
    }
}
