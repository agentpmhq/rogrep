//! Hermes exporter: `~/.hermes/state.db` (sessions + messages tables) →
//! per-session spool JSONL.

use super::{write_spool_file, SpoolReport, SpoolState};
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

    let sessions: Vec<SessionRow> = conn
        .prepare(
            "SELECT id, cwd, title, model, parent_session_id, started_at,
                    COALESCE(rewind_count, 0)
             FROM sessions WHERE COALESCE(archived, 0) = 0",
        )?
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

    let mut fp_stmt = conn.prepare(
        "SELECT COALESCE(MAX(id), 0), COUNT(*), COALESCE(MAX(timestamp), 0)
         FROM messages WHERE session_id = ?1 AND COALESCE(active, 1) = 1",
    )?;
    let mut msg_stmt = conn.prepare(
        "SELECT role, content, tool_calls, tool_call_id, tool_name, timestamp, token_count
         FROM messages WHERE session_id = ?1 AND COALESCE(active, 1) = 1
         ORDER BY id",
    )?;

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
