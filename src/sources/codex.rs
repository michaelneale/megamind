use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::time::Instant;

/// OpenAI Codex CLI conversation history stored as JSONL files in ~/.codex/sessions/
pub struct CodexSource {
    sessions_dir: PathBuf,
}

impl CodexSource {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            sessions_dir: home.join(".codex/sessions"),
        }
    }

    fn extract_text(payload: &serde_json::Value) -> Option<(String, String)> {
        // We only care about response_item with type=message
        if payload.get("type")?.as_str()? != "message" {
            return None;
        }

        let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("unknown");

        // Skip developer/system messages (prompt scaffolding)
        if role == "developer" || role == "system" {
            return None;
        }

        let content = payload.get("content")?.as_array()?;
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|block| {
                let ct = block.get("type")?.as_str()?;
                if ct == "output_text" || ct == "input_text" {
                    block.get("text")?.as_str()
                } else {
                    None
                }
            })
            .collect();

        let joined = texts.join("\n");
        if joined.trim().is_empty() {
            return None;
        }

        Some((role.to_string(), joined))
    }
}

#[async_trait]
impl MemorySource for CodexSource {
    fn name(&self) -> &str {
        "Codex"
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

            // Walk the sessions dir recursively to find all .jsonl files
            let jsonl_files = find_jsonl_files(&sessions_dir);

            for file_path in jsonl_files {
                let content = match std::fs::read_to_string(&file_path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let mut session_id = String::new();
                let mut session_cwd: Option<String> = None;

                for line in content.lines() {
                    let entry: serde_json::Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");

                    // Extract session metadata
                    if entry_type == "session_meta" {
                        if let Some(payload) = entry.get("payload") {
                            if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                                session_id = id.to_string();
                            }
                            if let Some(cwd) = payload.get("cwd").and_then(|v| v.as_str()) {
                                session_cwd = Some(cwd.to_string());
                            }
                        }
                        continue;
                    }

                    if entry_type != "response_item" {
                        continue;
                    }

                    let payload = match entry.get("payload") {
                        Some(p) => p,
                        None => continue,
                    };

                    let (role, text) = match CodexSource::extract_text(payload) {
                        Some(t) => t,
                        None => continue,
                    };

                    // Parse timestamp
                    let timestamp = entry
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

                    let (matches, hit_count) = query.matches_text(&text);
                    if !query.search_terms().is_empty() && !matches {
                        continue;
                    }

                    results.push(MemoryResult {
                        source: "codex".to_string(),
                        timestamp,
                        content: text,
                        role: Some(role),
                        session_id: Some(session_id.clone()),
                        session_name: session_cwd.clone(),
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
            source_name: "Codex".to_string(),
            results,
            total_matched: total,
            search_time_ms: elapsed,
            error: None,
        })
    }
}

/// Recursively find all .jsonl files under a directory
fn find_jsonl_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_jsonl_files(&path));
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files
}
