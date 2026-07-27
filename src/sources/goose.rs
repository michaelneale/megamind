use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use crate::types::{Role, SourceKind};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::Connection;
use std::path::PathBuf;
use std::time::Instant;

/// Upper bound on how many candidate rows/messages either Goose path will
/// materialize before precise filtering. This is a memory-safety backstop so a
/// broad date-only query can't pull an entire large history into memory — it is
/// deliberately far above any sane `--limit`. The user-facing `query.limit` is
/// applied only *after* filtering and relevance sorting, so it means "max
/// results returned", not "max rows scanned".
const MAX_CANDIDATE_ROWS: usize = 50_000;

/// Goose conversation history stored in SQLite
pub struct GooseSource {
    db_path: PathBuf,
    sessions_dir: PathBuf,
}

impl GooseSource {
    pub fn new() -> Self {
        // Goose stores data in ~/.local/share/goose on all platforms
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let sessions_dir = home.join(".local/share/goose/sessions");
        Self {
            db_path: sessions_dir.join("sessions.db"),
            sessions_dir,
        }
    }

    fn extract_text_content(content_json: &str) -> String {
        // content_json is a JSON array of content blocks
        if let Ok(blocks) = serde_json::from_str::<serde_json::Value>(content_json) {
            if let Some(arr) = blocks.as_array() {
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
            // Maybe it's just a plain string
            if let Some(s) = blocks.as_str() {
                return s.to_string();
            }
        }
        // Fallback: return raw json (might still be useful for keyword search)
        content_json.to_string()
    }

    fn extract_jsonl_text(entry: &serde_json::Value) -> String {
        let content = match entry.get("content") {
            Some(content) => content,
            None => return String::new(),
        };

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

        String::new()
    }

    fn search_sqlite(db_path: &PathBuf, query: &RecallQuery) -> anyhow::Result<Vec<MemoryResult>> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        let search_terms = query.search_terms();
        if search_terms.is_empty() && query.after.is_none() && query.before.is_none() {
            return Ok(vec![]);
        }

        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        let (like_clause, like_params) = query.sql_like_clause("m.content_json");
        if !like_clause.is_empty() {
            conditions.push(like_clause);
            for p in like_params {
                params.push(Box::new(p));
            }
        }

        if let Some(ref after) = query.after {
            conditions.push("m.timestamp >= ?".to_string());
            params.push(Box::new(after.to_rfc3339()));
        }
        if let Some(ref before) = query.before {
            conditions.push("m.timestamp <= ?".to_string());
            params.push(Box::new(before.to_rfc3339()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            r#"
            SELECT m.role, m.content_json, m.timestamp, m.created_timestamp,
                   s.id as session_id, s.name as session_name, s.working_dir
            FROM messages m
            JOIN sessions s ON m.session_id = s.id
            {}
            ORDER BY m.timestamp DESC
            LIMIT ?
            "#,
            where_clause
        );

        // Bind the SQL LIMIT to the safety cap, NOT query.limit: rows are
        // dropped by post-query filters (empty text extraction, role, precise
        // word-boundary match against extracted text vs the coarse content_json
        // LIKE), and results are ranked by relevance rather than SQL's timestamp
        // order. Applying query.limit here would spend the budget on recent
        // candidates before filtering/ranking, so small limits under-return.
        params.push(Box::new(MAX_CANDIDATE_ROWS as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            let role: String = row.get(0)?;
            let content_json: String = row.get(1)?;
            let timestamp: String = row.get(2)?;
            let _created_ts: Option<i64> = row.get(3)?;
            let session_id: String = row.get(4)?;
            let session_name: Option<String> = row.get(5)?;
            let working_dir: Option<String> = row.get(6)?;
            Ok((
                role,
                content_json,
                timestamp,
                session_id,
                session_name,
                working_dir,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            let (role, content_json, timestamp_str, session_id, session_name, working_dir) = row?;
            let content = GooseSource::extract_text_content(&content_json);

            if content.trim().is_empty() {
                continue;
            }

            let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            let role = Role::from_raw(&role);
            if !query.wants_role(Some(&role)) {
                continue;
            }

            // The SQL `LIKE` is only a coarse substring prefilter. Enforce the
            // precise (word-boundary or loose) match here so the SQLite path
            // agrees with the file-based sources.
            let (matches, hit_count) = query.matches_text(&content);
            if !query.search_terms().is_empty() && !matches {
                continue;
            }
            let relevance = hit_count as f64;

            let display_name = session_name
                .filter(|n| !n.is_empty())
                .or(working_dir.filter(|w| !w.is_empty()));

            results.push(MemoryResult {
                source: SourceKind::Goose,
                timestamp,
                content,
                role: Some(role),
                session_id: Some(session_id),
                session_name: display_name,
                relevance,
                metadata: None,
            });
        }

        results.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.timestamp.cmp(&a.timestamp))
        });
        results.truncate(query.limit);

        Ok(results)
    }

    fn search_jsonl(
        sessions_dir: &PathBuf,
        query: &RecallQuery,
    ) -> anyhow::Result<Vec<MemoryResult>> {
        if query.search_terms().is_empty() && query.after.is_none() && query.before.is_none() {
            return Ok(vec![]);
        }

        let entries = match std::fs::read_dir(sessions_dir) {
            Ok(entries) => entries,
            Err(_) => return Ok(vec![]),
        };

        let mut results = Vec::new();

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };

            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }

            let file_name = entry.file_name().to_string_lossy().to_string();
            if !file_name.ends_with(".jsonl") {
                continue;
            }

            let file_content = match std::fs::read_to_string(entry.path()) {
                Ok(content) => content,
                Err(_) => continue,
            };

            let mut session_id = file_name.trim_end_matches(".jsonl").to_string();
            let mut session_name: Option<String> = None;

            for line in file_content.lines() {
                let entry: serde_json::Value = match serde_json::from_str(line) {
                    Ok(value) => value,
                    Err(_) => continue,
                };

                if entry.get("working_dir").is_some() || entry.get("description").is_some() {
                    if let Some(id) = entry.get("id").and_then(|v| v.as_str()) {
                        session_id = id.to_string();
                    }

                    session_name = entry
                        .get("working_dir")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            entry
                                .get("description")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string())
                        });
                    continue;
                }

                let role = match entry.get("role").and_then(|v| v.as_str()) {
                    Some(role @ ("user" | "assistant")) => Role::from_raw(role),
                    _ => continue,
                };

                if !query.wants_role(Some(&role)) {
                    continue;
                }

                let timestamp = entry
                    .get("created")
                    .and_then(|v| v.as_i64())
                    .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
                    .unwrap_or_else(Utc::now);

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

                let text = Self::extract_jsonl_text(&entry);
                if text.trim().is_empty() {
                    continue;
                }

                let (matches, hit_count) = query.matches_text(&text);
                if !query.search_terms().is_empty() && !matches {
                    continue;
                }

                results.push(MemoryResult {
                    source: SourceKind::Goose,
                    timestamp,
                    content: text,
                    role: Some(role),
                    session_id: Some(session_id.clone()),
                    session_name: session_name.clone(),
                    relevance: hit_count as f64,
                    metadata: None,
                });

                // Stop only at the memory-safety cap. A `limit * 4` break here
                // would truncate by file/scan order *before* the relevance sort
                // below, dropping the most relevant matches when they appear
                // late. query.limit is applied after ranking instead.
                if results.len() >= MAX_CANDIDATE_ROWS {
                    break;
                }
            }

            if results.len() >= MAX_CANDIDATE_ROWS {
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
    }
}

