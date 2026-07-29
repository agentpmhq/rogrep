//! The store: checkpoints, conversations, turns, exchanges.

use crate::schema::{DDL, SCHEMA_VERSION};
use anyhow::Result;
use rogrep_model::{build_exchanges, is_visible_turn, Conversation, SpecialTurn};
use rogrep_parsers::driver::DriverOutput;
use rogrep_parsers::state::ParseState;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;

pub struct Store {
    pub(crate) conn: Connection,
}

#[derive(Clone, Debug)]
pub struct FileCheckpoint {
    pub conversation_id: String,
    pub provider: String,
    pub size: u64,
    pub mtime_ns: i128,
    pub state: ParseState,
}

/// Lightweight conversation row for listings.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ConversationRow {
    pub id: String,
    pub source_path: String,
    pub provider: String,
    pub normalized_project: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub title: Option<String>,
    pub origin: String,
    pub is_subagent: bool,
    pub started_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    pub turn_count: u32,
    pub exchange_count: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub estimated_tokens: u64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Everything in the store is derived: on a schema-version mismatch,
        // wipe the db and let the next sync re-derive from source files.
        for attempt in 0..2 {
            let conn = Connection::open(path)?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
            let version_ok = conn
                .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| {
                    r.get::<_, String>(0)
                })
                .optional()
                .unwrap_or(None)
                .map(|v| v == SCHEMA_VERSION.to_string());
            if version_ok == Some(false) && attempt == 0 {
                drop(conn);
                std::fs::remove_file(path)?;
                let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
                let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
                continue;
            }
            conn.execute_batch(DDL)?;
            let store = Store { conn };
            store.check_schema_version()?;
            return Ok(store);
        }
        unreachable!("schema wipe loop always returns");
    }

    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(DDL)?;
        let store = Store { conn };
        store.check_schema_version()?;
        Ok(store)
    }

    fn check_schema_version(&self) -> Result<()> {
        let existing: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0))
            .optional()?;
        match existing {
            None => {
                self.conn.execute(
                    "INSERT INTO meta(key,value) VALUES('schema_version', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(v) if v == SCHEMA_VERSION.to_string() => {}
            Some(v) => {
                anyhow::bail!(
                    "store schema version {v} != {SCHEMA_VERSION}; delete the data dir to rebuild (it is all derived data)"
                );
            }
        }
        Ok(())
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| r.get(0))
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn file_checkpoints(&self) -> Result<HashMap<String, FileCheckpoint>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, conversation_id, provider, size, mtime_ns, parse_state FROM files")?;
        let rows = stmt.query_map([], |r| {
            let path: String = r.get(0)?;
            let state_json: String = r.get(5)?;
            Ok((
                path,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                state_json,
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (path, conversation_id, provider, size, mtime_ns, state_json) = row?;
            let state: ParseState = serde_json::from_str(&state_json).unwrap_or_default();
            out.insert(
                path,
                FileCheckpoint {
                    conversation_id,
                    provider,
                    size: size as u64,
                    mtime_ns: mtime_ns as i128,
                    state,
                },
            );
        }
        Ok(out)
    }

    /// Apply one parse run atomically: upsert the conversation summary,
    /// replace turns/exchanges from the watermark, save the checkpoint.
    pub fn apply_parse(
        &mut self,
        out: &DriverOutput,
        file_size: u64,
        file_mtime_ns: i128,
    ) -> Result<u64> {
        let tx = self.conn.transaction()?;
        let conv = &out.conversation;
        let cid = conv.id.as_str();
        let replace_from = out.replace_from as i64;

        // Replace the open tail.
        tx.execute(
            "DELETE FROM turns WHERE conversation_id=?1 AND turn_index>=?2",
            params![cid, replace_from],
        )?;
        tx.execute(
            "DELETE FROM exchanges WHERE conversation_id=?1 AND start_turn>=?2",
            params![cid, replace_from],
        )?;
        tx.execute(
            "DELETE FROM tool_events WHERE conversation_id=?1 AND turn_index>=?2",
            params![cid, replace_from],
        )?;
        tx.execute(
            "DELETE FROM file_refs WHERE conversation_id=?1 AND turn_index>=?2",
            params![cid, replace_from],
        )?;

        // Exchange ordinals continue from the frozen prefix. The parse chain
        // tracks the frozen exchange count, keeping store and search index
        // ordinals identical.
        let base_ordinal: i64 = out.exchange_base as i64;
        let exchanges = build_exchanges(&conv.turns);
        let ordinal_for = |turn_index: u32| -> i64 {
            exchanges
                .iter()
                .find(|e| turn_index >= e.start_turn && turn_index < e.end_turn)
                .map(|e| base_ordinal + e.ordinal as i64)
                .unwrap_or(base_ordinal)
        };

        {
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO turns(conversation_id, turn_index, exchange_ordinal, role,
                   speaker, ts, model, cwd, text_len, visible, tool_name, tool_direction,
                   tool_status, special_kind, input_tokens, output_tokens,
                   cache_creation_tokens, cache_read_tokens, reasoning_tokens,
                   estimated_tokens, source_line, source_byte_start, source_byte_end)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
            )?;
            for t in &conv.turns {
                ins.execute(params![
                    cid,
                    t.turn_index,
                    ordinal_for(t.turn_index),
                    t.role.as_str(),
                    t.speaker,
                    t.ts,
                    t.model,
                    t.cwd,
                    t.text.len() as i64,
                    is_visible_turn(t),
                    t.tool.as_ref().map(|i| i.name.as_str()),
                    t.tool.as_ref().and_then(|i| i.direction).map(|d| match d {
                        rogrep_model::ToolDirection::Use => "use",
                        rogrep_model::ToolDirection::Output => "output",
                    }),
                    t.tool.as_ref().map(|i| i.status.as_str()),
                    special_kind(&t.special),
                    t.tokens.input,
                    t.tokens.output,
                    t.tokens.cache_creation,
                    t.tokens.cache_read,
                    t.tokens.reasoning_output,
                    t.tokens.estimated,
                    t.source.line,
                    t.source.byte_start,
                    t.source.byte_end,
                ])?;
            }
        }

        {
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO exchanges(conversation_id, ordinal, user_turn_index,
                   start_turn, end_turn, started_at, ended_at, duration_ms, user_preview,
                   assistant_turns, tool_calls, failed_tool_calls, rejected_tool_calls,
                   has_error, interrupted, compacted, input_tokens, output_tokens,
                   cache_creation_tokens, cache_read_tokens, reasoning_tokens, estimated_tokens)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
            )?;
            for e in &exchanges {
                ins.execute(params![
                    cid,
                    base_ordinal + e.ordinal as i64,
                    e.user_turn_index,
                    e.start_turn,
                    e.end_turn,
                    e.started_at,
                    e.ended_at,
                    e.duration_ms(),
                    e.user_preview,
                    e.assistant_turns,
                    e.tool_calls,
                    e.failed_tool_calls,
                    e.rejected_tool_calls,
                    e.signals.error,
                    e.signals.interrupted,
                    e.signals.compacted,
                    e.tokens.input,
                    e.tokens.output,
                    e.tokens.cache_creation,
                    e.tokens.cache_read,
                    e.tokens.reasoning_output,
                    e.tokens.estimated,
                ])?;
            }
        }

        // Tool events + file refs.
        {
            let mut ins_event = tx.prepare_cached(
                "INSERT OR REPLACE INTO tool_events(conversation_id, turn_index, seq,
                   exchange_ordinal, tool, mutating, status, cmd_head, git_facets, ts)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )?;
            let mut ins_ref = tx.prepare_cached(
                "INSERT OR REPLACE INTO file_refs(conversation_id, turn_index, path, mode)
                 VALUES(?1,?2,?3,?4)",
            )?;
            for t in &conv.turns {
                let Some(info) = &t.tool else { continue };
                if info.direction != Some(rogrep_model::ToolDirection::Use) {
                    continue;
                }
                let facets = rogrep_tooltree::facet_tokens_for_turn(t);
                let cmd_head = facets
                    .iter()
                    .find_map(|f| f.strip_prefix("tool_cmd:"))
                    .map(|s| s.to_string());
                let git_facets: Vec<&String> =
                    facets.iter().filter(|f| f.starts_with("git_")).collect();
                let mutating = facets.iter().any(|f| f == "tool_mutating:true");
                ins_event.execute(params![
                    cid,
                    t.turn_index,
                    0,
                    ordinal_for(t.turn_index),
                    info.name.to_lowercase(),
                    mutating,
                    info.status.as_str(),
                    cmd_head,
                    if git_facets.is_empty() {
                        None
                    } else {
                        Some(
                            git_facets
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(" "),
                        )
                    },
                    t.ts,
                ])?;
                for (path, mode) in rogrep_tooltree::facets::file_refs_for_turn(t) {
                    ins_ref.execute(params![cid, t.turn_index, path, mode])?;
                }
            }
        }

        // Conversation summary.
        let (turn_count, exchange_count): (i64, i64) = {
            let turn_count = replace_from + conv.turns.len() as i64;
            let exchange_count = base_ordinal + exchanges.len() as i64;
            (turn_count, exchange_count)
        };
        tx.execute(
            "INSERT INTO conversations(id, source_path, provider, project, normalized_project,
               cwd, model, title, origin, is_subagent, parent_conversation_id, subagent_id,
               started_at, last_activity_at, turn_count, exchange_count, malformed_lines,
               input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
               reasoning_tokens, estimated_tokens)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)
             ON CONFLICT(id) DO UPDATE SET
               project=excluded.project, normalized_project=excluded.normalized_project,
               cwd=excluded.cwd, model=excluded.model, title=excluded.title,
               origin=excluded.origin, started_at=excluded.started_at,
               last_activity_at=excluded.last_activity_at, turn_count=excluded.turn_count,
               exchange_count=excluded.exchange_count, malformed_lines=excluded.malformed_lines,
               input_tokens=excluded.input_tokens, output_tokens=excluded.output_tokens,
               cache_creation_tokens=excluded.cache_creation_tokens,
               cache_read_tokens=excluded.cache_read_tokens,
               reasoning_tokens=excluded.reasoning_tokens,
               estimated_tokens=excluded.estimated_tokens",
            params![
                cid,
                conv.source_path,
                conv.agent.as_str(),
                conv.project,
                conv.normalized_project,
                conv.cwd,
                conv.model,
                conv.title.clone().unwrap_or_else(|| fallback_title(conv)),
                conv.origin.as_str(),
                conv.subagent.is_some(),
                conv.subagent.as_ref().and_then(|s| s.parent_id.as_ref().map(|i| i.as_str().to_string())),
                conv.subagent.as_ref().and_then(|s| s.subagent_id.clone()),
                conv.first_seen,
                conv.last_seen,
                turn_count,
                exchange_count,
                conv.malformed_lines,
                conv.tokens.input,
                conv.tokens.output,
                conv.tokens.cache_creation,
                conv.tokens.cache_read,
                conv.tokens.reasoning_output,
                conv.tokens.estimated,
            ],
        )?;

        // Checkpoint.
        tx.execute(
            "INSERT OR REPLACE INTO files(path, conversation_id, provider, size, mtime_ns, parse_state)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                conv.source_path,
                cid,
                conv.agent.as_str(),
                file_size as i64,
                file_mtime_ns as i64,
                serde_json::to_string(&out.state)?,
            ],
        )?;

        tx.commit()?;
        Ok(conv.turns.len() as u64)
    }

    /// Remove a conversation whose source file disappeared.
    pub fn remove_conversation_by_path(&mut self, source_path: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let cid: Option<String> = tx
            .query_row(
                "SELECT conversation_id FROM files WHERE path=?1",
                params![source_path],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(cid) = cid {
            for table in ["turns", "exchanges", "tool_events", "file_refs"] {
                tx.execute(&format!("DELETE FROM {table} WHERE conversation_id=?1"), params![cid])?;
            }
            tx.execute("DELETE FROM conversations WHERE id=?1", params![cid])?;
            tx.execute("DELETE FROM files WHERE path=?1", params![source_path])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn conversation_row(&self, id: &str) -> Result<Option<ConversationRow>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{} WHERE id=?1",
            CONVERSATION_ROW_SELECT
        ))?;
        let row = stmt.query_row(params![id], row_to_conversation).optional()?;
        Ok(row)
    }

    pub fn conversation_by_prefix(&self, prefix: &str) -> Result<Option<ConversationRow>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{} WHERE id LIKE ?1 LIMIT 2", CONVERSATION_ROW_SELECT))?;
        let mut rows: Vec<ConversationRow> = stmt
            .query_map(params![format!("{prefix}%")], row_to_conversation)?
            .collect::<std::result::Result<_, _>>()?;
        if rows.len() == 1 {
            Ok(Some(rows.remove(0)))
        } else {
            Ok(None)
        }
    }

    pub fn recent_conversations(&self, limit: u32, project: Option<&str>) -> Result<Vec<ConversationRow>> {
        let (sql, args): (String, Vec<Box<dyn rusqlite::ToSql>>) = match project {
            Some(p) => (
                format!(
                    "{CONVERSATION_ROW_SELECT} WHERE normalized_project=?1 ORDER BY last_activity_at DESC LIMIT ?2"
                ),
                vec![Box::new(p.to_string()), Box::new(limit)],
            ),
            None => (
                format!("{CONVERSATION_ROW_SELECT} ORDER BY last_activity_at DESC LIMIT ?1"),
                vec![Box::new(limit)],
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter().map(|a| a.as_ref())), row_to_conversation)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn exchanges_for(&self, conversation_id: &str) -> Result<Vec<ExchangeRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT ordinal, user_turn_index, start_turn, end_turn, started_at, ended_at,
                    duration_ms, user_preview, assistant_turns, tool_calls, failed_tool_calls,
                    rejected_tool_calls, has_error, interrupted, compacted,
                    input_tokens, output_tokens, cache_read_tokens, estimated_tokens
             FROM exchanges WHERE conversation_id=?1 ORDER BY ordinal",
        )?;
        let rows = stmt
            .query_map(params![conversation_id], |r| {
                Ok(ExchangeRow {
                    ordinal: r.get(0)?,
                    user_turn_index: r.get(1)?,
                    start_turn: r.get(2)?,
                    end_turn: r.get(3)?,
                    started_at: r.get(4)?,
                    ended_at: r.get(5)?,
                    duration_ms: r.get(6)?,
                    user_preview: r.get(7)?,
                    assistant_turns: r.get(8)?,
                    tool_calls: r.get(9)?,
                    failed_tool_calls: r.get(10)?,
                    rejected_tool_calls: r.get(11)?,
                    has_error: r.get(12)?,
                    interrupted: r.get(13)?,
                    compacted: r.get(14)?,
                    input_tokens: r.get::<_, i64>(15)? as u64,
                    output_tokens: r.get::<_, i64>(16)? as u64,
                    cache_read_tokens: r.get::<_, i64>(17)? as u64,
                    estimated_tokens: r.get::<_, i64>(18)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Turn source spans for excerpt fallback and `show` rendering.
    pub fn turn_spans(
        &self,
        conversation_id: &str,
        from: u32,
        limit: u32,
    ) -> Result<Vec<(u32, String, u64, u64, u64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.turn_index, f.path, t.source_byte_start, t.source_byte_end, t.source_line
             FROM turns t JOIN files f ON f.conversation_id = t.conversation_id
             WHERE t.conversation_id=?1 AND t.turn_index>=?2
             ORDER BY t.turn_index LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![conversation_id, from, limit], |r| {
                Ok((
                    r.get::<_, u32>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? as u64,
                    r.get::<_, i64>(3)? as u64,
                    r.get::<_, i64>(4)? as u64,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ExchangeRow {
    pub ordinal: u32,
    pub user_turn_index: Option<u32>,
    pub start_turn: u32,
    pub end_turn: u32,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub user_preview: String,
    pub assistant_turns: u32,
    pub tool_calls: u32,
    pub failed_tool_calls: u32,
    pub rejected_tool_calls: u32,
    pub has_error: bool,
    pub interrupted: bool,
    pub compacted: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub estimated_tokens: u64,
}

const CONVERSATION_ROW_SELECT: &str = "SELECT id, source_path, provider, normalized_project, cwd,
    model, title, origin, is_subagent, started_at, last_activity_at, turn_count, exchange_count,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, estimated_tokens
    FROM conversations";

fn row_to_conversation(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationRow> {
    Ok(ConversationRow {
        id: r.get(0)?,
        source_path: r.get(1)?,
        provider: r.get(2)?,
        normalized_project: r.get(3)?,
        cwd: r.get(4)?,
        model: r.get(5)?,
        title: r.get(6)?,
        origin: r.get(7)?,
        is_subagent: r.get(8)?,
        started_at: r.get(9)?,
        last_activity_at: r.get(10)?,
        turn_count: r.get(11)?,
        exchange_count: r.get(12)?,
        input_tokens: r.get::<_, i64>(13)? as u64,
        output_tokens: r.get::<_, i64>(14)? as u64,
        cache_creation_tokens: r.get::<_, i64>(15)? as u64,
        cache_read_tokens: r.get::<_, i64>(16)? as u64,
        estimated_tokens: r.get::<_, i64>(17)? as u64,
    })
}

fn special_kind(special: &Option<SpecialTurn>) -> Option<&'static str> {
    special.as_ref().map(|s| match s {
        SpecialTurn::TaskNotification { queued: true, .. } => "task_notification_queued",
        SpecialTurn::TaskNotification { .. } => "task_notification",
        SpecialTurn::ScheduledPrompt { queued: true, .. } => "scheduled_prompt_queued",
        SpecialTurn::ScheduledPrompt { .. } => "scheduled_prompt",
        SpecialTurn::CompactBoundary => "compact_boundary",
        SpecialTurn::Attachment { .. } => "attachment",
        SpecialTurn::TurnAborted { .. } => "turn_aborted",
        SpecialTurn::Other { .. } => "other",
    })
}

fn fallback_title(conv: &Conversation) -> String {
    conv.display_title()
}

