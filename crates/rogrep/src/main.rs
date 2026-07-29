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
        Some(Command::Parse(args)) => cmd::parse::run(args),
        Some(Command::Doctor(args)) => cmd::doctor::run(args),
        None => {
            if cli.query.is_empty() {
                println!("rogrep — try `rogrep doctor`, `rogrep parse FILE`, or `rogrep --help`");
                Ok(())
            } else {
                anyhow::bail!("search is not wired up yet (coming in M3)");
            }
        }
    }
}
