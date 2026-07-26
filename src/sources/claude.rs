use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use crate::types::{Role, SourceKind};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::time::Instant;

/// Claude Code conversation history stored as JSONL files in ~/.claude/projects/
pub struct ClaudeSource {
    projects_dir: PathBuf,
}

impl ClaudeSource {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            projects_dir: home.join(".claude/projects"),
        }
    }

    /// Decode a Claude project dir name back to a path
    /// e.g. "-Users-micn-Documents-code-staged" -> "/Users/micn/Documents/code/staged"
    fn decode_project_name(name: &str) -> String {
        name.replace('-', "/")
    }
}

#[async_trait]
impl MemorySource for ClaudeSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Claude
    }

    fn is_available(&self) -> bool {
        self.projects_dir.exists()
    }

    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults> {
        let start = Instant::now();
        let projects_dir = self.projects_dir.clone();
        let query = query.clone();

        let results = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<MemoryResult>> {
            if query.search_terms().is_empty() && query.after.is_none() && query.before.is_none() {
                return Ok(vec![]);
            }

            let mut results = Vec::new();

            // Iterate all project directories
            let project_dirs = match std::fs::read_dir(&projects_dir) {
                Ok(d) => d,
                Err(_) => return Ok(vec![]),
            };

            for project_entry in project_dirs {
                let project_entry = match project_entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                if !project_entry
                    .file_type()
                    .map(|t| t.is_dir())
                    .unwrap_or(false)
                {
                    continue;
                }

                let project_name = project_entry.file_name().to_string_lossy().to_string();
                let project_path = ClaudeSource::decode_project_name(&project_name);

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

                    let session_id = file_name.trim_end_matches(".jsonl").to_string();

                    // Read and parse the file
                    let content = match std::fs::read_to_string(file_entry.path()) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    for line in content.lines() {
                        let entry: serde_json::Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");

                        // We care about user and assistant messages
                        if entry_type != "user" && entry_type != "assistant" {
                            continue;
                        }

                        // Parse timestamp
                        let timestamp_str = entry
                            .get("timestamp")
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
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

                        // Extract text content from message
                        let message = entry.get("message");
                        let text = Self::extract_text(message);

                        if text.trim().is_empty() {
                            continue;
                        }

                        // Keyword matching (AND or OR)
                        let (matches, hit_count) = query.matches_text(&text);

                        if !query.search_terms().is_empty() && !matches {
                            continue;
                        }

                        let role = Role::from_raw(
                            entry
                                .get("message")
                                .and_then(|m| m.get("role"))
                                .and_then(|r| r.as_str())
                                .unwrap_or(entry_type),
                        );

                        if !query.wants_role(Some(&role)) {
                            continue;
                        }

                        results.push(MemoryResult {
                            source: SourceKind::Claude,
                            timestamp,
                            content: text,
                            role: Some(role),
                            session_id: Some(session_id.clone()),
                            session_name: Some(project_path.clone()),
                            relevance: hit_count as f64,
                            metadata: None,
                        });

                        if results.len() >= query.limit * 2 {
                            // Early exit - we have enough candidates
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
        Ok(SourceResults::new(SourceKind::Claude, results, elapsed))
    }
}

impl ClaudeSource {
    fn extract_text(message: Option<&serde_json::Value>) -> String {
        let message = match message {
            Some(m) => m,
            None => return String::new(),
        };

        // message.content can be a string or array of content blocks
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

        // Fallback: try the top-level message field as a string
        if let Some(s) = message.get("message").and_then(|m| m.as_str()) {
            return s.to_string();
        }

        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_project_name_turns_dashes_into_slashes() {
        assert_eq!(
            ClaudeSource::decode_project_name("-Users-micn-code-staged"),
            "/Users/micn/code/staged"
        );
    }

    #[test]
    fn extract_text_reads_string_and_block_content() {
        let string_msg = serde_json::json!({"content": "hello there"});
        assert_eq!(ClaudeSource::extract_text(Some(&string_msg)), "hello there");

        let block_msg = serde_json::json!({
            "content": [
                {"type":"text","text":"line one"},
                {"type":"tool_use","name":"x"},
                {"type":"text","text":"line two"}
            ]
        });
        assert_eq!(
            ClaudeSource::extract_text(Some(&block_msg)),
            "line one\nline two"
        );

        assert_eq!(ClaudeSource::extract_text(None), "");
    }

    fn write_claude_session(projects_dir: &std::path::Path) {
        let project = projects_dir.join("-Users-micn-code-sandpit");
        std::fs::create_dir_all(&project).unwrap();
        let lines = [
            r#"{"type":"user","timestamp":"2026-03-11T04:19:00Z","message":{"role":"user","content":"how do we do hooks and security"}}"#,
            r#"{"type":"assistant","timestamp":"2026-03-11T04:20:00Z","message":{"role":"assistant","content":[{"type":"text","text":"CLI agent tools handle hooks like this"}]}}"#,
            r#"{"type":"summary","summary":"ignore me"}"#,
        ];
        std::fs::write(project.join("sess-1.jsonl"), lines.join("\n")).unwrap();
    }

    #[tokio::test]
    async fn search_finds_matches_and_decodes_project_path() {
        let dir = tempfile::tempdir().unwrap();
        write_claude_session(dir.path());
        let source = ClaudeSource {
            projects_dir: dir.path().to_path_buf(),
        };

        let query = RecallQuery {
            keywords: vec!["hooks".to_string()],
            ..Default::default()
        };
        let out = source.search(&query).await.unwrap();
        assert_eq!(out.source, SourceKind::Claude);
        assert_eq!(out.results.len(), 2);
        // Project dir name is decoded back into a filesystem path.
        assert!(out
            .results
            .iter()
            .all(|r| r.session_name.as_deref() == Some("/Users/micn/code/sandpit")));
        assert!(out.results.iter().all(|r| r.source == SourceKind::Claude));
    }

    #[tokio::test]
    async fn search_applies_role_filter() {
        let dir = tempfile::tempdir().unwrap();
        write_claude_session(dir.path());
        let source = ClaudeSource {
            projects_dir: dir.path().to_path_buf(),
        };

        let query = RecallQuery {
            keywords: vec!["hooks".to_string()],
            roles: vec![Role::User],
            ..Default::default()
        };
        let out = source.search(&query).await.unwrap();
        assert_eq!(out.results.len(), 1);
        assert_eq!(out.results[0].role, Some(Role::User));
    }

    #[tokio::test]
    async fn search_missing_dir_returns_empty() {
        let source = ClaudeSource {
            projects_dir: std::path::PathBuf::from("/nonexistent/path/xyz"),
        };
        let query = RecallQuery {
            keywords: vec!["anything".to_string()],
            ..Default::default()
        };
        let out = source.search(&query).await.unwrap();
        assert!(out.results.is_empty());
        assert!(out.error.is_none());
    }
}
