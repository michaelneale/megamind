use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use crate::types::{Role, SourceKind};
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

    fn extract_text(payload: &serde_json::Value) -> Option<(Role, String)> {
        // We only care about response_item with type=message
        if payload.get("type")?.as_str()? != "message" {
            return None;
        }

        let role = Role::from_raw(
            payload
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown"),
        );

        // Skip system/developer messages (prompt scaffolding). Codex spells
        // its scaffolding role "developer", which Role normalizes to System.
        if matches!(role, Role::System) {
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

        Some((role, joined))
    }
}

#[async_trait]
impl MemorySource for CodexSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Codex
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

                    if !query.wants_role(Some(&role)) {
                        continue;
                    }

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
                        source: SourceKind::Codex,
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
        Ok(SourceResults::new(SourceKind::Codex, results, elapsed))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_maps_roles_and_skips_scaffolding() {
        let assistant = serde_json::json!({
            "type":"message","role":"assistant",
            "content":[{"type":"output_text","text":"the answer"}]
        });
        let (role, text) = CodexSource::extract_text(&assistant).unwrap();
        assert_eq!(role, Role::Assistant);
        assert_eq!(text, "the answer");

        let user = serde_json::json!({
            "type":"message","role":"user",
            "content":[{"type":"input_text","text":"the question"}]
        });
        assert_eq!(CodexSource::extract_text(&user).unwrap().0, Role::User);

        // Codex prompt scaffolding uses the "developer" role -> System -> skipped.
        let dev = serde_json::json!({
            "type":"message","role":"developer",
            "content":[{"type":"input_text","text":"scaffolding"}]
        });
        assert!(CodexSource::extract_text(&dev).is_none());

        // Non-message payloads are ignored.
        let other = serde_json::json!({"type":"reasoning","content":[]});
        assert!(CodexSource::extract_text(&other).is_none());
    }

    #[test]
    fn find_jsonl_files_recurses() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("2026").join("03");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("a.jsonl"), "{}").unwrap();
        std::fs::write(dir.path().join("b.jsonl"), "{}").unwrap();
        std::fs::write(dir.path().join("c.txt"), "ignore").unwrap();

        let found = find_jsonl_files(&dir.path().to_path_buf());
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| p.extension().unwrap() == "jsonl"));
    }

    fn write_codex_session(sessions_dir: &std::path::Path) {
        let day = sessions_dir.join("2026").join("03").join("11");
        std::fs::create_dir_all(&day).unwrap();
        let lines = [
            r#"{"type":"session_meta","payload":{"id":"codex-1","cwd":"/repo/thing"}}"#,
            r#"{"type":"response_item","timestamp":"2026-03-11T04:19:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"error handling with Result"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-03-11T04:20:00Z","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"error scaffolding"}]}}"#,
        ];
        std::fs::write(day.join("rollout.jsonl"), lines.join("\n")).unwrap();
    }

    #[tokio::test]
    async fn search_reads_session_meta_and_skips_developer() {
        let dir = tempfile::tempdir().unwrap();
        write_codex_session(dir.path());
        let source = CodexSource {
            sessions_dir: dir.path().to_path_buf(),
        };

        let query = RecallQuery {
            keywords: vec!["error".to_string()],
            ..Default::default()
        };
        let out = source.search(&query).await.unwrap();
        assert_eq!(out.source, SourceKind::Codex);
        // Only the assistant message survives; the developer one is scaffolding.
        assert_eq!(out.results.len(), 1);
        let r = &out.results[0];
        assert_eq!(r.role, Some(Role::Assistant));
        assert_eq!(r.session_id.as_deref(), Some("codex-1"));
        assert_eq!(r.session_name.as_deref(), Some("/repo/thing"));
    }
}
