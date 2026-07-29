//! The shared parse loop. Providers implement `RolloutParser`; the driver
//! owns line/byte accounting, malformed counting, turn indexing, cwd/model
//! stickiness, special-turn annotation, exchange-boundary snapshots, and the
//! amendment barrier.

use crate::record::RawRecord;
use crate::reader::LineReader;
use crate::special::annotate_special;
use crate::state::{FrozenSummary, ParseState, PrefixFingerprint, FINGERPRINT_WINDOW};
use rogrep_model::{
    exchange::is_real_user_prompt, project, AgentKind, Conversation, ConversationId, Origin, Role,
    SourceSpan, SubagentLink, Turn, UnixMillis,
};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Static facts about a source file, derived from its path before parsing.
#[derive(Clone, Debug, Default)]
pub struct SourceInfo {
    pub source_path: String,
    /// Provider project slug from the path (claude dash-encoding etc.).
    pub project: String,
    /// Path-derived cwd seed (overridable by the first in-record cwd).
    pub cwd_seed: Option<String>,
    pub subagent: Option<SubagentLink>,
}

/// Mutable per-record context handed to providers.
pub struct ParseCtx<'a> {
    /// Timestamp of the current record, if any.
    pub record_ts: Option<UnixMillis>,
    emitted: Vec<Turn>,
    amendable: &'a mut Vec<Turn>,
    barrier: usize,
    signals: Signals,
}

#[derive(Default)]
struct Signals {
    cwd: Option<String>,
    model: Option<String>,
    title: Option<String>,
    origin: Option<Origin>,
}

impl<'a> ParseCtx<'a> {
    /// Emit a turn. The driver fills turn_index / source / ts fallback /
    /// cwd / model stickiness afterwards.
    pub fn emit(&mut self, turn: Turn) {
        self.emitted.push(turn);
    }

    /// Turns amendable by late-arriving records: everything since the current
    /// exchange opened. Earlier turns are frozen (checkpoint discipline).
    pub fn amendable(&mut self) -> &mut [Turn] {
        &mut self.amendable[self.barrier..]
    }

    /// Record-level cwd signal (first one claims the conversation cwd).
    pub fn set_cwd(&mut self, cwd: String) {
        if !cwd.trim().is_empty() {
            self.signals.cwd = Some(cwd);
        }
    }

    pub fn set_model(&mut self, model: String) {
        if !model.trim().is_empty() {
            self.signals.model = Some(model);
        }
    }

    /// Provider-generated title (AI title / session title). First wins.
    pub fn set_title(&mut self, title: String) {
        if !title.trim().is_empty() {
            self.signals.title = Some(title);
        }
    }

    pub fn set_origin(&mut self, origin: Origin) {
        self.signals.origin = Some(origin);
    }
}

