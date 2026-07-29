//! SearchIndex: one corpus-wide tantivy index, one doc per turn.
//!
//! Incremental contract: `apply` deletes the conversation's docs from the
//! parse watermark (`replace_from`) and re-adds the tail — identical to the
//! store's tail refresh, idempotent under crash-redo.

use crate::excerpt::{excerpt_for_terms, Highlight, EXCERPT_MAX_CHARS};
use crate::query::{facet_glob_regex, ParsedQuery};
use crate::schema::{build_schema, Fields, INDEX_SCHEMA_VERSION};
use anyhow::{Context, Result};
use rogrep_parsers::driver::DriverOutput;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, Occur, PhraseQuery, Query, RangeQuery, RegexQuery, TermQuery};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::{doc, Index, IndexWriter, TantivyDocument, Term};

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
        let (field, term_value): (tantivy::schema::Field, String) = match key {
            "provider" | "agent" => (self.fields.provider, value.to_string()),
            "origin" => (self.fields.origin, value.to_string()),
            "model" => (self.fields.model, value.to_string()),
            "project" => (self.fields.project, value.to_string()),
            "cwd" => (self.fields.cwd, value.to_string()),
            "file" => (self.fields.file, value.to_string()),
            "role" => (self.fields.role, value.to_string()),
            _ => (self.fields.turn_facets, format!("{key}:{value}")),
        };
        if let Some(re) = facet_glob_regex(&term_value) {
            return RegexQuery::from_pattern(&re, field)
                .ok()
                .map(|q| Box::new(q) as Box<dyn Query>);
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
        for term in parsed.terms.iter().chain(parsed.phrases.iter()) {
            if let Some(q) = self.text_matcher(term) {
                clauses.push((Occur::Must, q));
            }
        }
        for (key, value) in &parsed.facets {
            if let Some(q) = self.facet_clause(key, value) {
                clauses.push((Occur::Must, q));
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
        }
        let scaffold = usize::from(conversation_id.is_some()) + usize::from(visible_only);
        if clauses.len() <= scaffold {
            return None;
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
    ) -> Result<Vec<ConversationMatches>> {
        let Some(query) = self.build_query(parsed, None, ts_range, true) else {
            return Ok(vec![]);
        };
        let searcher = self.searcher()?;
        let (top, total) = searcher
            .search(&query, &(TopDocs::with_limit(candidate_docs.max(1)).order_by_score(), Count))
            .context("search")?;
        let mut grouped: BTreeMap<String, ConversationMatches> = BTreeMap::new();
        let highlight = parsed.highlight_terms();
        for (score, addr) in top {
            let doc: TantivyDocument = searcher.doc(addr)?;
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
                let text = doc
                    .get_first(self.fields.text)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let (excerpt, highlights) = excerpt_for_terms(text, &highlight, EXCERPT_MAX_CHARS);
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
        let _ = total;
        Ok(out)
    }

    /// Conversation-scoped find with the three-tier result model.
    pub fn find(
        &self,
        conversation_id: &str,
        parsed: &ParsedQuery,
        limit: usize,
    ) -> Result<FindResult> {
        let searcher = self.searcher()?;
        let highlight = parsed.highlight_terms();
        let mut result = FindResult::default();

        // Tier 1: strict AND turn hits.
        if let Some(query) = self.build_query(parsed, Some(conversation_id), None, false) {
            let top = searcher.search(&query, &TopDocs::with_limit(10_000).order_by_score())?;
            let mut hits = Vec::new();
            for (score, addr) in top {
                let doc: TantivyDocument = searcher.doc(addr)?;
                let text = doc
                    .get_first(self.fields.text)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let (excerpt, highlights) = excerpt_for_terms(text, &highlight, EXCERPT_MAX_CHARS);
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

        // Per-term hit sets (tier 2 + 3). Facets participate as constraints.
        let text_needles: Vec<String> = parsed
            .terms
            .iter()
            .chain(parsed.phrases.iter())
            .cloned()
            .collect();
        if text_needles.len() > 1 && result.turn_hits.is_empty() {
            let mut per_term_exchanges: Vec<HashSet<u32>> = Vec::new();
            for needle in &text_needles {
                let single = ParsedQuery {
                    terms: vec![needle.clone()],
                    phrases: vec![],
                    facets: parsed.facets.clone(),
                    dates: vec![],
                };
                let mut exchanges = HashSet::new();
                if let Some(q) = self.build_query(&single, Some(conversation_id), None, false) {
                    for (_score, addr) in searcher.search(&q, &TopDocs::with_limit(10_000).order_by_score())? {
                        let doc: TantivyDocument = searcher.doc(addr)?;
                        if let Some(ex) = doc
                            .get_first(self.fields.exchange_ordinal)
                            .and_then(|v| v.as_u64())
                        {
                            exchanges.insert(ex as u32);
                        }
                    }
                }
                per_term_exchanges.push(exchanges);
            }
            // Exchanges containing ALL terms (somewhere).
            let mut intersection: Vec<u32> = per_term_exchanges
                .iter()
                .skip(1)
                .fold(per_term_exchanges[0].clone(), |acc, s| {
                    acc.intersection(s).copied().collect()
                })
                .into_iter()
                .collect();
            intersection.sort_unstable();
            intersection.truncate(limit);
            result.passage_exchanges = intersection;
        }
        // Tier 3: per-term counts.
        for needle in &text_needles {
            let single = ParsedQuery {
                terms: vec![needle.clone()],
                phrases: vec![],
                facets: vec![],
                dates: vec![],
            };
            let count = match self.build_query(&single, Some(conversation_id), None, false) {
                Some(q) => searcher.search(&q, &Count)?,
                None => 0,
            };
            result.term_counts.push((needle.clone(), count));
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
            let mut doc = doc!(
                self.fields.text => t.text.as_str(),
                self.fields.doc_key => format!("{cid}:{}", t.turn_index),
                self.fields.conversation_id => cid,
                self.fields.turn_index => t.turn_index as u64,
                self.fields.exchange_ordinal => exchange_base + exchange_for(t.turn_index),
                self.fields.role => t.role.as_str(),
                self.fields.visible => if rogrep_model::is_visible_turn(t) { "true" } else { "false" },
                self.fields.project => conv.normalized_project.as_str(),
                self.fields.provider => conv.agent.as_str(),
                self.fields.origin => conv.origin.as_str(),
            );
            if let Some(ts) = t.ts {
                doc.add_i64(self.fields.ts, ts);
            }
            if let Some(cwd) = &t.cwd {
                doc.add_text(self.fields.cwd, cwd);
            }
            if let Some(model) = &t.model {
                doc.add_text(self.fields.model, model);
            }
            for f in facets {
                doc.add_text(self.fields.turn_facets, &f);
            }
            for (path, _mode) in rogrep_tooltree::facets::file_refs_for_turn(t) {
                doc.add_text(self.fields.file, &path);
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

#[derive(Clone, Debug, Default, Serialize)]
pub struct FindResult {
    pub turn_hits: Vec<TurnHit>,
    pub total_turn_hits: usize,
    /// Exchanges where every term appears somewhere (tier 2).
    pub passage_exchanges: Vec<u32>,
    /// Per-term match counts in the conversation (tier 3 / diagnostics).
    pub term_counts: Vec<(String, usize)>,
}
