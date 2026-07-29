mod cmd;
mod color;
mod tui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rogrep", version, about = "rollout grep: local search, stats, and trajectory over coding-agent sessions")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Free-form search query (bare `rogrep QUERY` == `rogrep search QUERY`).
    query: Vec<String>,
    // Search passthrough flags for the bare form, so
    // `rogrep hermes --limit 5` behaves like `rogrep search hermes --limit 5`.
    #[arg(long, default_value_t = 20, hide = true)]
    limit: usize,
    #[arg(long, hide = true)]
    project: Option<String>,
    #[arg(long, hide = true)]
    cwd: Option<String>,
    #[arg(long, hide = true)]
    since: Option<String>,
    #[arg(long, default_value = "relevance", value_parser = ["relevance", "recent"], hide = true)]
    sort: String,
    #[arg(long, hide = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Search across all conversations (also the bare default: `rogrep QUERY`).
    #[command(alias = "s")]
    Search(cmd::search::SearchArgs),
    /// Exchange-granularity search/listing (user prompt → all agent actions).
    #[command(name = "x", alias = "exchanges")]
    X(cmd::exchanges::ExchangesArgs),
    /// Conjunction find inside one conversation (three-tier results).
    Find(cmd::find::FindArgs),
    /// Show a conversation, exchange (rg_…#eN), or turn window.
    Show(cmd::show::ShowArgs),
    /// Git/GitHub timeline of one conversation.
    Git(cmd::git::GitArgs),
    /// Which conversations led to a PR, branch, or commit.
    Trajectory(cmd::trajectory::TrajectoryArgs),
    /// Interactive terminal UI.
    Tui {
        /// Optional initial query.
        query: Vec<String>,
    },
    /// Refresh the local index (runs automatically before other commands).
    Sync(cmd::sync::SyncArgs),
    /// List recent conversations.
    Ls(cmd::ls::LsArgs),
    /// Deterministic usage statistics and reports.
    Stats(cmd::stats::StatsArgs),
    /// Parse one rollout file and print the normalized conversation (debug).
    Parse(cmd::parse::ParseArgs),
    /// Report discovery roots, parse health, and index status.
    Doctor(cmd::doctor::DoctorArgs),
    /// Install/inspect the bundled agent skill.
    Skill(cmd::skill::SkillArgs),
}

fn main() -> anyhow::Result<()> {
    // Die quietly on closed pipes (`rogrep … | head`).
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Search(args)) => cmd::search::run(args),
        Some(Command::X(args)) => cmd::exchanges::run(args),
        Some(Command::Find(args)) => cmd::find::run(args),
        Some(Command::Show(args)) => cmd::show::run(args),
        Some(Command::Git(args)) => cmd::git::run(args),
        Some(Command::Trajectory(args)) => cmd::trajectory::run(args),
        Some(Command::Tui { query }) => {
            let q = if query.is_empty() { None } else { Some(query.join(" ")) };
            tui::run(q)
        }
        Some(Command::Sync(args)) => cmd::sync::run(args),
        Some(Command::Ls(args)) => cmd::ls::run(args),
        Some(Command::Stats(args)) => cmd::stats::run(args),
        Some(Command::Parse(args)) => cmd::parse::run(args),
        Some(Command::Doctor(args)) => cmd::doctor::run(args),
        Some(Command::Skill(args)) => cmd::skill::run(args),
        None => {
            if cli.query.is_empty() {
                println!("rogrep — try `rogrep QUERY`, `rogrep ls`, `rogrep stats`, or `rogrep --help`");
                Ok(())
            } else {
                cmd::search::run(cmd::search::SearchArgs {
                    query: cli.query,
                    limit: cli.limit,
                    project: cli.project,
                    cwd: cli.cwd,
                    since: cli.since,
                    sort: cli.sort,
                    json: cli.json,
                })
            }
        }
    }
}
