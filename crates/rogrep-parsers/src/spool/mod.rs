//! SQLite→JSONL spool materialization for hermes and opencode.
//!
//! Both agents store sessions in SQLite databases that mutate in place, so
//! they can't be tailed like JSONL logs. Each session is materialized to an
//! append-only-looking JSONL spool file that flows through the normal
//! parser pipeline. Strategy: per-session change fingerprints; unchanged
//! sessions are skipped entirely, changed sessions are atomically REWRITTEN
//! (tmp + rename) — the parser's prefix fingerprint then forces a clean full
//! reparse of just that session. Sessions are small; correctness over
//! cleverness.

#[cfg(feature = "sqlite")]
pub mod hermes_db;
#[cfg(feature = "sqlite")]
pub mod opencode_db;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(feature = "sqlite")]
use std::collections::HashSet;
use std::path::Path;

/// Column names present in `table`, or an empty set if the table is absent.
///
/// Agent databases are third-party and evolve without notice: a column an
/// exporter needs today may be gone tomorrow. Probing first lets a SELECT be
/// built from what actually exists instead of aborting the whole provider on
/// the first `no such column`.
#[cfg(feature = "sqlite")]
pub fn table_columns(conn: &rusqlite::Connection, table: &str) -> rusqlite::Result<HashSet<String>> {
    conn.prepare("SELECT name FROM pragma_table_info(?1)")?
        .query_map([table], |r| r.get::<_, String>(0))?
        .collect()
}

/// `name` if the table has that column, else the literal `NULL`.
///
/// Substituting a literal keeps column *positions* stable, so the row-mapping
/// indices below don't have to shift with the schema.
#[cfg(feature = "sqlite")]
pub fn col_or_null(cols: &HashSet<String>, name: &str) -> String {
    if cols.contains(name) {
        name.to_string()
    } else {
        "NULL".to_string()
    }
}

#[derive(Debug, Default)]
pub struct SpoolReport {
    pub sessions_seen: usize,
    pub sessions_written: usize,
    pub errors: Vec<String>,
}

/// Per-session change fingerprint, persisted at `<spool>/<agent>/state.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SpoolState {
    pub sessions: HashMap<String, String>,
}

impl SpoolState {
    pub fn load(dir: &Path) -> SpoolState {
        std::fs::read(dir.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("state.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default())?;
        std::fs::rename(tmp, dir.join("state.json"))
    }
}

/// Atomic write of one session spool file.
pub fn write_spool_file(dir: &Path, session_id: &str, lines: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let safe: String = session_id
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let target = dir.join(format!("{safe}.jsonl"));
    let tmp = dir.join(format!(".{safe}.jsonl.tmp"));
    std::fs::write(&tmp, lines.join("\n") + "\n")?;
    std::fs::rename(tmp, target)
}

/// Run all SQLite exporters for a home directory. Missing databases are not
/// errors — most machines have a subset of agents installed.
#[cfg(feature = "sqlite")]
pub fn export_all(home: &Path, spool_root: &Path) -> SpoolReport {
    let mut report = SpoolReport::default();
    let hermes_db = home.join(".hermes/state.db");
    if hermes_db.is_file() {
        hermes_db::export(&hermes_db, &spool_root.join("hermes"), &mut report);
    }
    let opencode_db = home.join(".local/share/opencode/opencode.db");
    if opencode_db.is_file() {
        opencode_db::export(&opencode_db, &spool_root.join("opencode"), &mut report);
    }
    report
}

#[cfg(not(feature = "sqlite"))]
pub fn export_all(_home: &Path, _spool_root: &Path) -> SpoolReport {
    SpoolReport::default()
}
