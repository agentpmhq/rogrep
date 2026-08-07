//! "How did we get here?" — rank conversations by real git evidence for a
//! PR, branch, or commit. Port of agentpm's trajectory design:
//! facet-priority queries → per-candidate git timelines → evidence-tier
//! rerank (origin > matching op > any git > text), dropping text-only noise
//! when real touchpoints exist.

use anyhow::Result;
use clap::Args;
use rogrep_index::ParsedQuery;
use rogrep_tooltree::git_ops_for_conversation;
use serde::Serialize;

#[derive(Args)]
pub struct TrajectoryArgs {
    /// Free-text fallback query.
    pub query: Vec<String>,
    #[arg(long)]
    pub pr: Option<u64>,
    #[arg(long)]
    pub branch: Option<String>,
    #[arg(long, visible_alias = "sha")]
    pub commit: Option<String>,
    /// Restrict to a project key or --cwd.
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub cwd: Option<String>,
    #[arg(long, default_value_t = 8)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize)]
enum Relevance {
    TextOnly = 0,
    AnyGit = 1,
    MatchingOp = 2,
    Origin = 3,
}

#[derive(Serialize)]
struct Entry {
    conversation_id: String,
    title: String,
    relevance: Relevance,
    origin: bool,
    first_seen: Option<i64>,
    matching_ops: usize,
    next_turn: Option<u32>,
    inspect: String,
    summary: String,
}

fn facet_query(pairs: &[(&str, String)], project: &Option<String>) -> ParsedQuery {
    let mut q = ParsedQuery {
        facets: pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        ..Default::default()
    };
    if let Some(p) = project {
        q.facets.push(("project".into(), p.clone()));
    }
    q
}

fn text_query(text: &str, project: &Option<String>) -> ParsedQuery {
    let mut q = rogrep_index::parse_query(text);
    if let Some(p) = project {
        q.facets.push(("project".into(), p.clone()));
    }
    q
}

pub fn run(args: TrajectoryArgs) -> Result<()> {
    let (layout, _config, store) = super::sync::sync_now(false, true)?;
    let index = super::index::open_search_index(&layout)?;
    let project = super::search::resolve_project_key(&args.project, &args.cwd);

    // Priority-ordered queries; earlier queries seed higher-confidence
    // candidates.
    let mut queries: Vec<ParsedQuery> = Vec::new();
    if let Some(n) = args.pr {
        queries.push(facet_query(&[("git_pr", format!("create-num:{n}"))], &project));
        queries.push(facet_query(&[("git_pr_num", n.to_string())], &project));
        queries.push(text_query(&format!("\"pull/{n}\""), &project));
        queries.push(text_query(&format!("\"PR {n}\""), &project));
    }
    if let Some(b) = &args.branch {
        queries.push(facet_query(&[("git_branch", b.clone())], &project));
        queries.push(text_query(&format!("\"{b}\""), &project));
    }
    if let Some(c) = &args.commit {
        let sha7 = rogrep_tooltree::git::short_sha(c);
        queries.push(facet_query(&[("git_commit", sha7.clone())], &project));
        queries.push(text_query(&sha7, &project));
    }
    let free_text = args.query.join(" ");
    if !free_text.trim().is_empty() {
        // Auto-detect sha- and branch-shaped queries.
        let t = free_text.trim();
        if t.len() >= 7 && t.len() <= 40 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            queries.push(facet_query(&[("git_commit", rogrep_tooltree::git::short_sha(t))], &project));
        } else if t.contains('/') && !t.contains(' ') {
            queries.push(facet_query(&[("git_branch", t.to_string())], &project));
        }
        queries.push(text_query(&free_text, &project));
    }
    if queries.is_empty() {
        anyhow::bail!("give --pr N, --branch NAME, --commit SHA, or a text query");
    }

    // Date facets in the free-text query bound every candidate search.
    let tz = jiff::tz::TimeZone::system();
    let all_dates: Vec<(String, String)> =
        queries.iter().flat_map(|q| q.dates.iter().cloned()).collect();
    let ts_range = super::dates::resolve_dates(&all_dates, None, &tz)?;

