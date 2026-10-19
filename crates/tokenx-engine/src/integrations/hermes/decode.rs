//! Hermes Agent session decoder.
//!
//! Parses aggregated session rows from Hermes Agent's SQLite state database:
//! - `~/.hermes/state.db`
//! - `~/.hermes/state.db`
//!
//! App-reported estimated and actual costs are ignored. Tokenx reports cost
//! only from token usage and its own pricing table.

use crate::input_health::{InputFailure, RecordRejectionReason, ScannedInput};
use crate::records::error::{SessionParseError, SessionParseResult};
use crate::records::utils::{open_readonly_sqlite, parse_epoch_f64_millis};
use crate::records::UsageRecord;
use crate::{provider_identity, TokenBreakdown};
use std::path::Path;

const HERMES_AGENT_NAME: &str = "Hermes Agent";

fn resolved_provider(billing_provider: Option<String>, model_id: &str) -> String {
    provider_identity::observed_provider_id(
        billing_provider.as_deref().unwrap_or_default(),
        model_id,
    )
}

pub fn parse_hermes_sqlite(db_path: &Path) -> SessionParseResult<ScannedInput> {
    let conn = open_readonly_sqlite(db_path)?;

    let query = r#"
        SELECT
            id,
            model,
            billing_provider,
            started_at,
            message_count,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens
        FROM sessions
    "#;

    let mut stmt = conn
        .prepare(query)
        .map_err(|error| SessionParseError::new("prepare Hermes session query", error))?;

    let mut rows = stmt
        .query([])
        .map_err(|error| SessionParseError::new("execute Hermes session query", error))?;

    let mut scanned = ScannedInput::default();
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => {
                let error = SessionParseError::new("iterate Hermes session rows", error);
                scanned.interrupted = Some(InputFailure::from(&error));
                break;
            }
        };
        type HermesRow = (
            String,
            Option<String>,
            Option<String>,
            Option<f64>,
            i32,
            i64,
            i64,
            i64,
            i64,
            i64,
        );
        let decoded = (|| -> rusqlite::Result<HermesRow> {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<i32>>(4)?.unwrap_or(0),
                row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                row.get::<_, Option<i64>>(9)?.unwrap_or(0),
            ))
        })();
        let (
            session_id,
            model_id,
            billing_provider,
            started_at,
            message_count,
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
        ) = match decoded {
            Ok(decoded) => decoded,
            Err(_error) => {
                scanned
                    .rejections
                    .record(RecordRejectionReason::MalformedRecord);
                continue;
            }
        };
        if [input, output, cache_read, cache_write, reasoning]
            .into_iter()
            .any(|tokens| tokens < 0)
        {
            scanned
                .rejections
                .record(RecordRejectionReason::MalformedRecord);
            continue;
        }
        // Hermes stores reasoning as a breakdown of output, not extra usage.
        // Preserve the authoritative output total even if its detail is larger.
        let reasoning = reasoning.min(output);
        let tokens = TokenBreakdown {
            input,
            output: output - reasoning,
            cache_read,
            cache_write,
            cache_write_1h: 0,
            reasoning,
        };
        let Some(token_total) = tokens.checked_total() else {
            scanned
                .rejections
                .record(RecordRejectionReason::MalformedRecord);
            continue;
        };
        if token_total == 0 {
            continue;
        }
        let Some(model_id) = model_id
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
        else {
            scanned
                .rejections
                .record(RecordRejectionReason::MissingModel);
            continue;
        };
        let Some(timestamp) = started_at.and_then(parse_epoch_f64_millis) else {
            scanned
                .rejections
                .record(RecordRejectionReason::MissingTimestamp);
            continue;
        };
        let billing = crate::BillingProvider::from_observation(billing_provider.as_deref());
        let provider = resolved_provider(billing_provider, &model_id);
        let mut msg = UsageRecord::new_with_agent(
            model_id,
            provider,
            session_id.clone(),
            timestamp,
            tokens,
            0.0,
            Some(HERMES_AGENT_NAME.to_string()),
        );
        msg.billing_provider = billing;
        msg.message_count = message_count;
        msg.dedup_key = Some(crate::records::dedup_hash_str(&session_id));
        scanned.messages.push(msg);
    }
    Ok(scanned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    fn usage_database() -> (tempfile::TempDir, std::path::PathBuf, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT, model TEXT, billing_provider TEXT, started_at REAL,
                message_count INTEGER, input_tokens INTEGER, output_tokens INTEGER,
                cache_read_tokens INTEGER, cache_write_tokens INTEGER, reasoning_tokens INTEGER
            );",
        )
        .unwrap();
        (dir, path, conn)
    }

    #[test]
    fn reasoning_is_an_output_breakdown_for_totals_and_pricing() {
        let (_dir, path, conn) = usage_database();
        for (id, output, reasoning) in [
            ("partial", 40, 10),
            ("all", 40, 40),
            ("oversized", 40, 60),
            ("none", 40, 0),
            ("empty", 0, 10),
        ] {
            conn.execute(
                "INSERT INTO sessions VALUES (?1, 'gpt-5', 'openai', 1700000000, 1, 100, ?2, 5, 3, ?3)",
                params![id, output, reasoning],
            ).unwrap();
        }
        drop(conn);
        let scanned = parse_hermes_sqlite(&path).unwrap();
        assert!(scanned.rejections.is_empty());
        assert_eq!(scanned.messages.len(), 5);
        let pricing = crate::pricing::ModelPricing {
            output_cost_per_token: Some(2.0),
            ..Default::default()
        };
        for message in scanned.messages {
            let expected_reasoning = match message.session_id.as_ref() {
                "partial" => 10,
                "all" | "oversized" => 40,
                "none" | "empty" => 0,
                other => panic!("unexpected session {other}"),
            };
            let expected_output = if message.session_id.as_ref() == "empty" {
                0
            } else {
                40
            };
            assert_eq!(message.tokens.total(), 108 + expected_output);
            assert_eq!(message.tokens.reasoning, expected_reasoning);
            assert_eq!(
                message.tokens.output + message.tokens.reasoning,
                expected_output
            );
            assert_eq!(
                crate::pricing::lookup::compute_cost(&pricing, &message.tokens).unwrap(),
                2.0 * expected_output as f64
            );
        }
    }

    #[test]
    fn parse_hermes_sqlite_reports_missing_schema_as_input_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        drop(Connection::open(&path).unwrap());

        let error = parse_hermes_sqlite(&path).unwrap_err();

        assert_eq!(error.operation(), "prepare Hermes session query");
    }

    #[test]
    fn parse_hermes_sqlite_keeps_good_rows_around_a_bad_row() {
        let (_dir, path, conn) = usage_database();
        for (id, started_at) in [
            ("01-good", rusqlite::types::Value::Real(1_700_000_000.0)),
            ("02-bad", rusqlite::types::Value::Text("bad".to_string())),
            ("03-good", rusqlite::types::Value::Real(1_700_000_002.0)),
        ] {
            conn.execute(
                "INSERT INTO sessions VALUES (?1, ?2, ?3, ?4, 1, 1, 1, 0, 0, 0)",
                params![id, "gpt-5", "openai", started_at],
            )
            .unwrap();
        }
        drop(conn);

        let scanned = parse_hermes_sqlite(&path).unwrap();

        assert_eq!(scanned.messages.len(), 2);
        assert_eq!(scanned.rejections.total(), 1);
        assert_eq!(
            scanned.rejections.entries().next().unwrap().key,
            "malformed-record"
        );
        assert!(scanned.interrupted.is_none());
    }

    #[test]
    fn token_bearing_row_without_model_is_reported() {
        let (_dir, path, conn) = usage_database();
        conn.execute(
            "INSERT INTO sessions VALUES ('missing-model', NULL, 'openai', 1700000000, 1, 1, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let scanned = parse_hermes_sqlite(&path).unwrap();

        assert!(scanned.messages.is_empty());
        let rejection = scanned.rejections.entries().next().unwrap();
        assert_eq!(rejection.key, "missing-model");
        assert_eq!(rejection.count, 1);
    }

    #[test]
    fn token_bearing_row_without_resolvable_provider_is_kept() {
        let (_dir, path, conn) = usage_database();
        conn.execute(
            "INSERT INTO sessions VALUES ('unknown-provider', 'private-model', NULL, 1700000000, 1, 1, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let scanned = parse_hermes_sqlite(&path).unwrap();

        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.messages[0].provider_id.as_ref(), "unknown");
        assert!(scanned.rejections.is_empty());
    }

    #[test]
    fn explicit_billing_provider_wins_over_model_inference() {
        let (_dir, path, conn) = usage_database();
        conn.execute(
            "INSERT INTO sessions VALUES ('explicit-route', 'claude-sonnet-4', 'OpenRouter.Route', 1700000000, 1, 1, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let messages = parse_hermes_sqlite(&path).unwrap().messages;

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].model_id.as_ref(), "claude-sonnet-4");
        assert_eq!(messages[0].provider_id.as_ref(), "OpenRouter.Route");
    }

    #[test]
    fn zero_token_row_is_filtered_but_negative_tokens_are_rejected() {
        let (_dir, path, conn) = usage_database();
        conn.execute(
            "INSERT INTO sessions VALUES ('zero', NULL, NULL, NULL, 0, 0, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES ('negative', 'gpt-5', 'openai', 1700000000, 1, -1, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let scanned = parse_hermes_sqlite(&path).unwrap();

        assert!(scanned.messages.is_empty());
        let rejection = scanned.rejections.entries().next().unwrap();
        assert_eq!(rejection.key, "malformed-record");
        assert_eq!(rejection.count, 1);
    }

    #[test]
    fn out_of_range_timestamp_is_rejected_instead_of_saturating() {
        let (_dir, path, conn) = usage_database();
        conn.execute(
            "INSERT INTO sessions VALUES ('huge-time', 'gpt-5', 'openai', 1e300, 1, 1, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let scanned = parse_hermes_sqlite(&path).unwrap();

        assert!(scanned.messages.is_empty());
        let rejection = scanned.rejections.entries().next().unwrap();
        assert_eq!(rejection.key, "missing-timestamp");
        assert_eq!(rejection.count, 1);
    }

    #[test]
    fn overflowing_token_total_is_rejected_and_later_row_is_kept() {
        let (_dir, path, conn) = usage_database();
        conn.execute(
            "INSERT INTO sessions VALUES ('01-overflow', 'gpt-5', 'openai', 1700000000, 1, ?1, 1, 0, 0, 0)",
            [i64::MAX],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES ('02-good', 'gpt-5', 'openai', 1700000001, 1, 1, 1, 0, 0, 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let scanned = parse_hermes_sqlite(&path).unwrap();

        assert_eq!(scanned.messages.len(), 1);
        let rejection = scanned.rejections.entries().next().unwrap();
        assert_eq!(rejection.key, "malformed-record");
        assert_eq!(rejection.count, 1);
    }
}
