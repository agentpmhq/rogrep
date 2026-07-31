use anyhow::Result;
use clap::Args;
use rogrep_index::parse_query;

#[derive(Args)]
pub struct FindArgs {
    /// Conversation id (rg_… or unique prefix).
    pub id: String,
    /// Query terms (all must match — conjunction).
    pub query: Vec<String>,
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: FindArgs) -> Result<()> {
    let (layout, _config, store) = super::sync::sync_now(false, true)?;
    let row = store
        .conversation_row(&args.id)?
        .or(store.conversation_by_prefix(&args.id)?)
        .ok_or_else(|| anyhow::anyhow!("conversation {} not found", args.id))?;
    let raw_query = args.query.join(" ");
    let parsed = parse_query(&raw_query);
    if parsed.is_empty() {
        anyhow::bail!("empty query");
    }
    let tz = jiff::tz::TimeZone::system();
    let ts_range = super::dates::resolve_dates(&parsed.dates, None, &tz)?;
    let index = super::index::open_search_index(&layout)?;
    let result = index.find(&row.id, &parsed, ts_range, args.limit)?;

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "schema": "rogrep/v1",
                "command": "find",
                "conversation_id": row.id,
                "query": {"raw": raw_query, "terms": parsed.terms, "phrases": parsed.phrases,
                          "facets": parsed.facets, "regexes": parsed.regexes},
                "result": result,
            })
        );
        return Ok(());
    }

    if !result.turn_hits.is_empty() {
        println!(
            "{} turn hits (showing {}):",
            result.total_turn_hits,
            result.turn_hits.len()
        );
        for h in &result.turn_hits {
            println!("  [{:>4}] {}", h.turn_index, h.excerpt.chars().take(200).collect::<String>());
        }
        if let Some(first) = result.turn_hits.first() {
            println!(
                "inspect: rogrep show {} --around {} --limit 20",
                row.id, first.turn_index
            );
        }
    } else if !result.passage_exchanges.is_empty() {
        println!(
            "no single turn matches all terms; {} exchange(s) contain every term:",
            result.passage_exchanges.len()
        );
        for e in &result.passage_exchanges {
            println!("  inspect: rogrep show {}#e{}", row.id, e + 1);
        }
    } else {
        println!("no turn or exchange matches all terms together.");
    }
    if result.term_counts.len() > 1 || result.turn_hits.is_empty() {
        println!("per-term hits (which term narrowed the query):");
        for (term, count) in &result.term_counts {
            println!("  {term:<24} {count}");
        }
    }
    Ok(())
}
