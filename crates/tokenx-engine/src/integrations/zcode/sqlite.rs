//! ZCode v2 SQLite usage decoder.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::input_health::{InputFailure, RecordRejectionReason, ScannedInput};
use crate::records::error::{SessionParseError, SessionParseResult};
use crate::records::utils::open_readonly_sqlite;
use crate::records::{
    dedup_hash_str, normalize_agent_name, normalize_workspace_key, workspace_label_from_key,
    UsageRecord,
};
use crate::TokenBreakdown;

const PROVIDER_ID: &str = "zai";

const MODERN_USAGE_QUERY: &str = r#"
    SELECT
        id,
        session_id,
        turn_id,
        model_id,
        started_at,
        completed_at,
        duration_ms,
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_input_tokens,
        cache_creation_input_tokens,
        computed_total_tokens,
        agent,
        mode
    FROM model_usage
    ORDER BY COALESCE(completed_at, started_at, 0), id
"#;

const LEGACY_USAGE_QUERY: &str = r#"
    SELECT
        id,
        session_id,
        turn_id,
        model_id,
        started_at,
        completed_at,
        duration_ms,
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_input_tokens,
        cache_creation_input_tokens,
        NULL,
        agent,
        mode
    FROM model_usage
    ORDER BY COALESCE(completed_at, started_at, 0), id
"#;

#[derive(Debug)]
struct ZcodeUsageRow {
    id: Option<String>,
    session_id: Option<String>,
    turn_id: Option<String>,
    model_id: Option<String>,
    started_at: Option<i64>,
    completed_at: Option<i64>,
    duration_ms: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    computed_total_tokens: Option<i64>,
    agent: Option<String>,
    mode: Option<String>,
}

impl ZcodeUsageRow {
    fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            session_id: row.get(1)?,
            turn_id: row.get(2)?,
            model_id: row.get(3)?,
            started_at: row.get(4)?,
            completed_at: row.get(5)?,
            duration_ms: row.get(6)?,
            input_tokens: row.get(7)?,
            output_tokens: row.get(8)?,
            reasoning_tokens: row.get(9)?,
            cache_read_input_tokens: row.get(10)?,
            cache_creation_input_tokens: row.get(11)?,
            computed_total_tokens: row.get(12)?,
            agent: row.get(13)?,
            mode: row.get(14)?,
        })
    }
}

struct ParsedZcodeRow {
    message: UsageRecord,
    turn_key: Option<(String, String)>,
}

enum RowOutcome {
    Message(Box<ParsedZcodeRow>),
    Filtered,
    Rejected(RecordRejectionReason),
}

pub fn parse_zcode_sqlite(db_path: &Path) -> SessionParseResult<ScannedInput> {
    let conn = open_readonly_sqlite(db_path).map_err(|source| {
        SessionParseError::at_path(db_path, "open ZCode database read-only", source)
    })?;

    let model_usage_columns = table_columns(&conn, db_path, "model_usage")?;
    if model_usage_columns.is_empty() {
        return Err(invalid_at_path(
            db_path,
            "inspect ZCode model_usage schema",
            "required model_usage table is missing",
        ));
    }
    let legacy_schema = !has_column(&model_usage_columns, "computed_total_tokens");

    let (workspaces, workspace_failure) = match load_session_workspaces(&conn, db_path) {
        Ok(workspaces) => (workspaces, None),
        Err(error) => (HashMap::new(), Some(InputFailure::from(&error))),
    };

    let query = if legacy_schema {
        LEGACY_USAGE_QUERY
    } else {
        MODERN_USAGE_QUERY
    };
    let mut stmt = conn.prepare(query).map_err(|error| {
        SessionParseError::at_path(db_path, "prepare ZCode model usage query", error)
    })?;
    let mut rows = stmt.query([]).map_err(|error| {
        SessionParseError::at_path(db_path, "execute ZCode model usage query", error)
    })?;

    let mut scanned = ScannedInput::default();
    let mut parsed_rows = Vec::new();
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => {
                scanned.interrupted = Some(InputFailure::new(
                    "iterate ZCode model usage rows",
                    error.to_string(),
                ));
                break;
            }
        };
        let row = match ZcodeUsageRow::decode(row) {
            Ok(row) => row,
            Err(_) => {
                scanned
                    .rejections
                    .record(RecordRejectionReason::MalformedRecord);
                continue;
            }
        };

        match parse_usage_row(row, legacy_schema, &workspaces) {
            RowOutcome::Message(row) => parsed_rows.push(*row),
            RowOutcome::Filtered => {}
            RowOutcome::Rejected(reason) => scanned.rejections.record(reason),
        }
    }

    mark_turn_starts(&mut parsed_rows);
    scanned
        .messages
        .extend(parsed_rows.into_iter().map(|row| row.message));
    if scanned.interrupted.is_none() {
        scanned.interrupted = workspace_failure;
    }
    Ok(scanned)
}

