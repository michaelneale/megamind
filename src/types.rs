//! Typed session vocabulary shared across every source.
//!
//! Conversation data from different agents is stringly-typed on disk — each
//! tool spells roles and origins its own way (`"gemini"` means assistant,
//! `"developer"` means system, and so on). These enums give the rest of the
//! crate a single normalized vocabulary to reason about:
//!
//! * [`SourceKind`] — which agent a memory came from (the *type* of session).
//! * [`Role`] — who produced a message within a session.
//!
//! Both serialize to the same lowercase strings the tool emitted before they
//! existed, so JSON output stays backwards compatible.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// The kind of agent session a memory originates from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Goose,
    Claude,
    Pi,
    Codex,
    Gemini,
    Amp,
    OpenCode,
}

impl SourceKind {
    /// Every known source kind, in the order they are discovered.
    pub const ALL: [SourceKind; 7] = [
        SourceKind::Goose,
        SourceKind::Claude,
        SourceKind::Pi,
        SourceKind::Codex,
        SourceKind::Gemini,
        SourceKind::Amp,
        SourceKind::OpenCode,
    ];

    /// Stable lowercase identifier used in JSON output and `--source` filters.
    pub fn id(self) -> &'static str {
        match self {
            SourceKind::Goose => "goose",
            SourceKind::Claude => "claude",
            SourceKind::Pi => "pi",
            SourceKind::Codex => "codex",
            SourceKind::Gemini => "gemini",
            SourceKind::Amp => "amp",
            SourceKind::OpenCode => "opencode",
        }
    }

    /// Human-facing name shown in `remember sources` and result headers.
    pub fn display_name(self) -> &'static str {
        match self {
            SourceKind::Goose => "Goose",
            SourceKind::Claude => "Claude Code",
            SourceKind::Pi => "Pi",
            SourceKind::Codex => "Codex",
            SourceKind::Gemini => "Gemini",
            SourceKind::Amp => "Amp",
            SourceKind::OpenCode => "OpenCode",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for SourceKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_lowercase();
        let kind = match normalized.as_str() {
            "goose" => SourceKind::Goose,
            "claude" | "claude code" | "claude-code" | "claudecode" => SourceKind::Claude,
            "pi" => SourceKind::Pi,
            "codex" => SourceKind::Codex,
            "gemini" => SourceKind::Gemini,
            "amp" => SourceKind::Amp,
            "opencode" | "open-code" | "open code" => SourceKind::OpenCode,
            other => {
                return Err(format!(
                    "unknown source '{other}' (valid: goose, claude, pi, codex, gemini, amp, opencode)"
                ));
            }
        };
        Ok(kind)
    }
}

impl Serialize for SourceKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for SourceKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// Who produced a message within a session.
///
/// Constructed from raw on-disk role strings via [`Role::from_raw`], which
/// normalizes per-agent quirks (e.g. Gemini's `"gemini"` and Codex's
/// `"developer"`). Anything unrecognized is preserved verbatim in
/// [`Role::Other`] so no information is lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
    Other(String),
}

impl Role {
    /// Normalize a raw role string from any source into a [`Role`].
    pub fn from_raw(raw: &str) -> Self {
        match raw.trim().to_lowercase().as_str() {
            "user" | "human" => Role::User,
            "assistant" | "gemini" | "model" | "ai" | "bot" => Role::Assistant,
            "system" | "developer" => Role::System,
            "tool" | "function" => Role::Tool,
            "" | "unknown" => Role::Other("unknown".to_string()),
            _ => Role::Other(raw.trim().to_string()),
        }
    }

    /// Lowercase string form, matching what the tool emitted historically.
    pub fn as_str(&self) -> &str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
            Role::Other(s) => s,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = String;

