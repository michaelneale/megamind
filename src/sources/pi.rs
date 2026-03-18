use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::time::Instant;

/// Pi agent session history stored as JSONL files in ~/.pi/agent/sessions/
pub struct PiSource {
    sessions_dir: PathBuf,
}

impl PiSource {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            sessions_dir: home.join(".pi/agent/sessions"),
        }
    }

    /// Decode a pi session dir name back to a path
    /// e.g. "--Users-micn-Documents-code-pi--" -> "/Users/micn/Documents/code/pi"
    fn decode_session_dir(name: &str) -> String {
        // Pi uses -- as prefix/suffix delimiters and - as path separator
        let trimmed = name.trim_start_matches('-').trim_end_matches('-');
        format!("/{}", trimmed.replace('-', "/"))
    }
}

#[async_trait]
impl MemorySource for PiSource {
    fn name(&self) -> &str {
        "Pi"
    }

    fn is_available(&self) -> bool {
        self.sessions_dir.exists()
    }

    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults> {
        let start = Instant::now();
        let sessions_dir = self.sessions_dir.clone();
        let query = query.clone();

        let results = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<MemoryResult>> {
            if query.search_terms().is_empty() && query.after.is_none() && query.before.is_none() {
                return Ok(vec![]);
            }

            let mut results = Vec::new();

            // Iterate project directories
            let project_dirs = match std::fs::read_dir(&sessions_dir) {
                Ok(d) => d,
                Err(_) => return Ok(vec![]),
            };

            for project_entry in project_dirs {
                let project_entry = match project_entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                if !project_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }

                let dir_name = project_entry.file_name().to_string_lossy().to_string();
                let project_path = PiSource::decode_session_dir(&dir_name);

                // Read all .jsonl files in the project
                let session_files = match std::fs::read_dir(project_entry.path()) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                for file_entry in session_files {
                    let file_entry = match file_entry {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    let file_name = file_entry.file_name().to_string_lossy().to_string();
                    if !file_name.ends_with(".jsonl") {
                        continue;
                    }

                    let content = match std::fs::read_to_string(file_entry.path()) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    // Extract session id from first line
                    let mut session_id = file_name.trim_end_matches(".jsonl").to_string();
                    
                    for line in content.lines() {
                        let entry: serde_json::Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");

                        // Update session_id from session header
                        if entry_type == "session" {
                            if let Some(id) = entry.get("id").and_then(|i| i.as_str()) {
                                session_id = id.to_string();
                            }
                            continue;
                        }

                        if entry_type != "message" {
                            continue;
                        }

                        // Parse timestamp
                        let timestamp_str = entry.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
                        let timestamp = DateTime::parse_from_rfc3339(timestamp_str)
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now());

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

                        // Extract text from message.message.content
                        let text = Self::extract_text(&entry);
                        if text.trim().is_empty() {
                            continue;
                        }

                        // Keyword matching (AND or OR)
                        let (matches, hit_count) = query.matches_text(&text);

                        if !query.search_terms().is_empty() && !matches {
                            continue;
                        }

                        let role = entry.get("message")
                            .and_then(|m| m.get("role"))
                            .and_then(|r| r.as_str())
                            .unwrap_or("unknown")
                            .to_string();

                        results.push(MemoryResult {
                            source: "pi".to_string(),
                            timestamp,
                            content: text,
                            role: Some(role),
                            session_id: Some(session_id.clone()),
                            session_name: Some(project_path.clone()),
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
            }

            // Sort by relevance then timestamp
            results.sort_by(|a, b| {
                b.relevance.partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.timestamp.cmp(&a.timestamp))
            });

            results.truncate(query.limit);
            Ok(results)
        }).await??;

        let elapsed = start.elapsed().as_millis() as u64;
        let total = results.len();

        Ok(SourceResults {
            source_name: "Pi".to_string(),
            results,
            total_matched: total,
            search_time_ms: elapsed,
            error: None,
        })
    }
}

impl PiSource {
    fn extract_text(entry: &serde_json::Value) -> String {
        // Pi format: entry.message.content is an array of {type: "text", text: "..."}
        let message = match entry.get("message") {
            Some(m) => m,
            None => return String::new(),
        };

        if let Some(content) = message.get("content") {
            if let Some(s) = content.as_str() {
                return s.to_string();
            }
            if let Some(arr) = content.as_array() {
                let texts: Vec<&str> = arr
                    .iter()
                    .filter_map(|block| {
                        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                            block.get("text").and_then(|t| t.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                return texts.join("\n");
            }
        }

        String::new()
    }
}
