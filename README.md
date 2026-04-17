# megamind

Cross-agent memory. If you use multiple coding agents, none of them know what you said to the others. megamind fixes that.

It's a small CLI (`remember`) that searches conversation histories from Goose, Claude Code, Pi, Codex, Gemini, Amp, and OpenCode in parallel. Give your agents access via either an [Agent Skill](https://agentskills.io) or an [MCP](https://modelcontextprotocol.io) server — pick whichever your tooling supports.


<img width="274" height="312" alt="image" src="https://github.com/user-attachments/assets/89f2ded5-282e-45d5-87ff-0fee03e24d80" />


## Install

**macOS (Apple Silicon) — no sudo required:**

```bash
mkdir -p ~/.local/bin && curl -fsSL https://github.com/michaelneale/megamind/releases/latest/download/remember-darwin-arm64.tar.gz | tar xz -C ~/.local/bin
```

> Add `export PATH="$HOME/.local/bin:$PATH"` to your shell profile if it's not already there.

**From source (any platform):**

```bash
cargo install --git https://github.com/michaelneale/megamind.git
```

[Latest release →](https://github.com/michaelneale/megamind/releases/latest)

### Give your agents access

Install the [Agent Skill](https://agentskills.io) so your agents know how to use `remember`:

```bash
# Pi
git clone https://github.com/michaelneale/megamind.git /tmp/megamind && cp -r /tmp/megamind/skill ~/.pi/agent/skills/remember

# Claude Code
git clone https://github.com/michaelneale/megamind.git /tmp/megamind && cp -r /tmp/megamind/skill ~/.claude/skills/remember

# Any agent that supports ~/.agents/skills
git clone https://github.com/michaelneale/megamind.git /tmp/megamind && mkdir -p ~/.agents/skills && cp -r /tmp/megamind/skill ~/.agents/skills/remember
```

Or use the [MCP server](#mcp-server) if your tooling supports it.


```
$ remember -l 2 -k goose -k sandbox
# Memory Recall: keywords: [goose, sandbox]
Found 5 results across 7 sources in 50ms

## Claude Code (2 results, 38ms)
1. [2026-03-11 04:19] (session: /Development/sandpit) [assistant]
   Here's how these CLI agent tools handle hooks and security policies...

## Pi (2 results, 49ms)
1. [2026-03-17 05:40] (session: /Development/sandpit) [assistant]
   sandpit wraps your agent with three invisible enforcement layers...

## Amp (1 results, 26ms)
1. [2026-02-18 21:19] (session: /Development/goose) [user]
   can we add sandbox support for goose sessions...
```


## Usage

```bash
remember "what was the auth approach we discussed"
remember -k rust -k sqlite
remember -k deploy --after 2026-01-01 --before 2026-02-01
remember -f json "error handling"
remember sources
remember clear-cache
```

Flags: `-k` keyword (repeatable), `--after`/`--before` date range, `-l` limit per source (default 20), `-f json` for machine output, `--any` for OR mode (default AND).

## Sources

Auto-discovered — only available sources are queried.

| Source | Storage |
|--------|---------|
| **Goose** | `~/.local/share/goose/sessions/` |
| **Claude Code** | `~/.claude/projects/*/` |
| **Pi** | `~/.pi/agent/sessions/*/` |
| **Codex** | `~/.codex/sessions/` |
| **Gemini** | `~/.gemini/tmp/*/chats/` |
| **Amp** | `~/.local/share/amp/threads/` |
| **OpenCode** | `~/.local/share/opencode/storage/` |

## MCP Server

`remember` can also run as an [MCP](https://modelcontextprotocol.io) server over stdio, exposing a single `remember` tool. This lets any MCP-compatible client (Claude Desktop, Cursor, etc.) search your agent histories directly.

```bash
remember mcp
```

Add it to your MCP client config:

```json
{
  "mcpServers": {
    "remember": {
      "command": "remember",
      "args": ["mcp"]
    }
  }
}
```

The tool accepts `query`, `keywords`, `after`, `before`, `limit`, and `mode` (`"all"` or `"any"`) — same parameters as the CLI.

## Adding Sources

Implement the `MemorySource` trait in `src/sources/`, register in `mod.rs`.
