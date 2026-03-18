use chrono::{DateTime, Utc};

/// Whether all search terms must match (AND) or any of them (OR)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// All terms must be present in a result (default)
    And,
    /// Any term matching is enough
    Or,
}

/// Represents a recall query from the user
#[derive(Debug, Clone)]
pub struct RecallQuery {
    /// Free-text query (optional - can search with just keywords)
    pub text: Option<String>,
    /// Keyword filters
    pub keywords: Vec<String>,
    /// Start of date range filter
    pub after: Option<DateTime<Utc>>,
    /// End of date range filter
    pub before: Option<DateTime<Utc>>,
    /// Maximum results per source
    pub limit: usize,
    /// Whether to AND or OR the search terms
    pub mode: MatchMode,
}

impl RecallQuery {
    /// Build search terms from keywords + query text.
    /// Quoted phrases in query text are kept as single terms.
    /// Individual words are also added.
    pub fn search_terms(&self) -> Vec<String> {
        let mut terms: Vec<String> = self.keywords.iter().map(|k| k.to_lowercase()).collect();

        if let Some(ref text) = self.text {
            // Extract quoted phrases first
            let mut remaining = text.as_str();
            while let Some(start) = remaining.find('"') {
                // Add words before the quote
                for word in remaining[..start].split_whitespace() {
                    let w = word.to_lowercase();
                    if !w.is_empty() && !terms.contains(&w) {
                        terms.push(w);
                    }
                }
                remaining = &remaining[start + 1..];
                if let Some(end) = remaining.find('"') {
                    let phrase = remaining[..end].to_lowercase();
                    if !phrase.is_empty() && !terms.contains(&phrase) {
                        terms.push(phrase);
                    }
                    remaining = &remaining[end + 1..];
                } else {
                    // Unclosed quote — treat rest as words
                    break;
                }
            }
            // Add remaining words after last quote (or all words if no quotes)
            for word in remaining.split_whitespace() {
                let w = word.to_lowercase();
                if !w.is_empty() && !terms.contains(&w) {
                    terms.push(w);
                }
            }
        }

        terms
    }

    /// Check if a piece of text matches this query's search terms
    /// respecting the AND/OR mode. Returns (matches, hit_count).
    pub fn matches_text(&self, text: &str) -> (bool, usize) {
        let terms = self.search_terms();
        if terms.is_empty() {
            return (true, 0);
        }

        let text_lower = text.to_lowercase();
        let hit_count = terms.iter()
            .filter(|t| text_lower.contains(t.as_str()))
            .count();

        let matches = match self.mode {
            MatchMode::And => hit_count == terms.len(),
            MatchMode::Or => hit_count > 0,
        };

        (matches, hit_count)
    }

    /// Returns true if there are any search constraints
    pub fn has_constraints(&self) -> bool {
        self.text.is_some() || !self.keywords.is_empty() || self.after.is_some() || self.before.is_some()
    }

    /// Build a cache key from this query for result caching
    pub fn cache_key(&self) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();

        let mut terms = self.search_terms();
        terms.sort();
        for t in &terms {
            hasher.update(t.as_bytes());
        }
        // Include match mode in cache key
        match self.mode {
            MatchMode::And => hasher.update(b"AND"),
            MatchMode::Or => hasher.update(b"OR"),
        };
        if let Some(ref after) = self.after {
            hasher.update(after.to_rfc3339().as_bytes());
        }
        if let Some(ref before) = self.before {
            hasher.update(before.to_rfc3339().as_bytes());
        }
        hasher.update(self.limit.to_le_bytes());

        hex::encode(hasher.finalize())
    }

    /// Build SQL WHERE clause for LIKE matching, respecting AND/OR mode.
    /// Returns (clause_string, params) where params are the `%term%` values.
    pub fn sql_like_clause(&self, column: &str) -> (String, Vec<String>) {
        let terms = self.search_terms();
        if terms.is_empty() {
            return (String::new(), vec![]);
        }

        let joiner = match self.mode {
            MatchMode::And => " AND ",
            MatchMode::Or => " OR ",
        };

        let conditions: Vec<String> = terms.iter()
            .map(|_| format!("{} LIKE ?", column))
            .collect();

        let params: Vec<String> = terms.iter()
            .map(|t| format!("%{}%", t))
            .collect();

        (format!("({})", conditions.join(joiner)), params)
    }
}
