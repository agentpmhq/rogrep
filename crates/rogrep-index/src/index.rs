//! SearchIndex: one corpus-wide tantivy index, one doc per turn.
//!
//! Incremental contract: `apply` deletes the conversation's docs from the
//! parse watermark (`replace_from`) and re-adds the tail — identical to the
//! store's tail refresh, idempotent under crash-redo.

use crate::excerpt::{excerpt_for_matchers, Highlight, Matcher, EXCERPT_MAX_CHARS};
use crate::query::{facet_glob_regex, regex_token, ParsedQuery};
use crate::schema::{build_schema, Fields, INDEX_SCHEMA_VERSION};
use anyhow::{Context, Result};
use rogrep_parsers::driver::DriverOutput;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{
    AllQuery, BooleanQuery, Occur, PhraseQuery, Query, RangeQuery, RegexQuery, TermQuery,
};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::{doc, DocAddress, Index, IndexWriter, Order, TantivyDocument, Term};

/// Upper bound on stored-text fetches when a query needs regex
/// post-filtering; regex-only queries scan the most recent turns up to
/// this cap.
pub const REGEX_SCAN_CAP: usize = 20_000;

pub struct SearchIndex {
    pub index: Index,
    pub fields: Fields,
    reader: tantivy::IndexReader,
}

pub fn index_dir(data_root: &Path) -> PathBuf {
    data_root.join(format!("index/v{INDEX_SCHEMA_VERSION}"))
}

impl SearchIndex {
    pub fn open_or_create(dir: &Path) -> Result<SearchIndex> {
        let (schema, fields) = build_schema();
        std::fs::create_dir_all(dir)?;
        let index = match Index::open_in_dir(dir) {
            Ok(index) => {
                if index.schema() != schema {
                    // Derived data: wipe and rebuild on any mismatch.
                    std::fs::remove_dir_all(dir)?;
                    std::fs::create_dir_all(dir)?;
                    Index::create_in_dir(dir, schema)?
                } else {
                    index
                }
            }
            Err(_) => {
                // Fresh or unopenable — recreate.
                let _ = std::fs::remove_dir_all(dir);
                std::fs::create_dir_all(dir)?;
                Index::create_in_dir(dir, schema)?
            }
        };
        let reader = index.reader()?;
        // Retire older schema generations (all derived data). Only touch
        // sibling directories that are unambiguously versioned index dirs
        // (`v<N>`) — the parent may be a shared directory.
        if let (Some(parent), Some(current)) = (dir.parent(), dir.file_name()) {
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let is_version_dir = name
                        .to_str()
                        .and_then(|n| n.strip_prefix('v'))
                        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()));
                    if is_version_dir && name != current && entry.path().is_dir() {
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
        Ok(SearchIndex {
            index,
            fields,
            reader,
        })
    }

    pub fn writer(&self) -> Result<IndexBatch> {
        let writer = self.index.writer(64 * 1024 * 1024)?;
        Ok(IndexBatch {
            writer,
            fields: self.fields.clone(),
            pending: 0,
        })
    }

    pub fn searcher(&self) -> Result<tantivy::Searcher> {
        self.reader.reload()?;
        Ok(self.reader.searcher())
    }

    /// Tokenize text with the index's default analyzer.
    fn tokens(&self, text: &str) -> Vec<String> {
        let mut analyzer = self.index.tokenizers().get("default").expect("default tokenizer");
        let mut stream = analyzer.token_stream(text);
        let mut out = Vec::new();
        while let Some(token) = stream.next() {
            out.push(token.text.clone());
        }
        out
    }

    fn text_matcher(&self, text: &str) -> Option<Box<dyn Query>> {
        let tokens = self.tokens(text);
        match tokens.len() {
            0 => None,
            1 => Some(Box::new(TermQuery::new(
                Term::from_field_text(self.fields.text, &tokens[0]),
                IndexRecordOption::WithFreqsAndPositions,
            ))),
            _ => Some(Box::new(PhraseQuery::new(
                tokens
                    .iter()
                    .map(|t| Term::from_field_text(self.fields.text, t))
                    .collect(),
            ))),
        }
    }

