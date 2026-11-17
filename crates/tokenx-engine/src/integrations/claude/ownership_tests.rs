#![cfg(test)]

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::acquisition::test_support::fold_prepared_local_inputs_with_pricing;
use crate::acquisition::{prepare_inventory, PreparedInventory};
use crate::{
    AcquisitionCancellation, AttributedUsageRecord, ClientId, ClientRootOverrides, ClientUniverse,
    DataHealth, DateRange, SourceFingerprint,
};

fn transcript(root: &Path, session: &str, relative: &str) {
    let path = root.join(".claude/projects/demo").join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let row = json!({"type":"assistant", "uuid":relative, "sessionId":session,
        "timestamp":"2026-11-01T00:00:00Z", "message":{"id":relative,
            "role":"assistant", "model":"claude-sonnet-4",
            "usage":{"input_tokens":100,"output_tokens":20,
                "cache_creation_input_tokens":0,"cache_read_input_tokens":0}}});
    std::fs::write(path, format!("{row}\n")).unwrap();
}

fn database(path: &Path) -> Connection {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session_message (id TEXT, session_id TEXT, data TEXT, type TEXT);",
    )
    .unwrap();
    conn
}

fn payload(native: &str) -> Value {
    json!({"time":{"created":1793491200000_i64},
        "model":{"id":"claude-sonnet-4","providerID":"claude-agent"},
        "tokens":{"input":100,"output":20,"reasoning":0,"cache":{"read":0,"write":0}},
        "parts":[{"type":"text","native":{"sessionID":native}}]})
}

fn insert(conn: &Connection, id: &str, role: &str, payload: &Value) {
    conn.execute(
        "INSERT INTO session_message VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, "opencode-session", payload.to_string(), role],
    )
    .unwrap();
}

fn prepare(
    root: &Path,
    clients: &[ClientId],
    settings: &crate::scanner::ScannerSettings,
    roots: &ClientRootOverrides,
) -> PreparedInventory {
    prepare_inventory(
        root,
        ClientUniverse::new(clients.to_vec()).unwrap(),
        DateRange::default(),
        settings,
        roots,
        root.join("cache/input"),
        &AcquisitionCancellation::default(),
    )
    .unwrap()
}

fn fold(
    prepared: PreparedInventory,
) -> (Vec<AttributedUsageRecord>, DataHealth, SourceFingerprint) {
    let mut messages = Vec::new();
    let result = fold_prepared_local_inputs_with_pricing(prepared, None, &mut messages).unwrap();
    (messages, result.health, result.source_fingerprint)
}

fn scan(
    root: &Path,
    clients: &[ClientId],
) -> (Vec<AttributedUsageRecord>, DataHealth, SourceFingerprint) {
    fold(prepare(
        root,
        clients,
        &Default::default(),
        &Default::default(),
    ))
}

fn db_path(root: &Path) -> PathBuf {
    root.join(".local/share/opencode/opencode.db")
}

#[test]
fn mirrored_sessions_are_owned_by_opencode_on_cold_and_cached_claude_scans() {
    let root = tempfile::tempdir().unwrap();
    transcript(root.path(), "native", "native.jsonl");
    transcript(root.path(), "native", "native/subagents/agent-worker.jsonl");
    transcript(root.path(), "independent", "independent.jsonl");
    let conn = database(&db_path(root.path()));
    insert(&conn, "message", "assistant", &payload("native"));
    drop(conn);

    for clients in [
        vec![ClientId::Claude],
        vec![ClientId::OpenCode, ClientId::Claude],
    ] {
        for _ in 0..2 {
            let (messages, health, _) = scan(root.path(), &clients);
            assert!(health.is_empty());
            let claude: Vec<_> = messages
                .iter()
                .filter(|m| m.client == ClientId::Claude)
                .collect();
            assert_eq!(claude.len(), 1);
            assert_eq!(claude[0].session_id.as_ref(), "independent");
            assert_eq!(
                messages.iter().map(|m| m.tokens.total()).sum::<i64>(),
                if clients.len() == 1 { 120 } else { 240 }
            );
        }
    }
}

#[test]
fn database_additions_wal_changes_and_removal_recompute_ownership_of_raw_cache_hits() {
    let root = tempfile::tempdir().unwrap();
    transcript(root.path(), "native", "native.jsonl");
    let (before, _, absent) = scan(root.path(), &[ClientId::Claude]);
    assert_eq!(before.len(), 1);

    let path = db_path(root.path());
    let conn = database(&path);
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    insert(&conn, "message", "assistant", &payload("native"));
    let (claimed, health, added) = scan(root.path(), &[ClientId::Claude]);
    assert!(health.is_empty());
    assert!(claimed.is_empty());
    assert_ne!(absent, added);

    conn.execute("DELETE FROM session_message", []).unwrap();
    let (restored, health, changed) = scan(root.path(), &[ClientId::Claude]);
    assert!(health.is_empty());
    assert_eq!(restored.len(), 1);
    assert_ne!(added, changed);

    insert(&conn, "message", "assistant", &payload("native"));
    assert!(scan(root.path(), &[ClientId::Claude]).0.is_empty());
    drop(conn);
    std::fs::remove_file(&path).unwrap();
    let (restored, health, removed) = scan(root.path(), &[ClientId::Claude]);
    assert!(health.is_empty());
    assert_eq!(restored.len(), 1);
    assert_eq!(absent, removed);
}

