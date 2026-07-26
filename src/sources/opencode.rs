use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use crate::types::{Role, SourceKind};
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
    fn kind(&self) -> SourceKind {
        SourceKind::OpenCode
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
                    if !project_entry
                        .file_type()
                        .map(|t| t.is_dir())
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    if let Ok(session_files) = std::fs::read_dir(project_entry.path()) {
                        for sf in session_files.flatten() {
                            if let Ok(content) = std::fs::read_to_string(sf.path()) {
                                if let Ok(s) = serde_json::from_str::<serde_json::Value>(&content) {
                                    let sid = s
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let title = s
                                        .get("title")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let dir = s
                                        .get("directory")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
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
                let (session_title, session_dir_path) =
                    session_info.get(&session_id).cloned().unwrap_or_default();

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
                        Some(r @ ("user" | "assistant")) => Role::from_raw(r),
                        _ => continue,
                    };

                    if !query.wants_role(Some(&role)) {
                        continue;
                    }

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
                                    let ptype =
                                        part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                    if ptype == "text" || ptype == "reasoning" {
                                        if let Some(text) =
                                            part.get("text").and_then(|t| t.as_str())
                                        {
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
                        source: SourceKind::OpenCode,
                        timestamp,
                        content: text,
                        role: Some(role),
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
        Ok(SourceResults::new(SourceKind::OpenCode, results, elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the three-level OpenCode storage layout:
    /// session/<project>/<session>.json, message/<sessionID>/<msg>.json,
    /// part/<msgID>/<part>.json.
    fn write_opencode_storage(storage_dir: &std::path::Path) {
        // Session metadata.
        let session_proj = storage_dir.join("session").join("proj-1");
        std::fs::create_dir_all(&session_proj).unwrap();
        let session = serde_json::json!({
            "id": "sess-oc-1",
            "title": "deploy chat",
            "directory": "/repo/opencode-thing"
        });
        std::fs::write(
            session_proj.join("sess-oc-1.json"),
            serde_json::to_string(&session).unwrap(),
        )
        .unwrap();

        // Messages keyed by session id.
        let msg_dir = storage_dir.join("message").join("sess-oc-1");
        std::fs::create_dir_all(&msg_dir).unwrap();
        for (mid, role, created) in [
            ("msg-a", "assistant", 1_772_000_000_000i64),
            ("msg-b", "user", 1_772_000_100_000i64),
        ] {
            let msg = serde_json::json!({
                "id": mid,
                "role": role,
                "time": {"created": created}
            });
            std::fs::write(
                msg_dir.join(format!("{mid}.json")),
                serde_json::to_string(&msg).unwrap(),
            )
            .unwrap();
        }

        // Parts keyed by message id.
        let part_a = storage_dir.join("part").join("msg-a");
        std::fs::create_dir_all(&part_a).unwrap();
        std::fs::write(
            part_a.join("p1.json"),
            r#"{"type":"text","text":"deploy the release to staging"}"#,
        )
        .unwrap();
        let part_b = storage_dir.join("part").join("msg-b");
        std::fs::create_dir_all(&part_b).unwrap();
        std::fs::write(
            part_b.join("p1.json"),
            r#"{"type":"text","text":"how do we deploy"}"#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn search_joins_sessions_messages_and_parts() {
        let dir = tempfile::tempdir().unwrap();
        write_opencode_storage(dir.path());
        let source = OpenCodeSource {
            storage_dir: dir.path().to_path_buf(),
        };

        let query = RecallQuery {
            keywords: vec!["deploy".to_string()],
            ..Default::default()
        };
        let out = source.search(&query).await.unwrap();
        assert_eq!(out.source, SourceKind::OpenCode);
        assert_eq!(out.results.len(), 2);
        // The session directory is joined onto every message via session id.
        assert!(out
            .results
            .iter()
            .all(|r| r.session_name.as_deref() == Some("/repo/opencode-thing")));
        assert!(out
            .results
            .iter()
            .all(|r| r.session_id.as_deref() == Some("sess-oc-1")));
    }

    #[tokio::test]
    async fn search_applies_role_filter() {
        let dir = tempfile::tempdir().unwrap();
        write_opencode_storage(dir.path());
        let source = OpenCodeSource {
            storage_dir: dir.path().to_path_buf(),
        };

        let query = RecallQuery {
            keywords: vec!["deploy".to_string()],
            roles: vec![Role::User],
            ..Default::default()
        };
        let out = source.search(&query).await.unwrap();
        assert_eq!(out.results.len(), 1);
        assert_eq!(out.results[0].role, Some(Role::User));
    }

    #[tokio::test]
    async fn search_missing_storage_returns_empty() {
        let source = OpenCodeSource {
            storage_dir: std::path::PathBuf::from("/nonexistent/oc/xyz"),
        };
        let query = RecallQuery {
            keywords: vec!["deploy".to_string()],
            ..Default::default()
        };
        let out = source.search(&query).await.unwrap();
        assert!(out.results.is_empty());
    }
}
