use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::time::Instant;

/// Gemini CLI conversation history stored as JSON files in ~/.gemini/tmp/<project>/chats/
pub struct GeminiSource {
    base_dir: PathBuf,
}

impl GeminiSource {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            base_dir: home.join(".gemini/tmp"),
        }
    }

    fn extract_message_text(message: &serde_json::Value) -> String {
        let content = match message.get("content") {
            Some(c) => c,
            None => return String::new(),
        };

        // content can be a string or array of {text: "..."}
        if let Some(s) = content.as_str() {
            return s.to_string();
        }

        if let Some(arr) = content.as_array() {
            let texts: Vec<&str> = arr
                .iter()
                .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                .collect();
            return texts.join("\n");
        }

        String::new()
    }
}

#[async_trait]
impl MemorySource for GeminiSource {
    fn name(&self) -> &str {
        "Gemini"
    }

    fn is_available(&self) -> bool {
        self.base_dir.exists()
    }

    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults> {
        let start = Instant::now();
        let base_dir = self.base_dir.clone();
        let query = query.clone();

        let results = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<MemoryResult>> {
            if query.search_terms().is_empty() && query.after.is_none() && query.before.is_none() {
                return Ok(vec![]);
            }

            let mut results = Vec::new();

            // Iterate project directories under ~/.gemini/tmp/
            let project_dirs = match std::fs::read_dir(&base_dir) {
                Ok(d) => d,
                Err(_) => return Ok(vec![]),
            };

            for project_entry in project_dirs.flatten() {
                if !project_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }

                let project_name = project_entry.file_name().to_string_lossy().to_string();
                let chats_dir = project_entry.path().join("chats");

                let chat_files = match std::fs::read_dir(&chats_dir) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                for file_entry in chat_files.flatten() {
                    let file_name = file_entry.file_name().to_string_lossy().to_string();
                    if !file_name.ends_with(".json") {
                        continue;
                    }

                    let content = match std::fs::read_to_string(file_entry.path()) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    let session: serde_json::Value = match serde_json::from_str(&content) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let session_id = session
                        .get("sessionId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let messages = match session.get("messages").and_then(|m| m.as_array()) {
                        Some(m) => m,
                        None => continue,
                    };

                    for msg in messages {
                        let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");

                        // Map gemini types to roles
                        let role = match msg_type {
                            "user" => "user",
                            "gemini" => "assistant",
                            _ => continue,
                        };

                        // Parse timestamp
                        let timestamp = msg
                            .get("timestamp")
                            .and_then(|t| t.as_str())
                            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or_else(Utc::now);

                        // Date range filter
                        if let Some(ref after) = query.after {
                            if timestamp < *after {
                                continue;
                            }
                        }
                        if let Some(ref before) = query.before {
                            if timestamp > *before {
                                continue;
                            }
                        }

                        let text = GeminiSource::extract_message_text(msg);
                        if text.trim().is_empty() {
                            continue;
                        }

                        let (matches, hit_count) = query.matches_text(&text);
                        if !query.search_terms().is_empty() && !matches {
                            continue;
                        }

                        results.push(MemoryResult {
                            source: "gemini".to_string(),
                            timestamp,
                            content: text,
                            role: Some(role.to_string()),
                            session_id: Some(session_id.clone()),
                            session_name: Some(project_name.clone()),
                            relevance: hit_count as f64,
                            metadata: None,
                        });

                        if results.len() >= query.limit * 4 {
                            break;
                        }
                    }

                    if results.len() >= query.limit * 4 {
                        break;
                    }
                }

                if results.len() >= query.limit * 4 {
                    break;
                }
            }

            results.sort_by(|a, b| {
                b.relevance
                    .partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.timestamp.cmp(&a.timestamp))
            });
            results.truncate(query.limit);

            Ok(results)
        })
        .await??;

        let elapsed = start.elapsed().as_millis() as u64;
        let total = results.len();

        Ok(SourceResults {
            source_name: "Gemini".to_string(),
            results,
            total_matched: total,
            search_time_ms: elapsed,
            error: None,
        })
    }
}
