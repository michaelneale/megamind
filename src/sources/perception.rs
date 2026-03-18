use crate::query::RecallQuery;
use crate::results::{MemoryResult, SourceResults};
use crate::sources::MemorySource;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::Connection;
use std::path::PathBuf;
use std::time::Instant;

/// GoosePerception data: screen captures (OCR), voice transcripts, face events, insights
pub struct PerceptionSource {
    db_path: PathBuf,
}

impl PerceptionSource {
    pub fn new() -> Self {
        let app_support = dirs::data_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join("Library/Application Support"));
        Self {
            db_path: app_support.join("GoosePerception/perception.sqlite"),
        }
    }

    fn parse_timestamp(s: &str) -> DateTime<Utc> {
        // Try RFC3339 first, then naive datetime
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| {
                NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                    .map(|ndt| ndt.and_utc())
                    .unwrap_or_else(|_| Utc::now())
            })
    }
}

#[async_trait]
impl MemorySource for PerceptionSource {
    fn name(&self) -> &str {
        "Perception"
    }

    fn is_available(&self) -> bool {
        self.db_path.exists()
    }

    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults> {
        let start = Instant::now();
        let db_path = self.db_path.clone();
        let query = query.clone();

        let results = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<MemoryResult>> {
            let conn = Connection::open_with_flags(
                &db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;

            let search_terms = query.search_terms();
            let mut all_results = Vec::new();

            // Search screen captures (OCR text)
            all_results.extend(search_screen_captures(&conn, &query, &search_terms)?);

            // Search voice segments
            all_results.extend(search_voice_segments(&conn, &query, &search_terms)?);

            // Search insights
            all_results.extend(search_insights(&conn, &query, &search_terms)?);

            // Search face events (emotion data) - only if relevant keywords
            let emotion_keywords = ["mood", "emotion", "happy", "sad", "angry", "stressed",
                "neutral", "surprise", "fear", "disgust", "face", "feeling", "wellness"];
            let has_emotion_query = search_terms.iter().any(|t| {
                emotion_keywords.iter().any(|ek| t.to_lowercase().contains(ek))
            });
            if has_emotion_query || (search_terms.is_empty() && (query.after.is_some() || query.before.is_some())) {
                all_results.extend(search_face_events(&conn, &query, &search_terms)?);
            }

            // Sort by relevance then timestamp
            all_results.sort_by(|a, b| {
                b.relevance.partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.timestamp.cmp(&a.timestamp))
            });

            all_results.truncate(query.limit);
            Ok(all_results)
        }).await??;

        let elapsed = start.elapsed().as_millis() as u64;
        let total = results.len();

        Ok(SourceResults {
            source_name: "Perception".to_string(),
            results,
            total_matched: total,
            search_time_ms: elapsed,
            error: None,
        })
    }
}

fn build_like_conditions(field: &str, query: &RecallQuery, params: &mut Vec<Box<dyn rusqlite::types::ToSql>>) -> String {
    let (clause, like_params) = query.sql_like_clause(field);
    for p in like_params {
        params.push(Box::new(p));
    }
    clause
}

fn add_date_conditions(conditions: &mut Vec<String>, params: &mut Vec<Box<dyn rusqlite::types::ToSql>>, 
                       field: &str, query: &RecallQuery) {
    if let Some(ref after) = query.after {
        conditions.push(format!("{} >= ?", field));
        params.push(Box::new(after.to_rfc3339()));
    }
    if let Some(ref before) = query.before {
        conditions.push(format!("{} <= ?", field));
        params.push(Box::new(before.to_rfc3339()));
    }
}

fn search_screen_captures(conn: &Connection, query: &RecallQuery, _terms: &[String]) -> anyhow::Result<Vec<MemoryResult>> {
    let mut conditions = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    let like_cond = build_like_conditions("ocr_text", query, &mut params);
    if !like_cond.is_empty() {
        conditions.push(like_cond);
    }
    add_date_conditions(&mut conditions, &mut params, "timestamp", query);
    conditions.push("ocr_text IS NOT NULL AND ocr_text != ''".to_string());

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT timestamp, focused_app, focused_window, ocr_text FROM screen_captures {} ORDER BY timestamp DESC LIMIT ?",
        where_clause
    );
    params.push(Box::new(query.limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        let ts: String = row.get(0)?;
        let app: Option<String> = row.get(1)?;
        let window: Option<String> = row.get(2)?;
        let ocr: String = row.get(3)?;
        Ok((ts, app, window, ocr))
    })?;

    let mut results = Vec::new();
    for row in rows {
        let (ts, app, window, ocr) = row?;
        let timestamp = PerceptionSource::parse_timestamp(&ts);
        
        let (_, hit_count) = query.matches_text(&ocr);
        let relevance = hit_count as f64;

        let app_info = match (app.as_deref(), window.as_deref()) {
            (Some(a), Some(w)) => format!("{} - {}", a, w),
            (Some(a), None) => a.to_string(),
            _ => "Unknown".to_string(),
        };

        // Truncate OCR for display
        let content = if ocr.len() > 800 {
            format!("[Screen: {}] {}...", app_info, &ocr[..800])
        } else {
            format!("[Screen: {}] {}", app_info, ocr)
        };

        results.push(MemoryResult {
            source: "perception/screen".to_string(),
            timestamp,
            content,
            role: None,
            session_id: None,
            session_name: Some(app_info),
            relevance,
            metadata: None,
        });
    }

    Ok(results)
}

