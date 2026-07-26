pub mod amp;
pub mod claude;
pub mod codex;
pub mod gemini;
pub mod goose;
pub mod opencode;
pub mod pi;

use crate::query::RecallQuery;
use crate::results::SourceResults;
use crate::types::SourceKind;
use async_trait::async_trait;

/// Trait that every data source must implement.
/// Each source is responsible for searching its own data store.
#[async_trait]
pub trait MemorySource: Send + Sync {
    /// The typed session kind this source represents.
    fn kind(&self) -> SourceKind;

    /// Human-readable name for this source.
    fn name(&self) -> &str {
        self.kind().display_name()
    }

    /// Whether this source is available (e.g., database file exists)
    fn is_available(&self) -> bool;

    /// Search this source for matching memories
    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults>;
}

/// Construct the concrete source for a given session type.
fn make_source(kind: SourceKind) -> Box<dyn MemorySource> {
    match kind {
        SourceKind::Goose => Box::new(goose::GooseSource::new()),
        SourceKind::Claude => Box::new(claude::ClaudeSource::new()),
        SourceKind::Pi => Box::new(pi::PiSource::new()),
        SourceKind::Codex => Box::new(codex::CodexSource::new()),
        SourceKind::Gemini => Box::new(gemini::GeminiSource::new()),
        SourceKind::Amp => Box::new(amp::AmpSource::new()),
        SourceKind::OpenCode => Box::new(opencode::OpenCodeSource::new()),
    }
}

/// Construct one instance of every known source, regardless of availability.
///
/// Driven by [`SourceKind::ALL`] so the source registry can never silently
/// drift out of sync with the set of known session types.
pub fn all_sources() -> Vec<Box<dyn MemorySource>> {
    SourceKind::ALL.into_iter().map(make_source).collect()
}

/// Discover the available memory sources, optionally restricted to the session
/// types requested by the query's `--source` filter.
pub fn discover_sources(query: &RecallQuery) -> Vec<Box<dyn MemorySource>> {
    let mut sources = all_sources();
    // Keep sources that are available AND wanted by the query's source filter.
    sources.retain(|s| s.is_available() && query.wants_source(s.kind()));
    sources
}
