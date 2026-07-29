//! Wires the tantivy index into the sync pipeline.

use anyhow::Result;
use rogrep_engine::Indexer;
use rogrep_index::{index_dir, IndexBatch, SearchIndex};
use rogrep_model::config::Config;
use rogrep_model::paths::DataLayout;
use rogrep_parsers::driver::DriverOutput;

pub struct TantivyIndexer {
    batch: IndexBatch,
}

pub fn open_search_index(layout: &DataLayout) -> Result<SearchIndex> {
    SearchIndex::open_or_create(&index_dir(&layout.root))
}

pub fn open_indexer(layout: &DataLayout, _config: &Config) -> Result<Box<dyn Indexer>> {
    let index = open_search_index(layout)?;
    let batch = index.writer()?;
    Ok(Box::new(TantivyIndexer { batch }))
}

impl Indexer for TantivyIndexer {
    fn apply(&mut self, out: &DriverOutput) -> Result<()> {
        self.batch.apply(out)
    }

    fn remove_conversation(&mut self, conversation_id: &str) -> Result<()> {
        self.batch.remove_conversation(conversation_id)
    }

    fn commit(&mut self) -> Result<()> {
        self.batch.commit()
    }

    fn generation(&self) -> String {
        format!("tantivy-v{}", rogrep_index::INDEX_SCHEMA_VERSION)
    }
}
