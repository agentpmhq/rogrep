//! Tantivy search index and query grammar.

pub mod excerpt;
pub mod index;
pub mod query;
pub mod schema;

pub use excerpt::{excerpt_for_matchers, Highlight, Matcher};
pub use index::{
    index_dir, ConversationMatches, FindResult, IndexBatch, SearchIndex, SearchMeta, TurnHit,
    REGEX_SCAN_CAP,
};
pub use query::{parse_query, ParsedQuery};
pub use schema::INDEX_SCHEMA_VERSION;