    // Collect candidates in priority order.
    let mut seen = std::collections::HashSet::new();
    let mut candidates: Vec<(String, u32)> = Vec::new(); // (cid, best turn)
    for q in &queries {
        for m in index.search(q, ts_range, 200)?.0 {
            if seen.insert(m.conversation_id.clone()) {
                let turn = m.best.as_ref().map(|b| b.turn_index).unwrap_or(0);
                candidates.push((m.conversation_id, turn));
            }
        }
        if candidates.len() >= 40 {
            break;
        }
    }

    // Evidence pass: load each candidate's git timeline and score.
    let mut entries: Vec<Entry> = Vec::new();
    for (cid, text_turn) in candidates.into_iter().take(40) {
        let Ok((row, conv)) = super::show::load_conversation(&store, &cid) else {
            continue;
        };
        let ops = git_ops_for_conversation(&conv);
        let matching: Vec<&rogrep_tooltree::GitOp> = ops
            .iter()
            .filter(|op| {
                args.pr.map(|n| op.touches_pr(n)).unwrap_or(false)
                    || args
                        .branch
                        .as_ref()
                        .map(|b| op.touches_branch(b))
                        .unwrap_or(false)
                    || args
                        .commit
                        .as_ref()
                        .map(|c| op.touches_commit(&rogrep_tooltree::git::short_sha(c)))
                        .unwrap_or(false)
            })
            .collect();
        let origin = args
            .pr
            .map(|n| ops.iter().any(|op| op.created_pr == Some(n)))
            .unwrap_or(false);
        let relevance = if origin {
            Relevance::Origin
        } else if !matching.is_empty() {
            Relevance::MatchingOp
        } else if !ops.is_empty() {
            Relevance::AnyGit
        } else {
            Relevance::TextOnly
        };
        let next_turn = matching
            .first()
            .map(|op| op.turn_index)
            .or(Some(text_turn));
        let summary = matching
            .first()
            .map(|op| op.command.clone())
            .unwrap_or_default();
        entries.push(Entry {
            inspect: format!(
                "rogrep show {} --around {} --limit 40",
                row.id,
                next_turn.unwrap_or(0)
            ),
            conversation_id: row.id.clone(),
            title: row.title.clone().unwrap_or_default(),
            relevance,
            origin,
            first_seen: row.started_at,
            matching_ops: matching.len(),
            next_turn,
            summary,
        });
    }

    // Drop text-only noise when real touchpoints exist; order by relevance
    // then chronology (trajectories read forward in time).
    let has_real = entries.iter().any(|e| e.relevance >= Relevance::MatchingOp);
    if has_real {
        entries.retain(|e| e.relevance >= Relevance::MatchingOp);
    }
    entries.sort_by(|a, b| {
        b.relevance
            .cmp(&a.relevance)
            .then(a.first_seen.cmp(&b.first_seen))
    });
    entries.truncate(args.limit);

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "schema": "rogrep/v1",
                "command": "trajectory",
                "entries": entries,
            })
        );
        return Ok(());
    }
    if entries.is_empty() {
        println!("no conversations with matching evidence; unindexed or out-of-band work is not ruled out.");
        return Ok(());
    }
    for e in &entries {
        let when = e
            .first_seen
            .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
            .map(|ts| {
                ts.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%Y-%m-%d")
                    .to_string()
            })
            .unwrap_or_else(|| "?".into());
        let marker = if e.origin { "[origin] " } else { "" };
        println!(
            "{} {}{}  ({:?}, {} matching ops)",
            when,
            marker,
            e.title.chars().take(70).collect::<String>(),
            e.relevance,
            e.matching_ops
        );
        if !e.summary.is_empty() {
            println!("    {}", e.summary.chars().take(100).collect::<String>());
        }
        println!("    inspect: {}", e.inspect);
    }
    Ok(())
}
