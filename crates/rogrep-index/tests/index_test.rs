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

    let hits = index.search(&parse_query("flaky"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 2, "both fixtures mention flaky");

    let hits = index.search(&parse_query("tokenizer offsets crlf"), None, 100).unwrap().0;
    assert!(hits.is_empty() || hits.iter().all(|h| h.match_count == 0) == false);

    let hits = index.search(&parse_query("retry network"), None, 100).unwrap().0;
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
    let hits = index.search(&parse_query("tool_status:failed"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 2);
    // tool:bash only in the claude fixture.
    let hits = index.search(&parse_query("tool:bash"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].conversation_id, claude.conversation.id.as_str());
    // provider facet.
    let hits = index.search(&parse_query("provider:codex tool_status:rejected"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1);
    // tool_cmd from shell parsing (claude's Bash and codex's exec_command
    // both ran cargo).
    let hits = index.search(&parse_query("tool_cmd:cargo"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 2);
    let hits = index.search(&parse_query("tool_cmd:rg"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1, "only codex ran rg");
}

#[test]
fn phrase_and_literal_colon_queries() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude]);
    let hits = index.search(&parse_query("\"offsets drift on crlf\""), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1);
    let hits = index.search(&parse_query("\"drift crlf on\""), None, 100).unwrap().0;
    assert!(hits.is_empty(), "phrase order matters");
}

#[test]
fn find_three_tiers() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let cid = claude.conversation.id.as_str().to_string();
    let (_tmp, index) = build_index(&[&claude]);

    // Strict: both terms in one turn.
    let r = index.find(&cid, &parse_query("offsets drift"), None, 50).unwrap();
    assert_eq!(r.total_turn_hits, 1);
    assert!(r.turn_hits[0].excerpt.to_lowercase().contains("offsets drift"));

    // Passage: terms in different turns of the same exchange ("parser" only
    // in the user prompt, "crlf" only in the closing assistant turn).
    let r = index.find(&cid, &parse_query("parser crlf"), None, 50).unwrap();
    assert_eq!(r.total_turn_hits, 0);
    assert_eq!(r.passage_exchanges, vec![0], "both in exchange 0");

    // Per-term counts always present.
    let r = index.find(&cid, &parse_query("flaky nonexistentterm"), None, 50).unwrap();
    let counts: std::collections::HashMap<_, _> = r.term_counts.iter().cloned().collect();
    assert!(counts["flaky"] > 0);
    assert_eq!(counts["nonexistentterm"], 0);
}