    fn facet_clause(&self, key: &str, value: &str) -> Option<Box<dyn Query>> {
        // Metadata facets substring-match (agentpm semantics: `model:sonnet`
        // matches `claude-sonnet-4`); vocabulary facets are exact tokens.
        enum Kind {
            Substring,
            Exact,
            Token,
        }
        let (field, kind) = match key {
            "provider" | "agent" => (self.fields.provider, Kind::Substring),
            "model" => (self.fields.model, Kind::Substring),
            "project" => (self.fields.project, Kind::Substring),
            "cwd" => (self.fields.cwd, Kind::Substring),
            "file" => (self.fields.file, Kind::Substring),
            "source" => (self.fields.source, Kind::Substring),
            "origin" => (self.fields.origin, Kind::Exact),
            "role" => (self.fields.role, Kind::Exact),
            _ => (self.fields.turn_facets, Kind::Token),
        };
        // A `/pattern/` facet value regex-matches the whole indexed value
        // (tantivy term regexes are anchored). An invalid pattern falls
        // through to a TermQuery on the raw value, which matches nothing.
        if let Some(pat) = regex_token(value) {
            let pattern = match kind {
                Kind::Token => format!("{key}:(?:{pat})"),
                _ => pat.to_string(),
            };
            if let Ok(q) = RegexQuery::from_pattern(&pattern, field) {
                return Some(Box::new(q));
            }
        }
        // Central normalization: synthesized queries (trajectory's --branch,
        // --project flags) reach here without going through parse_query.
        let value = crate::query::normalize_facet_value(key, value);
        let term_value = match kind {
            Kind::Token => format!("{key}:{value}"),
            _ => value.clone(),
        };
        if let Some(re) = facet_glob_regex(&term_value) {
            return RegexQuery::from_pattern(&re, field)
                .ok()
                .map(|q| Box::new(q) as Box<dyn Query>);
        }
        if matches!(kind, Kind::Substring) {
            let pattern = format!("(?s).*{}.*", regex::escape(&value));
            if let Ok(q) = RegexQuery::from_pattern(&pattern, field) {
                return Some(Box::new(q));
            }
        }
        Some(Box::new(TermQuery::new(
            Term::from_field_text(field, &term_value),
            IndexRecordOption::Basic,
        )))
    }

