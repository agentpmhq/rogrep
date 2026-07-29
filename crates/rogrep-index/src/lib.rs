//! Tantivy search index and query grammar.

pub mod excerpt;
pub mod index;
pub mod query;
pub mod schema;

pub use excerpt::{excerpt_for_terms, Highlight};
pub use index::{index_dir, ConversationMatches, FindResult, IndexBatch, SearchIndex, TurnHit};
pub use query::{parse_query, ParsedQuery};
pub use schema::INDEX_SCHEMA_VERSION;
