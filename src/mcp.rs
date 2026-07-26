use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::io::stdio,
    ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::RecallEngine;
use crate::query::{MatchMode, RecallQuery};
use crate::types::{Role, SourceKind};

/// Parameters for the remember tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RememberParams {
    /// Free-text search query (e.g. "what was the auth approach we discussed")
    #[serde(default)]
    pub query: Option<String>,

    /// Keyword filters — results must contain these terms
    #[serde(default)]
    pub keywords: Vec<String>,

    /// Only return results after this date (YYYY-MM-DD)
    #[serde(default)]
    pub after: Option<String>,

    /// Only return results before this date (YYYY-MM-DD)
    #[serde(default)]
    pub before: Option<String>,

    /// Maximum results per source (default: 20)
    #[serde(default)]
    pub limit: Option<usize>,

    /// Match mode: "all" requires every term to match (default), "any" matches if any term is present
    #[serde(default)]
    pub mode: Option<String>,

    /// Restrict to specific session types / sources. Valid values:
    /// goose, claude, pi, codex, gemini, amp, opencode. Empty = all sources.
    #[serde(default)]
    pub sources: Vec<String>,

    /// Restrict to specific message roles. Valid values:
    /// user, assistant, system, tool. Empty = all roles.
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RememberServer {
    tool_router: ToolRouter<Self>,
}

impl RememberServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

fn parse_date(s: &str) -> Result<chrono::DateTime<chrono::Utc>, String> {
    use chrono::{NaiveDate, TimeZone, Utc};
    let naive = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| format!("Invalid date '{}': {} (expected YYYY-MM-DD)", s, e))?;
    Ok(Utc.from_utc_datetime(&naive.and_hms_opt(0, 0, 0).unwrap()))
}

#[tool_router]
impl RememberServer {
    /// Search across agent conversation histories to recall past context.
    #[tool(
        name = "remember",
        description = "Search across agent conversation histories (Goose, Claude Code, Pi, Codex, Gemini, Amp, OpenCode) to recall past context. Provide a free-text query and/or keyword filters. Optionally narrow by session type via `sources` (e.g. [\"goose\",\"claude\"]) and by speaker via `roles` (e.g. [\"user\",\"assistant\"]). Returns matching conversation snippets with timestamps and source info."
    )]
    async fn remember(&self, Parameters(params): Parameters<RememberParams>) -> String {
        let after = match params.after.as_deref().map(parse_date) {
            Some(Ok(d)) => Some(d),
            Some(Err(e)) => return format!("Error: {}", e),
            None => None,
        };
        let before = match params.before.as_deref().map(parse_date) {
            Some(Ok(d)) => Some(d),
            Some(Err(e)) => return format!("Error: {}", e),
            None => None,
        };

        let mode = match params.mode.as_deref() {
            Some("any") | Some("or") => MatchMode::Or,
            _ => MatchMode::And,
        };

        let sources = match params
            .sources
            .iter()
            .map(|s| s.parse::<SourceKind>())
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(sources) => sources,
            Err(e) => return format!("Error: {}", e),
        };

        let roles = match params
            .roles
            .iter()
            .map(|r| r.parse::<Role>())
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(roles) => roles,
            Err(e) => return format!("Error: {}", e),
        };

        let query = RecallQuery {
            text: params.query,
            keywords: params.keywords,
            after,
            before,
            limit: params.limit.unwrap_or(20),
            mode,
            sources,
            roles,
        };

        if !query.has_constraints() {
            return "Error: Please provide a query, keywords, or date range.".to_string();
        }

        let engine = RecallEngine::new();
        let results = engine.recall(&query).await;
        results.format_text()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RememberServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("remember", env!("CARGO_PKG_VERSION"))
                    .with_title("Remember")
                    .with_description(
                        "Cross-agent memory recall — searches conversation histories from Goose, Claude Code, Pi, Codex, Gemini, Amp, and OpenCode",
                    ),
            )
            .with_instructions(
                "Use the 'remember' tool to search across conversation histories from multiple coding agents. \
                 Provide a free-text query and/or keyword filters to find relevant past discussions.",
            )
    }
}

/// Run the MCP server over stdio.
pub async fn run_mcp_server() -> anyhow::Result<()> {
    let server = RememberServer::new();
    let transport = stdio();
    server.serve(transport).await?.waiting().await?;
    Ok(())
}
