use anyhow::Result;
use clap::Args;

#[derive(Args)]
pub struct LsArgs {
    /// Filter by normalized project key or cwd path.
    #[arg(long)]
    pub project: Option<String>,
    /// Filter by cwd (converted to the project key).
    #[arg(long)]
    pub cwd: Option<String>,
    #[arg(long, default_value_t = 20)]
    pub limit: u32,
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: LsArgs) -> Result<()> {
    let (_layout, _config, store) = super::sync::sync_now(false, true)?;
    let project = match (&args.project, &args.cwd) {
        (Some(p), _) => Some(p.clone()),
        (None, Some(cwd)) => {
            let abs = std::fs::canonicalize(cwd)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| cwd.clone());
            Some(rogrep_model::project::dash_project_from_cwd(&abs))
        }
        _ => None,
    };
    let rows = store.recent_conversations(args.limit, project.as_deref())?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "{:<28} {:<8} {:>6} {:>5}  {:<19} TITLE",
        "ID", "AGENT", "TURNS", "EXCH", "LAST ACTIVE"
    );
    for r in rows {
        let last = r
            .last_activity_at
            .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
            .map(|ts| {
                ts.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "?".into());
        let title = r.title.clone().unwrap_or_default();
        let marker = match (r.origin.as_str(), r.is_subagent) {
            ("auxiliary", _) => " [aux]",
            ("scheduled", _) => " [sched]",
            (_, true) => " [sub]",
            _ => "",
        };
        println!(
            "{:<28} {:<8} {:>6} {:>5}  {:<19} {}{}",
            r.id,
            r.provider,
            r.turn_count,
            r.exchange_count,
            last,
            title.chars().take(60).collect::<String>(),
            marker
        );
    }
    Ok(())
}