/// A stateful per-file parser. One instance per parse run; state that must
/// survive across incremental runs goes through export/import.
pub trait RolloutParser {
    fn process(&mut self, rec: &RawRecord, ctx: &mut ParseCtx<'_>);
    /// End-of-run pass over the open-exchange turns.
    fn finish(&mut self, _amendable: &mut [Turn]) {}
    fn export_state(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn import_state(&mut self, _state: &serde_json::Value) {}
}

pub trait Provider: Sync + Send {
    fn kind(&self) -> AgentKind;
    fn parser_version(&self) -> u32;
    /// Does this provider own the given absolute path? Checked in registry
    /// order (more specific providers first).
    fn claims_path(&self, path: &str) -> bool;
    fn source_info(&self, path: &str) -> SourceInfo;
    fn new_parser(&self) -> Box<dyn RolloutParser>;
}

/// Result of one parse run.
pub struct DriverOutput {
    /// Summary over the whole file; `turns` holds only the turns from the
    /// previous watermark onward (all turns on a fresh parse).
    pub conversation: Conversation,
    /// Turn index at which `conversation.turns` begins — stored turns >= this
    /// must be replaced by them.
    pub replace_from: u32,
    /// Ordinal base for exchanges built over `conversation.turns` (the count
    /// of exchanges wholly before `replace_from`).
    pub exchange_base: u32,
    /// Checkpoint for the next run.
    pub state: ParseState,
}

/// Validate a seed against the file on disk. Returns None when the seed is
/// unusable (version bump, shrink, rewrite) and a full parse is needed.
fn validate_seed(file: &mut File, seed: ParseState, parser_version: u32) -> io::Result<Option<ParseState>> {
    if seed.parser_version != parser_version {
        return Ok(None);
    }
    let len = file.metadata()?.len();
    if len < seed.byte_offset {
        return Ok(None);
    }
    let window = seed.byte_offset.min(FINGERPRINT_WINDOW as u64);
    let mut buf = vec![0u8; window as usize];
    file.seek(SeekFrom::Start(seed.byte_offset - window))?;
    file.read_exact(&mut buf)?;
    let fp = PrefixFingerprint::compute(&buf, seed.byte_offset);
    if fp != seed.fingerprint {
        return Ok(None);
    }
    Ok(Some(seed))
}

pub fn parse_source(
    provider: &dyn Provider,
    path: &Path,
    seed: Option<ParseState>,
) -> io::Result<DriverOutput> {
    let mut file = File::open(path)?;
    let seed = match seed {
        Some(s) => validate_seed(&mut file, s, provider.parser_version())?,
        None => None,
    };
    let state = seed.unwrap_or_else(|| ParseState::fresh(provider.parser_version()));
    let info = provider.source_info(&path.to_string_lossy());
    parse_from(provider, &info, &mut file, state)
}

/// Parse from an already-validated state. Exposed for tests that parse from
/// in-memory buffers via a temp file.
pub fn parse_from(
    provider: &dyn Provider,
    info: &SourceInfo,
    file: &mut File,
    state: ParseState,
) -> io::Result<DriverOutput> {
    // Pre-fill the fingerprint window with the bytes just before the resume
    // offset so snapshots taken early in a resumed run hash the same window
    // a full parse would.
    let window_len = state.byte_offset.min(FINGERPRINT_WINDOW as u64);
    let mut prefix_window = vec![0u8; window_len as usize];
    file.seek(SeekFrom::Start(state.byte_offset - window_len))?;
    file.read_exact(&mut prefix_window)?;
    let reader = LineReader::new(BufReader::new(file), state.line_number, state.byte_offset);
    run(provider, info, reader, state, prefix_window)
}

struct Snapshot {
    state: ParseState,
    /// Bytes of the last <=4KiB before the snapshot offset, for the
    /// fingerprint.
    tail_window: Vec<u8>,
}

fn run<R: io::BufRead>(
    provider: &dyn Provider,
    info: &SourceInfo,
    mut reader: LineReader<R>,
    seed: ParseState,
    prefix_window: Vec<u8>,
) -> io::Result<DriverOutput> {
    let replace_from = seed.next_turn_index;
    let mut parser = provider.new_parser();
    parser.import_state(&seed.provider_state);

    // Running conversation-level state (mutated as records arrive).
    let mut running = seed.clone();
    if running.conversation_cwd.is_none() {
        running.conversation_cwd = info.cwd_seed.clone();
    }
    let mut turns: Vec<Turn> = Vec::new();
    let mut barrier: usize = 0;
    let mut snapshot: Option<Snapshot> = None;
    let mut malformed_tail: u32 = 0;
    // Rolling window of recent raw bytes for fingerprints, seeded with the
    // bytes preceding the resume offset.
    let mut window: Vec<u8> = prefix_window;

    while let Some(line) = reader.next_line()? {
        // Snapshot candidate state BEFORE this record mutates anything.
        let pre_state = {
            let mut s = running.clone();
            s.line_number = line.line;
            s.byte_offset = line.byte_start;
            s.next_turn_index = replace_from + turns.len() as u32;
            s.frozen.malformed_lines = seed.frozen.malformed_lines + malformed_tail;
            s.provider_state = serde_json::Value::Null; // filled lazily below
            s
        };
        let pre_provider_state = parser.export_state();
        let pre_window_tail: Vec<u8> = {
            let start = window.len().saturating_sub(FINGERPRINT_WINDOW);
            window[start..].to_vec()
        };

        // Extend rolling window with this line's raw bytes (approximation of
        // the file bytes: bytes + newline; CR loss is fine — the fingerprint
        // only needs to be self-consistent, and it is recomputed from the
        // file on resume… so it must match file bytes exactly).
        // NOTE: to stay byte-exact we re-read from the file on resume, so the
        // window must mirror file bytes. LineReader strips \r; compensate by
        // tracking raw length: reconstruct exact bytes via byte offsets is
        // costly — instead fingerprint uses the LAST 4KiB ending at the
        // snapshot offset, gathered from raw line bytes incl. newline.
        push_window(&mut window, &line.bytes, line.byte_end - line.byte_start);

        if line.bytes.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        let Some(rec) = (if line.oversized { None } else { RawRecord::parse(&line.bytes) }) else {
            malformed_tail += 1;
            continue;
        };

        let record_ts = rec.timestamp_millis();
        let mut ctx = ParseCtx {
            record_ts,
            emitted: Vec::new(),
            amendable: &mut turns,
            barrier,
            signals: Signals::default(),
        };
        parser.process(&rec, &mut ctx);
        let Signals { cwd, model, title, origin } = ctx.signals;
        let mut emitted = ctx.emitted;

        // Apply record-level signals.
        if let Some(cwd) = cwd {
            if !running.cwd_from_record {
                running.conversation_cwd = Some(cwd.clone());
                running.cwd_from_record = true;
            }
            running.current_cwd = Some(cwd);
        }
        if let Some(model) = model {
            if running.conversation_model.is_none() {
                running.conversation_model = Some(model.clone());
            }
            running.current_model = Some(model);
        }
        if let Some(title) = title {
            running.title = Some(title);
        }
        if let Some(origin) = origin {
            running.origin = origin;
        }

        // Finalize emitted turns.
        let mut record_opens_exchange = false;
        for mut turn in emitted.drain(..) {
            turn.turn_index = replace_from + turns.len() as u32;
            turn.source = SourceSpan {
                line: line.line,
                byte_start: line.byte_start,
                byte_end: line.byte_end,
            };
            if turn.ts.is_none() {
                turn.ts = record_ts;
            }
            if turn.cwd.is_none() {
                turn.cwd = running.current_cwd.clone();
            }
            if turn.model.is_none() && turn.role == Role::Assistant {
                turn.model = running.current_model.clone();
            }
            // Text-scraped cwd (weakest layer): only when nothing better yet.
            if running.conversation_cwd.is_none() {
                if let Some(cwd) = project::cwd_from_text(&turn.text) {
                    running.conversation_cwd = Some(cwd);
                }
            }
            annotate_special(&mut turn);
            if is_real_user_prompt(&turn) {
                record_opens_exchange = true;
            }
            // Stamp a tool output's status back onto its paired call within
            // the open exchange (barrier-respecting, so incremental parses
            // behave identically to full parses).
            if let Some(rogrep_model::ToolInfo {
                direction: Some(rogrep_model::ToolDirection::Output),
                pair_id: Some(pair),
                status,
                ..
            }) = &turn.tool
            {
                if *status != rogrep_model::ToolStatus::Unknown {
                    let (pair, status) = (pair.clone(), *status);
                    for prev in turns[barrier..].iter_mut().rev() {
                        if let Some(info) = &mut prev.tool {
                            if info.direction == Some(rogrep_model::ToolDirection::Use)
                                && info.pair_id.as_deref() == Some(pair.as_str())
                            {
                                if info.status == rogrep_model::ToolStatus::Unknown {
                                    info.status = status;
                                }
                                break;
                            }
                        }
                    }
                }
            }
            turns.push(turn);
        }

        if record_opens_exchange {
            let mut s = pre_state;
            s.provider_state = pre_provider_state;
            let new_barrier = (s.next_turn_index - replace_from) as usize;
            // Frozen rollup: seed frozen + turns emitted before this record.
            let frozen = rollup(&seed.frozen, &turns[..new_barrier]);
            s.frozen = FrozenSummary {
                malformed_lines: s.frozen.malformed_lines,
                ..frozen
            };
            // Tail starts at an exchange boundary, so tail-local exchanges
            // before the new watermark are all wholly frozen.
            s.frozen_exchange_count = seed.frozen_exchange_count
                + rogrep_model::build_exchanges(&turns[..new_barrier]).len() as u32;
            snapshot = Some(Snapshot {
                state: s,
                tail_window: pre_window_tail,
            });
            barrier = new_barrier;
        }
    }

    parser.finish(&mut turns[barrier..]);

    // Final checkpoint: the last snapshot, or the seed watermark unchanged.
    let mut out_state = match snapshot {
        Some(snap) => {
            let mut s = snap.state;
            s.fingerprint = PrefixFingerprint::compute(&snap.tail_window, s.byte_offset);
            s
        }
        None => {
            let mut s = seed.clone();
            s.parser_version = provider.parser_version();
            s
        }
    };
    // The final provider_state in the checkpoint reflects the snapshot point,
    // already captured. For the no-snapshot case keep the seed's state.

    // Full-file summary = seed frozen + all turns this run.
    let mut all_rollup = rollup(&seed.frozen, &turns);
    all_rollup.malformed_lines = seed.frozen.malformed_lines + malformed_tail;
    out_state.frozen.turn_count = out_state.next_turn_index;

    let conversation = Conversation {
        id: conversation_id_for(info),
        agent: provider.kind(),
        source_path: info.source_path.clone(),
        title: running.title.clone(),
        model: running.conversation_model.clone(),
        project: info.project.clone(),
        normalized_project: project::normalized_project(
            &info.project,
            running.conversation_cwd.as_deref().unwrap_or(""),
            provider.kind(),
        ),
        cwd: running.conversation_cwd.clone(),
        first_seen: all_rollup.first_seen,
        last_seen: all_rollup.last_seen,
        tokens: all_rollup.tokens,
        malformed_lines: all_rollup.malformed_lines,
        origin: if info.subagent.is_some() {
            Origin::Subagent
        } else {
            running.origin.clone()
        },
        subagent: info.subagent.clone(),
        turns,
    };

    // Carry conversation-level signals into the checkpoint... they were
    // captured pre-snapshot; when no snapshot happened they equal the seed's.
    Ok(DriverOutput {
        conversation,
        replace_from,
        exchange_base: seed.frozen_exchange_count,
        state: out_state,
    })
}

fn rollup(frozen: &FrozenSummary, turns: &[Turn]) -> FrozenSummary {
    let mut out = frozen.clone();
    for t in turns {
        out.turn_count += 1;
        out.tokens.add(&t.tokens);
        if let Some(ts) = t.ts {
            out.first_seen = Some(out.first_seen.map_or(ts, |f: i64| f.min(ts)));
            out.last_seen = Some(out.last_seen.map_or(ts, |l: i64| l.max(ts)));
        }
    }
    out
}

fn conversation_id_for(info: &SourceInfo) -> ConversationId {
    ConversationId::from_source_path(&info.source_path)
}

fn push_window(window: &mut Vec<u8>, line_bytes: &[u8], raw_len: u64) {
    // Reconstruct the raw file bytes for this line: content + line ending.
    // (LineReader strips a trailing \r; oversized lines can't be
    // reconstructed, which only costs a fingerprint mismatch → full reparse.)
    window.extend_from_slice(line_bytes);
    match raw_len.saturating_sub(line_bytes.len() as u64) {
        1 => window.push(b'\n'),
        2 => window.extend_from_slice(b"\r\n"),
        _ => {}
    }
    if window.len() > FINGERPRINT_WINDOW * 4 {
        let excess = window.len() - FINGERPRINT_WINDOW * 2;
        window.drain(..excess);
    }
}
