use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single memory result from any source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryResult {
    /// Which source this came from
    pub source: String,
    /// When this memory was created/occurred
    pub timestamp: DateTime<Utc>,
    /// The content/text of the memory
    pub content: String,
    /// Role (user/assistant) if from a conversation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
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
    pub source_name: String,
    pub results: Vec<MemoryResult>,
    pub total_matched: usize,
    pub search_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
        out.push_str(&format!("Found {} results across {} sources in {}ms",
            self.total_results,
            self.sources.len(),
            self.total_time_ms,
        ));
        if self.from_cache {
            out.push_str(" (cached)");
        }
        out.push_str("\n\n");

        for source in &self.sources {
            out.push_str(&format!("## {} ({} results, {}ms)\n", 
                source.source_name, source.results.len(), source.search_time_ms));
            
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
                let role_tag = result.role.as_deref().map(|r| format!("[{}] ", r)).unwrap_or_default();
                let session_tag = result.session_name.as_deref()
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
