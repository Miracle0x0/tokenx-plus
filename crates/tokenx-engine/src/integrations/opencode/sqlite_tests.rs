use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::decode::{parse_opencode_sqlite, OpenCodeSqliteError};
use super::schema::MessageFormat;

struct Store {
    conn: Connection,
    _dir: TempDir,
    path: PathBuf,
}

impl Store {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("opencode-next.db");
        let conn = Connection::open(&path).unwrap();
        Self {
            _dir: dir,
            path,
            conn,
        }
    }

    fn messages(&self, format: MessageFormat) {
        let sql = match format {
            MessageFormat::Message => "CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)",
            MessageFormat::SessionMessage => {
                "CREATE TABLE session_message (id TEXT, session_id TEXT, data TEXT, type TEXT)"
            }
        };
        self.conn.execute_batch(sql).unwrap();
    }

    fn metadata(&self, table: &str) {
        self.conn
            .execute_batch(&format!(
                "CREATE TABLE {table} (id TEXT PRIMARY KEY, directory TEXT, parent_id TEXT);
             INSERT INTO {table} VALUES ('root', '/repo/{table}', NULL);
             INSERT INTO {table} VALUES ('child', '/repo/{table}', 'root');"
            ))
            .unwrap();
    }

    fn insert(&self, format: MessageFormat, id: &str, session: &str, role: &str, payload: &Value) {
        let data = payload.to_string();
        match format {
            MessageFormat::Message => self.conn.execute(
                "INSERT INTO message VALUES (?1, ?2, ?3)",
                params![id, session, data],
            ),
            MessageFormat::SessionMessage => self.conn.execute(
                "INSERT INTO session_message VALUES (?1, ?2, ?3, ?4)",
                params![id, session, data, role],
            ),
        }
        .unwrap();
    }

    fn parse(&self) -> crate::ScannedInput {
        parse_opencode_sqlite(&self.path).unwrap()
    }
}

fn payload(format: MessageFormat, id: Option<&str>, input: i64) -> Value {
    let mut data = json!({
        "time": {"created": 1783882279705_i64, "completed": 1783882279943_i64},
        "agent": "build",
        "tokens": {"input": input, "output": 20, "reasoning": 23, "cache": {"read": 100, "write": 50}},
    });
    if let Some(id) = id {
        data["id"] = json!(id);
    }
    match format {
        MessageFormat::Message => {
            data["role"] = json!("assistant");
            data["modelID"] = json!("claude-sonnet-4");
            data["providerID"] = json!("anthropic");
        }
        MessageFormat::SessionMessage => {
            data["model"] = json!({"id": "claude-sonnet-4", "providerID": "anthropic"});
        }
    }
    data
}

#[test]
fn v2_reads_both_metadata_layouts_and_parent_identity() {
    for metadata in ["session_v2", "session"] {
        let store = Store::new();
        store.messages(MessageFormat::SessionMessage);
        store.metadata(metadata);
        for (id, input) in [("root", 5519), ("child", 101)] {
            store.insert(
                MessageFormat::SessionMessage,
                id,
                id,
                "assistant",
                &payload(MessageFormat::SessionMessage, None, input),
            );
        }
        let scanned = store.parse();
        assert!(scanned.rejections.is_empty());
        assert!(scanned.interrupted.is_none());
        assert_eq!(scanned.messages.len(), 2);
        for message in scanned.messages {
            assert_eq!(message.model_id.as_ref(), "claude-sonnet-4");
            assert_eq!(message.provider_id.as_ref(), "anthropic");
            assert_eq!(message.tokens.output, 20);
            assert_eq!(message.tokens.reasoning, 23);
            assert_eq!(message.tokens.cache_read, 100);
            assert_eq!(message.tokens.cache_write, 50);
            assert_eq!(message.timestamp, 1783882279705);
            assert_eq!(
                message.workspace_key.as_deref(),
                Some(format!("/repo/{metadata}").as_str())
            );
            assert_eq!(
                message.is_main_session,
                message.session_id.as_ref() == "root"
            );
        }
    }
}

#[test]
fn mixed_tables_count_new_usage_and_collapse_copied_turns() {
    let store = Store::new();
    for format in [MessageFormat::Message, MessageFormat::SessionMessage] {
        store.messages(format);
        store.insert(
            format,
            "shared-row",
            "root",
            "assistant",
            &payload(format, Some("shared"), 10),
        );
    }
    store.insert(
        MessageFormat::SessionMessage,
        "new-row",
        "root",
        "assistant",
        &payload(MessageFormat::SessionMessage, Some("new"), 70),
    );
    let scanned = store.parse();
    assert_eq!(scanned.messages.len(), 2);
    assert_eq!(
        scanned
            .messages
            .iter()
            .map(|message| message.tokens.input)
            .sum::<i64>(),
        80
    );
}

#[test]
fn empty_message_table_does_not_hide_v2_usage() {
    let store = Store::new();
    store.messages(MessageFormat::Message);
    store.messages(MessageFormat::SessionMessage);
    store.insert(
        MessageFormat::SessionMessage,
        "new",
        "root",
        "assistant",
        &payload(MessageFormat::SessionMessage, None, 90),
    );
    assert_eq!(store.parse().messages[0].tokens.input, 90);
}

#[test]
fn v2_metadata_has_explicit_priority_over_v1_metadata() {
    let store = Store::new();
    store.messages(MessageFormat::SessionMessage);
    store.metadata("session");
    store.metadata("session_v2");
    store.insert(
        MessageFormat::SessionMessage,
        "new",
        "root",
        "assistant",
        &payload(MessageFormat::SessionMessage, None, 10),
    );
    assert_eq!(
        store.parse().messages[0].workspace_key.as_deref(),
        Some("/repo/session_v2")
    );
}

