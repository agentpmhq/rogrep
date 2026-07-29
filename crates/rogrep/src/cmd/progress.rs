//! Shared sync progress reporting: a braille spinner + bar on stderr while
//! files re-derive. indicatif hides it automatically when stderr is not a
//! terminal, so scripts and agents see nothing.

use indicatif::{ProgressBar, ProgressStyle};
use rogrep_engine::SyncEvent;
use std::time::Duration;

/// Only substantial work gets a bar; a couple of tailed files finish in
/// milliseconds and would just flash.
const MIN_FILES_FOR_BAR: usize = 4;

#[derive(Default)]
pub struct SyncProgress {
    bar: Option<ProgressBar>,
}

impl SyncProgress {
    pub fn handle(&mut self, event: SyncEvent<'_>) {
        match event {
            SyncEvent::Discovered(_) => {}
            SyncEvent::Parsing { path, index, total } => {
                if total < MIN_FILES_FOR_BAR {
                    return;
                }
                let bar = self.bar.get_or_insert_with(|| {
                    let bar = ProgressBar::new(total as u64);
                    bar.set_style(
                        ProgressStyle::with_template(
                            "{spinner:.cyan} indexing {pos}/{len} [{bar:24}] {wide_msg:.dim}",
                        )
                        .expect("static template")
                        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏✓")
                        .progress_chars("█▓░"),
                    );
                    bar.enable_steady_tick(Duration::from_millis(80));
                    bar
                });
                bar.set_length(total as u64);
                bar.set_position(index as u64);
                bar.set_message(short_path(path));
            }
            SyncEvent::Done => self.clear(),
        }
    }

    pub fn clear(&mut self) {
        if let Some(bar) = self.bar.take() {
            bar.finish_and_clear();
        }
    }
}

impl Drop for SyncProgress {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Last few path components fit the message slot better than a full path.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.rsplit('/').take(3).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_path_keeps_tail() {
        assert_eq!(
            short_path("/home/u/.codex/sessions/2026/07/03/rollout-x.jsonl"),
            "07/03/rollout-x.jsonl"
        );
        assert_eq!(short_path("x.jsonl"), "x.jsonl");
    }

    /// Events drive the bar without a terminal (hidden draw target).
    #[test]
    fn event_stream_smoke() {
        let mut p = SyncProgress::default();
        p.handle(SyncEvent::Discovered(10));
        for i in 1..=10 {
            p.handle(SyncEvent::Parsing {
                path: "/a/b/c.jsonl",
                index: i,
                total: 10,
            });
        }
        p.handle(SyncEvent::Done);
        assert!(p.bar.is_none());
    }
}