    /// Strict parse for user-supplied `--role` filters. Only the canonical
    /// roles are accepted here; unknown values are rejected rather than being
    /// silently turned into [`Role::Other`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "user" => Ok(Role::User),
            "assistant" => Ok(Role::Assistant),
            "system" => Ok(Role::System),
            "tool" => Ok(Role::Tool),
            other => Err(format!(
                "unknown role '{other}' (valid: user, assistant, system, tool)"
            )),
        }
    }
}

impl Serialize for Role {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Role::from_raw(&raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kind_ids_are_stable_lowercase() {
        assert_eq!(SourceKind::Goose.id(), "goose");
        assert_eq!(SourceKind::Claude.id(), "claude");
        assert_eq!(SourceKind::OpenCode.id(), "opencode");
        // Every kind's id round-trips through FromStr.
        for kind in SourceKind::ALL {
            assert_eq!(kind.id().parse::<SourceKind>().unwrap(), kind);
        }
    }

    #[test]
    fn source_kind_display_names_match_headers() {
        assert_eq!(SourceKind::Claude.display_name(), "Claude Code");
        assert_eq!(SourceKind::OpenCode.display_name(), "OpenCode");
        assert_eq!(SourceKind::Goose.display_name(), "Goose");
    }

    #[test]
    fn source_kind_parse_accepts_aliases_case_insensitively() {
        assert_eq!("Goose".parse::<SourceKind>().unwrap(), SourceKind::Goose);
        assert_eq!("CLAUDE".parse::<SourceKind>().unwrap(), SourceKind::Claude);
        assert_eq!(
            "claude-code".parse::<SourceKind>().unwrap(),
            SourceKind::Claude
        );
        assert_eq!(
            "  open-code ".parse::<SourceKind>().unwrap(),
            SourceKind::OpenCode
        );
    }

    #[test]
    fn source_kind_parse_rejects_unknown() {
        let err = "vscode".parse::<SourceKind>().unwrap_err();
        assert!(err.contains("unknown source"));
        assert!(err.contains("goose"));
    }

    #[test]
    fn source_kind_serializes_as_id() {
        let json = serde_json::to_string(&SourceKind::OpenCode).unwrap();
        assert_eq!(json, "\"opencode\"");
        let back: SourceKind = serde_json::from_str("\"claude\"").unwrap();
        assert_eq!(back, SourceKind::Claude);
    }

    #[test]
    fn role_from_raw_normalizes_agent_quirks() {
        assert_eq!(Role::from_raw("user"), Role::User);
        assert_eq!(Role::from_raw("USER"), Role::User);
        assert_eq!(Role::from_raw("assistant"), Role::Assistant);
        // Gemini labels the model turn "gemini".
        assert_eq!(Role::from_raw("gemini"), Role::Assistant);
        // Codex uses "developer" for system scaffolding.
        assert_eq!(Role::from_raw("developer"), Role::System);
        assert_eq!(Role::from_raw("tool"), Role::Tool);
    }

    #[test]
    fn role_from_raw_preserves_unknown_values() {
        assert_eq!(Role::from_raw("wizard"), Role::Other("wizard".to_string()));
        // Empty / unknown collapse to a readable placeholder.
        assert_eq!(Role::from_raw(""), Role::Other("unknown".to_string()));
        assert_eq!(
            Role::from_raw("unknown"),
            Role::Other("unknown".to_string())
        );
    }

    #[test]
    fn role_as_str_round_trips_through_serde() {
        for role in [
            Role::User,
            Role::Assistant,
            Role::System,
            Role::Tool,
            Role::Other("custom".to_string()),
        ] {
            let json = serde_json::to_string(&role).unwrap();
            let back: Role = serde_json::from_str(&json).unwrap();
            assert_eq!(role, back);
        }
    }

    #[test]
    fn role_strict_parse_rejects_non_canonical() {
        assert_eq!("user".parse::<Role>().unwrap(), Role::User);
        assert!("gemini".parse::<Role>().is_err());
        assert!("banana".parse::<Role>().is_err());
    }
}
