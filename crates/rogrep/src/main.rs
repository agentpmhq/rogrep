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
        Some(Command::Sync(args)) => cmd::sync::run(args),
        Some(Command::Ls(args)) => cmd::ls::run(args),
        Some(Command::Stats(args)) => cmd::stats::run(args),
        Some(Command::Parse(args)) => cmd::parse::run(args),
        Some(Command::Doctor(args)) => cmd::doctor::run(args),
        None => {
            if cli.query.is_empty() {
                println!("rogrep — try `rogrep sync`, `rogrep ls`, `rogrep stats`, or `rogrep --help`");
                Ok(())
            } else {
                anyhow::bail!("search lands in M3; try `rogrep ls` or `rogrep stats` for now");
            }
        }
    }
}
