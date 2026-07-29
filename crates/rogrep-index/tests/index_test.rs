//! Index round-trip tests: apply fixtures, search, find, tail refresh.

use rogrep_index::{parse_query, SearchIndex};
use rogrep_model::AgentKind;
use rogrep_parsers::driver::{parse_from, DriverOutput};
use rogrep_parsers::state::ParseState;
use std::io::Write;
use std::path::Path;

fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../rogrep-parsers/fixtures")
            .join(rel),
    )
    .unwrap()
}

fn parse_bytes(kind: AgentKind, name: &str, bytes: &[u8], seed: Option<ParseState>) -> DriverOutput {
    let provider = rogrep_parsers::provider_for_kind(kind).unwrap();
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(bytes).unwrap();
    tmp.flush().unwrap();
    let path = match kind {
        AgentKind::Claude => format!("/home/u/.claude/projects/-home-u-src-proj/{name}"),
        AgentKind::Codex => format!("/home/u/.codex/sessions/2026/07/03/{name}"),
        _ => format!("/home/u/logs/{name}"),
    };
    let info = provider.source_info(&path);
    let mut file = tmp.reopen().unwrap();
    let state = seed.unwrap_or_else(|| ParseState::fresh(provider.parser_version()));
    parse_from(provider, &info, &mut file, state).unwrap()
}

fn build_index(outs: &[&DriverOutput]) -> (tempfile::TempDir, SearchIndex) {
    let tmp = tempfile::tempdir().unwrap();
    let index = SearchIndex::open_or_create(tmp.path()).unwrap();
    let mut batch = index.writer().unwrap();
    for out in outs {
        batch.apply(out).unwrap();
    }
    batch.commit().unwrap();
    (tmp, index)
}

#[test]
fn search_finds_terms_across_conversations() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let codex = parse_bytes(AgentKind::Codex, "b.jsonl", &fixture("codex/session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude, &codex]);

    let hits = index.search(&parse_query("flaky"), None, 100).unwrap();
    assert_eq!(hits.len(), 2, "both fixtures mention flaky");

    let hits = index.search(&parse_query("tokenizer offsets crlf"), None, 100).unwrap();
    assert!(hits.is_empty() || hits.iter().all(|h| h.match_count == 0) == false);

    let hits = index.search(&parse_query("retry network"), None, 100).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].conversation_id, codex.conversation.id.as_str());
    let best = hits[0].best.as_ref().unwrap();
    assert!(best.excerpt.contains("retry"));
}

#[test]
fn facet_queries_narrow() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let codex = parse_bytes(AgentKind::Codex, "b.jsonl", &fixture("codex/session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude, &codex]);

    // tool_status:failed → both have a failing tool call.
    let hits = index.search(&parse_query("tool_status:failed"), None, 100).unwrap();
    assert_eq!(hits.len(), 2);
    // tool:bash only in the claude fixture.
    let hits = index.search(&parse_query("tool:bash"), None, 100).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].conversation_id, claude.conversation.id.as_str());
    // provider facet.
    let hits = index.search(&parse_query("provider:codex tool_status:rejected"), None, 100).unwrap();
    assert_eq!(hits.len(), 1);
    // tool_cmd from shell parsing (claude's Bash and codex's exec_command
    // both ran cargo).
    let hits = index.search(&parse_query("tool_cmd:cargo"), None, 100).unwrap();
    assert_eq!(hits.len(), 2);
    let hits = index.search(&parse_query("tool_cmd:rg"), None, 100).unwrap();
    assert_eq!(hits.len(), 1, "only codex ran rg");
}

#[test]
fn phrase_and_literal_colon_queries() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude]);
    let hits = index.search(&parse_query("\"offsets drift on crlf\""), None, 100).unwrap();
    assert_eq!(hits.len(), 1);
    let hits = index.search(&parse_query("\"drift crlf on\""), None, 100).unwrap();
    assert!(hits.is_empty(), "phrase order matters");
}

#[test]
fn find_three_tiers() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let cid = claude.conversation.id.as_str().to_string();
    let (_tmp, index) = build_index(&[&claude]);

    // Strict: both terms in one turn.
    let r = index.find(&cid, &parse_query("offsets drift"), 50).unwrap();
    assert_eq!(r.total_turn_hits, 1);
    assert!(r.turn_hits[0].excerpt.to_lowercase().contains("offsets drift"));

    // Passage: terms in different turns of the same exchange ("parser" only
    // in the user prompt, "crlf" only in the closing assistant turn).
    let r = index.find(&cid, &parse_query("parser crlf"), 50).unwrap();
    assert_eq!(r.total_turn_hits, 0);
    assert_eq!(r.passage_exchanges, vec![0], "both in exchange 0");

    // Per-term counts always present.
    let r = index.find(&cid, &parse_query("flaky nonexistentterm"), 50).unwrap();
    let counts: std::collections::HashMap<_, _> = r.term_counts.iter().cloned().collect();
    assert!(counts["flaky"] > 0);
    assert_eq!(counts["nonexistentterm"], 0);
}

