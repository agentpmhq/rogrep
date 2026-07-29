mod cmd;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rogrep", version, about = "rollout grep: local search, stats, and trajectory over coding-agent sessions")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Free-form search query (bare `rogrep QUERY` == `rogrep search QUERY`).
    #[arg(trailing_var_arg = true)]
    query: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Search across all conversations (also the bare default: `rogrep QUERY`).
    #[command(alias = "s")]
    Search(cmd::search::SearchArgs),
    /// Conjunction find inside one conversation (three-tier results).
    Find(cmd::find::FindArgs),
    /// Show a conversation, exchange (rg_…#eN), or turn window.
    Show(cmd::show::ShowArgs),
    /// Git/GitHub timeline of one conversation.
    Git(cmd::git::GitArgs),
    /// Which conversations led to a PR, branch, or commit.
    Trajectory(cmd::trajectory::TrajectoryArgs),
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
        Some(Command::Find(args)) => cmd::find::run(args),
        Some(Command::Show(args)) => cmd::show::run(args),
        Some(Command::Git(args)) => cmd::git::run(args),
        Some(Command::Trajectory(args)) => cmd::trajectory::run(args),
        Some(Command::Sync(args)) => cmd::sync::run(args),
        Some(Command::Ls(args)) => cmd::ls::run(args),
        Some(Command::Stats(args)) => cmd::stats::run(args),
        Some(Command::Parse(args)) => cmd::parse::run(args),
        Some(Command::Doctor(args)) => cmd::doctor::run(args),
        None => {
            if cli.query.is_empty() {
                println!("rogrep — try `rogrep QUERY`, `rogrep ls`, `rogrep stats`, or `rogrep --help`");
                Ok(())
            } else {
                cmd::search::run(cmd::search::SearchArgs {
                    query: cli.query,
                    limit: 20,
                    project: None,
                    cwd: None,
                    since: None,
                    sort: "relevance".into(),
                    json: false,
                })
            }
        }
    }
}