    /// Build the boolean MUST query for a parsed query (+ optional scope +
    /// ts range).
    fn build_query(
        &self,
        parsed: &ParsedQuery,
        conversation_id: Option<&str>,
        ts_range: Option<(i64, i64)>,
        visible_only: bool,
    ) -> Option<Box<dyn Query>> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if visible_only {
            // Keep harness-injected context (skill catalogs, system prompts,
            // env blocks) out of corpus results — it conjunctively matches
            // almost anything.
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.visible, "true"),
                    IndexRecordOption::Basic,
                )),
            ));
            // Auxiliary sessions (codex auto-review judges) are machine
            // evaluation, not user work — excluded unless the query names an
            // origin explicitly.
            if !parsed.facets.iter().any(|(k, _)| k == "origin") {
                clauses.push((
                    Occur::MustNot,
                    Box::new(TermQuery::new(
                        Term::from_field_text(self.fields.origin, "auxiliary"),
                        IndexRecordOption::Basic,
                    )),
                ));
            }
        }
        if let Some(cid) = conversation_id {
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.conversation_id, cid),
                    IndexRecordOption::Basic,
                )),
            ));
        }
        let mut real = 0usize; // clauses beyond the visible/aux/cid scaffold
        let mut negated = 0usize;
        for term in parsed.terms.iter().chain(parsed.phrases.iter()) {
            if let Some(q) = self.text_matcher(term) {
                clauses.push((Occur::Must, q));
                real += 1;
            }
        }
        for (key, value) in &parsed.facets {
            // subagent:/is:subagent map onto the origin field (subagent is
            // an origin value, not an emitted turn-facet token).
            if key == "subagent" || (key == "is" && value == "subagent") {
                let truthy =
                    key == "is" || matches!(value.as_str(), "true" | "1" | "yes" | "subagent");
                let falsey = matches!(value.as_str(), "false" | "0" | "no" | "normal");
                let term: Box<dyn Query> = Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.origin, "subagent"),
                    IndexRecordOption::Basic,
                ));
                if truthy {
                    clauses.push((Occur::Must, term));
                    real += 1;
                } else if falsey {
                    clauses.push((Occur::MustNot, term));
                    negated += 1;
                }
                continue;
            }
            if let Some(q) = self.facet_clause(key, value) {
                clauses.push((Occur::Must, q));
                real += 1;
            }
        }
        if let Some((from, to)) = ts_range {
            clauses.push((
                Occur::Must,
                Box::new(RangeQuery::new(
                    Bound::Included(Term::from_field_i64(self.fields.ts, from)),
                    Bound::Excluded(Term::from_field_i64(self.fields.ts, to)),
                )),
            ));
            real += 1;
        }
        if real == 0 {
            // Regexes are enforced by post-filtering stored text, and
            // MustNot clauses match nothing on their own — both need a
            // match-all base to select candidates from.
            if parsed.regexes.is_empty() && negated == 0 {
                return None;
            }
            clauses.push((Occur::Must, Box::new(AllQuery)));
        }
        Some(Box::new(BooleanQuery::new(clauses)))
    }

    /// Corpus search: BM25 over turns, grouped per conversation, recency
    /// decay applied by the caller (which owns "now").
    pub fn search(
        &self,
        parsed: &ParsedQuery,
        ts_range: Option<(i64, i64)>,
        candidate_docs: usize,
    ) -> Result<(Vec<ConversationMatches>, SearchMeta)> {
        let mut meta = SearchMeta::default();
        let regexes = parsed.compile_regexes()?;
        let Some(query) = self.build_query(parsed, None, ts_range, true) else {
            return Ok((vec![], meta));
        };
        let searcher = self.searcher()?;
        // Regexes match full stored text, which term queries can't narrow —
        // widen the candidate pool up to the scan cap and post-filter.
        let limit = if regexes.is_empty() {
            candidate_docs.max(1)
        } else {
            candidate_docs.max(REGEX_SCAN_CAP)
        };
        // A regex-only query has no scoring signal (every clause is a
        // filter), so take the most recent turns instead of BM25 order.
        let unranked =
            !regexes.is_empty() && parsed.terms.is_empty() && parsed.phrases.is_empty();
        let (top, total): (Vec<(f32, DocAddress)>, usize) = if unranked {
            let collector = TopDocs::with_limit(limit).order_by_fast_field::<i64>("ts", Order::Desc);
            let (hits, total) = searcher.search(&query, &(collector, Count)).context("search")?;
            (hits.into_iter().map(|(_ts, addr)| (1.0, addr)).collect(), total)
        } else {
            searcher
                .search(&query, &(TopDocs::with_limit(limit).order_by_score(), Count))
                .context("search")?
        };
        meta.scan_capped = !regexes.is_empty() && total > limit;
        let mut grouped: BTreeMap<String, ConversationMatches> = BTreeMap::new();
        let mut matchers: Vec<Matcher> =
            parsed.highlight_terms().into_iter().map(Matcher::Literal).collect();
        matchers.extend(regexes.iter().cloned().map(Matcher::Regex));
        for (score, addr) in top {
            let doc: TantivyDocument = searcher.doc(addr)?;
            let text = doc
                .get_first(self.fields.text)
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if !regexes.iter().all(|re| re.is_match(text)) {
                continue;
            }
            let cid = doc
                .get_first(self.fields.conversation_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let turn_index = doc
                .get_first(self.fields.turn_index)
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let exchange_ordinal = doc
                .get_first(self.fields.exchange_ordinal)
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let ts = doc.get_first(self.fields.ts).and_then(|v| v.as_i64());
            let entry = grouped.entry(cid.clone()).or_insert_with(|| ConversationMatches {
                conversation_id: cid,
                match_count: 0,
                best_score: 0.0,
                last_ts: None,
                best: None,
            });
            entry.match_count += 1;
            if ts > entry.last_ts {
                entry.last_ts = ts;
            }
            if score > entry.best_score || entry.best.is_none() {
                entry.best_score = score;
                let (excerpt, highlights) = excerpt_for_matchers(text, &matchers, EXCERPT_MAX_CHARS);
                entry.best = Some(TurnHit {
                    turn_index,
                    exchange_ordinal,
                    ts,
                    score,
                    excerpt,
                    highlights,
                });
            }
        }
        let mut out: Vec<ConversationMatches> = grouped.into_values().collect();
        out.sort_by(|a, b| b.best_score.total_cmp(&a.best_score));
        Ok((out, meta))
    }

    /// Conversation-scoped find with the three-tier result model.
    pub fn find(
        &self,
        conversation_id: &str,
        parsed: &ParsedQuery,
        ts_range: Option<(i64, i64)>,
        limit: usize,
    ) -> Result<FindResult> {
        let searcher = self.searcher()?;
        let regexes = parsed.compile_regexes()?;
        let mut matchers: Vec<Matcher> =
            parsed.highlight_terms().into_iter().map(Matcher::Literal).collect();
        matchers.extend(regexes.iter().cloned().map(Matcher::Regex));
        let mut result = FindResult::default();

        // Tier 1: strict AND turn hits.
        if let Some(query) = self.build_query(parsed, Some(conversation_id), ts_range, false) {
            let top = searcher.search(&query, &TopDocs::with_limit(10_000).order_by_score())?;
            let mut hits = Vec::new();
            for (score, addr) in top {
                let doc: TantivyDocument = searcher.doc(addr)?;
                let text = doc
                    .get_first(self.fields.text)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if !regexes.iter().all(|re| re.is_match(text)) {
                    continue;
                }
                let (excerpt, highlights) = excerpt_for_matchers(text, &matchers, EXCERPT_MAX_CHARS);
                hits.push(TurnHit {
                    turn_index: doc
                        .get_first(self.fields.turn_index)
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    exchange_ordinal: doc
                        .get_first(self.fields.exchange_ordinal)
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    ts: doc.get_first(self.fields.ts).and_then(|v| v.as_i64()),
                    score,
                    excerpt,
                    highlights,
                });
            }
            hits.sort_by_key(|h| h.turn_index);
            result.total_turn_hits = hits.len();
            hits.truncate(limit);
            result.turn_hits = hits;
        }

        // Per-needle hit sets (tier 2 + 3): every term, phrase, and regex
        // is one needle. Facets participate as constraints in tier 2.
        struct Needle {
            label: String,
            query: ParsedQuery,
            filter: Option<regex::Regex>,
        }
        let mut needles: Vec<Needle> = parsed
            .terms
            .iter()
            .chain(parsed.phrases.iter())
            .map(|t| Needle {
                label: t.clone(),
                query: ParsedQuery { terms: vec![t.clone()], ..Default::default() },
                filter: None,
            })
            .collect();
        needles.extend(parsed.regexes.iter().zip(&regexes).map(|(pat, re)| Needle {
            label: format!("/{pat}/"),
            query: ParsedQuery { regexes: vec![pat.clone()], ..Default::default() },
            filter: Some(re.clone()),
        }));

        // Matching exchange ordinals for one needle under the given extra
        // facet constraints.
        let exchanges_for = |needle: &Needle, facets: &[(String, String)]| -> Result<HashSet<u32>> {
            let mut single = needle.query.clone();
            single.facets = facets.to_vec();
            let mut exchanges = HashSet::new();
            if let Some(q) = self.build_query(&single, Some(conversation_id), ts_range, false) {
                for (_score, addr) in
                    searcher.search(&q, &TopDocs::with_limit(10_000).order_by_score())?
                {
                    let doc: TantivyDocument = searcher.doc(addr)?;
                    if let Some(re) = &needle.filter {
                        let text = doc
                            .get_first(self.fields.text)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        if !re.is_match(text) {
                            continue;
                        }
                    }
                    if let Some(ex) = doc
                        .get_first(self.fields.exchange_ordinal)
                        .and_then(|v| v.as_u64())
                    {
                        exchanges.insert(ex as u32);
                    }
                }
            }
            Ok(exchanges)
        };

        if needles.len() > 1 && result.turn_hits.is_empty() {
            let mut per_needle_exchanges: Vec<HashSet<u32>> = Vec::new();
            for needle in &needles {
                per_needle_exchanges.push(exchanges_for(needle, &parsed.facets)?);
            }
            // Exchanges containing ALL needles (somewhere).
            let mut intersection: Vec<u32> = per_needle_exchanges
                .iter()
                .skip(1)
                .fold(per_needle_exchanges[0].clone(), |acc, s| {
                    acc.intersection(s).copied().collect()
                })
                .into_iter()
                .collect();
            intersection.sort_unstable();
            intersection.truncate(limit);
            result.passage_exchanges = intersection;
        }
        // Tier 3: per-needle turn counts (no facet constraints).
        for needle in &needles {
            let count = if let Some(re) = &needle.filter {
                let mut n = 0;
                if let Some(q) = self.build_query(&needle.query, Some(conversation_id), ts_range, false)
                {
                    for (_score, addr) in
                        searcher.search(&q, &TopDocs::with_limit(10_000).order_by_score())?
                    {
                        let doc: TantivyDocument = searcher.doc(addr)?;
                        let text = doc
                            .get_first(self.fields.text)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        if re.is_match(text) {
                            n += 1;
                        }
                    }
                }
                n
            } else {
                match self.build_query(&needle.query, Some(conversation_id), ts_range, false) {
                    Some(q) => searcher.search(&q, &Count)?,
                    None => 0,
                }
            };
            result.term_counts.push((needle.label.clone(), count));
        }
        Ok(result)
    }

    /// Total docs in the index (doctor).
    pub fn doc_count(&self) -> Result<u64> {
        Ok(self.searcher()?.num_docs())
    }
}

