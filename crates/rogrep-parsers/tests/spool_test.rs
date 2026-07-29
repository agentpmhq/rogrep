//! Spool exporter tests against synthetic hermes/opencode databases.

#![cfg(feature = "sqlite")]

use rogrep_parsers::spool::{self, SpoolReport};
use rusqlite::Connection;
use std::path::Path;

fn make_hermes_db(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions(id TEXT PRIMARY KEY, source TEXT, model TEXT, cwd TEXT,
            title TEXT, parent_session_id TEXT, started_at REAL, rewind_count INTEGER,
            message_count INTEGER, archived INTEGER);
         CREATE TABLE messages(id INTEGER PRIMARY KEY, session_id TEXT, role TEXT,
            content TEXT, tool_calls TEXT, tool_call_id TEXT, tool_name TEXT,
            timestamp REAL, token_count INTEGER, active INTEGER DEFAULT 1);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions VALUES('s1','cli','gpt-5.5','/home/u/src/x','Fix swap',NULL,1776440300.0,0,2,0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages(session_id, role, content, timestamp) VALUES('s1','user','why swap full?',1776440302.0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages(session_id, role, content, timestamp, token_count) VALUES('s1','assistant','because reasons',1776440310.0, 12)",
        [],
    )
    .unwrap();
    conn
}

#[test]
fn hermes_export_skip_and_rewrite() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("state.db");
    let conn = make_hermes_db(&db_path);
    let spool = tmp.path().join("spool/hermes");

    let mut report = SpoolReport::default();
    spool::hermes_db::export(&db_path, &spool, &mut report);
    assert_eq!(report.sessions_written, 1, "{:?}", report.errors);
    let file = spool.join("s1.jsonl");
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content.lines().count(), 3, "meta + 2 messages");
    assert!(content.contains("why swap full?"));

    // Unchanged → skipped.
    let mut report2 = SpoolReport::default();
    spool::hermes_db::export(&db_path, &spool, &mut report2);
    assert_eq!(report2.sessions_written, 0);

    // New message → rewritten.
    conn.execute(
        "INSERT INTO messages(session_id, role, content, timestamp) VALUES('s1','user','follow-up',1776440400.0)",
        [],
    )
    .unwrap();
    let mut report3 = SpoolReport::default();
    spool::hermes_db::export(&db_path, &spool, &mut report3);
    assert_eq!(report3.sessions_written, 1);
    assert!(std::fs::read_to_string(&file).unwrap().contains("follow-up"));

    // Rewind (deactivate a row) → rewritten without it.
    conn.execute("UPDATE messages SET active=0 WHERE content='follow-up'", []).unwrap();
    conn.execute("UPDATE sessions SET rewind_count=1 WHERE id='s1'", []).unwrap();
    let mut report4 = SpoolReport::default();
    spool::hermes_db::export(&db_path, &spool, &mut report4);
    assert_eq!(report4.sessions_written, 1);
    assert!(!std::fs::read_to_string(&file).unwrap().contains("follow-up"));
}

#[test]
fn opencode_export_and_parse_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("opencode.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session(id TEXT PRIMARY KEY, directory TEXT, title TEXT, agent TEXT,
            model TEXT, parent_id TEXT, time_created INTEGER, time_archived INTEGER);
         CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, data TEXT);
         CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
            data TEXT, time_created INTEGER, time_updated INTEGER);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session VALUES('ses_1','/home/u/src/x','Review code','build',
            '{\"id\":\"glm-5.2\"}',NULL,1782311855000,NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message VALUES('msg_1','ses_1','{\"role\":\"user\"}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part VALUES('prt_1','msg_1','ses_1',
            '{\"type\":\"text\",\"text\":\"review the deriver\"}',1782311855413,1782311855413)",
        [],
    )
    .unwrap();

    let spool = tmp.path().join("spool/opencode");
    let mut report = SpoolReport::default();
    spool::opencode_db::export(&db_path, &spool, &mut report);
    assert_eq!(report.sessions_written, 1, "{:?}", report.errors);

    // Parse the spool file through the opencode provider.
    let provider = rogrep_parsers::provider_for_kind(rogrep_model::AgentKind::Opencode).unwrap();
    let spool_file = spool.join("ses_1.jsonl");
    // Claimed by path shape.
    assert!(provider.claims_path(&spool_file.to_string_lossy()));
    let out = rogrep_parsers::parse_source(provider, &spool_file, None).unwrap();
    assert_eq!(out.conversation.turns.len(), 1);
    assert_eq!(out.conversation.turns[0].text, "review the deriver");
    assert_eq!(out.conversation.cwd.as_deref(), Some("/home/u/src/x"));
    assert_eq!(out.conversation.title.as_deref(), Some("Review code"));

    // Part mutation (tool completes) → rewrite with new content.
    conn.execute(
        "UPDATE part SET data='{\"type\":\"text\",\"text\":\"review the deriver NOW\"}',
            time_updated=1782311860000 WHERE id='prt_1'",
        [],
    )
    .unwrap();
    let mut report2 = SpoolReport::default();
    spool::opencode_db::export(&db_path, &spool, &mut report2);
    assert_eq!(report2.sessions_written, 1);
    let out2 = rogrep_parsers::parse_source(provider, &spool_file, Some(out.state)).unwrap();
    // The rewrite invalidates the fingerprint → clean full reparse.
    assert_eq!(out2.replace_from, 0);
    assert!(out2.conversation.turns[0].text.contains("NOW"));
}
