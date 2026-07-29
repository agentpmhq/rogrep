//! Exchange-granularity search/listing: `rogrep x`.

use anyhow::Result;
use clap::Args;
use rogrep_index::parse_query;
use rusqlite::ToSql;

#[derive(Args)]
pub struct ExchangesArgs {
    /// Optional text/facet query; exchanges must contain a matching turn.
    pub query: Vec<String>,
    /// Only exchanges with failed tool calls.
    #[arg(long)]
    pub failed: bool,
    /// Only interrupted exchanges.
    #[arg(long)]
    pub interrupted: bool,
    /// Minimum wall duration, e.g. 5m, 90s.
    #[arg(long)]
    pub min_duration: Option<String>,
    /// Minimum turn count.
    #[arg(long)]
    pub min_turns: Option<u32>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub cwd: Option<String>,
    /// Since date (YYYY-MM-DD or Nd).
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
}

fn parse_duration_ms(s: &str) -> Result<i64> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('h') {
        return Ok((num.parse::<f64>()? * 3_600_000.0) as i64);
    }
    if let Some(num) = s.strip_suffix('m') {
        return Ok((num.parse::<f64>()? * 60_000.0) as i64);
    }
    let num = s.strip_suffix('s').unwrap_or(s);
    Ok((num.parse::<f64>()? * 1000.0) as i64)
}

pub fn run(args: ExchangesArgs) -> Result<()> {
    let (layout, _config, store) = super::sync::sync_now(false, true)?;
    let tz = jiff::tz::TimeZone::system();
    let since = super::stats::parse_since(&args.since, &tz)?;
    let project = super::search::resolve_project_key(&args.project, &args.cwd);

    // Text/facet query narrows to exchanges containing matching turns.
    let raw_query = args.query.join(" ");
    let allowed: Option<std::collections::HashSet<(String, u32)>> = if raw_query.trim().is_empty() {
        None
    } else {
        let mut parsed = parse_query(&raw_query);
        if let Some(p) = &project {
            parsed.facets.push(("project".into(), p.clone()));
        }
        let index = super::index::open_search_index(&layout)?;
        let matches = index.search(&parsed, since.map(|s| (s, i64::MAX)), 2000)?;
        let mut set = std::collections::HashSet::new();
        for m in matches {
            if let Some(best) = &m.best {
                set.insert((m.conversation_id.clone(), best.exchange_ordinal));
            }
        }
        Some(set)
    };

    let mut sql = String::from(
        "SELECT e.conversation_id, e.ordinal, e.started_at, e.duration_ms,
                e.end_turn - e.start_turn, e.tool_calls, e.failed_tool_calls,
                e.interrupted, e.user_preview,
                e.input_tokens + e.output_tokens + e.cache_creation_tokens +
                  e.cache_read_tokens + e.estimated_tokens,
                c.provider, c.normalized_project
         FROM exchanges e JOIN conversations c ON c.id = e.conversation_id
         WHERE e.user_turn_index IS NOT NULL",
    );
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();
    if args.failed {
        sql.push_str(" AND e.failed_tool_calls > 0");
    }
    if args.interrupted {
        sql.push_str(" AND e.interrupted = 1");
    }
    if let Some(d) = &args.min_duration {
        sql.push_str(" AND e.duration_ms >= ?");
        params.push(Box::new(parse_duration_ms(d)?));
    }
    if let Some(t) = args.min_turns {
        sql.push_str(" AND e.end_turn - e.start_turn >= ?");
        params.push(Box::new(t));
    }
    if let Some(p) = &project {
        sql.push_str(" AND c.normalized_project = ?");
        params.push(Box::new(p.clone()));
    }
    if let Some(s) = since {
        sql.push_str(" AND e.started_at >= ?");
        params.push(Box::new(s));
    }
    sql.push_str(" ORDER BY e.started_at DESC LIMIT 5000");

    #[derive(serde::Serialize)]
    struct Row {
        conversation_id: String,
        ordinal: u32,
        started_at: Option<i64>,
        duration_ms: Option<i64>,
        turns: u32,
        tool_calls: u32,
        failed_tool_calls: u32,
        interrupted: bool,
        user_preview: String,
        total_tokens: u64,
        provider: String,
        project: String,
        inspect: String,
    }

    let conn = store.raw();
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<Row> = stmt
        .query_map(rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())), |r| {
            let conversation_id: String = r.get(0)?;
            let ordinal: u32 = r.get(1)?;
            Ok(Row {
                inspect: format!("rogrep show {conversation_id}#e{}", ordinal + 1),
                conversation_id,
                ordinal,
                started_at: r.get(2)?,
                duration_ms: r.get(3)?,
                turns: r.get(4)?,
                tool_calls: r.get(5)?,
                failed_tool_calls: r.get(6)?,
                interrupted: r.get(7)?,
                user_preview: r.get(8)?,
                total_tokens: r.get::<_, i64>(9)? as u64,
                provider: r.get(10)?,
                project: r.get(11)?,
            })
        })?
        .filter_map(|r| r.ok())
        .filter(|r| {
            allowed
                .as_ref()
                .map(|set| set.contains(&(r.conversation_id.clone(), r.ordinal)))
                .unwrap_or(true)
        })
        .take(args.limit)
        .collect();

    if args.json {
        println!(
            "{}",
            serde_json::json!({"ok": true, "schema": "rogrep/v1", "command": "x", "results": rows, "total": rows.len()})
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("no exchanges matched; unindexed or differently-phrased work is not ruled out.");
        return Ok(());
    }
    println!(
        "{:<36} {:>8} {:>5} {:>6} {:>9}  PROMPT",
        "EXCHANGE", "DUR", "TURNS", "TOOLS", "TOKENS"
    );
    for r in &rows {
        let dur = r
            .duration_ms
            .map(|ms| format!("{:.0}s", ms as f64 / 1000.0))
            .unwrap_or_else(|| "?".into());
        let mut flags = String::new();
        if r.failed_tool_calls > 0 {
            flags.push('✗');
        }
        if r.interrupted {
            flags.push('⏻');
        }
        println!(
            "{:<36} {:>8} {:>5} {:>6} {:>9}  {}{}",
            format!("{}#e{}", r.conversation_id, r.ordinal + 1),
            dur,
            r.turns,
            r.tool_calls,
            r.total_tokens,
            flags,
            r.user_preview.chars().take(56).collect::<String>()
        );
    }
    println!("inspect: rogrep show <EXCHANGE>");
    Ok(())
}
