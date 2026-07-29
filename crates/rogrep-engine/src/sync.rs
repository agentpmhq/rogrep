//! The on-invocation sync pipeline: discover → checkpoint diff → parse
//! changed tails (parallel) → apply to store + search index (serial).
//!
//! Steady-state contract: when nothing changed, no parse, no index commit,
//! and the whole pass stays well under 100ms.

use anyhow::Result;
use rayon::prelude::*;
use rogrep_model::config::Config;
use rogrep_model::paths::DataLayout;
use rogrep_parsers::driver::DriverOutput;
use rogrep_parsers::{discover_files, provider_for_kind, DiscoveredFile};
use rogrep_store::Store;
use std::path::PathBuf;
use std::time::Instant;

/// Search-index hook (implemented by rogrep-index; no-op before M3 and in
/// stats-only contexts). `apply` receives each parse output; `commit` runs
/// once per sync tick BEFORE the store checkpoints (crash between the two is
/// resolved by the idempotent tail refresh on the next tick).
pub trait Indexer {
    fn apply(&mut self, out: &DriverOutput) -> Result<()>;
    fn remove_conversation(&mut self, conversation_id: &str) -> Result<()>;
    fn commit(&mut self) -> Result<()>;
}

pub struct NoopIndexer;

impl Indexer for NoopIndexer {
    fn apply(&mut self, _out: &DriverOutput) -> Result<()> {
        Ok(())
    }
    fn remove_conversation(&mut self, _conversation_id: &str) -> Result<()> {
        Ok(())
    }
    fn commit(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub files_discovered: usize,
    pub files_changed: usize,
    pub files_removed: usize,
    pub turns_written: u64,
    pub errors: Vec<String>,
    pub elapsed_ms: u128,
    /// False when another rogrep held the writer lock and we skipped syncing.
    pub synced: bool,
}

pub enum SyncEvent<'a> {
    Discovered(usize),
    Parsing { path: &'a str, index: usize, total: usize },
    Done,
}

pub struct SyncOptions {
    pub full: bool,
    pub home: PathBuf,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions {
            full: false,
            home: rogrep_model::paths::home_dir(),
        }
    }
}

