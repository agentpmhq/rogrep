use anyhow::Result;
use clap::Args;
use rogrep_index::{parse_query, ConversationMatches};
use rogrep_model::ids::{ConversationId, ExchangeRef};
use rogrep_store::Store;

#[derive(Args)]
pub struct SearchArgs {
    /// Query: bare terms, "quoted phrases", and key:value facets
    /// (tool:, tool_cmd:, tool_status:, git_pr_num:, file:, project:, …).
    pub query: Vec<String>,
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Restrict to a project (normalized key) or use --cwd.
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub cwd: Option<String>,
    /// Since date (YYYY-MM-DD or Nd).
    #[arg(long)]
    pub since: Option<String>,
    /// relevance (default) or recent.
    #[arg(long, default_value = "relevance", value_parser = ["relevance", "recent"])]
    pub sort: String,
    #[arg(long)]
    pub json: bool,
}

pub fn resolve_project_key(project: &Option<String>, cwd: &Option<String>) -> Option<String> {
    match (project, cwd) {
        (Some(p), _) => Some(p.clone()),
        (None, Some(c)) => {
            let abs = std::fs::canonicalize(c)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| c.clone());
            Some(rogrep_model::project::dash_project_from_cwd(&abs))
        }
        _ => None,
    }
}

pub fn run(args: SearchArgs) -> Result<()> {
    let (layout, _config, store) = super::sync::sync_now(false, true)?;
    let raw_query = args.query.join(" ");

    // Exact-id short-circuit.
    let trimmed = raw_query.trim();
    if ConversationId::looks_like_id(trimmed) || ExchangeRef::parse(trimmed).is_some() {
        return super::show::show_by_ref(&store, &layout, trimmed, args.json);
    }

    let mut parsed = parse_query(&raw_query);
    if let Some(key) = resolve_project_key(&args.project, &args.cwd) {
        parsed.facets.push(("project".into(), key));
    }
    let tz = jiff::tz::TimeZone::system();
    let since = super::stats::parse_since(&args.since, &tz)?;
    let ts_range = since.map(|s| (s, i64::MAX));

    if parsed.is_empty() {
        anyhow::bail!("empty query; try `rogrep ls` for recent conversations");
    }

    let index = super::index::open_search_index(&layout)?;
    let mut matches = index.search(&parsed, ts_range, 500)?;

    // Recency-decayed rerank.
    if args.sort == "relevance" {
        let now = jiff::Timestamp::now().as_millisecond();
        let half_life_days = 30.0f64;
        for m in &mut matches {
            if let Some(ts) = m.last_ts {
                let age_days = ((now - ts).max(0) as f64) / 86_400_000.0;
                m.best_score *= 2f64.powf(-age_days / half_life_days) as f32;
            }
        }
        matches.sort_by(|a, b| b.best_score.total_cmp(&a.best_score));
    } else {
        matches.sort_by_key(|m| std::cmp::Reverse(m.last_ts));
    }
    matches.truncate(args.limit);

    if args.json {
        let rows: Vec<serde_json::Value> = matches
            .iter()
            .map(|m| result_json(&store, m))
            .collect::<Result<_>>()?;
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "schema": "rogrep/v1",
                "command": "search",
                "query": {"raw": raw_query, "terms": parsed.terms, "phrases": parsed.phrases,
                          "facets": parsed.facets},
                "results": rows,
                "total": rows.len(),
            })
        );
        return Ok(());
    }

    if !parsed.facets.is_empty() || !parsed.phrases.is_empty() {
        eprintln!(
            "parsed: terms={:?} phrases={:?} facets={:?}",
            parsed.terms, parsed.phrases, parsed.facets
        );
    }
    if matches.is_empty() {
        println!("no matches; unindexed or differently-phrased work is not ruled out — try broader terms or `rogrep ls`");
        return Ok(());
    }
    for m in &matches {
        let row = store.conversation_row(&m.conversation_id)?;
        let (title, project, provider) = row
            .as_ref()
            .map(|r| {
                (
                    r.title.clone().unwrap_or_default(),
                    r.normalized_project.clone(),
                    r.provider.clone(),
                )
            })
            .unwrap_or_default();
        let when = m
            .last_ts
            .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
            .map(|ts| {
                ts.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "?".into());
        println!(
            "{}  [{}] {}  ({} hits, {})",
            m.conversation_id,
            provider,
            title.chars().take(64).collect::<String>(),
            m.match_count,
            when
        );
        if let Some(best) = &m.best {
            println!("    #{}: {}", best.turn_index, best.excerpt.chars().take(220).collect::<String>());
            println!(
                "    inspect: rogrep show {} --around {} ; project {}",
                m.conversation_id, best.turn_index, project
            );
        }
    }
    Ok(())
}

fn result_json(store: &Store, m: &ConversationMatches) -> Result<serde_json::Value> {
    let row = store.conversation_row(&m.conversation_id)?;
    Ok(serde_json::json!({
        "conversation": row,
        "match_count": m.match_count,
        "score": m.best_score,
        "last_ts": m.last_ts,
        "best": m.best,
        "inspect": m.best.as_ref().map(|b|
            format!("rogrep show {} --around {}", m.conversation_id, b.turn_index)),
        "next_turn": m.best.as_ref().map(|b| b.turn_index),
    }))
}
