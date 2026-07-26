use crate::results::RecallResults;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

/// Maximum total size of the on-disk cache before eviction (500 MB).
///
/// The cache has no time-based expiry: entries live until the on-disk footprint
/// exceeds this budget, at which point the oldest entries are evicted. Repeat
/// queries therefore stay warm indefinitely as long as the cache stays under
/// budget; `remember clear-cache` wipes it on demand.
const MAX_CACHE_BYTES: u64 = 500 * 1024 * 1024;

/// Cap on the in-memory tier (per process). Only meaningful for the long-lived
/// MCP server; the CLI is a fresh process per invocation and reads from disk.
const MAX_MEMORY_ENTRIES: usize = 256;

/// An in-memory cache entry. Memory-only, so it can track a non-serializable
/// last-access instant for LRU eviction within a process.
struct MemEntry {
    results: RecallResults,
    last_access: Instant,
}

/// File-backed + in-memory cache for recall results.
///
/// Keys are SHA256 hashes of normalized query parameters. On disk each entry is
/// a bare `RecallResults` JSON document named `<key>.json`; freshness is not
/// tracked, so the only bound is the total-size budget enforced after writes.
pub struct ResultCache {
    cache_dir: PathBuf,
    max_bytes: u64,
    memory: Mutex<HashMap<String, MemEntry>>,
}

impl ResultCache {
    pub fn new() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".cache"))
            .join("remember");

        Self::with_dir_and_budget(cache_dir, MAX_CACHE_BYTES)
    }

    /// Construct a cache rooted at a specific directory with a specific disk
    /// budget. Used by `new` and by tests (which need a temp dir + tiny budget).
    fn with_dir_and_budget(cache_dir: PathBuf, max_bytes: u64) -> Self {
        std::fs::create_dir_all(&cache_dir).ok();
        Self {
            cache_dir,
            max_bytes,
            memory: Mutex::new(HashMap::new()),
        }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.json", key))
    }

    /// Try to get cached results for this query key.
    pub fn get(&self, key: &str) -> Option<RecallResults> {
        // In-memory tier first (bumping recency for LRU).
        if let Ok(mut mem) = self.memory.lock() {
            if let Some(entry) = mem.get_mut(key) {
                entry.last_access = Instant::now();
                let mut results = entry.results.clone();
                results.from_cache = true;
                return Some(results);
            }
        }

        // Disk tier.
        let path = self.path_for(key);
        let data = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<RecallResults>(&data) {
            Ok(results) => {
                // Promote to the memory tier.
                if let Ok(mut mem) = self.memory.lock() {
                    insert_memory(&mut mem, key, results.clone());
                }
                let mut results = results;
                results.from_cache = true;
                Some(results)
            }
            Err(_) => {
                // Unparseable (e.g. a stale format from an older build) — drop it.
                std::fs::remove_file(&path).ok();
                None
            }
        }
    }

    /// Store results in cache and enforce the disk budget.
    pub fn put(&self, key: &str, results: &RecallResults) -> Result<()> {
        if let Ok(mut mem) = self.memory.lock() {
            insert_memory(&mut mem, key, results.clone());
        }

        let path = self.path_for(key);
        let data = serde_json::to_string(results)?;
        std::fs::write(&path, data)?;

        self.enforce_disk_budget();
        Ok(())
    }

    /// Evict oldest-written entries until the on-disk cache is within budget.
    fn enforce_disk_budget(&self) {
        let entries = match std::fs::read_dir(&self.cache_dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
        let mut total: u64 = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(meta) = entry.metadata() {
                    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    total += meta.len();
                    files.push((path, meta.len(), mtime));
                }
            }
        }

        if total <= self.max_bytes {
            return;
        }

        // Oldest first, so the most recently written entries survive.
        files.sort_by_key(|(_, _, mtime)| *mtime);
        for (path, size, _) in files {
            if total <= self.max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }

    /// Clear all cached results (memory + disk).
    pub fn clear(&self) -> Result<()> {
        if let Ok(mut mem) = self.memory.lock() {
            mem.clear();
        }

        if self.cache_dir.exists() {
            for entry in std::fs::read_dir(&self.cache_dir)? {
                let entry = entry?;
                if entry
                    .path()
                    .extension()
                    .map(|e| e == "json")
                    .unwrap_or(false)
                {
                    std::fs::remove_file(entry.path()).ok();
                }
            }
        }

        Ok(())
    }
}

