use crate::results::RecallResults;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// TTL for cache entries (5 minutes)
const CACHE_TTL_SECS: i64 = 300;

/// Maximum cache entries before eviction
const MAX_CACHE_ENTRIES: usize = 100;

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    results: RecallResults,
    created_at: DateTime<Utc>,
}

/// Simple file-backed + in-memory cache for recall results.
/// Keys are SHA256 hashes of normalized query parameters.
pub struct ResultCache {
    cache_dir: PathBuf,
    memory: Mutex<HashMap<String, CacheEntry>>,
}

impl ResultCache {
    pub fn new() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".cache"))
            .join("remember");

        std::fs::create_dir_all(&cache_dir).ok();

        Self {
            cache_dir,
            memory: Mutex::new(HashMap::new()),
        }
    }

    /// Try to get cached results for this query key
    pub fn get(&self, key: &str) -> Option<RecallResults> {
        let now = Utc::now();

        // Check in-memory first
        if let Ok(mem) = self.memory.lock() {
            if let Some(entry) = mem.get(key) {
                if (now - entry.created_at).num_seconds() < CACHE_TTL_SECS {
                    let mut results = entry.results.clone();
                    results.from_cache = true;
                    return Some(results);
                }
            }
        }

        // Check file cache
        let path = self.cache_dir.join(format!("{}.json", key));
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(entry) = serde_json::from_str::<CacheEntry>(&data) {
                if (now - entry.created_at).num_seconds() < CACHE_TTL_SECS {
                    // Promote to memory cache
                    if let Ok(mut mem) = self.memory.lock() {
                        mem.insert(key.to_string(), CacheEntry {
                            results: entry.results.clone(),
                            created_at: entry.created_at,
                        });
                    }
                    let mut results = entry.results;
                    results.from_cache = true;
                    return Some(results);
                } else {
                    // Expired - remove
                    std::fs::remove_file(&path).ok();
                }
            }
        }

        None
    }

    /// Store results in cache
    pub fn put(&self, key: &str, results: &RecallResults) -> Result<()> {
        let entry = CacheEntry {
            results: results.clone(),
            created_at: Utc::now(),
        };

        // Write to memory
        if let Ok(mut mem) = self.memory.lock() {
            // Evict oldest if too many
            if mem.len() >= MAX_CACHE_ENTRIES {
                if let Some(oldest_key) = mem.iter()
                    .min_by_key(|(_, v)| v.created_at)
                    .map(|(k, _)| k.clone())
                {
                    mem.remove(&oldest_key);
                }
            }
            mem.insert(key.to_string(), CacheEntry {
                results: results.clone(),
                created_at: Utc::now(),
            });
        }

        // Write to file
        let path = self.cache_dir.join(format!("{}.json", key));
        let data = serde_json::to_string(&entry)?;
        std::fs::write(&path, data)?;

        Ok(())
    }

    /// Clear all cached results
    pub fn clear(&self) -> Result<()> {
        if let Ok(mut mem) = self.memory.lock() {
            mem.clear();
        }

        if self.cache_dir.exists() {
            for entry in std::fs::read_dir(&self.cache_dir)? {
                let entry = entry?;
                if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                    std::fs::remove_file(entry.path()).ok();
                }
            }
        }

        Ok(())
    }
}