#[test]
fn only_valid_countable_assistant_rows_establish_native_session_ownership() {
    let root = tempfile::tempdir().unwrap();
    let conn = database(&db_path(root.path()));
    for id in [
        "valid",
        "zero",
        "cost-only",
        "user",
        "missing-model",
        "missing-time",
        "negative",
    ] {
        transcript(root.path(), id, &format!("{id}.jsonl"));
        let mut data = payload(id);
        match id {
            "zero" => data["tokens"] = json!({"input":0,"output":0,"cache":{"read":0,"write":0}}),
            "cost-only" => {
                data["tokens"] = Value::Null;
                data["cost"] = json!(12.0);
            }
            "missing-model" => {
                data.as_object_mut().unwrap().remove("model");
            }
            "missing-time" => {
                data.as_object_mut().unwrap().remove("time");
            }
            "negative" => data["tokens"]["input"] = json!(-1),
            _ => {}
        }
        insert(
            &conn,
            id,
            if id == "user" { "user" } else { "assistant" },
            &data,
        );
    }
    // Provider is optional metadata: it cannot disable an otherwise valid claim.
    let mut no_provider = payload("valid");
    no_provider["model"]
        .as_object_mut()
        .unwrap()
        .remove("providerID");
    insert(&conn, "without-provider", "assistant", &no_provider);
    drop(conn);
    let (messages, health, _) = scan(root.path(), &[ClientId::Claude]);
    assert_eq!(messages.len(), 6);
    assert!(!messages.iter().any(|m| m.session_id.as_ref() == "valid"));
    assert_eq!(health.rejected_records(), 3);
}

#[test]
fn unreadable_ownership_does_not_hide_local_usage_and_is_retried_after_repair() {
    let root = tempfile::tempdir().unwrap();
    transcript(root.path(), "native", "native.jsonl");
    let path = db_path(root.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "not a SQLite database").unwrap();
    for _ in 0..2 {
        let (messages, health, _) = scan(root.path(), &[ClientId::Claude]);
        assert_eq!(messages.len(), 1);
        assert_eq!(health.failed_inputs(), 1);
        assert!(health.summarize().requires_input_retry());
    }
    std::fs::remove_file(&path).unwrap();
    let conn = database(&path);
    insert(&conn, "message", "assistant", &payload("native"));
    drop(conn);
    let (messages, health, _) = scan(root.path(), &[ClientId::Claude]);
    assert!(messages.is_empty());
    assert!(health.is_empty());
}

#[test]
fn an_ownership_database_changed_after_preparation_is_an_explicit_error() {
    let root = tempfile::tempdir().unwrap();
    transcript(root.path(), "native", "native.jsonl");
    let conn = database(&db_path(root.path()));
    conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    let prepared = prepare(
        root.path(),
        &[ClientId::Claude],
        &Default::default(),
        &Default::default(),
    );
    insert(&conn, "message", "assistant", &payload("native"));
    let (messages, health, _) = fold(prepared);
    assert_eq!(messages.len(), 1);
    assert_eq!(health.failed_inputs(), 1);
    assert_eq!(
        health.inputs()[0].status.failure().unwrap().operation,
        "verify OpenCode ownership snapshot"
    );
    assert!(scan(root.path(), &[ClientId::Claude]).0.is_empty());
}

#[test]
fn claude_ownership_uses_captured_opencode_roots_and_configured_database_paths() {
    let root = tempfile::tempdir().unwrap();
    transcript(root.path(), "native", "native.jsonl");
    let custom = root.path().join("custom/opencode.db");
    let conn = database(&custom);
    insert(&conn, "message", "assistant", &payload("native"));
    drop(conn);
    assert_eq!(scan(root.path(), &[ClientId::Claude]).0.len(), 1);
    let roots = ClientRootOverrides {
        opencode_data_dir: Some(custom.parent().unwrap().to_path_buf()),
        ..Default::default()
    };
    assert!(fold(prepare(
        root.path(),
        &[ClientId::Claude],
        &Default::default(),
        &roots
    ))
    .0
    .is_empty());
    let settings = crate::scanner::ScannerSettings {
        opencode_db_paths: vec![custom],
        ..Default::default()
    };
    assert!(fold(prepare(
        root.path(),
        &[ClientId::Claude],
        &settings,
        &Default::default()
    ))
    .0
    .is_empty());
}

