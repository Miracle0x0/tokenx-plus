//! Native Claude session ownership proved by countable OpenCode messages.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::input_health::{InputFailure, RecordRejectionReason, RejectionSummary};
use crate::records::error::{SessionParseError, SessionParseResult};
use crate::records::utils::open_readonly_sqlite;

use super::decode::decode_row;
use super::schema::{message_queries, MessageFormat};

#[derive(Debug, Default)]
pub(crate) struct Claims {
    pub sessions: HashSet<String>,
    pub rejections: RejectionSummary,
    pub interrupted: Option<InputFailure>,
}

pub(crate) fn read(
    path: &Path,
    cancellation: &crate::AcquisitionCancellation,
) -> SessionParseResult<Claims> {
    let conn = open_readonly_sqlite(path)?;
    let queries = message_queries(&conn)
        .map_err(|error| SessionParseError::new("read OpenCode ownership schema", error))?;
    if queries.is_empty() {
        return Err(SessionParseError::invalid(
            "read OpenCode ownership schema",
            "database has neither message nor session_message tables",
        ));
    }
    let mut claims = Claims::default();
    for query in queries
        .into_iter()
        .filter(|q| q.format == MessageFormat::SessionMessage)
    {
        // The selected schema's columns and countability rules are shared with
        // normal OpenCode ingestion. The filter avoids decoding unrelated payloads.
        let sql = format!(
            "SELECT * FROM ({}) WHERE instr(data, '\"native\"') > 0",
            query.sql
        );
        let mut statement = conn
            .prepare(&sql)
            .map_err(|error| SessionParseError::new("prepare OpenCode ownership query", error))?;
        let mut rows = statement
            .query([])
            .map_err(|error| SessionParseError::new("query OpenCode ownership records", error))?;
        loop {
            cancellation
                .check(crate::engine::AcquisitionPhase::Folding)
                .map_err(|error| {
                    SessionParseError::new("read OpenCode ownership records", error)
                })?;
            let row = match rows.next() {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(error) => {
                    claims.interrupted = Some(InputFailure::new(
                        "read OpenCode ownership records",
                        error.to_string(),
                    ));
                    break;
                }
            };
            match decode_row(row, query.format) {
                Ok(Some(_)) => {
                    // decode_row has already validated the usage. Ownership
                    // metadata has its own record failure domain and must not
                    // discard earlier claims from this database.
                    let data = row.get_ref(2).ok().and_then(|data| data.as_str().ok());
                    match data.map(extract_session_ids) {
                        Some(Ok(sessions)) => claims.sessions.extend(sessions),
                        _ => claims
                            .rejections
                            .record(RecordRejectionReason::MalformedRecord),
                    }
                }
                Ok(None) => {}
                Err(reason) => claims.rejections.record(reason),
            }
        }
    }
    Ok(claims)
}

#[derive(Deserialize)]
struct NativeSession<'a> {
    #[serde(rename = "sessionID", borrow)]
    session_id: Option<&'a RawValue>,
}

fn extract_session_ids(data: &str) -> Result<HashSet<String>, serde_json::Error> {
    // Retain unrelated scalars as raw JSON, including numbers outside f64's
    // range that the usage decoder intentionally ignores. Walk iteratively so
    // nested tool metadata does not consume the Rust call stack.
    let root: &RawValue = serde_json::from_str(data)?;
    let mut pending = vec![root];
    let mut sessions = HashSet::new();
    while let Some(value) = pending.pop() {
        if value.get().starts_with('{') {
            let fields: HashMap<String, &RawValue> = serde_json::from_str(value.get())?;
            if let Some(native) = fields
                .get("native")
                .filter(|raw| raw.get().starts_with('{'))
            {
                let native: NativeSession<'_> = serde_json::from_str(native.get())?;
                if let Some(raw) = native.session_id.filter(|raw| raw.get().starts_with('"')) {
                    let id: String = serde_json::from_str(raw.get())?;
                    if !id.trim().is_empty() {
                        sessions.insert(id);
                    }
                }
            }
            pending.extend(fields.into_values());
        } else if value.get().starts_with('[') {
            pending.extend(serde_json::from_str::<Vec<&RawValue>>(value.get())?);
        }
    }
    Ok(sessions)
}
