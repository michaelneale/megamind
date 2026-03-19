use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::path::PathBuf;
use std::time::Instant;

/// OpenCode conversation history stored in ~/.local/share/opencode/storage/
/// Structure: session/<projectID>/<session>.json, message/<sessionID>/<msg>.json, part/<msgID>/<part>.json
pub struct OpenCodeSource {
    storage_dir: PathBuf,
}

impl OpenCodeSource {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            storage_dir: home.join(".local/share/opencode/storage"),
        }
    }
}

#[async_trait]
impl MemorySource for OpenCodeSource {
    fn name(&self) -> &str {
        "OpenCode"
    }

    fn is_available(&self) -> bool {
        self.storage_dir.join("session").exists()
    }

    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults> {
        let start = Instant::now();
        let storage_dir = self.storage_dir.clone();
        let query = query.clone();

        let results = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<MemoryResult>> {
            if query.search_terms().is_empty() && query.after.is_none() && query.before.is_none() {
                return Ok(vec![]);
            }

            let mut results = Vec::new();

            // Build a map of session_id -> (title, directory)
            let session_dir = storage_dir.join("session");
            let mut session_info: std::collections::HashMap<String, (String, String)> =
                std::collections::HashMap::new();

            if let Ok(project_dirs) = std::fs::read_dir(&session_dir) {
                for project_entry in project_dirs.flatten() {
                    if !project_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        continue;
                    }
                    if let Ok(session_files) = std::fs::read_dir(project_entry.path()) {
                        for sf in session_files.flatten() {
                            if let Ok(content) = std::fs::read_to_string(sf.path()) {
                                if let Ok(s) = serde_json::from_str::<serde_json::Value>(&content) {
                                    let sid = s.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let title = s.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let dir = s.get("directory").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    if !sid.is_empty() {
                                        session_info.insert(sid, (title, dir));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Iterate message directories (keyed by session ID)
            let message_dir = storage_dir.join("message");
            let part_dir = storage_dir.join("part");

            let msg_session_dirs = match std::fs::read_dir(&message_dir) {
                Ok(d) => d,
                Err(_) => return Ok(vec![]),
            };

            for session_entry in msg_session_dirs.flatten() {
                let session_id = session_entry.file_name().to_string_lossy().to_string();
                let (session_title, session_dir_path) = session_info
                    .get(&session_id)
                    .cloned()
                    .unwrap_or_default();

                let session_name = if !session_dir_path.is_empty() {
                    Some(session_dir_path)
                } else if !session_title.is_empty() {
                    Some(session_title)
                } else {
                    None
                };

                // Read all messages in this session
                let msg_files = match std::fs::read_dir(session_entry.path()) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                for msg_entry in msg_files.flatten() {
                    let msg_content = match std::fs::read_to_string(msg_entry.path()) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    let msg: serde_json::Value = match serde_json::from_str(&msg_content) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let role = match msg.get("role").and_then(|r| r.as_str()) {
                        Some(r @ ("user" | "assistant")) => r,
                        _ => continue,
                    };

                    let msg_id = match msg.get("id").and_then(|v| v.as_str()) {
                        Some(id) => id.to_string(),
                        None => continue,
                    };

                    // Parse timestamp from time.created (epoch ms)
                    let timestamp = msg
                        .get("time")
                        .and_then(|t| t.get("created"))
                        .and_then(|c| c.as_i64())
                        .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
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

                    // Collect text parts for this message
                    let msg_part_dir = part_dir.join(&msg_id);
                    let mut text_parts = Vec::new();

                    if let Ok(part_files) = std::fs::read_dir(&msg_part_dir) {
                        for pf in part_files.flatten() {
                            if let Ok(pc) = std::fs::read_to_string(pf.path()) {
                                if let Ok(part) = serde_json::from_str::<serde_json::Value>(&pc) {
                                    let ptype = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                    if ptype == "text" || ptype == "reasoning" {
                                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                            if !text.trim().is_empty() {
                                                text_parts.push(text.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let text = text_parts.join("\n");
                    if text.trim().is_empty() {
                        continue;
                    }

                    let (matches, hit_count) = query.matches_text(&text);
                    if !query.search_terms().is_empty() && !matches {
                        continue;
                    }

                    results.push(MemoryResult {
                        source: "opencode".to_string(),
                        timestamp,
                        content: text,
                        role: Some(role.to_string()),
                        session_id: Some(session_id.clone()),
                        session_name: session_name.clone(),
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
            source_name: "OpenCode".to_string(),
            results,
            total_matched: total,
            search_time_ms: elapsed,
            error: None,
        })
    }
}