#[test]
fn absent_metadata_preserves_usage_and_embedded_workspace() {
    for format in [MessageFormat::Message, MessageFormat::SessionMessage] {
        let store = Store::new();
        store.messages(format);
        let mut data = payload(format, None, 10);
        data["path"] = json!({"root": "/embedded/project"});
        store.insert(format, "row", "root", "assistant", &data);
        let scanned = store.parse();
        assert_eq!(scanned.messages.len(), 1);
        assert_eq!(
            scanned.messages[0].workspace_key.as_deref(),
            Some("/embedded/project")
        );
    }
}

#[test]
fn distinct_embedded_ids_survive_fingerprint_collisions_and_forks() {
    for format in [MessageFormat::Message, MessageFormat::SessionMessage] {
        let store = Store::new();
        store.messages(format);
        for (row, session, id) in [
            ("a", "root", "msg_a"),
            ("b", "root", "msg_b"),
            ("c", "fork", "msg_a"),
        ] {
            store.insert(
                format,
                row,
                session,
                "assistant",
                &payload(format, Some(id), 10),
            );
        }
        let scanned = store.parse();
        assert_eq!(scanned.messages.len(), 2);
        let mut keys: Vec<_> = scanned
            .messages
            .iter()
            .map(|message| message.dedup_key.unwrap())
            .collect();
        keys.sort();
        let mut expected = ["msg_a", "msg_b"].map(crate::records::dedup_hash_str);
        expected.sort();
        assert_eq!(keys, expected);
    }
}

#[test]
fn v2_filters_non_assistant_rows_and_reports_bad_assistant_records() {
    let store = Store::new();
    store.messages(MessageFormat::SessionMessage);
    for (id, role) in [("user", "user"), ("synthetic", "synthetic")] {
        store.insert(MessageFormat::SessionMessage, id, "root", role, &json!({}));
    }
    let valid = payload(MessageFormat::SessionMessage, None, 50);
    let mut bad_model = valid.clone();
    bad_model["model"]["id"] = json!("");
    let mut negative = valid.clone();
    negative["tokens"]["input"] = json!(-1);
    let mut bad_role = valid.clone();
    bad_role["role"] = json!("user");
    let mut blank_id = valid.clone();
    blank_id["id"] = json!(" ");
    for (id, data) in [
        ("a", bad_model),
        ("b", negative),
        ("c", bad_role),
        ("d", blank_id),
        (" ", valid.clone()),
        ("good", valid),
    ] {
        store.insert(
            MessageFormat::SessionMessage,
            id,
            "root",
            "assistant",
            &data,
        );
    }
    store
        .conn
        .execute(
            "INSERT INTO session_message VALUES ('bad-json', 'root', '{', 'assistant')",
            [],
        )
        .unwrap();
    let scanned = store.parse();
    assert_eq!(scanned.messages.len(), 1);
    assert_eq!(scanned.rejections.total(), 6);
    assert!(scanned.interrupted.is_none());
}

#[test]
fn top_level_identity_wins_and_missing_provider_is_inferred() {
    let store = Store::new();
    store.messages(MessageFormat::SessionMessage);
    let mut data = payload(MessageFormat::SessionMessage, None, 10);
    data["modelID"] = json!("gpt-5.5");
    data["providerID"] = json!("openai");
    store.insert(
        MessageFormat::SessionMessage,
        "top-level",
        "root",
        "assistant",
        &data,
    );
    let mut inferred = payload(MessageFormat::SessionMessage, None, 11);
    inferred["model"]
        .as_object_mut()
        .unwrap()
        .remove("providerID");
    store.insert(
        MessageFormat::SessionMessage,
        "inferred",
        "root",
        "assistant",
        &inferred,
    );
    let scanned = store.parse();
    assert_eq!(scanned.messages.len(), 2);
    assert_eq!(scanned.messages[0].provider_id.as_ref(), "anthropic");
    assert_eq!(scanned.messages[1].model_id.as_ref(), "gpt-5.5");
    assert_eq!(scanned.messages[1].provider_id.as_ref(), "openai");
}

#[test]
fn unknown_or_damaged_schemas_are_explicit_errors() {
    let store = Store::new();
    assert!(matches!(
        parse_opencode_sqlite(&store.path),
        Err(OpenCodeSqliteError::UnsupportedSchema { .. })
    ));
    store.messages(MessageFormat::SessionMessage);
    store
        .conn
        .execute_batch("CREATE TABLE session_v2 (id TEXT)")
        .unwrap();
    assert!(matches!(
        parse_opencode_sqlite(&store.path),
        Err(OpenCodeSqliteError::CurrentSessionSchema { .. })
    ));
}

#[test]
fn discovery_uses_the_captured_root_and_keeps_explicit_database_paths() {
    use crate::integrations::{DiscoveryContext, IntegrationDriver};
    let home = TempDir::new().unwrap();
    let store = Store::new();
    let settings = crate::scanner::ScannerSettings {
        opencode_db_paths: vec![home.path().join("external.db")],
        ..Default::default()
    };
    let ctx = DiscoveryContext {
        client: crate::ClientId::OpenCode,
        home_dir: home.path(),
        root_override: Some(store.path.parent().unwrap()),
        scanner_settings: &settings,
        cancellation: Default::default(),
    };
    let inputs = super::DRIVER.discover_inputs(&ctx).unwrap();
    let paths: Vec<&Path> = inputs.iter().map(|input| input.path.as_path()).collect();
    assert!(paths.contains(&store.path.as_path()));
    assert!(paths.contains(&settings.opencode_db_paths[0].as_path()));
    assert_eq!(paths.len(), 2);
}