pub fn sync(
    layout: &DataLayout,
    config: &Config,
    store: &mut Store,
    indexer: &mut dyn Indexer,
    options: &SyncOptions,
    progress: &mut dyn FnMut(SyncEvent<'_>),
) -> Result<SyncReport> {
    let started = Instant::now();
    let mut report = SyncReport::default();

    // Writer lock: skip syncing (read-only mode) if another rogrep holds it.
    let lock_path = layout.writer_lock_path();
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    let mut lock = fd_lock::RwLock::new(lock_file);
    let guard = match lock.try_write() {
        Ok(g) => g,
        Err(_) => {
            report.elapsed_ms = started.elapsed().as_millis();
            return Ok(report); // synced=false
        }
    };

    // Materialize SQLite-backed sessions (hermes, opencode) into spool
    // JSONL so they flow through the same parser pipeline.
    let spool_root = layout.root.join("spool");
    let spool_report = rogrep_parsers::spool::export_all(&options.home, &spool_root);
    report.errors.extend(spool_report.errors);

    let mut extra_roots: Vec<PathBuf> = config.sources.extra_roots.iter().map(PathBuf::from).collect();
    for agent in ["hermes", "opencode"] {
        let dir = spool_root.join(agent);
        if dir.is_dir() {
            extra_roots.push(dir);
        }
    }
    let disabled: Vec<&str> = config.sources.disabled_providers.iter().map(|s| s.as_str()).collect();
    let files: Vec<DiscoveredFile> = discover_files(&options.home, &extra_roots)
        .into_iter()
        .filter(|f| !disabled.contains(&f.kind.as_str()))
        .collect();
    report.files_discovered = files.len();
    progress(SyncEvent::Discovered(files.len()));

    let checkpoints = store.file_checkpoints()?;

    // Removed files → drop their conversations.
    let live: std::collections::HashSet<String> = files
        .iter()
        .map(|f| f.path.to_string_lossy().to_string())
        .collect();
    for path in checkpoints.keys() {
        if !live.contains(path) {
            store.remove_conversation_by_path(path)?;
            if let Some(cp) = checkpoints.get(path) {
                indexer.remove_conversation(&cp.conversation_id)?;
            }
            report.files_removed += 1;
        }
    }

    // Changed files, most recent first so fresh conversations land early.
    // A parser-version bump also invalidates the checkpoint (re-derive that
    // provider's conversations even though the file bytes are unchanged).
    let mut changed: Vec<&DiscoveredFile> = files
        .iter()
        .filter(|f| {
            if options.full {
                return true;
            }
            let current_version = provider_for_kind(f.kind).map(|p| p.parser_version());
            match checkpoints.get(&f.path.to_string_lossy().to_string()) {
                Some(cp) => {
                    cp.size != f.size
                        || cp.mtime_ns != f.mtime_ns
                        || Some(cp.state.parser_version) != current_version
                }
                None => true,
            }
        })
        .collect();
    changed.sort_by_key(|f| std::cmp::Reverse(f.mtime_ns));
    report.files_changed = changed.len();

    if changed.is_empty() {
        report.synced = true;
        report.elapsed_ms = started.elapsed().as_millis();
        progress(SyncEvent::Done);
        drop(guard);
        return Ok(report);
    }

    // Parse in parallel batches, apply serially (index first, then store) so
    // progress is visible and memory stays bounded.
    const BATCH: usize = 32;
    let total = changed.len();
    let mut done = 0usize;
    for batch in changed.chunks(BATCH) {
        let outputs: Vec<(String, u64, i128, Result<DriverOutput, String>)> = batch
            .par_iter()
            .map(|f| {
                let path_str = f.path.to_string_lossy().to_string();
                let seed = if options.full {
                    None
                } else {
                    checkpoints.get(&path_str).map(|cp| cp.state.clone())
                };
                let result = (|| {
                    let provider = provider_for_kind(f.kind)
                        .ok_or_else(|| format!("no provider for {}", f.kind))?;
                    rogrep_parsers::parse_source(provider, &f.path, seed)
                        .map_err(|e| format!("{}: {e}", f.path.display()))
                })();
                (path_str, f.size, f.mtime_ns, result)
            })
            .collect();

        // Index first and COMMIT the index before checkpointing the store:
        // a crash in between leaves checkpoints behind the index, and the
        // next tick's tail refresh (delete-from-watermark + re-add) is
        // idempotent. The reverse order would strand a stale index behind
        // advanced checkpoints.
        let mut applied: Vec<(String, u64, i128, DriverOutput)> = Vec::new();
        for (path_str, size, mtime_ns, result) in outputs {
            done += 1;
            progress(SyncEvent::Parsing {
                path: &path_str,
                index: done,
                total,
            });
            match result {
                Ok(out) => {
                    indexer.apply(&out)?;
                    applied.push((path_str, size, mtime_ns, out));
                }
                Err(e) => report.errors.push(e),
            }
        }
        indexer.commit()?;
        for (_path, size, mtime_ns, out) in applied {
            report.turns_written += store.apply_parse(&out, size, mtime_ns)?;
        }
    }

    report.synced = true;
    report.elapsed_ms = started.elapsed().as_millis();
    progress(SyncEvent::Done);
    drop(guard);
    Ok(report)
}

impl Indexer for Box<dyn Indexer> {
    fn apply(&mut self, out: &DriverOutput) -> Result<()> {
        (**self).apply(out)
    }
    fn remove_conversation(&mut self, conversation_id: &str) -> Result<()> {
        (**self).remove_conversation(conversation_id)
    }
    fn commit(&mut self) -> Result<()> {
        (**self).commit()
    }
}
