//! Indexer wiring. Until M3 lands the tantivy index, this returns a no-op.

use anyhow::Result;
use rogrep_engine::{Indexer, NoopIndexer};
use rogrep_model::config::Config;
use rogrep_model::paths::DataLayout;

pub fn open_indexer(_layout: &DataLayout, _config: &Config) -> Result<Box<dyn Indexer>> {
    Ok(Box::new(NoopIndexer))
}
