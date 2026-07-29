//! opencode exporter: `~/.local/share/opencode/opencode.db`
//! (session/message/part tables) → per-session spool JSONL. Parts merge a
//! tool call and its result into one row and MUTATE in place while running;
//! the fingerprint covers max(part id) + max(time_updated) + count so any
//! mutation rewrites the session spool.

use super::{write_spool_file, SpoolReport, SpoolState};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::Path;

pub fn export(db_path: &Path, spool_dir: &Path, report: &mut SpoolReport) {
    if let Err(e) = export_inner(db_path, spool_dir, report) {
        report.errors.push(format!("opencode export: {e}"));
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
        directory: Option<String>,
        title: Option<String>,
        agent: Option<String>,
        model_json: Option<String>,
        parent: Option<String>,
        created: Option<i64>,
    }

    let sessions: Vec<SessionRow> = conn
        .prepare(
            "SELECT id, directory, title, agent, model, parent_id, time_created
             FROM session WHERE time_archived IS NULL",
        )?
        .query_map([], |r| {
            Ok(SessionRow {
                id: r.get(0)?,
                directory: r.get(1)?,
                title: r.get(2)?,
                agent: r.get(3)?,
                model_json: r.get(4)?,
                parent: r.get(5)?,
                created: r.get(6)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    let mut fp_stmt = conn.prepare(
        "SELECT COALESCE(MAX(id), ''), COUNT(*), COALESCE(MAX(time_updated), 0)
         FROM part WHERE session_id = ?1",
    )?;
    let mut part_stmt = conn.prepare(
        "SELECT p.id, p.message_id, p.data, p.time_created, m.data
         FROM part p LEFT JOIN message m ON m.id = p.message_id
         WHERE p.session_id = ?1 ORDER BY p.id",
    )?;

    let mut changed = false;
    for s in sessions {
        report.sessions_seen += 1;
        let (max_id, count, max_updated): (String, i64, i64) =
            fp_stmt.query_row([&s.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        if count == 0 {
            continue;
        }
        let fingerprint = format!("{max_id}:{count}:{max_updated}:{:?}", s.title);
        if state.sessions.get(&s.id) == Some(&fingerprint) {
            continue;
        }
        let model = s
            .model_json
            .as_deref()
            .and_then(|m| serde_json::from_str::<Value>(m).ok())
            .and_then(|v| v.get("id").and_then(Value::as_str).map(|s| s.to_string()));
        let mut lines = vec![json!({
            "type": "session_meta",
            "session_id": s.id,
            "source": "opencode",
            "cwd": s.directory,
            "title": s.title,
            "agent": s.agent,
            "model": model,
            "parent_session_id": s.parent,
            "started_at": s.created,
        })
        .to_string()];
        let rows = part_stmt.query_map([&s.id], |r| {
            let part_data: Option<String> = r.get(2)?;
            let message_data: Option<String> = r.get(4)?;
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                part_data,
                r.get::<_, Option<i64>>(3)?,
                message_data,
            ))
        })?;
        for row in rows {
            let (part_id, message_id, part_data, time_created, message_data) = row?;
            let data: Value = part_data
                .as_deref()
                .and_then(|d| serde_json::from_str(d).ok())
                .unwrap_or(Value::Null);
            let mdata: Value = message_data
                .as_deref()
                .and_then(|d| serde_json::from_str(d).ok())
                .unwrap_or(Value::Null);
            let message_role = mdata.get("role").and_then(Value::as_str).unwrap_or("");
            let model_id = mdata.get("modelID").and_then(Value::as_str);
            lines.push(
                json!({
                    "type": "part",
                    "part_id": part_id,
                    "message_id": message_id,
                    "message_role": message_role,
                    "model": model_id,
                    "ts": time_created,
                    "data": data,
                })
                .to_string(),
            );
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
