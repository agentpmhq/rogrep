//! SQLite schema. Everything here is derived data: schema changes bump
//! SCHEMA_VERSION and trigger a wipe + re-derive, never a migration.

pub const SCHEMA_VERSION: u32 = 2;

pub const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations(
  id TEXT PRIMARY KEY,
  source_path TEXT NOT NULL UNIQUE,
  provider TEXT NOT NULL,
  project TEXT NOT NULL DEFAULT '',
  normalized_project TEXT NOT NULL DEFAULT 'home',
  cwd TEXT,
  model TEXT,
  title TEXT,
  origin TEXT NOT NULL DEFAULT 'interactive',
  is_subagent INTEGER NOT NULL DEFAULT 0,
  parent_conversation_id TEXT,
  subagent_id TEXT,
  started_at INTEGER,
  last_activity_at INTEGER,
  turn_count INTEGER NOT NULL DEFAULT 0,
  exchange_count INTEGER NOT NULL DEFAULT 0,
  malformed_lines INTEGER NOT NULL DEFAULT 0,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  estimated_tokens INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS conv_activity ON conversations(last_activity_at DESC);
CREATE INDEX IF NOT EXISTS conv_project ON conversations(normalized_project, last_activity_at DESC);

CREATE TABLE IF NOT EXISTS files(
  path TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  size INTEGER NOT NULL,
  mtime_ns INTEGER NOT NULL,
  parse_state TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS turns(
  conversation_id TEXT NOT NULL,
  turn_index INTEGER NOT NULL,
  exchange_ordinal INTEGER NOT NULL DEFAULT 0,
  role TEXT NOT NULL,
  speaker TEXT NOT NULL DEFAULT '',
  ts INTEGER,
  model TEXT,
  cwd TEXT,
  text_len INTEGER NOT NULL DEFAULT 0,
  visible INTEGER NOT NULL DEFAULT 1,
  tool_name TEXT,
  tool_direction TEXT,
  tool_status TEXT,
  special_kind TEXT,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  estimated_tokens INTEGER NOT NULL DEFAULT 0,
  source_line INTEGER NOT NULL DEFAULT 0,
  source_byte_start INTEGER NOT NULL DEFAULT 0,
  source_byte_end INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(conversation_id, turn_index)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS turns_ts ON turns(ts);

CREATE TABLE IF NOT EXISTS exchanges(
  conversation_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  user_turn_index INTEGER,
  start_turn INTEGER NOT NULL,
  end_turn INTEGER NOT NULL,
  started_at INTEGER,
  ended_at INTEGER,
  duration_ms INTEGER,
  user_preview TEXT NOT NULL DEFAULT '',
  assistant_turns INTEGER NOT NULL DEFAULT 0,
  tool_calls INTEGER NOT NULL DEFAULT 0,
  failed_tool_calls INTEGER NOT NULL DEFAULT 0,
  rejected_tool_calls INTEGER NOT NULL DEFAULT 0,
  has_error INTEGER NOT NULL DEFAULT 0,
  interrupted INTEGER NOT NULL DEFAULT 0,
  compacted INTEGER NOT NULL DEFAULT 0,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  estimated_tokens INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(conversation_id, ordinal)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS exchanges_started ON exchanges(started_at);

CREATE TABLE IF NOT EXISTS tool_events(
  conversation_id TEXT NOT NULL,
  turn_index INTEGER NOT NULL,
  seq INTEGER NOT NULL DEFAULT 0,
  exchange_ordinal INTEGER NOT NULL DEFAULT 0,
  tool TEXT NOT NULL,
  mutating INTEGER,
  status TEXT NOT NULL DEFAULT 'unknown',
  cmd_head TEXT,
  git_facets TEXT,
  ts INTEGER,
  PRIMARY KEY(conversation_id, turn_index, seq)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS tool_events_tool ON tool_events(tool, ts);

CREATE TABLE IF NOT EXISTS file_refs(
  conversation_id TEXT NOT NULL,
  turn_index INTEGER NOT NULL,
  path TEXT NOT NULL,
  mode TEXT NOT NULL DEFAULT 'read',
  PRIMARY KEY(conversation_id, turn_index, path)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS file_refs_path ON file_refs(path);

CREATE TABLE IF NOT EXISTS spool_state(
  db_path TEXT PRIMARY KEY,
  watermark TEXT NOT NULL
);
"#;