/// Insert into the memory tier, evicting the least-recently-accessed entry when
/// the tier is full.
fn insert_memory(mem: &mut HashMap<String, MemEntry>, key: &str, results: RecallResults) {
    if !mem.contains_key(key) && mem.len() >= MAX_MEMORY_ENTRIES {
        if let Some(lru_key) = mem
            .iter()
            .min_by_key(|(_, v)| v.last_access)
            .map(|(k, _)| k.clone())
        {
            mem.remove(&lru_key);
        }
    }
    mem.insert(
        key.to_string(),
        MemEntry {
            results,
            last_access: Instant::now(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results_of_size(approx_bytes: usize) -> RecallResults {
        // query_summary is free-form text; pad it to hit a target on-disk size.
        RecallResults {
            query_summary: "x".repeat(approx_bytes),
            sources: vec![],
            total_results: 0,
            total_time_ms: 1,
            from_cache: false,
        }
    }

    fn total_disk_bytes(dir: &std::path::Path) -> u64 {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .map(|e| e.metadata().unwrap().len())
            .sum()
    }

    #[test]
    fn put_then_get_round_trips_and_marks_cache_hit() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::with_dir_and_budget(dir.path().to_path_buf(), MAX_CACHE_BYTES);

        let results = results_of_size(10);
        cache.put("key1", &results).unwrap();

        let got = cache.get("key1").expect("should be cached");
        assert!(got.from_cache);
        assert_eq!(got.query_summary, results.query_summary);
    }

    #[test]
    fn get_miss_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::with_dir_and_budget(dir.path().to_path_buf(), MAX_CACHE_BYTES);
        assert!(cache.get("nope").is_none());
    }

    #[test]
    fn get_survives_fresh_process_via_disk() {
        let dir = tempfile::tempdir().unwrap();
        // First "process" writes.
        {
            let cache = ResultCache::with_dir_and_budget(dir.path().to_path_buf(), MAX_CACHE_BYTES);
            cache.put("shared", &results_of_size(10)).unwrap();
        }
        // Second "process" (empty memory tier) still finds it on disk.
        let cache = ResultCache::with_dir_and_budget(dir.path().to_path_buf(), MAX_CACHE_BYTES);
        assert!(cache.get("shared").is_some());
    }

    #[test]
    fn clear_removes_disk_and_memory() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::with_dir_and_budget(dir.path().to_path_buf(), MAX_CACHE_BYTES);
        cache.put("k", &results_of_size(10)).unwrap();
        assert!(cache.get("k").is_some());

        cache.clear().unwrap();
        assert!(cache.get("k").is_none());
        assert_eq!(total_disk_bytes(dir.path()), 0);
    }

    #[test]
    fn disk_budget_is_enforced_after_writes() {
        let dir = tempfile::tempdir().unwrap();
        // Tiny 20 KB budget; each entry is ~10 KB of payload.
        let budget = 20 * 1024;
        let cache = ResultCache::with_dir_and_budget(dir.path().to_path_buf(), budget);

        for i in 0..50 {
            cache
                .put(&format!("key{i}"), &results_of_size(10 * 1024))
                .unwrap();
        }

        // The core invariant: disk footprint stays bounded regardless of how
        // many distinct queries were cached.
        let on_disk = total_disk_bytes(dir.path());
        assert!(
            on_disk <= budget,
            "on-disk {on_disk} exceeded budget {budget}"
        );
        // And the most recently written entry is still retrievable.
        assert!(cache.get("key49").is_some());
    }

    #[test]
    fn stale_format_file_is_dropped_on_get() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::with_dir_and_budget(dir.path().to_path_buf(), MAX_CACHE_BYTES);

        // Write an old-style wrapper document that no longer deserializes into
        // a bare RecallResults.
        let path = dir.path().join("legacy.json");
        std::fs::write(
            &path,
            r#"{"results":{},"created_at":"2020-01-01T00:00:00Z"}"#,
        )
        .unwrap();

        assert!(cache.get("legacy").is_none());
        assert!(!path.exists(), "stale file should be removed");
    }
}