fn search_voice_segments(conn: &Connection, query: &RecallQuery, _terms: &[String]) -> anyhow::Result<Vec<MemoryResult>> {
    let mut conditions = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    let like_cond = build_like_conditions("transcript", query, &mut params);
    if !like_cond.is_empty() {
        conditions.push(like_cond);
    }
    add_date_conditions(&mut conditions, &mut params, "timestamp", query);

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT timestamp, transcript, confidence FROM voice_segments {} ORDER BY timestamp DESC LIMIT ?",
        where_clause
    );
    params.push(Box::new(query.limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        let ts: String = row.get(0)?;
        let transcript: String = row.get(1)?;
        let confidence: Option<f64> = row.get(2)?;
        Ok((ts, transcript, confidence))
    })?;

    let mut results = Vec::new();
    for row in rows {
        let (ts, transcript, confidence) = row?;
        let timestamp = PerceptionSource::parse_timestamp(&ts);
        
        let (_, hit_count) = query.matches_text(&transcript);
        let relevance = hit_count as f64;

        let conf_str = confidence.map(|c| format!(" (conf: {:.0}%)", c * 100.0)).unwrap_or_default();

        results.push(MemoryResult {
            source: "perception/voice".to_string(),
            timestamp,
            content: format!("[Voice{}] {}", conf_str, transcript),
            role: None,
            session_id: None,
            session_name: None,
            relevance,
            metadata: confidence.map(|c| serde_json::json!({"confidence": c})),
        });
    }

    Ok(results)
}

fn search_insights(conn: &Connection, query: &RecallQuery, _terms: &[String]) -> anyhow::Result<Vec<MemoryResult>> {
    let mut conditions = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    let like_cond = build_like_conditions("content", query, &mut params);
    if !like_cond.is_empty() {
        conditions.push(like_cond);
    }
    add_date_conditions(&mut conditions, &mut params, "created_at", query);

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT created_at, type, content FROM insights {} ORDER BY created_at DESC LIMIT ?",
        where_clause
    );
    params.push(Box::new(query.limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        let ts: String = row.get(0)?;
        let insight_type: String = row.get(1)?;
        let content: String = row.get(2)?;
        Ok((ts, insight_type, content))
    })?;

    let mut results = Vec::new();
    for row in rows {
        let (ts, insight_type, content) = row?;
        let timestamp = PerceptionSource::parse_timestamp(&ts);
        
        let (_, hit_count) = query.matches_text(&content);
        let relevance = hit_count as f64 + 0.5; // Boost insights slightly

        results.push(MemoryResult {
            source: "perception/insight".to_string(),
            timestamp,
            content: format!("[Insight/{}] {}", insight_type, content),
            role: None,
            session_id: None,
            session_name: None,
            relevance,
            metadata: Some(serde_json::json!({"insight_type": insight_type})),
        });
    }

    Ok(results)
}

fn search_face_events(conn: &Connection, query: &RecallQuery, _terms: &[String]) -> anyhow::Result<Vec<MemoryResult>> {
    let mut conditions = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    conditions.push("emotion IS NOT NULL AND emotion != ''".to_string());
    conditions.push("present = 1".to_string());
    add_date_conditions(&mut conditions, &mut params, "timestamp", query);

    let where_clause = format!("WHERE {}", conditions.join(" AND "));

    // Group emotions by hour for a summary view
    let sql = format!(
        r#"SELECT 
            strftime('%Y-%m-%d %H:00:00', timestamp) as hour,
            emotion,
            COUNT(*) as count,
            AVG(confidence) as avg_conf
        FROM face_events
        {}
        GROUP BY hour, emotion
        ORDER BY hour DESC
        LIMIT ?"#,
        where_clause
    );
    params.push(Box::new(query.limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        let hour: String = row.get(0)?;
        let emotion: String = row.get(1)?;
        let count: i64 = row.get(2)?;
        let avg_conf: f64 = row.get(3)?;
        Ok((hour, emotion, count, avg_conf))
    })?;

    let mut results = Vec::new();
    for row in rows {
        let (hour, emotion, count, avg_conf) = row?;
        let timestamp = PerceptionSource::parse_timestamp(&hour);

        results.push(MemoryResult {
            source: "perception/mood".to_string(),
            timestamp,
            content: format!("[Mood] Detected '{}' {} times (avg confidence: {:.0}%)", 
                emotion, count, avg_conf * 100.0),
            role: None,
            session_id: None,
            session_name: None,
            relevance: 0.3,
            metadata: Some(serde_json::json!({
                "emotion": emotion,
                "count": count,
                "avg_confidence": avg_conf,
            })),
        });
    }

    Ok(results)
}
