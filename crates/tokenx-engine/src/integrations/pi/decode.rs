//! Pi session decoder.
//!
//! Pi owns this wire format. OMP has a separate decoder even where the JSONL
//! records currently resemble Pi records, so either client can evolve without
//! a format switch or cross-client identity parameter.

use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::Deserialize;

use crate::input_health::{InputFailure, RecordRejectionReason, ScannedInput};
use crate::records::error::{SessionParseError, SessionParseResult};
use crate::records::{normalize_workspace_key, workspace_label_from_key, UsageRecord};
use crate::{provider_identity, TokenBreakdown};

const USAGE_OPERATION: &str = "validate Pi usage";

#[derive(Debug, Deserialize)]
struct SessionHeader {
    #[serde(rename = "type")]
    entry_type: String,
    id: String,
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    entry_type: String,
    timestamp: Option<String>,
    message: Option<Message>,
    usage: Option<Usage>,
    model: Option<String>,
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    role: Option<String>,
    usage: Option<Usage>,
    model: Option<String>,
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    input: Option<i64>,
    output: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
    cache_write_1h: Option<i64>,
    reasoning: Option<i64>,
    total_tokens: Option<i64>,
    // These fields belong to OMP. Keeping their presence observable lets Pi
    // reject the wrong wire contract instead of silently accepting it.
    reasoning_tokens: Option<i64>,
    orchestration: Option<serde_json::Value>,
}

