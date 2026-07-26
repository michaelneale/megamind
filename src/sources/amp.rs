use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use crate::types::{Role, SourceKind};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::path::PathBuf;
use std::time::Instant;

/// Amp (Sourcegraph) conversation history stored as JSON thread files in ~/.local/share/amp/threads/
pub struct AmpSource {
    threads_dir: PathBuf,
}

impl AmpSource {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            threads_dir: home.join(".local/share/amp/threads"),
        }
    }

    fn extract_text(content: &[serde_json::Value]) -> String {
        content
            .iter()
            .filter_map(|block| {
                let btype = block.get("type")?.as_str()?;
                if btype == "text" {
                    block
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait]
impl MemorySource for AmpSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Amp
    }

    fn is_available(&self) -> bool {
        self.threads_dir.exists()
    }

    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults> {
        let start = Instant::now();
        let threads_dir = self.threads_dir.clone();
        let query = query.clone();

        let results = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<MemoryResult>> {
            if query.search_terms().is_empty() && query.after.is_none() && query.before.is_none() {
                return Ok(vec![]);
            }

            let mut results = Vec::new();

            let thread_files = match std::fs::read_dir(&threads_dir) {
                Ok(d) => d,
                Err(_) => return Ok(vec![]),
            };

            for entry in thread_files.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if !file_name.ends_with(".json") {
                    continue;
                }

                let content = match std::fs::read_to_string(entry.path()) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let thread: serde_json::Value = match serde_json::from_str(&content) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let thread_id = thread
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let title = thread
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                // Get workspace directory from env.initial.trees[0].uri
                let session_name = thread
                    .get("env")
                    .and_then(|e| e.get("initial"))
                    .and_then(|i| i.get("trees"))
                    .and_then(|t| t.as_array())
                    .and_then(|trees| trees.first())
                    .and_then(|t| t.get("uri"))
                    .and_then(|u| u.as_str())
                    .map(|uri| uri.strip_prefix("file://").unwrap_or(uri).to_string())
                    .or_else(|| {
                        if !title.is_empty() {
                            Some(title.clone())
                        } else {
                            None
                        }
                    });

                // Thread-level timestamp for coarse date filtering
                let thread_created = thread
                    .get("created")
                    .and_then(|c| c.as_i64())
                    .and_then(|ms| Utc.timestamp_millis_opt(ms).single());

                // Quick date check on thread creation to skip old threads entirely
                if let Some(ref before) = query.before {
                    if let Some(tc) = thread_created {
                        if tc > *before {
                            continue;
                        }
                    }
                }

                let messages = match thread.get("messages").and_then(|m| m.as_array()) {
                    Some(m) => m,
                    None => continue,
                };

                for msg in messages {
                    let role = match msg.get("role").and_then(|r| r.as_str()) {
                        Some(r @ ("user" | "assistant")) => Role::from_raw(r),
                        _ => continue,
                    };

                    if !query.wants_role(Some(&role)) {
                        continue;
                    }

                    let content_arr = match msg.get("content").and_then(|c| c.as_array()) {
                        Some(c) => c,
                        None => continue,
                    };

                    let text = AmpSource::extract_text(content_arr);
                    if text.trim().is_empty() {
                        continue;
                    }

                    // Amp messages don't have individual timestamps, use thread created
                    let timestamp = thread_created.unwrap_or_else(Utc::now);

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
                        source: SourceKind::Amp,
                        timestamp,
                        content: text,
                        role: Some(role),
                        session_id: Some(thread_id.clone()),
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
        Ok(SourceResults::new(SourceKind::Amp, results, elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_joins_only_text_blocks() {
        let blocks = vec![
            serde_json::json!({"type":"text","text":"hello"}),
            serde_json::json!({"type":"tool_use","name":"bash"}),
            serde_json::json!({"type":"text","text":"world"}),
        ];
        assert_eq!(AmpSource::extract_text(&blocks), "hello\nworld");
        assert_eq!(AmpSource::extract_text(&[]), "");
    }

    fn write_amp_thread(threads_dir: &std::path::Path) {
        // created = 2026-02-18T21:19:00Z in epoch millis.
        let created_ms = 1_771_449_540_000i64;
        let thread = serde_json::json!({
            "id": "thread-1",
            "title": "sandbox chat",
            "created": created_ms,
            "env": {"initial": {"trees": [{"uri": "file:///Development/goose"}]}},
            "messages": [
                {"role":"user","content":[{"type":"text","text":"can we add sandbox support for goose"}]},
                {"role":"assistant","content":[{"type":"text","text":"yes, sandbox support is feasible"}]}
            ]
        });
        std::fs::write(
            threads_dir.join("thread-1.json"),
            serde_json::to_string(&thread).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn search_reads_workspace_uri_and_messages() {
        let dir = tempfile::tempdir().unwrap();
        write_amp_thread(dir.path());
        let source = AmpSource {
            threads_dir: dir.path().to_path_buf(),
        };

        let query = RecallQuery {
            keywords: vec!["sandbox".to_string()],
            ..Default::default()
        };
        let out = source.search(&query).await.unwrap();
        assert_eq!(out.source, SourceKind::Amp);
        assert_eq!(out.results.len(), 2);
        // The file:// prefix is stripped from the workspace URI.
        assert!(out
            .results
            .iter()
            .all(|r| r.session_name.as_deref() == Some("/Development/goose")));
        assert!(out
            .results
            .iter()
            .all(|r| r.session_id.as_deref() == Some("thread-1")));
    }

    #[tokio::test]
    async fn search_applies_role_filter() {
        let dir = tempfile::tempdir().unwrap();
        write_amp_thread(dir.path());
        let source = AmpSource {
            threads_dir: dir.path().to_path_buf(),
        };

        let query = RecallQuery {
            keywords: vec!["sandbox".to_string()],
            roles: vec![Role::User],
            ..Default::default()
        };
        let out = source.search(&query).await.unwrap();
        assert_eq!(out.results.len(), 1);
        assert_eq!(out.results[0].role, Some(Role::User));
    }
}