pub struct IndexBatch {
    writer: IndexWriter,
    fields: Fields,
    pending: usize,
}

impl IndexBatch {
    /// Apply one parse output: tail refresh (delete docs >= watermark) then
    /// add the new turns.
    pub fn apply(&mut self, out: &DriverOutput) -> Result<()> {
        let conv = &out.conversation;
        let cid = conv.id.as_str();
        let exchanges = rogrep_model::build_exchanges(&conv.turns);
        let exchange_for = |turn_index: u32| -> u64 {
            exchanges
                .iter()
                .find(|e| turn_index >= e.start_turn && turn_index < e.end_turn)
                .map(|e| e.ordinal as u64)
                .unwrap_or(0)
        };

        // Tail refresh delete: conversation AND turn_index >= replace_from.
        let delete: Box<dyn Query> = Box::new(BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.conversation_id, cid),
                    IndexRecordOption::Basic,
                )) as Box<dyn Query>,
            ),
            (
                Occur::Must,
                Box::new(RangeQuery::new(
                    Bound::Included(Term::from_field_u64(
                        self.fields.turn_index,
                        out.replace_from as u64,
                    )),
                    Bound::Unbounded,
                )) as Box<dyn Query>,
            ),
        ]));
        self.writer.delete_query(delete)?;

        // Pair tool outputs back to their commands for output-mined facets.
        let mut command_facets_by_pair: HashMap<&str, Vec<String>> = HashMap::new();
        for t in &conv.turns {
            if let Some(info) = &t.tool {
                if info.direction == Some(rogrep_model::ToolDirection::Use) {
                    if let Some(pair) = &info.pair_id {
                        command_facets_by_pair
                            .insert(pair.as_str(), rogrep_tooltree::facet_tokens_for_turn(t));
                    }
                }
            }
        }

        let exchange_base = out.exchange_base as u64;
        for t in &conv.turns {
            let mut facets = rogrep_tooltree::facet_tokens_for_turn(t);
            if let Some(info) = &t.tool {
                if info.direction == Some(rogrep_model::ToolDirection::Output) {
                    if let Some(pair) = &info.pair_id {
                        let empty = Vec::new();
                        let cmd = command_facets_by_pair.get(pair.as_str()).unwrap_or(&empty);
                        facets.extend(rogrep_tooltree::output_facet_tokens(&t.text, cmd));
                    }
                }
            }
            // Metadata fields are lowercased at write so substring facet
            // matching (`model:sonnet`) is case-insensitive, like agentpm.
            let mut doc = doc!(
                self.fields.text => t.text.as_str(),
                self.fields.doc_key => format!("{cid}:{}", t.turn_index),
                self.fields.conversation_id => cid,
                self.fields.turn_index => t.turn_index as u64,
                self.fields.exchange_ordinal => exchange_base + exchange_for(t.turn_index),
                self.fields.role => t.role.as_str(),
                self.fields.visible => if rogrep_model::is_visible_turn(t) { "true" } else { "false" },
                self.fields.project => conv.normalized_project.to_lowercase(),
                self.fields.provider => conv.agent.as_str().to_lowercase(),
                self.fields.origin => conv.origin.as_str(),
                self.fields.source => conv.source_path.to_lowercase(),
            );
            if let Some(ts) = t.ts {
                doc.add_i64(self.fields.ts, ts);
            }
            // Per-turn value when present, conversation-level fallback so
            // model:/cwd: behave as conversation attributes (agentpm
            // matches them on the conversation record).
            if let Some(cwd) = t.cwd.as_ref().or(conv.cwd.as_ref()) {
                doc.add_text(self.fields.cwd, cwd.to_lowercase());
            }
            if let Some(model) = t.model.as_ref().or(conv.model.as_ref()) {
                doc.add_text(self.fields.model, model.to_lowercase());
            }
            for f in facets {
                doc.add_text(self.fields.turn_facets, &f);
            }
            for (path, _mode) in rogrep_tooltree::facets::file_refs_for_turn(t) {
                doc.add_text(self.fields.file, path.to_lowercase());
            }
            self.writer.add_document(doc)?;
            self.pending += 1;
        }
        Ok(())
    }

    pub fn remove_conversation(&mut self, conversation_id: &str) -> Result<()> {
        self.writer.delete_term(Term::from_field_text(
            self.fields.conversation_id,
            conversation_id,
        ));
        self.pending += 1;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<()> {
        if self.pending > 0 {
            self.writer.commit()?;
            self.pending = 0;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TurnHit {
    pub turn_index: u32,
    pub exchange_ordinal: u32,
    pub ts: Option<i64>,
    pub score: f32,
    pub excerpt: String,
    pub highlights: Vec<Highlight>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationMatches {
    pub conversation_id: String,
    pub match_count: usize,
    pub best_score: f32,
    pub last_ts: Option<i64>,
    pub best: Option<TurnHit>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct SearchMeta {
    /// A regex post-filter ran against a capped candidate pool; older
    /// matches beyond the cap were not scanned.
    pub scan_capped: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FindResult {
    pub turn_hits: Vec<TurnHit>,
    pub total_turn_hits: usize,
    /// Exchanges where every term appears somewhere (tier 2).
    pub passage_exchanges: Vec<u32>,
    /// Per-term match counts in the conversation (tier 3 / diagnostics).
    pub term_counts: Vec<(String, usize)>,
}