pub(crate) fn parse_pi_file(path: &Path) -> SessionParseResult<ScannedInput> {
    let file = std::fs::File::open(path)
        .map_err(|source| SessionParseError::new("open Pi JSONL input", source))?;
    let reader = BufReader::new(file);
    let mut scanned = ScannedInput {
        messages: Vec::with_capacity(64),
        ..ScannedInput::default()
    };
    let mut buffer = Vec::with_capacity(4096);
    let mut session_id = None;
    let mut workspace_key = None;
    let mut workspace_label = None;
    let agent_instance = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned);

    for (line_index, line) in reader.lines().enumerate() {
        let line_number = line_index + 1;
        let line = match line {
            Ok(line) => line,
            Err(source) if session_id.is_none() => {
                return Err(SessionParseError::new("read Pi JSONL line", source));
            }
            Err(source) => {
                scanned.interrupted = Some(InputFailure::new(
                    "read Pi JSONL line",
                    format!("{} line {line_number}: {source}", path.display()),
                ));
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if session_id.is_none() {
            refill_json_buffer(trimmed, &mut buffer);
            let header = match simd_json::from_slice::<SessionHeader>(&mut buffer) {
                Ok(header) if header.entry_type == "session" && !header.id.trim().is_empty() => {
                    header
                }
                Ok(_) | Err(_) => {
                    reject_record(&mut scanned);
                    continue;
                }
            };
            session_id = Some(header.id);
            workspace_key = header.cwd.as_deref().and_then(normalize_workspace_key);
            workspace_label = workspace_key.as_deref().and_then(workspace_label_from_key);
            continue;
        }

        refill_json_buffer(trimmed, &mut buffer);
        let entry = match simd_json::from_slice::<Entry>(&mut buffer) {
            Ok(entry) => entry,
            Err(_) => {
                reject_record(&mut scanned);
                continue;
            }
        };
        let message = match entry.entry_type.as_str() {
            "message" => match entry.message {
                Some(message)
                    if matches!(message.role.as_deref(), Some("assistant" | "toolResult")) =>
                {
                    message
                }
                _ => continue,
            },
            "compaction" | "branch_summary" | "usage" => Message {
                role: None,
                usage: entry.usage,
                model: entry.model,
                provider: entry.provider,
            },
            _ => continue,
        };
        let Some(usage) = message.usage else {
            continue;
        };
        let tokens = match token_breakdown(&usage) {
            Ok(tokens) => tokens,
            Err(_) => {
                reject_record(&mut scanned);
                continue;
            }
        };
        if !crate::has_positive_tokens(&tokens) {
            continue;
        }

        let Some(raw_model) = message.model.filter(|model| !model.trim().is_empty()) else {
            scanned
                .rejections
                .record(RecordRejectionReason::MissingModel);
            continue;
        };
        let model = raw_model.trim().to_owned();
        let provider = provider_identity::observed_provider_id(
            message.provider.as_deref().unwrap_or_default(),
            &model,
        );
        let Some(timestamp_text) = entry.timestamp else {
            scanned
                .rejections
                .record(RecordRejectionReason::MissingTimestamp);
            continue;
        };
        let timestamp = match chrono::DateTime::parse_from_rfc3339(&timestamp_text) {
            Ok(timestamp) => timestamp.timestamp_millis(),
            Err(_) => {
                scanned
                    .rejections
                    .record(RecordRejectionReason::MissingTimestamp);
                continue;
            }
        };

        let mut parsed = UsageRecord::new(
            model,
            provider,
            session_id
                .clone()
                .expect("Pi message parsing requires a validated session header"),
            timestamp,
            tokens,
            0.0,
        );
        parsed.set_workspace(workspace_key.clone(), workspace_label.clone());
        parsed.set_agent_instance(agent_instance.clone());
        scanned.messages.push(parsed);
    }

    if session_id.is_none() {
        return Err(SessionParseError::invalid(
            "validate Pi JSONL input",
            "session header is missing",
        ));
    }
    Ok(scanned)
}

fn token_breakdown(usage: &Usage) -> SessionParseResult<TokenBreakdown> {
    if usage.reasoning_tokens.is_some() || usage.orchestration.is_some() {
        return Err(SessionParseError::invalid(
            USAGE_OPERATION,
            "Pi usage contains OMP-only fields",
        ));
    }

    let input = required(usage.input, "input")?;
    let raw_output = required(usage.output, "output")?;
    let cache_read = required(usage.cache_read, "cacheRead")?;
    let cache_write = required(usage.cache_write, "cacheWrite")?;
    let cache_write_1h = usage.cache_write_1h.unwrap_or_default();
    let reported_total = required(usage.total_tokens, "totalTokens")?;
    let reasoning = usage.reasoning.unwrap_or_default();

    for (field, value) in [
        ("input", input),
        ("output", raw_output),
        ("cacheRead", cache_read),
        ("cacheWrite", cache_write),
        ("cacheWrite1h", cache_write_1h),
        ("reasoning", reasoning),
        ("totalTokens", reported_total),
    ] {
        if value < 0 {
            return Err(SessionParseError::invalid(
                USAGE_OPERATION,
                format!("token counts must not be negative: `{field}`"),
            ));
        }
    }

    let expected_total = checked_sum(
        [input, raw_output, cache_read, cache_write],
        "reported totalTokens",
    )?;
    if reported_total != expected_total {
        return Err(SessionParseError::invalid(
            USAGE_OPERATION,
            format!(
                "reported totalTokens is {reported_total}, expected {expected_total} from usage buckets"
            ),
        ));
    }

    // Pi reports reasoning as an inclusive breakdown of output. Some clients
    // have emitted an oversized breakdown while output and total remained
    // authoritative, so clamp only the breakdown.
    let reasoning = reasoning.min(raw_output);
    let tokens = TokenBreakdown {
        input,
        output: raw_output - reasoning,
        cache_read,
        cache_write,
        // Retention duration is detail within the authoritative write total.
        cache_write_1h: cache_write_1h.min(cache_write),
        reasoning,
    };
    if tokens.checked_total() != Some(reported_total) {
        return Err(SessionParseError::invalid(
            USAGE_OPERATION,
            "normalized token total does not match reported totalTokens",
        ));
    }
    Ok(tokens)
}

fn required(value: Option<i64>, field: &str) -> SessionParseResult<i64> {
    value.ok_or_else(|| {
        SessionParseError::invalid(
            USAGE_OPERATION,
            format!("current usage is missing `{field}`"),
        )
    })
}

fn checked_sum(
    values: impl IntoIterator<Item = i64>,
    description: &str,
) -> SessionParseResult<i64> {
    values.into_iter().try_fold(0_i64, |total, value| {
        total.checked_add(value).ok_or_else(|| {
            SessionParseError::invalid(USAGE_OPERATION, format!("{description} exceeds i64::MAX"))
        })
    })
}

fn refill_json_buffer(trimmed: &str, buffer: &mut Vec<u8>) {
    buffer.clear();
    buffer.extend_from_slice(trimmed.as_bytes());
}

fn reject_record(scanned: &mut ScannedInput) {
    scanned
        .rejections
        .record(RecordRejectionReason::MalformedRecord);
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    fn parse(content: &str) -> ScannedInput {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file.flush().unwrap();
        parse_pi_file(file.path()).unwrap()
    }

    #[test]
    fn parses_pi_usage_and_workspace() {
        let scanned = parse(
            r#"{"type":"session","id":"s1","cwd":"/work/project"}
{"type":"message","timestamp":"2026-01-01T00:00:00Z","message":{"role":"assistant","model":"gpt-5","provider":"openai","usage":{"input":10,"output":7,"cacheRead":2,"cacheWrite":1,"reasoning":3,"totalTokens":20}}}"#,
        );

        assert_eq!(scanned.messages.len(), 1);
        let message = &scanned.messages[0];
        assert_eq!(message.session_id.as_ref(), "s1");
        assert_eq!(message.tokens.input, 10);
        assert_eq!(message.tokens.output, 4);
        assert_eq!(message.tokens.reasoning, 3);
        assert_eq!(message.workspace_key.as_deref(), Some("/work/project"));
        assert!(message.is_main_session);
        assert!(message.agent.is_none());
    }

    #[test]
    fn rejects_omp_usage_contract_without_losing_later_pi_usage() {
        let scanned = parse(
            r#"{"type":"session","id":"s1"}
{"type":"message","timestamp":"2026-01-01T00:00:00Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"reasoningTokens":1,"totalTokens":2}}}
{"type":"message","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":2,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":3}}}"#,
        );

        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.rejections.total(), 1);
    }

    #[test]
    fn requires_a_session_header() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"{\"type\":\"message\"}\n").unwrap();
        file.flush().unwrap();

        let error = parse_pi_file(file.path()).unwrap_err();
        assert!(error.to_string().contains("session header is missing"));
    }
    fn operation(kind: &str, model: Option<&str>) -> serde_json::Value {
        let fields = serde_json::json!({"model":model,"provider":"openrouter","usage":{
            "input":10,"output":7,"cacheRead":2,"cacheWrite":3,"cacheWrite1h":2,
            "reasoning":3,"totalTokens":22,"cost":{"total":999}
        }});
        let mut row = serde_json::json!({"type":kind,"timestamp":"2026-10-18T00:00:00Z"});
        if matches!(kind, "assistant" | "toolResult" | "user") {
            row["type"] = serde_json::json!("message");
            row["message"] = fields;
            row["message"]["role"] = serde_json::json!(kind);
        } else {
            row.as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
        }
        row
    }

    fn parse_operations(rows: &[serde_json::Value]) -> ScannedInput {
        let content = std::iter::once(serde_json::json!({"type":"session","id":"pi-operations"}))
            .chain(rows.iter().cloned())
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        parse(&content)
    }

    #[test]
    fn bills_supported_operations_with_recorded_models_and_catalog_costs() {
        let rows = [
            "assistant",
            "toolResult",
            "usage",
            "compaction",
            "branch_summary",
            "user",
        ]
        .map(|kind| operation(kind, Some("gpt-5")));
        let scanned = parse_operations(&rows);
        assert_eq!(scanned.messages.len(), 5);
        assert_eq!(scanned.rejections.total(), 0);
        for record in scanned.messages {
            assert_eq!(record.tokens.total(), 22);
            assert_eq!(record.tokens.reasoning, 3);
            assert_eq!(record.tokens.output, 4);
            assert_eq!(record.tokens.cache_write_1h, 2);
            assert_eq!(record.provider_id.as_ref(), "openrouter");
            assert_eq!(record.cost, 0.0);
        }
    }

    #[test]
    fn missing_operation_models_are_reported_without_inheriting_the_chat_model() {
        let mut rows = vec![operation("assistant", Some("gpt-5"))];
        rows.extend(
            ["usage", "compaction", "branch_summary", "toolResult"]
                .map(|kind| operation(kind, None)),
        );
        rows.push(operation("usage", Some("private-model")));
        let scanned = parse_operations(&rows);
        assert_eq!(scanned.messages.len(), 2);
        assert_eq!(scanned.messages[1].model_id.as_ref(), "private-model");
        assert_eq!(scanned.rejections.total(), 4);
        assert_eq!(
            scanned.rejections.entries().next().unwrap().key,
            "missing-model"
        );
    }

    #[test]
    fn malformed_operation_detail_is_rejected_and_later_operations_survive() {
        let mut bad = operation("usage", Some("gpt-5"));
        bad["usage"]["cacheWrite1h"] = serde_json::json!(-1);
        let mut oversize = operation("usage", Some("gpt-5"));
        oversize["usage"]["cacheWrite1h"] = serde_json::json!(100);
        let scanned = parse_operations(&[bad, oversize]);
        assert_eq!(scanned.rejections.total(), 1);
        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.messages[0].tokens.cache_write_1h, 3);
        assert_eq!(scanned.messages[0].tokens.total(), 22);
    }
}
