use anyhow::Result;
use clap::Args;
use rogrep_engine::{SyncEvent, SyncOptions};
use rogrep_model::config::Config;
use rogrep_model::paths::DataLayout;
use rogrep_store::Store;

#[derive(Args)]
pub struct SyncArgs {
    /// Re-parse and re-index everything from scratch.
    #[arg(long)]
    pub full: bool,
    #[arg(long)]
    pub json: bool,
}

/// Open the store and run one sync pass. Shared by every command that wants
/// fresh data before answering.
pub fn sync_now(full: bool, quiet: bool) -> Result<(DataLayout, Config, Store)> {
    let layout = DataLayout::default_layout();
    let config = Config::load_default()?;
    let mut store = Store::open(&layout.db_path())?;
    let options = SyncOptions {
        full,
        ..Default::default()
    };
    // Substantial re-indexing gets a progress bar even on implicit syncs
    // (the `quiet` flag only suppresses text notes); indicatif hides it when
    // stderr is not a terminal.
    let mut progress = super::progress::SyncProgress::default();
    let report = rogrep_engine::sync(
        &layout,
        &config,
        &mut store,
        &mut super::index::open_indexer(&layout, &config)?,
        &options,
        &mut |event| progress.handle(event),
    )?;
    progress.clear();
    if !report.synced && !quiet {
        eprintln!("note: another rogrep is syncing; results may be seconds stale");
    }
    for err in &report.errors {
        eprintln!("warn: {err}");
    }
    Ok((layout, config, store))
}

pub fn run(args: SyncArgs) -> Result<()> {
    let layout = DataLayout::default_layout();
    let config = Config::load_default()?;
    let mut store = Store::open(&layout.db_path())?;
    let options = SyncOptions {
        full: args.full,
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let mut progress = super::progress::SyncProgress::default();
    let report = rogrep_engine::sync(
        &layout,
        &config,
        &mut store,
        &mut super::index::open_indexer(&layout, &config)?,
        &options,
        &mut |event| {
            if let SyncEvent::Discovered(n) = event {
                eprintln!("discovered {n} rollout files");
            }
            progress.handle(event);
        },
    )?;
    progress.clear();
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "schema": "rogrep/v1",
                "files_discovered": report.files_discovered,
                "files_changed": report.files_changed,
                "files_removed": report.files_removed,
                "turns_written": report.turns_written,
                "errors": report.errors,
                "elapsed_ms": report.elapsed_ms,
                "synced": report.synced,
            })
        );
    } else {
        println!(
            "synced: {} files scanned, {} changed, {} turns written in {:.1}s",
            report.files_discovered,
            report.files_changed,
            report.turns_written,
            started.elapsed().as_secs_f64()
        );
        if !report.errors.is_empty() {
            println!("{} files had errors (see stderr)", report.errors.len());
        }
    }
    Ok(())
}
