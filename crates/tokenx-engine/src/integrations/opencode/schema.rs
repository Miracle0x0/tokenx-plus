use std::collections::HashSet;

use rusqlite::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MessageFormat {
    Message,
    SessionMessage,
}

pub(super) struct MessageQuery {
    pub format: MessageFormat,
    pub sql: String,
}

/// Select known schemas by table identity, never by retrying failed queries.
/// Metadata is optional when its table is absent; a selected table must have
/// the declared id/directory columns. Preparation validates that contract.
pub(super) fn message_queries(conn: &Connection) -> rusqlite::Result<Vec<MessageQuery>> {
    let tables = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table'")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    let mut queries = Vec::new();
    for (table, format) in [
        ("session_message", MessageFormat::SessionMessage),
        ("message", MessageFormat::Message),
    ] {
        if !tables.contains(table) {
            continue;
        }
        let metadata = if format == MessageFormat::SessionMessage && tables.contains("session_v2") {
            Some("session_v2")
        } else if tables.contains("session") {
            Some("session")
        } else {
            None
        };
        let (workspace, parent, join) = match metadata {
            Some(metadata) => {
                let has_parent: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = 'parent_id')",
                    [metadata],
                    |row| row.get(0),
                )?;
                (
                    "NULLIF(s.directory, '')",
                    if has_parent {
                        "NULLIF(TRIM(s.parent_id), '')"
                    } else {
                        "NULL"
                    },
                    format!("LEFT JOIN {metadata} s ON s.id = m.session_id"),
                )
            }
            None => ("NULL", "NULL", String::new()),
        };
        let role = match format {
            MessageFormat::Message => "NULL",
            MessageFormat::SessionMessage => "m.type",
        };
        let sql = format!(
            "SELECT m.id, m.session_id, m.data, {workspace}, {parent}, {role}
             FROM {table} m {join} ORDER BY m.id, m.session_id"
        );
        conn.prepare(&sql)?;
        queries.push(MessageQuery { format, sql });
    }
    Ok(queries)
}
