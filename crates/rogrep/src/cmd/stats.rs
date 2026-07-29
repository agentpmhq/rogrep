use anyhow::Result;
use clap::{Args, Subcommand};
use jiff::tz::TimeZone;
use rogrep_store::stats::{self, Period};

#[derive(Args)]
pub struct StatsArgs {
    #[command(subcommand)]
    pub view: Option<StatsView>,
    #[arg(long, global = true)]
    pub json: bool,
    /// IANA timezone for bucketing (default: system local).
    #[arg(long, global = true)]
    pub timezone: Option<String>,
    /// Only include activity since (YYYY-MM-DD or Nd, e.g. 7d).
    #[arg(long, global = true)]
    pub since: Option<String>,
}

#[derive(Subcommand)]
pub enum StatsView {
    /// Usage per day.
    Daily,
    /// Usage per ISO week.
    Weekly,
    /// Usage per month.
    Monthly,
    /// Hour-of-week activity heatmap.
    Heatmap,
    /// Exchange leaderboard.
    Top {
        #[arg(long, default_value = "tokens", value_parser = ["tokens", "duration", "turns", "tools"])]
        by: String,
        #[arg(long, default_value_t = 10)]
        limit: u32,
    },
    /// Per-project activity.
    Projects,
}

pub fn resolve_tz(spec: &Option<String>) -> Result<TimeZone> {
    match spec {
        Some(name) => Ok(TimeZone::get(name)?),
        None => Ok(TimeZone::system()),
    }
}

pub fn parse_since(spec: &Option<String>, tz: &TimeZone) -> Result<Option<i64>> {
    let Some(spec) = spec else { return Ok(None) };
    let spec = spec.trim();
    if let Some(days) = spec.strip_suffix('d').and_then(|d| d.parse::<i64>().ok()) {
        let now = jiff::Timestamp::now();
        return Ok(Some(now.as_millisecond() - days * 86_400_000));
    }
    if let Ok(date) = spec.parse::<jiff::civil::Date>() {
        let zoned = date.to_zoned(tz.clone())?;
        return Ok(Some(zoned.timestamp().as_millisecond()));
    }
    anyhow::bail!("cannot parse --since {spec}; use YYYY-MM-DD or Nd")
}

fn fmt_tokens(n: u64) -> String {
    if n >= 10_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{:.0}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn run(args: StatsArgs) -> Result<()> {
    let (_layout, _config, store) = super::sync::sync_now(false, true)?;
    let tz = resolve_tz(&args.timezone)?;
    let since = parse_since(&args.since, &tz)?;

    match args.view.as_ref().unwrap_or(&StatsView::Daily) {
        StatsView::Daily | StatsView::Weekly | StatsView::Monthly => {
            let period = match args.view.as_ref() {
                Some(StatsView::Weekly) => Period::Weekly,
                Some(StatsView::Monthly) => Period::Monthly,
                _ => Period::Daily,
            };
            let usage = stats::usage_report(&store, period, &tz, since, None)?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&usage)?);
                return Ok(());
            }
            println!(
                "{:<12} {:>6} {:>6} {:>8} {:>8} {:>9} {:>9} {:>9}",
                "PERIOD", "CONVS", "EXCH", "TURNS", "IN", "OUT", "CACHE-RD", "TOTAL"
            );
            let mut totals = stats::UsageBucket::default();
            for (label, b) in &usage {
                println!(
                    "{:<12} {:>6} {:>6} {:>8} {:>8} {:>9} {:>9} {:>9}",
                    label,
                    b.conversations,
                    b.exchanges,
                    b.turns,
                    fmt_tokens(b.input_tokens),
                    fmt_tokens(b.output_tokens),
                    fmt_tokens(b.cache_read_tokens),
                    fmt_tokens(
                        b.input_tokens + b.output_tokens + b.cache_creation_tokens + b.cache_read_tokens
                    ),
                );
                totals.conversations += b.conversations;
                totals.exchanges += b.exchanges;
                totals.turns += b.turns;
                totals.input_tokens += b.input_tokens;
                totals.output_tokens += b.output_tokens;
                totals.cache_read_tokens += b.cache_read_tokens;
                totals.cache_creation_tokens += b.cache_creation_tokens;
            }
            println!(
                "{:<12} {:>6} {:>6} {:>8} {:>8} {:>9} {:>9} {:>9}",
                "TOTAL",
                totals.conversations,
                totals.exchanges,
                totals.turns,
                fmt_tokens(totals.input_tokens),
                fmt_tokens(totals.output_tokens),
                fmt_tokens(totals.cache_read_tokens),
                fmt_tokens(
                    totals.input_tokens
                        + totals.output_tokens
                        + totals.cache_creation_tokens
                        + totals.cache_read_tokens
                ),
            );
        }
        StatsView::Heatmap => {
            let grid = stats::heatmap(&store, &tz, since)?;
            if args.json {
                println!("{}", serde_json::to_string(&grid)?);
                return Ok(());
            }
            let max = grid.iter().flatten().copied().max().unwrap_or(0).max(1);
            let shades = [' ', '·', '▂', '▄', '▆', '█'];
            println!("        00    04    08    12    16    20");
            for (d, name) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
                let mut row = String::new();
                for h in 0..24 {
                    let v = grid[d][h];
                    let idx = if v == 0 {
                        0
                    } else {
                        1 + ((v * (shades.len() as u64 - 2)) / max) as usize
                    };
                    row.push(shades[idx.min(shades.len() - 1)]);
                }
                println!("{name}  |{row}|");
            }
            println!("(turn activity by local hour, shaded by volume)");
        }
        StatsView::Top { by, limit } => {
            let by = match by.as_str() {
                "duration" => stats::TopBy::Duration,
                "turns" => stats::TopBy::Turns,
                "tools" => stats::TopBy::ToolCalls,
                _ => stats::TopBy::Tokens,
            };
            let rows = stats::top_exchanges(&store, by, *limit, since)?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
                return Ok(());
            }
            println!(
                "{:<32} {:>8} {:>6} {:>6} {:>9}  PROMPT",
                "EXCHANGE", "DUR", "TURNS", "TOOLS", "TOKENS"
            );
            for r in rows {
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
                    "{:<32} {:>8} {:>6} {:>6} {:>9}  {}{}",
                    format!("{}#e{}", r.conversation_id, r.ordinal + 1),
                    dur,
                    r.turns,
                    r.tool_calls,
                    fmt_tokens(r.total_tokens),
                    flags,
                    r.user_preview.chars().take(48).collect::<String>()
                );
            }
            println!("inspect: rogrep show <EXCHANGE>");
        }
        StatsView::Projects => {
            let rows = stats::projects(&store)?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
                return Ok(());
            }
            println!(
                "{:<44} {:>6} {:>6} {:>8} {:>9}  LAST ACTIVE",
                "PROJECT", "CONVS", "EXCH", "TURNS", "OUT-TOK"
            );
            for r in rows {
                let last = r
                    .last_activity_at
                    .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
                    .map(|ts| ts.to_zoned(tz.clone()).strftime("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "?".into());
                println!(
                    "{:<44} {:>6} {:>6} {:>8} {:>9}  {}",
                    r.normalized_project.chars().take(44).collect::<String>(),
                    r.conversations,
                    r.exchanges,
                    r.turns,
                    fmt_tokens(r.output_tokens),
                    last
                );
            }
        }
    }
    Ok(())
}
