use crate::cache::ResultCache;
use crate::query::RecallQuery;
use crate::results::{RecallResults, SourceResults};
use crate::sources;
use futures::future::join_all;
use std::time::Instant;

/// The recall engine: discovers sources, fans out queries in parallel, aggregates results
pub struct RecallEngine {
    cache: ResultCache,
}

impl RecallEngine {
    pub fn new() -> Self {
        Self {
            cache: ResultCache::new(),
        }
    }

    /// Execute a recall query across all available sources in parallel
    pub async fn recall(&self, query: &RecallQuery) -> RecallResults {
        let total_start = Instant::now();

        // Check cache first
        let cache_key = query.cache_key();
        if let Some(cached) = self.cache.get(&cache_key) {
            return cached;
        }

        // Discover available sources
        let sources = sources::discover_sources();
        
        if sources.is_empty() {
            return RecallResults {
                query_summary: format_query_summary(query),
                sources: vec![],
                total_results: 0,
                total_time_ms: total_start.elapsed().as_millis() as u64,
                from_cache: false,
            };
        }

        // Fan out to all sources in parallel
        let futures: Vec<_> = sources
            .iter()
            .map(|source| {
                let query = query.clone();
                let name = source.name().to_string();
                async move {
                    match source.search(&query).await {
                        Ok(results) => results,
                        Err(e) => SourceResults {
                            source_name: name,
                            results: vec![],
                            total_matched: 0,
                            search_time_ms: 0,
                            error: Some(e.to_string()),
                        },
                    }
                }
            })
            .collect();

        let source_results = join_all(futures).await;

        let total_results: usize = source_results.iter().map(|s| s.results.len()).sum();
        let total_time = total_start.elapsed().as_millis() as u64;

        let results = RecallResults {
            query_summary: format_query_summary(query),
            sources: source_results,
            total_results,
            total_time_ms: total_time,
            from_cache: false,
        };

        // Cache the results
        self.cache.put(&cache_key, &results).ok();

        results
    }

    /// List all available sources and their status
    pub fn list_sources(&self) -> Vec<(String, bool)> {
        let all_sources: Vec<Box<dyn sources::MemorySource>> = vec![
            Box::new(sources::goose::GooseSource::new()),
            Box::new(sources::claude::ClaudeSource::new()),
            Box::new(sources::pi::PiSource::new()),
            Box::new(sources::codex::CodexSource::new()),
            Box::new(sources::gemini::GeminiSource::new()),
            Box::new(sources::amp::AmpSource::new()),
            Box::new(sources::opencode::OpenCodeSource::new()),
        ];

        all_sources
            .iter()
            .map(|s| (s.name().to_string(), s.is_available()))
            .collect()
    }

    /// Clear the result cache
    pub fn clear_cache(&self) -> anyhow::Result<()> {
        self.cache.clear()
    }
}

fn format_query_summary(query: &RecallQuery) -> String {
    use crate::query::MatchMode;
    
    let mut parts = Vec::new();
    
    if let Some(ref text) = query.text {
        parts.push(format!("\"{}\"", text));
    }
    
    if !query.keywords.is_empty() {
        parts.push(format!("keywords: [{}]", query.keywords.join(", ")));
    }

    let terms = query.search_terms();
    if terms.len() > 1 {
        parts.push(format!("mode: {}", match query.mode {
            MatchMode::And => "ALL must match",
            MatchMode::Or => "ANY can match",
        }));
    }
    
    if let Some(ref after) = query.after {
        parts.push(format!("after: {}", after.format("%Y-%m-%d")));
    }
    
    if let Some(ref before) = query.before {
        parts.push(format!("before: {}", before.format("%Y-%m-%d")));
    }

    if parts.is_empty() {
        "all memories".to_string()
    } else {
        parts.join(", ")
    }
}