fn parse_usage_row(
    row: ZcodeUsageRow,
    legacy_schema: bool,
    workspaces: &HashMap<String, (String, String)>,
) -> RowOutcome {
    let raw_input = row.input_tokens.unwrap_or(0);
    let raw_output = row.output_tokens.unwrap_or(0);
    let raw_reasoning = row.reasoning_tokens.unwrap_or(0);
    let raw_cache_read = row.cache_read_input_tokens.unwrap_or(0);
    let raw_cache_write = row.cache_creation_input_tokens.unwrap_or(0);
    if [
        raw_input,
        raw_output,
        raw_reasoning,
        raw_cache_read,
        raw_cache_write,
    ]
    .into_iter()
    .any(|value| value < 0)
        || row.computed_total_tokens.is_some_and(|value| value < 0)
        || row.duration_ms.is_some_and(|value| value < 0)
    {
        return RowOutcome::Rejected(RecordRejectionReason::MalformedRecord);
    }

    let Some((input, output)) = normalize_input_and_output(
        raw_input,
        raw_output,
        raw_cache_read,
        raw_cache_write,
        raw_reasoning,
        row.computed_total_tokens,
        legacy_schema,
    ) else {
        return RowOutcome::Rejected(RecordRejectionReason::MalformedRecord);
    };
    let tokens = TokenBreakdown {
        input,
        output,
        cache_read: raw_cache_read,
        cache_write: raw_cache_write,
        reasoning: raw_reasoning,
    };
    let Some(total) = tokens.checked_total() else {
        return RowOutcome::Rejected(RecordRejectionReason::MalformedRecord);
    };
    if total == 0 {
        return RowOutcome::Filtered;
    }

    let Some(id) = non_empty(row.id) else {
        return RowOutcome::Rejected(RecordRejectionReason::MalformedRecord);
    };
    let Some(session_id) = non_empty(row.session_id) else {
        return RowOutcome::Rejected(RecordRejectionReason::MissingSession);
    };
    let Some(model_id) = non_empty(row.model_id) else {
        return RowOutcome::Rejected(RecordRejectionReason::MissingModel);
    };
    let Some(timestamp) = resolve_timestamp(row.started_at, row.completed_at, row.duration_ms)
    else {
        return RowOutcome::Rejected(RecordRejectionReason::MissingTimestamp);
    };

    let agent = [row.agent, row.mode]
        .into_iter()
        .flatten()
        .find_map(|value| non_empty(Some(value)))
        .map(|value| normalize_agent_name(&value));
    let turn_key = non_empty(row.turn_id).map(|turn_id| (session_id.clone(), turn_id));
    let mut message = UsageRecord::new_with_agent(
        model_id,
        PROVIDER_ID,
        &session_id,
        timestamp,
        tokens,
        0.0,
        agent,
    );
    message.dedup_key = Some(dedup_hash_str(&format!("zcode-sqlite:{id}")));
    if let Some((workspace_key, workspace_label)) = workspaces.get(&session_id) {
        message.set_workspace(Some(workspace_key.clone()), Some(workspace_label.clone()));
    }

    RowOutcome::Message(Box::new(ParsedZcodeRow { message, turn_key }))
}

fn normalize_input_and_output(
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    computed_total: Option<i64>,
    legacy_schema: bool,
) -> Option<(i64, i64)> {
    if let Some(total) = computed_total {
        let inclusive_total = input.checked_add(output)?;
        let exclusive_total = [input, output, cache_read, cache_write, reasoning]
            .into_iter()
            .try_fold(0_i64, i64::checked_add)?;
        if (cache_read > 0 || cache_write > 0 || reasoning > 0)
            && total == inclusive_total
            && total != exclusive_total
        {
            return Some((
                subtract_overlap(input, cache_read.checked_add(cache_write)?),
                subtract_overlap(output, reasoning),
            ));
        }
        return Some((input, output));
    }

    if legacy_schema {
        return Some((
            subtract_overlap(input, cache_read.checked_add(cache_write)?),
            subtract_overlap(output, reasoning),
        ));
    }
    Some((input, output))
}

