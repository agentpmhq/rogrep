//! Rollout discovery and provider parsers.
//!
//! Adding provider #N: one module in `providers/`, one registry entry, an
//! `AgentKind` variant, fixtures + snapshot tests. See docs/providers/.

pub mod discovery;
pub mod driver;
pub mod providers;
pub mod reader;
pub mod record;
pub mod special;
pub mod spool;
pub mod state;

pub use discovery::{discover_files, DiscoveredFile};
pub use driver::{parse_source, DriverOutput, Provider, RolloutParser, SourceInfo};
pub use providers::{provider_for_kind, provider_for_path, registry};
pub use state::ParseState;
