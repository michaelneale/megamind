use crate::types::{Role, SourceKind};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single memory result from any source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryResult {
    /// Which source (session type) this came from
    pub source: SourceKind,
    /// When this memory was created/occurred
    pub timestamp: DateTime<Utc>,
    /// The content/text of the memory
    pub content: String,
    /// Normalized role (user/assistant/system/tool) if from a conversation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Session/conversation identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Session name if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// Relevance score (higher = more relevant)
    pub relevance: f64,
    /// Extra metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Results from a single data source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceResults {
    /// Which source (session type) produced these results
    pub source: SourceKind,
    pub results: Vec<MemoryResult>,
    pub total_matched: usize,
    pub search_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SourceResults {
    /// Convenience constructor for a successful search.
    pub fn new(source: SourceKind, results: Vec<MemoryResult>, search_time_ms: u64) -> Self {
        let total_matched = results.len();
        Self {
            source,
            results,
            total_matched,
            search_time_ms,
            error: None,
        }
    }

    /// Convenience constructor for a failed search.
    pub fn failed(source: SourceKind, error: impl Into<String>) -> Self {
        Self {
            source,
            results: vec![],
            total_matched: 0,
            search_time_ms: 0,
            error: Some(error.into()),
        }
    }
}

/// Aggregated results from all sources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResults {
    pub query_summary: String,
    pub sources: Vec<SourceResults>,
    pub total_results: usize,
    pub total_time_ms: u64,
    pub from_cache: bool,
}

impl RecallResults {
    /// Format results as human-readable text for agent consumption
    pub fn format_text(&self) -> String {
        let mut out = String::new();

        out.push_str(&format!("# Memory Recall: {}\n", self.query_summary));
        out.push_str(&format!(
            "Found {} results across {} sources in {}ms",
            self.total_results,
            self.sources.len(),
            self.total_time_ms,
        ));
        if self.from_cache {
            out.push_str(" (cached)");
        }
        out.push_str("\n\n");

        for source in &self.sources {
            out.push_str(&format!(
                "## {} ({} results, {}ms)\n",
                source.source.display_name(),
                source.results.len(),
                source.search_time_ms
            ));

            if let Some(ref err) = source.error {
                out.push_str(&format!("⚠ Error: {}\n\n", err));
                continue;
            }

            if source.results.is_empty() {
                out.push_str("No matching results.\n\n");
                continue;
            }

            for (i, result) in source.results.iter().enumerate() {
                let ts = result.timestamp.format("%Y-%m-%d %H:%M");
                let role_tag = result
                    .role
                    .as_ref()
                    .map(|r| format!("[{}] ", r))
                    .unwrap_or_default();
                let session_tag = result
                    .session_name
                    .as_deref()
                    .or(result.session_id.as_deref())
                    .map(|s| format!(" (session: {})", s))
                    .unwrap_or_default();

                out.push_str(&format!("{}. [{}]{}{}\n", i + 1, ts, session_tag, role_tag));

                // Truncate very long content (safely at char boundary)
                let content = if result.content.len() > 500 {
                    let mut end = 500;
                    while !result.content.is_char_boundary(end) && end > 0 {
                        end -= 1;
                    }
                    format!("{}...", &result.content[..end])
                } else {
                    result.content.clone()
                };
                out.push_str(&format!("   {}\n\n", content.replace('\n', "\n   ")));
            }
        }

        out
    }

    /// Format as JSON
    pub fn format_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result(source: SourceKind, role: Role, content: &str) -> MemoryResult {
        MemoryResult {
            source,
            timestamp: DateTime::parse_from_rfc3339("2026-03-11T04:19:00Z")
                .unwrap()
                .with_timezone(&Utc),
            content: content.to_string(),
            role: Some(role),
            session_id: Some("sess-1".to_string()),
            session_name: Some("/Development/sandpit".to_string()),
            relevance: 2.0,
            metadata: None,
        }
    }

    fn sample_results() -> RecallResults {
        RecallResults {
            query_summary: "keywords: [goose]".to_string(),
            sources: vec![SourceResults::new(
                SourceKind::Claude,
                vec![sample_result(
                    SourceKind::Claude,
                    Role::Assistant,
                    "Here's how these CLI agent tools handle hooks",
                )],
                38,
            )],
            total_results: 1,
            total_time_ms: 50,
            from_cache: false,
        }
    }

    #[test]
    fn source_results_new_sets_total_matched() {
        let sr = SourceResults::new(
            SourceKind::Goose,
            vec![sample_result(SourceKind::Goose, Role::User, "hi")],
            12,
        );
        assert_eq!(sr.total_matched, 1);
        assert!(sr.error.is_none());
        assert_eq!(sr.source, SourceKind::Goose);
    }

    #[test]
    fn source_results_failed_carries_error() {
        let sr = SourceResults::failed(SourceKind::Pi, "db locked");
        assert!(sr.results.is_empty());
        assert_eq!(sr.error.as_deref(), Some("db locked"));
    }

    #[test]
    fn format_text_uses_display_name_and_role() {
        let text = sample_results().format_text();
        // Header uses the human-facing display name, not the id.
        assert!(text.contains("## Claude Code (1 results, 38ms)"));
        // Role renders via the typed Display impl.
        assert!(text.contains("[assistant]"));
        assert!(text.contains("(session: /Development/sandpit)"));
        assert!(text.contains("# Memory Recall: keywords: [goose]"));
    }

    #[test]
    fn format_text_marks_cache_hits() {
        let mut results = sample_results();
        results.from_cache = true;
        assert!(results.format_text().contains("(cached)"));
    }

    #[test]
    fn format_text_reports_errors_and_empty_sources() {
        let results = RecallResults {
            query_summary: "x".to_string(),
            sources: vec![
                SourceResults::failed(SourceKind::Goose, "boom"),
                SourceResults::new(SourceKind::Pi, vec![], 3),
            ],
            total_results: 0,
            total_time_ms: 1,
            from_cache: false,
        };
        let text = results.format_text();
        assert!(text.contains("⚠ Error: boom"));
        assert!(text.contains("No matching results."));
    }

    #[test]
    fn format_json_serializes_typed_fields_as_strings() {
        let json = sample_results().format_json();
        // Source and role serialize to their stable lowercase ids for
        // backwards compatibility with existing JSON consumers.
        assert!(json.contains("\"source\": \"claude\""));
        assert!(json.contains("\"role\": \"assistant\""));
    }

    #[test]
    fn recall_results_round_trip_through_json() {
        let original = sample_results();
        let json = serde_json::to_string(&original).unwrap();
        let back: RecallResults = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sources.len(), 1);
        assert_eq!(back.sources[0].source, SourceKind::Claude);
        assert_eq!(back.sources[0].results[0].role, Some(Role::Assistant));
        assert_eq!(back.sources[0].results[0].source, SourceKind::Claude);
    }

    #[test]
    fn format_text_truncates_long_content_at_char_boundary() {
        let long = "é".repeat(400); // 800 bytes, forces boundary handling
        let results = RecallResults {
            query_summary: "x".to_string(),
            sources: vec![SourceResults::new(
                SourceKind::Goose,
                vec![sample_result(SourceKind::Goose, Role::User, &long)],
                1,
            )],
            total_results: 1,
            total_time_ms: 1,
            from_cache: false,
        };
        // Must not panic on multi-byte truncation and must add an ellipsis.
        let text = results.format_text();
        assert!(text.contains("..."));
    }
}