fn subtract_overlap(value: i64, overlap: i64) -> i64 {
    value - overlap.min(value)
}

fn resolve_timestamp(
    started_at: Option<i64>,
    completed_at: Option<i64>,
    duration_ms: Option<i64>,
) -> Option<i64> {
    if let Some(started_at) = started_at.and_then(valid_timestamp) {
        return Some(started_at);
    }

    let completed_at = completed_at.and_then(valid_timestamp)?;
    let anchored = duration_ms
        .filter(|duration| *duration > 0)
        .and_then(|duration| completed_at.checked_sub(duration))
        .and_then(valid_timestamp);
    anchored.or(Some(completed_at))
}

fn valid_timestamp(timestamp: i64) -> Option<i64> {
    (timestamp > 0 && chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp).is_some())
        .then_some(timestamp)
}

fn mark_turn_starts(rows: &mut [ParsedZcodeRow]) {
    let mut earliest_per_turn: HashMap<(String, String), usize> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        let Some((session_id, turn_id)) = row.turn_key.as_ref() else {
            continue;
        };
        earliest_per_turn
            .entry((session_id.clone(), turn_id.clone()))
            .and_modify(|current| {
                if row.message.timestamp < rows[*current].message.timestamp {
                    *current = index;
                }
            })
            .or_insert(index);
    }
    for index in earliest_per_turn.into_values() {
        rows[index].message.is_turn_start = true;
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn table_columns(
    conn: &rusqlite::Connection,
    db_path: &Path,
    table: &str,
) -> SessionParseResult<HashSet<String>> {
    let query = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&query).map_err(|error| {
        SessionParseError::at_path(db_path, "prepare ZCode schema query", error)
    })?;
    let mut rows = stmt.query([]).map_err(|error| {
        SessionParseError::at_path(db_path, "execute ZCode schema query", error)
    })?;
    let mut columns = HashSet::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| SessionParseError::at_path(db_path, "iterate ZCode schema rows", error))?
    {
        let name: String = row.get(1).map_err(|error| {
            SessionParseError::at_path(db_path, "decode ZCode schema row", error)
        })?;
        columns.insert(name);
    }
    Ok(columns)
}

fn has_column(columns: &HashSet<String>, expected: &str) -> bool {
    columns
        .iter()
        .any(|column| column.eq_ignore_ascii_case(expected))
}

fn load_session_workspaces(
    conn: &rusqlite::Connection,
    db_path: &Path,
) -> SessionParseResult<HashMap<String, (String, String)>> {
    let columns = table_columns(conn, db_path, "session")?;
    if columns.is_empty() {
        return Ok(HashMap::new());
    }
    if !has_column(&columns, "id") {
        return Err(invalid_at_path(
            db_path,
            "inspect ZCode session metadata schema",
            "session table is missing its id column",
        ));
    }
    let directory = if has_column(&columns, "directory") {
        "directory"
    } else {
        "NULL"
    };
    let path = if has_column(&columns, "path") {
        "path"
    } else {
        "NULL"
    };
    if directory == "NULL" && path == "NULL" {
        return Ok(HashMap::new());
    }

    let query = format!("SELECT id, {directory}, {path} FROM session");
    let mut stmt = conn.prepare(&query).map_err(|error| {
        SessionParseError::at_path(db_path, "prepare ZCode session metadata query", error)
    })?;
    let mut rows = stmt.query([]).map_err(|error| {
        SessionParseError::at_path(db_path, "execute ZCode session metadata query", error)
    })?;
    let mut workspaces = HashMap::new();
    while let Some(row) = rows.next().map_err(|error| {
        SessionParseError::at_path(db_path, "iterate ZCode session metadata rows", error)
    })? {
        let (session_id, directory, path): (Option<String>, Option<String>, Option<String>) =
            (|| -> rusqlite::Result<_> { Ok((row.get(0)?, row.get(1)?, row.get(2)?)) })().map_err(
                |error| {
                    SessionParseError::at_path(db_path, "decode ZCode session metadata row", error)
                },
            )?;
        let Some(session_id) = non_empty(session_id) else {
            return Err(invalid_at_path(
                db_path,
                "decode ZCode session metadata row",
                "session metadata row has an empty id",
            ));
        };
        let workspace_key = non_empty(directory)
            .or_else(|| non_empty(path))
            .and_then(|root| normalize_workspace_key(&root));
        let Some(workspace_key) = workspace_key else {
            continue;
        };
        let Some(workspace_label) = workspace_label_from_key(&workspace_key) else {
            continue;
        };
        workspaces.insert(session_id, (workspace_key, workspace_label));
    }
    Ok(workspaces)
}

