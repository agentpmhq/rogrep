use clap::Args;
use rogrep_model::paths;

#[derive(Args)]
pub struct DoctorArgs {
    #[arg(long)]
    pub json: bool,
}

pub fn run(_args: DoctorArgs) -> anyhow::Result<()> {
    let home = paths::home_dir();
    let config = rogrep_model::config::Config::load_default()?;
    let extra: Vec<std::path::PathBuf> = config
        .sources
        .extra_roots
        .iter()
        .map(std::path::PathBuf::from)
        .collect();

    println!("data dir:   {}", paths::data_dir().display());
    println!("config:     {}", paths::config_path().display());
    println!();
    println!("discovery roots:");
    for (root, kind) in rogrep_parsers::discovery::provider_roots(&home) {
        let status = if root.is_dir() { "found" } else { "absent" };
        println!("  [{status:>6}] {:<14} {}", kind.to_string(), root.display());
    }
    for root in &extra {
        let status = if root.is_dir() { "found" } else { "absent" };
        println!("  [{status:>6}] {:<14} {}", "extra", root.display());
    }
    for agent in ["hermes", "opencode"] {
        let db = match agent {
            "hermes" => home.join(".hermes/state.db"),
            _ => home.join(".local/share/opencode/opencode.db"),
        };
        let status = if db.is_file() { "found" } else { "absent" };
        println!("  [{status:>6}] {:<14} {} (via spool)", agent, db.display());
    }
    println!();
    let mut extra = extra;
    let layout = paths::DataLayout::default_layout();
    for agent in ["hermes", "opencode"] {
        let dir = layout.spool_dir(agent);
        if dir.is_dir() {
            extra.push(dir);
        }
    }
    let files = rogrep_parsers::discover_files(&home, &extra);
    let total_bytes: u64 = files.iter().map(|f| f.size).sum();
    println!(
        "discovered {} rollout files ({:.1} MiB) across providers:",
        files.len(),
        total_bytes as f64 / (1024.0 * 1024.0)
    );
    let mut by_kind: std::collections::BTreeMap<String, (usize, u64)> = Default::default();
    for f in &files {
        let e = by_kind.entry(f.kind.to_string()).or_default();
        e.0 += 1;
        e.1 += f.size;
    }
    for (kind, (count, bytes)) in by_kind {
        println!("  {kind:<14} {count:>6} files  {:>10.1} MiB", bytes as f64 / (1024.0 * 1024.0));
    }
    Ok(())
}
