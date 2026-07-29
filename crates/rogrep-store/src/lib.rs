//! SQLite metadata/stats store. All contents are derived data — schema
//! version bumps wipe and rebuild, never migrate.

pub mod schema;
pub mod stats;
pub mod store;

pub use store::{ConversationRow, ExchangeRow, FileCheckpoint, Store};
