pub mod goose;
pub mod claude;
pub mod pi;
pub mod perception;

use crate::query::RecallQuery;
use crate::results::SourceResults;
use async_trait::async_trait;

/// Trait that every data source must implement.
/// Each source is responsible for searching its own data store.
#[async_trait]
pub trait MemorySource: Send + Sync {
    /// Human-readable name for this source
    fn name(&self) -> &str;

    /// Whether this source is available (e.g., database file exists)
    fn is_available(&self) -> bool;

    /// Search this source for matching memories
    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults>;
}

/// Discover and return all available memory sources
pub fn discover_sources() -> Vec<Box<dyn MemorySource>> {
    let mut sources: Vec<Box<dyn MemorySource>> = vec![
        Box::new(goose::GooseSource::new()),
        Box::new(claude::ClaudeSource::new()),
        Box::new(pi::PiSource::new()),
        Box::new(perception::PerceptionSource::new()),
    ];

    // Only keep sources that are actually available
    sources.retain(|s| s.is_available());
    sources
}
