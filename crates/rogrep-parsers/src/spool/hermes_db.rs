//! Hermes exporter: `~/.hermes/state.db` (sessions + messages tables) →
//! per-session spool JSONL.
//!
//! The Hermes schema is not stable across versions — `schema_version` 12, for
//! instance, has no `cwd`, `archived`, or `rewind_count` on `sessions` and no
//! `active` on `messages`. Rather than pin to one revision, every optional
//! column is probed with `pragma_table_info` and either selected or replaced
//! with a literal; the soft-delete filters are only applied when the columns
//! backing them exist. A schema that lacks a field yields a null in the spool
//! record, which the parser already tolerates.

use super::{col_or_null, table_columns, write_spool_file, SpoolReport, SpoolState};
use anyhow::bail;
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;

pub fn export(db_path: &Path, spool_dir: &Path, report: &mut SpoolReport) {
    if let Err(e) = export_inner(db_path, spool_dir, report) {
        report.errors.push(format!("hermes export: {e}"));
    }
}

fn export_inner(db_path: &Path, spool_dir: &Path, report: &mut SpoolReport) -> anyhow::Result<()> {
    let uri = format!("file:{}?mode=ro", db_path.display());
    let conn = Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(400))?;
    let mut state = SpoolState::load(spool_dir);

    struct SessionRow {
        id: String,
        cwd: Option<String>,
        title: Option<String>,
        model: Option<String>,
        parent: Option<String>,
        started_at: Option<f64>,
        rewind_count: i64,
    }

    let scols = table_columns(&conn, "sessions")?;
    let mcols = table_columns(&conn, "messages")?;
    if scols.is_empty() || mcols.is_empty() {
        bail!("not a hermes database (missing sessions/messages tables)");
    }

    // Older revisions soft-delete; newer ones just don't have the concept.
    // Absent column → no filter, i.e. every row is live.
    let live_sessions = if scols.contains("archived") {
        "WHERE COALESCE(archived, 0) = 0"
    } else {
        ""
    };
    let live_messages = if mcols.contains("active") {
        "AND COALESCE(active, 1) = 1"
    } else {
        ""
    };
    let rewind = if scols.contains("rewind_count") {
        "COALESCE(rewind_count, 0)"
    } else {
        "0"
    };

    let sessions: Vec<SessionRow> = conn
        .prepare(&format!(
            "SELECT id, {}, {}, {}, {}, {}, {rewind}
             FROM sessions {live_sessions}",
            col_or_null(&scols, "cwd"),
            col_or_null(&scols, "title"),
            col_or_null(&scols, "model"),
            col_or_null(&scols, "parent_session_id"),
            col_or_null(&scols, "started_at"),
        ))?
        .query_map([], |r| {
            Ok(SessionRow {
                id: r.get(0)?,
                cwd: r.get(1)?,
                title: r.get(2)?,
                model: r.get(3)?,
                parent: r.get(4)?,
                started_at: r.get::<_, Option<f64>>(5)?,
                rewind_count: r.get(6)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    // Kept as a real literal, not `col_or_null`: the fingerprint column is
    // read back as f64, and `MAX(NULL)` would coalesce to an integer.
    let max_ts = if mcols.contains("timestamp") {
        "COALESCE(MAX(timestamp), 0.0)"
    } else {
        "0.0"
    };
    let mut fp_stmt = conn.prepare(&format!(
        "SELECT COALESCE(MAX(id), 0), COUNT(*), {max_ts}
         FROM messages WHERE session_id = ?1 {live_messages}",
    ))?;
    let mut msg_stmt = conn.prepare(&format!(
        "SELECT {}, {}, {}, {}, {}, {}, {}
         FROM messages WHERE session_id = ?1 {live_messages}
         ORDER BY id",
        col_or_null(&mcols, "role"),
        col_or_null(&mcols, "content"),
        col_or_null(&mcols, "tool_calls"),
        col_or_null(&mcols, "tool_call_id"),
        col_or_null(&mcols, "tool_name"),
        col_or_null(&mcols, "timestamp"),
        col_or_null(&mcols, "token_count"),
    ))?;

    let mut changed = false;
    for s in sessions {
        report.sessions_seen += 1;
        let (max_id, count, max_ts): (i64, i64, f64) =
            fp_stmt.query_row([&s.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        if count == 0 {
            continue;
        }
        let fingerprint = format!("{max_id}:{count}:{max_ts}:{}:{:?}", s.rewind_count, s.title);
        if state.sessions.get(&s.id) == Some(&fingerprint) {
            continue;
        }
        let mut lines = vec![json!({
            "type": "session_meta",
            "session_id": s.id,
            "source": "hermes",
            "cwd": s.cwd,
            "title": s.title,
            "model": s.model,
            "parent_session_id": s.parent,
            "started_at": s.started_at,
        })
        .to_string()];
        let rows = msg_stmt.query_map([&s.id], |r| {
            Ok(json!({
                "type": "message",
                "role": r.get::<_, Option<String>>(0)?,
                "content": r.get::<_, Option<String>>(1)?,
                "tool_calls": r.get::<_, Option<String>>(2)?,
                "tool_call_id": r.get::<_, Option<String>>(3)?,
                "tool_name": r.get::<_, Option<String>>(4)?,
                "timestamp": r.get::<_, Option<f64>>(5)?,
                "token_count": r.get::<_, Option<i64>>(6)?,
            })
            .to_string())
        })?;
        for row in rows {
            lines.push(row?);
        }
        write_spool_file(spool_dir, &s.id, &lines)?;
        state.sessions.insert(s.id.clone(), fingerprint);
        report.sessions_written += 1;
        changed = true;
    }
    if changed {
        state.save(spool_dir)?;
    }
    Ok(())
}