#[test]
fn tail_refresh_replaces_open_exchange() {
    let bytes = fixture("claude/basic_session.jsonl");
    let full = parse_bytes(AgentKind::Claude, "grow.jsonl", &bytes, None);

    // Simulate incremental: index prefix, then extended file.
    let cut = bytes
        .windows(1)
        .enumerate()
        .filter(|(_, w)| w[0] == b'\n')
        .map(|(i, _)| i + 1)
        .nth(4)
        .unwrap();
    let run1 = parse_bytes(AgentKind::Claude, "grow.jsonl", &bytes[..cut], None);
    let mut extended = bytes.clone();
    extended.extend_from_slice(
        br#"{"type":"user","message":{"role":"user","content":"brand new tail prompt zebra"},"uuid":"u9","timestamp":"2026-07-01T12:00:00.000Z","sessionId":"sess-1"}
"#,
    );
    let run2 = parse_bytes(AgentKind::Claude, "grow.jsonl", &extended, Some(run1.state.clone()));

    let tmp = tempfile::tempdir().unwrap();
    let index = SearchIndex::open_or_create(tmp.path()).unwrap();
    let mut batch = index.writer().unwrap();
    batch.apply(&run1).unwrap();
    batch.commit().unwrap();
    drop(batch);
    let mut batch = index.writer().unwrap();
    batch.apply(&run2).unwrap();
    batch.commit().unwrap();

    // No duplicate docs: doc_count == turns in extended file.
    let expected = full.conversation.turns.len() as u64 + 1;
    assert_eq!(index.doc_count().unwrap(), expected);
    let hits = index.search(&parse_query("zebra"), None, 10).unwrap();
    assert_eq!(hits.len(), 1);
    // The re-added tail keeps content searchable exactly once.
    let hits = index.search(&parse_query("ship"), None, 10).unwrap();
    assert_eq!(hits[0].match_count, 1);
}

#[test]
fn remove_conversation_deletes_docs() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude]);
    let mut batch = index.writer().unwrap();
    batch.remove_conversation(claude.conversation.id.as_str()).unwrap();
    batch.commit().unwrap();
    assert_eq!(index.doc_count().unwrap(), 0);
}

#[test]
fn date_range_filters() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude]);
    // Fixture is 2026-07-01.
    let july = (1_782_864_000_000i64, 1_782_950_400_000i64);
    let hits = index.search(&parse_query("flaky"), Some(july), 10).unwrap();
    assert_eq!(hits.len(), 1);
    let jan = (1_767_225_600_000i64, 1_767_312_000_000i64);
    let hits = index.search(&parse_query("flaky"), Some(jan), 10).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn injected_context_excluded_from_corpus_search_but_findable() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/edge_cases.jsonl"), None);
    let cid = claude.conversation.id.as_str().to_string();
    let (_tmp, index) = build_index(&[&claude]);

    // The fixture's injected reminder and IDE attachment are invisible turns.
    // Corpus search must not surface conversations through them…
    let hits = index.search(&parse_query("injected reminder"), None, 10).unwrap();
    assert!(hits.is_empty(), "injected context leaked into corpus search: {hits:?}");
    let hits = index.search(&parse_query("broken"), None, 10).unwrap();
    assert!(hits.is_empty(), "synthetic attachment leaked into corpus search: {hits:?}");

    // …but visible content still matches…
    let hits = index.search(&parse_query("directory work"), None, 10).unwrap();
    assert_eq!(hits.len(), 1);

    // …and conversation-scoped find greps EVERYTHING, including context.
    let r = index.find(&cid, &parse_query("injected reminder"), 10).unwrap();
    assert_eq!(r.total_turn_hits, 1, "find must keep grep-everything semantics");
    let r = index.find(&cid, &parse_query("broken"), 10).unwrap();
    assert_eq!(r.total_turn_hits, 1);
}

#[test]
fn auxiliary_sessions_excluded_from_corpus_search() {
    let judge = parse_bytes(AgentKind::Codex, "judge.jsonl", &fixture("codex/auto_review.jsonl"), None);
    assert_eq!(judge.conversation.origin, rogrep_model::Origin::Auxiliary);
    let (_tmp, index) = build_index(&[&judge]);

    // Default corpus search never surfaces the judge session…
    let hits = index.search(&parse_query("zebrajudge"), None, 10).unwrap();
    assert!(hits.is_empty(), "auxiliary session leaked into corpus search: {hits:?}");
    // …but an explicit origin facet opts in…
    let hits = index.search(&parse_query("zebrajudge origin:auxiliary"), None, 10).unwrap();
    assert_eq!(hits.len(), 1);
    // …and conversation-scoped find still greps it.
    let r = index
        .find(judge.conversation.id.as_str(), &parse_query("zebrajudge"), 10)
        .unwrap();
    assert_eq!(r.total_turn_hits, 1);
}