#[test]
fn shared_opencode_snapshot_prevents_ownership_from_hiding_usage_missing_from_a_cache_hit() {
    let root = tempfile::tempdir().unwrap();
    transcript(root.path(), "native-new", "native-new.jsonl");
    let conn = database(&db_path(root.path()));
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    let mut old = payload("native-old");
    old["id"] = json!("old");
    insert(&conn, "old", "assistant", &old);
    assert_eq!(scan(root.path(), &[ClientId::OpenCode]).0.len(), 1);

    // Model discovery finishing on either side of a commit. The same assembly
    // step used by prepare_inventory must bind both consumers to OpenCode's
    // existing prepared input rather than Claude's independently newer view.
    let mut prepared = prepare(
        root.path(),
        &[ClientId::OpenCode],
        &Default::default(),
        &Default::default(),
    );
    let mut new = payload("native-new");
    new["id"] = json!("new");
    insert(&conn, "new", "assistant", &new);
    let mut claude = prepare(
        root.path(),
        &[ClientId::Claude],
        &Default::default(),
        &Default::default(),
    );
    let mut outcomes = vec![
        crate::acquisition::PreparedIntegrationOutcome {
            group: prepared.groups.pop().unwrap(),
            health: std::mem::take(&mut prepared.health),
        },
        crate::acquisition::PreparedIntegrationOutcome {
            group: claude.groups.pop().unwrap(),
            health: claude.health,
        },
    ];
    crate::acquisition::attach_claude_ownership(&mut outcomes, || {
        panic!("selected OpenCode inputs must be shared without a second discovery")
    })
    .unwrap();
    for outcome in outcomes {
        prepared.groups.push(outcome.group);
        prepared.health.merge(outcome.health);
    }
    let clients = ClientUniverse::new(vec![ClientId::OpenCode, ClientId::Claude]).unwrap();
    prepared.signature = crate::acquisition::source_fingerprint(&clients, &prepared.groups);
    prepared.input_footprint =
        crate::acquisition::inventory_input_footprint(&clients, &prepared.groups);
    let (messages, health, _) = fold(prepared);
    assert_eq!(messages.iter().map(|m| m.tokens.total()).sum::<i64>(), 240);
    assert!(health.summarize().requires_input_retry());
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.client == ClientId::Claude)
            .count(),
        1
    );

    let (messages, health, _) = scan(root.path(), &[ClientId::OpenCode, ClientId::Claude]);
    assert_eq!(messages.iter().map(|m| m.tokens.total()).sum::<i64>(), 240);
    assert!(health.is_empty());
    assert!(messages.iter().all(|m| m.client == ClientId::OpenCode));
}

#[test]
fn opaque_large_numbers_do_not_discard_valid_ownership() {
    let root = tempfile::tempdir().unwrap();
    transcript(root.path(), "native-a", "native-a.jsonl");
    transcript(root.path(), "native-z", "native-z.jsonl");
    let conn = database(&db_path(root.path()));
    let mut a = payload("native-a");
    a["id"] = json!("a");
    insert(&conn, "a", "assistant", &a);
    let mut z = payload("native-z");
    z["id"] = json!("z");
    let raw = z
        .to_string()
        .replacen("{", "{\"opaque\":{\"values\":[1e400]},", 1);
    conn.execute(
        "INSERT INTO session_message VALUES ('z','opencode-session',?1,'assistant')",
        [raw],
    )
    .unwrap();
    drop(conn);
    for _ in 0..2 {
        let (messages, health, _) = scan(root.path(), &[ClientId::OpenCode, ClientId::Claude]);
        assert_eq!(messages.iter().map(|m| m.tokens.total()).sum::<i64>(), 240);
        assert!(health.is_empty());
        assert!(messages.iter().all(|m| m.client == ClientId::OpenCode));
    }
}

#[test]
fn malformed_native_identity_keeps_claims_before_and_after_its_record() {
    let root = tempfile::tempdir().unwrap();
    let conn = database(&db_path(root.path()));
    for id in ["a", "b", "c"] {
        transcript(root.path(), id, &format!("{id}.jsonl"));
        let mut row = payload(id);
        row["id"] = json!(id);
        let raw = if id == "b" {
            row.to_string()
                .replace(r#""sessionID":"b""#, r#""sessionID":"\uD800""#)
        } else {
            row.to_string()
        };
        conn.execute(
            "INSERT INTO session_message VALUES (?1,'opencode-session',?2,'assistant')",
            rusqlite::params![id, raw],
        )
        .unwrap();
    }
    drop(conn);
    // Usage decoding ignores unrelated native metadata, so all three rows
    // are independently valid local OpenCode usage.
    let decoded =
        crate::integrations::opencode::decode::parse_opencode_sqlite(&db_path(root.path()))
            .unwrap();
    assert_eq!(decoded.messages.len(), 3);
    let (messages, health, _) = scan(root.path(), &[ClientId::Claude]);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].session_id.as_ref(), "b");
    assert_eq!(health.rejected_records(), 1);
    assert_eq!(health.failed_inputs(), 0);
}