fn invalid_at_path(
    path: &Path,
    operation: &'static str,
    detail: impl Into<String>,
) -> SessionParseError {
    SessionParseError::at_path(
        path,
        operation,
        std::io::Error::new(std::io::ErrorKind::InvalidData, detail.into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    fn create_database(include_computed_total: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("db.sqlite");
        let conn = Connection::open(&path).unwrap();
        let computed_total = if include_computed_total {
            ", computed_total_tokens INTEGER"
        } else {
            ""
        };
        conn.execute_batch(&format!(
            "CREATE TABLE model_usage (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                turn_id TEXT,
                model_id TEXT,
                started_at INTEGER,
                completed_at INTEGER,
                duration_ms INTEGER,
                input_tokens INTEGER,
                output_tokens INTEGER,
                reasoning_tokens INTEGER,
                cache_read_input_tokens INTEGER,
                cache_creation_input_tokens INTEGER,
                agent TEXT,
                mode TEXT
                {computed_total}
            );
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                directory TEXT,
                path TEXT
            );"
        ))
        .unwrap();
        (dir, path)
    }

    #[test]
    fn parses_modern_usage_workspace_agent_and_turn_identity() {
        let (_dir, path) = create_database(true);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO session (id, directory, path) VALUES (?1, ?2, ?3)",
            params!["session-1", "/Users/alice/work/demo", "/ignored"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO model_usage (
                id, session_id, turn_id, model_id, started_at, completed_at, duration_ms,
                input_tokens, output_tokens, reasoning_tokens, cache_read_input_tokens,
                cache_creation_input_tokens, computed_total_tokens, agent, mode
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                "usage-1",
                "session-1",
                "turn-1",
                "GLM-5.2",
                1_782_718_000_000_i64,
                1_782_718_001_000_i64,
                1_000_i64,
                100_i64,
                20_i64,
                5_i64,
                7_i64,
                3_i64,
                120_i64,
                "zcode-agent",
                "yolo",
            ],
        )
        .unwrap();
        drop(conn);

        let scanned = parse_zcode_sqlite(&path).unwrap();

        assert!(scanned.interrupted.is_none());
        assert!(scanned.rejections.is_empty());
        assert_eq!(scanned.messages.len(), 1);
        let message = &scanned.messages[0];
        assert_eq!(message.model_id.as_ref(), "GLM-5.2");
        assert_eq!(message.provider_id.as_ref(), "zai");
        assert_eq!(message.session_id.as_ref(), "session-1");
        assert_eq!(message.timestamp, 1_782_718_000_000);
        assert_eq!(message.tokens.input, 90);
        assert_eq!(message.tokens.output, 15);
        assert_eq!(message.tokens.reasoning, 5);
        assert_eq!(message.tokens.cache_read, 7);
        assert_eq!(message.tokens.cache_write, 3);
        assert_eq!(message.tokens.total(), 120);
        assert_eq!(message.agent.as_deref(), Some("ZCode Agent"));
        assert_eq!(
            message.dedup_key,
            Some(dedup_hash_str("zcode-sqlite:usage-1"))
        );
        assert_eq!(
            message.workspace_key.as_deref(),
            Some("/Users/alice/work/demo")
        );
        assert_eq!(message.workspace_label.as_deref(), Some("demo"));
        assert!(message.is_turn_start);
    }

    #[test]
    fn anchors_missing_start_at_completion_minus_duration_and_marks_earliest_request() {
        let (_dir, path) = create_database(true);
        let conn = Connection::open(&path).unwrap();
        for values in [
            ("finishes-first", 2_000_i64, 3_000_i64, 1_000_i64),
            ("starts-first", 1_000_i64, 4_000_i64, 3_000_i64),
        ] {
            conn.execute(
                "INSERT INTO model_usage (
                    id, session_id, turn_id, model_id, started_at, completed_at, duration_ms,
                    input_tokens, output_tokens
                 ) VALUES (?1, 'session-1', 'turn-1', 'glm-5.2', ?2, ?3, ?4, 10, 1)",
                params![values.0, values.1, values.2, values.3],
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE model_usage SET started_at = NULL WHERE id = 'starts-first'",
            [],
        )
        .unwrap();
        drop(conn);

        let messages = parse_zcode_sqlite(&path).unwrap().messages;

        assert_eq!(messages.len(), 2);
        let finishes_first = messages
            .iter()
            .find(|message| message.timestamp == 2_000)
            .unwrap();
        let starts_first = messages
            .iter()
            .find(|message| message.timestamp == 1_000)
            .unwrap();
        assert!(!finishes_first.is_turn_start);
        assert!(starts_first.is_turn_start);
    }

    #[test]
    fn legacy_schema_subtracts_cache_and_reasoning_overlap() {
        let (_dir, path) = create_database(false);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO model_usage (
                id, session_id, model_id, completed_at, input_tokens, output_tokens,
                reasoning_tokens, cache_read_input_tokens, cache_creation_input_tokens
             ) VALUES ('legacy', 'session-1', 'glm-5.2', 1000, 100, 50, 10, 80, 5)",
            [],
        )
        .unwrap();
        drop(conn);

        let messages = parse_zcode_sqlite(&path).unwrap().messages;

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tokens.input, 15);
        assert_eq!(messages[0].tokens.output, 40);
        assert_eq!(messages[0].tokens.total(), 150);
    }

    #[test]
    fn modern_null_total_preserves_input_and_output_shape() {
        let (_dir, path) = create_database(true);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO model_usage (
                id, session_id, model_id, completed_at, input_tokens, output_tokens,
                reasoning_tokens, cache_read_input_tokens, cache_creation_input_tokens,
                computed_total_tokens
             ) VALUES ('modern-null', 'session-1', 'glm-5.2', 1000, 100, 50, 10, 80, 5, NULL)",
            [],
        )
        .unwrap();
        drop(conn);

        let messages = parse_zcode_sqlite(&path).unwrap().messages;

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tokens.input, 100);
        assert_eq!(messages[0].tokens.output, 50);
        assert_eq!(messages[0].tokens.total(), 245);
    }

    #[test]
    fn rejects_bad_rows_without_erasing_valid_usage() {
        let (_dir, path) = create_database(true);
        let conn = Connection::open(&path).unwrap();
        let rows = [
            (
                "good",
                Some("session"),
                Some("glm-5.2"),
                Some(1000),
                1_i64,
                1_i64,
            ),
            ("missing-session", None, Some("glm-5.2"), Some(1000), 1, 1),
            ("missing-model", Some("session"), None, Some(1000), 1, 1),
            ("missing-time", Some("session"), Some("glm-5.2"), None, 1, 1),
            (
                "negative",
                Some("session"),
                Some("glm-5.2"),
                Some(1000),
                -1,
                1,
            ),
            (
                "overflow",
                Some("session"),
                Some("glm-5.2"),
                Some(1000),
                i64::MAX,
                1,
            ),
        ];
        for (id, session, model, completed, input, output) in rows {
            conn.execute(
                "INSERT INTO model_usage (
                    id, session_id, model_id, completed_at, input_tokens, output_tokens
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, session, model, completed, input, output],
            )
            .unwrap();
        }
        drop(conn);

        let scanned = parse_zcode_sqlite(&path).unwrap();
        let rejections = scanned
            .rejections
            .entries()
            .map(|entry| (entry.key, entry.count))
            .collect::<HashMap<_, _>>();

        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(scanned.messages[0].session_id.as_ref(), "session");
        assert_eq!(rejections.get("missing-session"), Some(&1));
        assert_eq!(rejections.get("missing-model"), Some(&1));
        assert_eq!(rejections.get("missing-timestamp"), Some(&1));
        assert_eq!(rejections.get("malformed-record"), Some(&2));
    }

    #[test]
    fn missing_model_usage_schema_is_an_input_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("db.sqlite");
        drop(Connection::open(&path).unwrap());

        let error = parse_zcode_sqlite(&path).unwrap_err();

        assert_eq!(error.operation(), "inspect ZCode model_usage schema");
    }

    #[test]
    fn broken_optional_session_metadata_keeps_usage_and_marks_input_partial() {
        let (_dir, path) = create_database(true);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO session (id, directory) VALUES ('session-1', x'ff')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO model_usage (
                id, session_id, model_id, completed_at, input_tokens, output_tokens
             ) VALUES ('usage-1', 'session-1', 'glm-5.2', 1000, 1, 1)",
            [],
        )
        .unwrap();
        drop(conn);

        let scanned = parse_zcode_sqlite(&path).unwrap();

        assert_eq!(scanned.messages.len(), 1);
        assert!(scanned.messages[0].workspace_key.is_none());
        assert!(scanned.interrupted.is_some());
    }
}