#[async_trait]
impl MemorySource for GooseSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Goose
    }

    fn is_available(&self) -> bool {
        self.db_path.exists() || self.sessions_dir.exists()
    }

    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults> {
        let start = Instant::now();
        let db_path = self.db_path.clone();
        let sessions_dir = self.sessions_dir.clone();
        let query = query.clone();

        // SQLite is blocking, so run in a blocking task
        let results = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<MemoryResult>> {
            match GooseSource::search_sqlite(&db_path, &query) {
                Ok(results) => Ok(results),
                Err(_) => GooseSource::search_jsonl(&sessions_dir, &query),
            }
        })
        .await??;

        let elapsed = start.elapsed().as_millis() as u64;
        Ok(SourceResults::new(SourceKind::Goose, results, elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn query_for(terms: &[&str]) -> RecallQuery {
        RecallQuery {
            keywords: terms.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn extract_text_content_joins_text_blocks() {
        let json = r#"[{"type":"text","text":"hello"},{"type":"image","url":"x"},{"type":"text","text":"world"}]"#;
        assert_eq!(GooseSource::extract_text_content(json), "hello\nworld");
    }

    #[test]
    fn extract_text_content_handles_plain_string() {
        let json = r#""just a string""#;
        assert_eq!(GooseSource::extract_text_content(json), "just a string");
    }

    #[test]
    fn extract_text_content_falls_back_to_raw_on_garbage() {
        // Not valid JSON — the raw text is still returned for keyword search.
        assert_eq!(GooseSource::extract_text_content("not json"), "not json");
    }

    #[test]
    fn extract_jsonl_text_reads_string_and_array_content() {
        let string_entry = serde_json::json!({"content": "plain"});
        assert_eq!(GooseSource::extract_jsonl_text(&string_entry), "plain");

        let array_entry = serde_json::json!({
            "content": [{"type":"text","text":"a"},{"type":"text","text":"b"}]
        });
        assert_eq!(GooseSource::extract_jsonl_text(&array_entry), "a\nb");

        let no_content = serde_json::json!({"other": 1});
        assert_eq!(GooseSource::extract_jsonl_text(&no_content), "");
    }

    /// Build a minimal Goose sessions.db matching the schema the search query
    /// expects, then confirm search finds and normalizes rows.
    fn write_goose_db(path: &std::path::Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sessions (id TEXT PRIMARY KEY, name TEXT, working_dir TEXT);
            CREATE TABLE messages (
                session_id TEXT,
                role TEXT,
                content_json TEXT,
                timestamp TEXT,
                created_timestamp INTEGER
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, working_dir) VALUES (?1, ?2, ?3)",
            params!["s1", "My Session", "/work/dir"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (session_id, role, content_json, timestamp, created_timestamp) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "s1",
                "assistant",
                r#"[{"type":"text","text":"we discussed the sandbox approach"}]"#,
                "2026-03-11T04:19:00Z",
                1_772_000_000i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (session_id, role, content_json, timestamp, created_timestamp) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "s1",
                "user",
                r#"[{"type":"text","text":"unrelated chatter"}]"#,
                "2026-03-11T04:20:00Z",
                1_772_000_100i64
            ],
        )
        .unwrap();
    }

    #[test]
    fn search_sqlite_finds_matches_and_normalizes_types() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.db");
        write_goose_db(&db);

        let results = GooseSource::search_sqlite(&db, &query_for(&["sandbox"])).unwrap();
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.source, SourceKind::Goose);
        assert_eq!(r.role, Some(Role::Assistant));
        assert!(r.content.contains("sandbox"));
        // session name prefers the human name over working_dir.
        assert_eq!(r.session_name.as_deref(), Some("My Session"));
    }

    #[test]
    fn search_sqlite_applies_role_filter() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.db");
        write_goose_db(&db);

        // Only the assistant message mentions "sandbox", so a user-role
        // filter should exclude it.
        let mut q = query_for(&["sandbox"]);
        q.roles = vec![Role::User];
        let results = GooseSource::search_sqlite(&db, &q).unwrap();
        assert!(results.is_empty());

        let mut q = query_for(&["sandbox"]);
        q.roles = vec![Role::Assistant];
        let results = GooseSource::search_sqlite(&db, &q).unwrap();
        assert_eq!(results.len(), 1);
    }

    /// Insert a single message row.
    fn insert_msg(conn: &Connection, session_id: &str, role: &str, content_json: &str, ts: &str) {
        conn.execute(
            "INSERT INTO messages (session_id, role, content_json, timestamp, created_timestamp) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, role, content_json, ts, 0i64],
        )
        .unwrap();
    }

    /// Build a DB that reproduces issue #5: many *recent* rows that match the
    /// SQL `LIKE` prefilter (the term lives inside a tool-call payload) but
    /// extract to empty text and get dropped by the Rust post-filters, plus a
    /// few genuine matches with *older* timestamps.
    fn write_goose_db_limit_fixture(path: &std::path::Path, decoys: usize, reals: usize) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sessions (id TEXT PRIMARY KEY, name TEXT, working_dir TEXT);
            CREATE TABLE messages (
                session_id TEXT, role TEXT, content_json TEXT,
                timestamp TEXT, created_timestamp INTEGER
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, working_dir) VALUES ('s1', 'S', '/w')",
            [],
        )
        .unwrap();

        // Decoys at recent timestamps (05:xx): match `LIKE '%recipe%'` only
        // inside a tool call, so extracted text is empty -> dropped after SQL.
        for i in 0..decoys {
            let ts = format!("2026-03-11T05:{:02}:00Z", i % 60);
            insert_msg(
                &conn,
                "s1",
                "assistant",
                r#"[{"type":"toolRequest","toolCall":{"name":"load_recipe","arguments":{}}}]"#,
                &ts,
            );
        }
        // Genuine matches at older timestamps (04:xx): "recipe" in real text.
        for i in 0..reals {
            let ts = format!("2026-03-11T04:{:02}:00Z", i % 60);
            insert_msg(
                &conn,
                "s1",
                "assistant",
                r#"[{"type":"text","text":"the recipe we discussed"}]"#,
                &ts,
            );
        }
    }

    #[test]
    fn search_sqlite_limit_counts_results_not_scanned_rows() {
        // Regression for issue #5: 20 recent decoy rows that match the SQL LIKE
        // but extract to empty, plus 3 genuine older matches. A small --limit
        // must still return all 3 real results rather than spending its budget
        // on the most-recent decoy rows (which returned 0 before the fix).
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.db");
        write_goose_db_limit_fixture(&db, 20, 3);

        let mut q = query_for(&["recipe"]);
        q.limit = 10;
        let results = GooseSource::search_sqlite(&db, &q).unwrap();
        assert_eq!(
            results.len(),
            3,
            "should return all 3 real matches despite 20 more-recent decoys"
        );
        assert!(results.iter().all(|r| r.content.contains("recipe")));
    }

    #[test]
    fn search_sqlite_truncates_to_limit_after_filtering() {
        // When genuine matches exceed the limit, exactly `limit` are returned.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.db");
        write_goose_db_limit_fixture(&db, 5, 8);

        let mut q = query_for(&["recipe"]);
        q.limit = 4;
        let results = GooseSource::search_sqlite(&db, &q).unwrap();
        assert_eq!(results.len(), 4);
    }

    #[test]
    fn search_sqlite_ranks_by_relevance_across_all_candidates() {
        // The most relevant row (2 term hits) is the OLDEST. Ranking must span
        // every candidate, not just the most-recent `limit`, so it ranks first.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sessions (id TEXT PRIMARY KEY, name TEXT, working_dir TEXT);
            CREATE TABLE messages (
                session_id TEXT, role TEXT, content_json TEXT,
                timestamp TEXT, created_timestamp INTEGER
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, working_dir) VALUES ('s1', 'S', '/w')",
            [],
        )
        .unwrap();
        // Oldest row matches both terms; newer rows match only one.
        insert_msg(
            &conn,
            "s1",
            "assistant",
            r#"[{"type":"text","text":"the recipe and the deploy plan"}]"#,
            "2026-03-11T04:00:00Z",
        );
        for i in 1..10 {
            let ts = format!("2026-03-11T05:{:02}:00Z", i);
            insert_msg(
                &conn,
                "s1",
                "assistant",
                r#"[{"type":"text","text":"just a recipe note"}]"#,
                &ts,
            );
        }

        let mut q = query_for(&["recipe", "deploy"]);
        q.mode = crate::query::MatchMode::Or;
        q.limit = 20;
        let results = GooseSource::search_sqlite(&db, &q).unwrap();
        assert!(results.len() >= 2);
        assert_eq!(results[0].relevance, 2.0, "2-hit row should rank first");
        assert!(results[0].content.contains("deploy"));
    }

    #[test]
    fn search_jsonl_ranks_by_relevance_across_all_matches() {
        // Regression for issue #5 (JSONL path): the old `limit * 4` break
        // stopped scanning by file order before ranking, so a highly-relevant
        // match appearing late was missed. It must now be found and rank first.
        let dir = tempfile::tempdir().unwrap();
        let mut lines = vec![r#"{"working_dir":"/w","id":"s1"}"#.to_string()];
        // 10 one-hit matches first (well past the old limit*4 cutoff)...
        for _ in 0..10 {
            lines.push(
                r#"{"role":"assistant","created":1772000000,"content":[{"type":"text","text":"a recipe note"}]}"#
                    .to_string(),
            );
        }
        // ...then the two-hit match last.
        lines.push(
            r#"{"role":"assistant","created":1772000001,"content":[{"type":"text","text":"the recipe and deploy plan"}]}"#
                .to_string(),
        );
        std::fs::write(dir.path().join("s1.jsonl"), lines.join("\n")).unwrap();

        let mut q = query_for(&["recipe", "deploy"]);
        q.mode = crate::query::MatchMode::Or;
        q.limit = 1;
        let results = GooseSource::search_jsonl(&dir.path().to_path_buf(), &q).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].relevance, 2.0,
            "the late two-hit match must win despite appearing last"
        );
        assert!(results[0].content.contains("deploy"));
    }

    #[test]
    fn search_jsonl_parses_header_and_messages() {
        let dir = tempfile::tempdir().unwrap();
        let session_file = dir.path().join("abc.jsonl");
        let lines = [
            r#"{"id":"sess-xyz","working_dir":"/proj/foo","description":"desc"}"#,
            r#"{"role":"user","created":1772000000,"content":"how do we handle auth"}"#,
            r#"{"role":"assistant","created":1772000100,"content":[{"type":"text","text":"use tokens for auth"}]}"#,
        ];
        std::fs::write(&session_file, lines.join("\n")).unwrap();

        let results =
            GooseSource::search_jsonl(&dir.path().to_path_buf(), &query_for(&["auth"])).unwrap();
        assert_eq!(results.len(), 2);
        // Header id + working_dir are threaded onto results.
        assert!(results
            .iter()
            .all(|r| r.session_id.as_deref() == Some("sess-xyz")));
        assert!(results
            .iter()
            .all(|r| r.session_name.as_deref() == Some("/proj/foo")));
        assert!(results.iter().all(|r| r.source == SourceKind::Goose));
    }
}
