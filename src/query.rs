use crate::types::{Role, SourceKind};
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
    /// Restrict to these session types (empty = all sources)
    pub sources: Vec<SourceKind>,
    /// Restrict to these roles (empty = all roles)
    pub roles: Vec<Role>,
    /// Loose (substring) matching. When false (default), terms must match on
    /// word boundaries so `auth` no longer matches `authenticate`.
    pub loose: bool,
}

impl Default for RecallQuery {
    fn default() -> Self {
        Self {
            text: None,
            keywords: Vec::new(),
            after: None,
            before: None,
            limit: 20,
            mode: MatchMode::And,
            sources: Vec::new(),
            roles: Vec::new(),
            loose: false,
        }
    }
}

/// A "word character" for boundary detection: Unicode alphanumerics plus `_`,
/// matching the conventional regex `\w` class.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `term` occurs in `haystack` respecting word boundaries. Both inputs
/// are expected to already be lowercased. `term` may contain interior spaces
/// (a quoted phrase); only its outer edges are boundary-checked.
///
/// A boundary is enforced on an edge only when that edge of the term is itself
/// a word character — so punctuation-bearing terms (e.g. `c++`) still match.
fn contains_word_bounded(haystack: &str, term: &str) -> bool {
    let first = match term.chars().next() {
        Some(c) => c,
        None => return false,
    };
    let last = term.chars().next_back().unwrap();
    let check_left = is_word_char(first);
    let check_right = is_word_char(last);

    for (idx, matched) in haystack.match_indices(term) {
        let left_ok = idx == 0
            || !check_left
            || !haystack[..idx]
                .chars()
                .next_back()
                .map(is_word_char)
                .unwrap_or(false);
        if !left_ok {
            continue;
        }

        let end = idx + matched.len();
        let right_ok = end >= haystack.len()
            || !check_right
            || !haystack[end..]
                .chars()
                .next()
                .map(is_word_char)
                .unwrap_or(false);
        if right_ok {
            return true;
        }
    }

    false
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
    ///
    /// By default terms must match on word boundaries; `loose` restores plain
    /// substring matching.
    pub fn matches_text(&self, text: &str) -> (bool, usize) {
        let terms = self.search_terms();
        if terms.is_empty() {
            return (true, 0);
        }

        let text_lower = text.to_lowercase();
        let hit_count = terms
            .iter()
            .filter(|t| self.term_matches(&text_lower, t))
            .count();

        let matches = match self.mode {
            MatchMode::And => hit_count == terms.len(),
            MatchMode::Or => hit_count > 0,
        };

        (matches, hit_count)
    }

    /// Whether a single (already-lowercased) term matches the (already-
    /// lowercased) text, honoring the query's `loose` setting.
    fn term_matches(&self, text_lower: &str, term: &str) -> bool {
        if self.loose {
            text_lower.contains(term)
        } else {
            contains_word_bounded(text_lower, term)
        }
    }

    /// Returns true if there are any search constraints
    pub fn has_constraints(&self) -> bool {
        self.text.is_some()
            || !self.keywords.is_empty()
            || self.after.is_some()
            || self.before.is_some()
    }

    /// Whether the given source kind should be queried under this query's
    /// `--source` filter. An empty filter matches every source.
    pub fn wants_source(&self, source: SourceKind) -> bool {
        self.sources.is_empty() || self.sources.contains(&source)
    }

    /// Whether a result with the given role passes this query's `--role`
    /// filter. An empty filter matches every role; results with no role are
    /// only kept when no role filter is active.
    pub fn wants_role(&self, role: Option<&Role>) -> bool {
        if self.roles.is_empty() {
            return true;
        }
        match role {
            Some(role) => self.roles.contains(role),
            None => false,
        }
    }

    /// Build a cache key from this query for result caching
    pub fn cache_key(&self) -> String {
        use sha2::{Digest, Sha256};
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
        // Loose vs word-boundary matching yields different results.
        hasher.update(if self.loose {
            b"loose".as_slice()
        } else {
            b"strict".as_slice()
        });
        if let Some(ref after) = self.after {
            hasher.update(after.to_rfc3339().as_bytes());
        }
        if let Some(ref before) = self.before {
            hasher.update(before.to_rfc3339().as_bytes());
        }
        hasher.update(self.limit.to_le_bytes());

        // Fold in source/role filters so a narrowed query never collides with
        // a broader cached result. Sort first so ordering is irrelevant.
        let mut sources: Vec<&str> = self.sources.iter().map(|s| s.id()).collect();
        sources.sort_unstable();
        hasher.update(b"sources:");
        for s in &sources {
            hasher.update(s.as_bytes());
            hasher.update(b",");
        }
        let mut roles: Vec<String> = self.roles.iter().map(|r| r.as_str().to_string()).collect();
        roles.sort_unstable();
        hasher.update(b"roles:");
        for r in &roles {
            hasher.update(r.as_bytes());
            hasher.update(b",");
        }

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

        let conditions: Vec<String> = terms.iter().map(|_| format!("{} LIKE ?", column)).collect();

        let params: Vec<String> = terms.iter().map(|t| format!("%{}%", t)).collect();

        (format!("({})", conditions.join(joiner)), params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_with_text(text: &str) -> RecallQuery {
        RecallQuery {
            text: Some(text.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn search_terms_splits_words_and_lowercases() {
        let q = query_with_text("Auth Approach");
        assert_eq!(q.search_terms(), vec!["auth", "approach"]);
    }

    #[test]
    fn search_terms_keeps_quoted_phrases_intact() {
        let q = query_with_text("before \"exact phrase\" after");
        let terms = q.search_terms();
        assert!(terms.contains(&"exact phrase".to_string()));
        assert!(terms.contains(&"before".to_string()));
        assert!(terms.contains(&"after".to_string()));
    }

    #[test]
    fn search_terms_merges_keywords_and_dedupes() {
        let q = RecallQuery {
            text: Some("rust sqlite".to_string()),
            keywords: vec!["rust".to_string(), "SQLITE".to_string()],
            ..Default::default()
        };
        let terms = q.search_terms();
        assert_eq!(terms.iter().filter(|t| *t == "rust").count(), 1);
        assert_eq!(terms.iter().filter(|t| *t == "sqlite").count(), 1);
    }

    #[test]
    fn search_terms_handles_unclosed_quote() {
        let q = query_with_text("start \"unclosed rest");
        let terms = q.search_terms();
        // Falls back to treating the remainder as individual words.
        assert!(terms.contains(&"start".to_string()));
        assert!(terms.contains(&"unclosed".to_string()));
        assert!(terms.contains(&"rest".to_string()));
    }

    #[test]
    fn matches_text_and_mode_requires_all_terms() {
        let q = RecallQuery {
            keywords: vec!["rust".to_string(), "sqlite".to_string()],
            mode: MatchMode::And,
            ..Default::default()
        };
        assert_eq!(q.matches_text("rust and sqlite together"), (true, 2));
        assert!(!q.matches_text("rust only").0);
    }

    #[test]
    fn matches_text_or_mode_requires_any_term() {
        let q = RecallQuery {
            keywords: vec!["rust".to_string(), "sqlite".to_string()],
            mode: MatchMode::Or,
            ..Default::default()
        };
        assert_eq!(q.matches_text("rust only"), (true, 1));
        assert!(!q.matches_text("nothing here").0);
    }

    #[test]
    fn matches_text_empty_query_matches_everything() {
        let q = RecallQuery::default();
        assert_eq!(q.matches_text("anything"), (true, 0));
    }

    #[test]
    fn word_boundary_is_the_default() {
        let q = RecallQuery {
            keywords: vec!["auth".to_string()],
            ..Default::default()
        };
        // Whole-word hit.
        assert!(q.matches_text("what auth approach").0);
        // Substring-only occurrences must NOT match by default.
        assert!(!q.matches_text("we authenticate users").0);
        assert!(!q.matches_text("the author wrote").0);
    }

    #[test]
    fn word_boundary_respects_punctuation_edges() {
        let q = RecallQuery {
            keywords: vec!["auth".to_string()],
            ..Default::default()
        };
        // Adjacent punctuation is a boundary, so these still match.
        assert!(q.matches_text("(auth)").0);
        assert!(q.matches_text("auth.rs uses it").0);
        assert!(q.matches_text("re-auth, please").0);
    }

    #[test]
    fn loose_mode_restores_substring_matching() {
        let q = RecallQuery {
            keywords: vec!["auth".to_string()],
            loose: true,
            ..Default::default()
        };
        assert!(q.matches_text("we authenticate users").0);
        assert!(q.matches_text("the author wrote").0);
    }

    #[test]
    fn word_boundary_applies_to_quoted_phrases() {
        let q = RecallQuery {
            text: Some("\"auth token\"".to_string()),
            ..Default::default()
        };
        assert!(q.matches_text("the auth token expired").0);
        // Phrase boundary respected on the right edge.
        assert!(!q.matches_text("auth tokenizer").0);
    }

    #[test]
    fn word_boundary_matches_terms_with_symbols() {
        // A term whose edges are non-word chars should not demand a boundary
        // there, so symbol-bearing terms still match.
        assert!(contains_word_bounded("i love c++ a lot", "c++"));
        assert!(contains_word_bounded("prefix .env suffix", ".env"));
    }

    #[test]
    fn word_boundary_unicode_aware() {
        let q = RecallQuery {
            keywords: vec!["café".to_string()],
            ..Default::default()
        };
        assert!(q.matches_text("at the café today").0);
        // Extra combining/adjacent word char defeats the boundary.
        assert!(!q.matches_text("cafés everywhere").0);
    }

    #[test]
    fn has_constraints_detects_each_dimension() {
        assert!(!RecallQuery::default().has_constraints());
        assert!(query_with_text("x").has_constraints());
        assert!(RecallQuery {
            keywords: vec!["k".to_string()],
            ..Default::default()
        }
        .has_constraints());
    }

    #[test]
    fn wants_source_respects_filter() {
        let unfiltered = RecallQuery::default();
        assert!(unfiltered.wants_source(SourceKind::Goose));
        assert!(unfiltered.wants_source(SourceKind::Amp));

        let filtered = RecallQuery {
            sources: vec![SourceKind::Goose, SourceKind::Pi],
            ..Default::default()
        };
        assert!(filtered.wants_source(SourceKind::Goose));
        assert!(filtered.wants_source(SourceKind::Pi));
        assert!(!filtered.wants_source(SourceKind::Amp));
    }

    #[test]
    fn wants_role_respects_filter() {
        let unfiltered = RecallQuery::default();
        assert!(unfiltered.wants_role(Some(&Role::User)));
        assert!(unfiltered.wants_role(None));

        let filtered = RecallQuery {
            roles: vec![Role::User],
            ..Default::default()
        };
        assert!(filtered.wants_role(Some(&Role::User)));
        assert!(!filtered.wants_role(Some(&Role::Assistant)));
        // A role filter excludes results that have no role at all.
        assert!(!filtered.wants_role(None));
    }

    #[test]
    fn sql_like_clause_builds_parameterized_conditions() {
        let q = RecallQuery {
            keywords: vec!["rust".to_string(), "sqlite".to_string()],
            mode: MatchMode::And,
            ..Default::default()
        };
        let (clause, params) = q.sql_like_clause("m.content_json");
        assert_eq!(clause, "(m.content_json LIKE ? AND m.content_json LIKE ?)");
        assert_eq!(params, vec!["%rust%".to_string(), "%sqlite%".to_string()]);
    }

    #[test]
    fn sql_like_clause_or_mode_uses_or_joiner() {
        let q = RecallQuery {
            keywords: vec!["a".to_string(), "b".to_string()],
            mode: MatchMode::Or,
            ..Default::default()
        };
        let (clause, _) = q.sql_like_clause("c");
        assert_eq!(clause, "(c LIKE ? OR c LIKE ?)");
    }

    #[test]
    fn cache_key_is_stable_and_order_independent() {
        let a = RecallQuery {
            keywords: vec!["rust".to_string(), "sqlite".to_string()],
            ..Default::default()
        };
        let b = RecallQuery {
            keywords: vec!["sqlite".to_string(), "rust".to_string()],
            ..Default::default()
        };
        assert_eq!(a.cache_key(), b.cache_key());
    }

    #[test]
    fn cache_key_changes_with_mode() {
        let and = RecallQuery {
            keywords: vec!["a".to_string()],
            mode: MatchMode::And,
            ..Default::default()
        };
        let or = RecallQuery {
            mode: MatchMode::Or,
            ..and.clone()
        };
        assert_ne!(and.cache_key(), or.cache_key());
    }

    #[test]
    fn cache_key_distinguishes_source_filter() {
        let base = RecallQuery {
            keywords: vec!["deploy".to_string()],
            ..Default::default()
        };
        let filtered = RecallQuery {
            sources: vec![SourceKind::Goose],
            ..base.clone()
        };
        // A source-narrowed query must not collide with the broad query.
        assert_ne!(base.cache_key(), filtered.cache_key());
    }

    #[test]
    fn cache_key_distinguishes_role_filter() {
        let base = RecallQuery {
            keywords: vec!["deploy".to_string()],
            ..Default::default()
        };
        let filtered = RecallQuery {
            roles: vec![Role::User],
            ..base.clone()
        };
        assert_ne!(base.cache_key(), filtered.cache_key());
    }

    #[test]
    fn cache_key_source_filter_order_independent() {
        let a = RecallQuery {
            sources: vec![SourceKind::Goose, SourceKind::Pi],
            ..Default::default()
        };
        let b = RecallQuery {
            sources: vec![SourceKind::Pi, SourceKind::Goose],
            ..Default::default()
        };
        assert_eq!(a.cache_key(), b.cache_key());
    }
}