#[test]
fn regex_queries_match_across_token_boundaries() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let codex = parse_bytes(AgentKind::Codex, "b.jsonl", &fixture("codex/session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude, &codex]);

    // Case-sensitive by default.
    let hits = index.search(&parse_query("/flaky/"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 2);
    let hits = index.search(&parse_query("/FLAKY/"), None, 100).unwrap().0;
    assert!(hits.is_empty(), "regexes are case-sensitive");
    let hits = index.search(&parse_query("/(?i)FLAKY/"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 2, "(?i) opts into case-insensitivity");

    // Patterns cross tokenizer boundaries (whitespace via \s, alternation)
    // where a single term query cannot.
    let hits = index.search(&parse_query(r"/offsets\s+drift/"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1);
    let best = hits[0].best.as_ref().unwrap();
    assert!(!best.highlights.is_empty(), "regex matches highlight");

    // Mixed regex + facet narrows.
    let hits = index
        .search(&parse_query("/retry|drift/ provider:codex"), None, 100)
        .unwrap()
        .0;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].conversation_id, codex.conversation.id.as_str());

    // Invalid pattern surfaces a clear error.
    let err = index.search(&parse_query("/re(try/"), None, 100).unwrap_err();
    assert!(err.to_string().contains("invalid regex /re(try/"));
}

#[test]
fn pure_regex_queries_respect_default_scope() {
    let judge = parse_bytes(AgentKind::Codex, "judge.jsonl", &fixture("codex/auto_review.jsonl"), None);
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/edge_cases.jsonl"), None);
    let (_tmp, index) = build_index(&[&judge, &claude]);

    // The AllQuery fallback must not leak auxiliary sessions or invisible
    // turns into corpus results.
    let hits = index.search(&parse_query("/zebrajudge/"), None, 100).unwrap().0;
    assert!(hits.is_empty(), "auxiliary leaked through regex path: {hits:?}");
    let hits = index.search(&parse_query("/injected reminder/"), None, 100).unwrap().0;
    assert!(hits.is_empty(), "invisible turn leaked through regex path: {hits:?}");

    // But find still greps everything.
    let r = index
        .find(judge.conversation.id.as_str(), &parse_query("/zebra[a-z]+/"), None, 10)
        .unwrap();
    assert_eq!(r.total_turn_hits, 1);
}

#[test]
fn regex_facet_values_match_indexed_vocabulary() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let codex = parse_bytes(AgentKind::Codex, "b.jsonl", &fixture("codex/session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude, &codex]);

    // Anchored regex over turn-facet tokens: tool_cmd of cargo or rg.
    let hits = index.search(&parse_query("tool_cmd:/cargo|rg/"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 2);
    let hits = index.search(&parse_query("tool_cmd:/r./"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1, "anchored: matches rg, not cargo");
    // Metadata field regex.
    let hits = index.search(&parse_query("flaky provider:/cod.x/"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1);
}

#[test]
fn find_regex_needles_participate_in_tiers() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let cid = claude.conversation.id.as_str().to_string();
    let (_tmp, index) = build_index(&[&claude]);

    // Passage tier: term and regex in different turns of one exchange (the
    // closing turn says "CRLF", so the regex needs its (?i) flag).
    let r = index.find(&cid, &parse_query("parser /(?i)crlf/"), None, 50).unwrap();
    assert_eq!(r.total_turn_hits, 0);
    assert_eq!(r.passage_exchanges, vec![0]);

    // Per-needle counts include the regex, labeled /pattern/.
    let counts: std::collections::HashMap<_, _> = r.term_counts.iter().cloned().collect();
    assert!(counts["parser"] > 0);
    assert!(counts["/(?i)crlf/"] > 0);
}

#[test]
fn metadata_facets_substring_match() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let codex = parse_bytes(AgentKind::Codex, "b.jsonl", &fixture("codex/session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude, &codex]);

    // model: substring, case-insensitive (fixture models: claude-fable-5,
    // gpt-5.2-codex).
    let hits = index.search(&parse_query("flaky model:Fable"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].conversation_id, claude.conversation.id.as_str());
    let hits = index.search(&parse_query("flaky model:codex"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1);
    // provider: substring too.
    let hits = index.search(&parse_query("flaky provider:code"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1);
    // source: matches the rollout file path.
    let hits = index.search(&parse_query("flaky source:.codex/sessions"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].conversation_id, codex.conversation.id.as_str());
    // project: substring on the normalized key (both fixtures live in
    // -home-u-src-proj).
    let hits = index.search(&parse_query("flaky project:src-proj"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 2);
    // No accidental matches.
    let hits = index.search(&parse_query("flaky model:nonexistent"), None, 100).unwrap().0;
    assert!(hits.is_empty());
}

#[test]
fn file_facet_matches_relative_substring() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/edge_cases.jsonl"), None);
    let codex = parse_bytes(AgentKind::Codex, "b.jsonl", &fixture("codex/session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude, &codex]);
    // The index stores absolute paths; a relative query must still match by
    // substring. Find any file-ref the fixtures produced first.
    let all_refs: Vec<String> = [&claude, &codex]
        .iter()
        .flat_map(|out| &out.conversation.turns)
        .flat_map(|t| rogrep_tooltree::facets::file_refs_for_turn(t))
        .map(|(p, _)| p)
        .collect();
    if let Some(path) = all_refs.first() {
        let tail: String = path.rsplit('/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/");
        let q = format!("file:{}", tail);
        let hits = index.search(&parse_query(&q), None, 100).unwrap().0;
        assert!(!hits.is_empty(), "file:{tail} should substring-match {path}");
    }
}

#[test]
fn tool_type_and_qualifier_facets_indexed() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    let codex = parse_bytes(AgentKind::Codex, "b.jsonl", &fixture("codex/session.jsonl"), None);
    let (_tmp, index) = build_index(&[&claude, &codex]);

    // Both fixtures ran `cargo test …` → tests, local, mutating.
    let hits = index.search(&parse_query("tool_type:tests"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 2);
    let hits = index.search(&parse_query("tool_type:tests tool_location:remote"), None, 100).unwrap().0;
    assert!(hits.is_empty());
    let hits = index.search(&parse_query("tool_type:tests tool_mutability:mutating"), None, 100).unwrap().0;
    assert_eq!(hits.len(), 2);
    // Underscore form normalizes to the dashed vocabulary.
    let hits = index.search(&parse_query("tool_mutability:read_only"), None, 100).unwrap().0;
    let dashed = index.search(&parse_query("tool_mutability:read-only"), None, 100).unwrap().0;
    assert_eq!(hits.len(), dashed.len());
}

#[test]
fn subagent_facet_filters_on_origin() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/basic_session.jsonl"), None);
    assert_eq!(claude.conversation.origin, rogrep_model::Origin::Interactive);
    let (_tmp, index) = build_index(&[&claude]);

    let hits = index.search(&parse_query("flaky subagent:true"), None, 10).unwrap().0;
    assert!(hits.is_empty(), "interactive session is not a subagent");
    let hits = index.search(&parse_query("flaky subagent:false"), None, 10).unwrap().0;
    assert_eq!(hits.len(), 1);
    let hits = index.search(&parse_query("flaky is:subagent"), None, 10).unwrap().0;
    assert!(hits.is_empty());
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
    let hits = index.search(&parse_query("zebra"), None, 10).unwrap().0;
    assert_eq!(hits.len(), 1);
    // The re-added tail keeps content searchable exactly once.
    let hits = index.search(&parse_query("ship"), None, 10).unwrap().0;
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
    let hits = index.search(&parse_query("flaky"), Some(july), 10).unwrap().0;
    assert_eq!(hits.len(), 1);
    let jan = (1_767_225_600_000i64, 1_767_312_000_000i64);
    let hits = index.search(&parse_query("flaky"), Some(jan), 10).unwrap().0;
    assert!(hits.is_empty());
}

#[test]
fn injected_context_excluded_from_corpus_search_but_findable() {
    let claude = parse_bytes(AgentKind::Claude, "a.jsonl", &fixture("claude/edge_cases.jsonl"), None);
    let cid = claude.conversation.id.as_str().to_string();
    let (_tmp, index) = build_index(&[&claude]);

    // The fixture's injected reminder and IDE attachment are invisible turns.
    // Corpus search must not surface conversations through them…
    let hits = index.search(&parse_query("injected reminder"), None, 10).unwrap().0;
    assert!(hits.is_empty(), "injected context leaked into corpus search: {hits:?}");
    let hits = index.search(&parse_query("broken"), None, 10).unwrap().0;
    assert!(hits.is_empty(), "synthetic attachment leaked into corpus search: {hits:?}");

    // …but visible content still matches…
    let hits = index.search(&parse_query("directory work"), None, 10).unwrap().0;
    assert_eq!(hits.len(), 1);

    // …and conversation-scoped find greps EVERYTHING, including context.
    let r = index.find(&cid, &parse_query("injected reminder"), None, 10).unwrap();
    assert_eq!(r.total_turn_hits, 1, "find must keep grep-everything semantics");
    let r = index.find(&cid, &parse_query("broken"), None, 10).unwrap();
    assert_eq!(r.total_turn_hits, 1);
}

#[test]
fn auxiliary_sessions_excluded_from_corpus_search() {
    let judge = parse_bytes(AgentKind::Codex, "judge.jsonl", &fixture("codex/auto_review.jsonl"), None);
    assert_eq!(judge.conversation.origin, rogrep_model::Origin::Auxiliary);
    let (_tmp, index) = build_index(&[&judge]);

    // Default corpus search never surfaces the judge session…
    let hits = index.search(&parse_query("zebrajudge"), None, 10).unwrap().0;
    assert!(hits.is_empty(), "auxiliary session leaked into corpus search: {hits:?}");
    // …but an explicit origin facet opts in…
    let hits = index.search(&parse_query("zebrajudge origin:auxiliary"), None, 10).unwrap().0;
    assert_eq!(hits.len(), 1);
    // …and conversation-scoped find still greps it.
    let r = index
        .find(judge.conversation.id.as_str(), &parse_query("zebrajudge"), None, 10)
        .unwrap();
    assert_eq!(r.total_turn_hits, 1);
}
