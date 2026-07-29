//! Sync orchestration: ties discovery, parsing, the store, and the search
//! index together behind a single `sync()` entry point.

pub mod sync;

pub use sync::{sync, Indexer, NoopIndexer, SyncEvent, SyncOptions, SyncReport};
