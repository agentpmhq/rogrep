//! Deterministic stats queries.
//!
//! All aggregation happens at UTC-hour resolution in SQL; hour buckets are
//! then mapped to local days with jiff in Rust, so day boundaries are
//! DST-correct without trusting SQLite's `localtime`.

use crate::store::Store;
use anyhow::Result;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use rusqlite::params;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize)]
pub struct UsageBucket {
    pub conversations: u64,
    pub exchanges: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub estimated_tokens: u64,
    pub turns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Daily,
    Weekly,
    Monthly,
}

/// Hour-resolution usage rows straight from SQL.
struct HourRow {
    hour_epoch: i64, // unix hours
    turns: u64,
    conversations: u64,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    estimated: u64,
}

fn hourly_usage(store: &Store, since_ms: Option<i64>, until_ms: Option<i64>) -> Result<Vec<HourRow>> {
    let mut sql = String::from(
        "SELECT ts/3600000 AS h, COUNT(*), COUNT(DISTINCT conversation_id),
                SUM(input_tokens), SUM(output_tokens), SUM(cache_creation_tokens),
                SUM(cache_read_tokens), SUM(estimated_tokens)
         FROM turns WHERE ts IS NOT NULL",
    );
    let mut args: Vec<i64> = Vec::new();
    if let Some(s) = since_ms {
        sql.push_str(" AND ts >= ?");
        args.push(s);
    }
    if let Some(u) = until_ms {
        sql.push_str(" AND ts < ?");
        args.push(u);
    }
    sql.push_str(" GROUP BY h ORDER BY h");
    let mut stmt = store.conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(args), |r| {
            Ok(HourRow {
                hour_epoch: r.get(0)?,
                turns: r.get::<_, i64>(1)? as u64,
                conversations: r.get::<_, i64>(2)? as u64,
                input: r.get::<_, Option<i64>>(3)?.unwrap_or(0) as u64,
                output: r.get::<_, Option<i64>>(4)?.unwrap_or(0) as u64,
                cache_creation: r.get::<_, Option<i64>>(5)?.unwrap_or(0) as u64,
                cache_read: r.get::<_, Option<i64>>(6)?.unwrap_or(0) as u64,
                estimated: r.get::<_, Option<i64>>(7)?.unwrap_or(0) as u64,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn bucket_label(period: Period, hour_epoch: i64, tz: &TimeZone) -> String {
    let ts = Timestamp::from_second(hour_epoch * 3600).unwrap_or_default();
    let zoned = ts.to_zoned(tz.clone());
    match period {
        Period::Daily => zoned.strftime("%Y-%m-%d").to_string(),
        Period::Weekly => {
            // ISO week label.
            zoned.strftime("%G-W%V").to_string()
        }
        Period::Monthly => zoned.strftime("%Y-%m").to_string(),
    }
}

/// Usage grouped by period. Exchange counts join exchanges by started_at.
pub fn usage_report(
    store: &Store,
    period: Period,
    tz: &TimeZone,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
) -> Result<BTreeMap<String, UsageBucket>> {
    let mut out: BTreeMap<String, UsageBucket> = BTreeMap::new();
    for row in hourly_usage(store, since_ms, until_ms)? {
        let label = bucket_label(period, row.hour_epoch, tz);
        let b = out.entry(label).or_default();
        b.turns += row.turns;
        // conversations per hour over-counts across hours; recomputed below.
        b.input_tokens += row.input;
        b.output_tokens += row.output;
        b.cache_creation_tokens += row.cache_creation;
        b.cache_read_tokens += row.cache_read;
        b.estimated_tokens += row.estimated;
        let _ = row.conversations;
    }
    // Exact distinct conversation/exchange counts per bucket.
    {
        let mut stmt = store.conn.prepare(
            "SELECT ts/3600000 AS h, conversation_id FROM turns
             WHERE ts IS NOT NULL GROUP BY h, conversation_id",
        )?;
        let mut per_bucket: BTreeMap<String, std::collections::HashSet<String>> = BTreeMap::new();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (h, cid) = row?;
            let ms = h * 3_600_000;
            if since_ms.is_some_and(|s| ms < s) || until_ms.is_some_and(|u| ms >= u) {
                continue;
            }
            per_bucket.entry(bucket_label(period, h, tz)).or_default().insert(cid);
        }
        for (label, set) in per_bucket {
            if let Some(b) = out.get_mut(&label) {
                b.conversations = set.len() as u64;
            }
        }
    }
    {
        let mut stmt = store.conn.prepare(
            "SELECT started_at FROM exchanges WHERE started_at IS NOT NULL AND user_turn_index IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        for ts in rows {
            let ts = ts?;
            if since_ms.is_some_and(|s| ts < s) || until_ms.is_some_and(|u| ts >= u) {
                continue;
            }
            let label = bucket_label(period, ts / 3_600_000, tz);
            if let Some(b) = out.get_mut(&label) {
                b.exchanges += 1;
            }
        }
    }
    Ok(out)
}

/// Hour-of-week activity heatmap: [weekday 0=Mon..6][hour 0..23] = turns.
pub fn heatmap(store: &Store, tz: &TimeZone, since_ms: Option<i64>) -> Result<[[u64; 24]; 7]> {
    let mut grid = [[0u64; 24]; 7];
    for row in hourly_usage(store, since_ms, None)? {
        let ts = Timestamp::from_second(row.hour_epoch * 3600).unwrap_or_default();
        let zoned = ts.to_zoned(tz.clone());
        let weekday = zoned.weekday().to_monday_zero_offset() as usize;
        let hour = zoned.hour() as usize;
        grid[weekday][hour] += row.turns;
    }
    Ok(grid)
}

/// Per-project rollup.
#[derive(Clone, Debug, Serialize)]
pub struct ProjectRow {
    pub normalized_project: String,
    pub conversations: u64,
    pub exchanges: u64,
    pub turns: u64,
    pub last_activity_at: Option<i64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
}

pub fn projects(store: &Store) -> Result<Vec<ProjectRow>> {
    let mut stmt = store.conn.prepare(
        "SELECT normalized_project, COUNT(*), SUM(exchange_count), SUM(turn_count),
                MAX(last_activity_at), SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens)
         FROM conversations GROUP BY normalized_project ORDER BY MAX(last_activity_at) DESC",
    )?;
    let rows = stmt
        .query_map(params![], |r| {
            Ok(ProjectRow {
                normalized_project: r.get(0)?,
                conversations: r.get::<_, i64>(1)? as u64,
                exchanges: r.get::<_, Option<i64>>(2)?.unwrap_or(0) as u64,
                turns: r.get::<_, Option<i64>>(3)?.unwrap_or(0) as u64,
                last_activity_at: r.get(4)?,
                input_tokens: r.get::<_, Option<i64>>(5)?.unwrap_or(0) as u64,
                output_tokens: r.get::<_, Option<i64>>(6)?.unwrap_or(0) as u64,
                cache_read_tokens: r.get::<_, Option<i64>>(7)?.unwrap_or(0) as u64,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Exchange leaderboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopBy {
    Tokens,
    Duration,
    Turns,
    ToolCalls,
}

#[derive(Clone, Debug, Serialize)]
pub struct TopExchange {
    pub conversation_id: String,
    pub ordinal: u32,
    pub started_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub turns: u32,
    pub tool_calls: u32,
    pub failed_tool_calls: u32,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub user_preview: String,
    pub interrupted: bool,
}

pub fn top_exchanges(
    store: &Store,
    by: TopBy,
    limit: u32,
    since_ms: Option<i64>,
) -> Result<Vec<TopExchange>> {
    let order = match by {
        TopBy::Tokens => "input_tokens+output_tokens+cache_creation_tokens+cache_read_tokens+estimated_tokens DESC",
        TopBy::Duration => "duration_ms DESC",
        TopBy::Turns => "(end_turn-start_turn) DESC",
        TopBy::ToolCalls => "tool_calls DESC",
    };
    let mut sql = format!(
        "SELECT conversation_id, ordinal, started_at, duration_ms, end_turn-start_turn,
                tool_calls, failed_tool_calls, output_tokens,
                input_tokens+output_tokens+cache_creation_tokens+cache_read_tokens+estimated_tokens,
                user_preview, interrupted
         FROM exchanges WHERE user_turn_index IS NOT NULL"
    );
    if since_ms.is_some() {
        sql.push_str(" AND started_at >= ?1");
    }
    sql.push_str(&format!(" ORDER BY {order} LIMIT {limit}"));
    let mut stmt = store.conn.prepare(&sql)?;
    let map = |r: &rusqlite::Row<'_>| {
        Ok(TopExchange {
            conversation_id: r.get(0)?,
            ordinal: r.get(1)?,
            started_at: r.get(2)?,
            duration_ms: r.get(3)?,
            turns: r.get(4)?,
            tool_calls: r.get(5)?,
            failed_tool_calls: r.get(6)?,
            output_tokens: r.get::<_, i64>(7)? as u64,
            total_tokens: r.get::<_, i64>(8)? as u64,
            user_preview: r.get(9)?,
            interrupted: r.get(10)?,
        })
    };
    let rows = match since_ms {
        Some(s) => stmt.query_map(params![s], map)?.collect::<std::result::Result<Vec<_>, _>>()?,
        None => stmt.query_map([], map)?.collect::<std::result::Result<Vec<_>, _>>()?,
    };
    Ok(rows)
}
