use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::Deserialize;

use crate::input_health::{InputFailure, RecordRejectionReason, ScannedInput};
use crate::records::error::{SessionParseError, SessionParseResult};
use crate::records::{dedup_hash_str, utils::parse_timestamp_str, UsageRecord};
use crate::{provider_identity, TokenBreakdown};

// <session>/.system_generated/logs/transcript{,_full}.jsonl
pub(super) fn session_name(path: &Path) -> Option<&OsStr> {
    path.ancestors().nth(3)?.file_name()
}

#[derive(Deserialize)]
struct Step {
    #[serde(rename = "type")]
    kind: String,
    source: Option<String>,
    step_index: Option<u64>,
    created_at: Option<String>,
    model: Option<String>,
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cache_read_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
}

fn parse_step(line: &[u8], session: &str) -> Result<Option<UsageRecord>, RecordRejectionReason> {
    let step: Step =
        serde_json::from_slice(line).map_err(|_| RecordRejectionReason::MalformedRecord)?;
    if step.kind != "PLANNER_RESPONSE" || step.source.as_deref() != Some("MODEL") {
        return Ok(None);
    }
    // The log records uncached input and inclusive output. Its thinking text
    // carries no separate counter, so all generated tokens stay in output.
    let tokens = TokenBreakdown {
        input: step.input_tokens,
        cache_read: step.cache_read_tokens,
        output: step.output_tokens,
        ..Default::default()
    };
    match crate::finalization::positive_token_total(&tokens) {
        None => return Err(RecordRejectionReason::InvalidUsageRecord),
        Some(0) => return Ok(None),
        Some(_) => {}
    }
    let step_index = step
        .step_index
        .ok_or(RecordRejectionReason::MalformedRecord)?;
    let model = step
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or(RecordRejectionReason::MissingModel)?;
    let timestamp = step
        .created_at
        .as_deref()
        .and_then(parse_timestamp_str)
        .filter(|ms| *ms > 0)
        .ok_or(RecordRejectionReason::MissingTimestamp)?;
    Ok(Some(UsageRecord::new_with_dedup(
        model,
        provider_identity::observed_provider_id("", model),
        session,
        timestamp,
        tokens,
        0.0,
        Some(dedup_hash_str(&format!(
            "antigravity-step:{session}:{step_index}"
        ))),
    )))
}

pub(super) fn parse(path: &Path) -> SessionParseResult<ScannedInput> {
    let session = session_name(path)
        .and_then(OsStr::to_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            SessionParseError::invalid(
                "read Antigravity transcript session",
                "expected a UTF-8 session directory",
            )
        })?;
    let file = File::open(path)
        .map_err(|err| SessionParseError::new("open Antigravity transcript", err))?;
    let mut reader = BufReader::new(file);
    let mut scanned = ScannedInput::default();
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) if line.iter().all(u8::is_ascii_whitespace) => continue,
            Ok(_) => match parse_step(&line, session) {
                Ok(Some(record)) => scanned.messages.push(record),
                Ok(None) => {}
                Err(reason) => scanned.rejections.record(reason),
            },
            Err(error) => {
                scanned.interrupted = Some(InputFailure::new(
                    "read Antigravity transcript",
                    error.to_string(),
                ));
                break;
            }
        }
    }
    Ok(scanned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transcript_counts_recorded_steps_and_reports_bad_rows() {
        let root = tempfile::tempdir().unwrap();
        let logs = root.path().join("session/.system_generated/logs");
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("transcript.jsonl");
        let good = json!({
            "type": "PLANNER_RESPONSE", "source": "MODEL", "step_index": 1,
            "model": "gemini-4-neon-medium", "created_at": "2026-11-01T00:01:02Z",
            "input_tokens": 5, "cache_read_tokens": 7, "output_tokens": 3
        });
        let mut negative = good.clone();
        negative["cache_read_tokens"] = json!(-1);
        let mut overflow = good.clone();
        overflow["output_tokens"] = json!(i64::MAX);
        let mut untimed = good.clone();
        untimed["created_at"] = json!(null);
        let mut unnamed = good.clone();
        unnamed["model"] = json!(" ");
        let non_usage = json!({"type": "USER_INPUT", "source":"USER_EXPLICIT", "input_tokens":80});
        let lines =
            [non_usage, negative, overflow, untimed, unnamed, good].map(|row| row.to_string());
        std::fs::write(&path, format!("{{bad json\n{}", lines.join("\n"))).unwrap();
        let scanned = parse(&path).unwrap();
        assert_eq!(scanned.rejections.total(), 5);
        let [record] = scanned.messages.as_slice() else {
            panic!("expected the valid planner response")
        };
        assert_eq!(record.tokens.total(), 15);
        assert_eq!(record.raw_model_id.as_ref(), "gemini-4-neon-medium");
        assert_eq!(record.session_id.as_ref(), "session");
        assert!(scanned.interrupted.is_none());
    }
}
