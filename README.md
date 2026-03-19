# megamind

Cross-agent memory. If you use multiple coding agents, none of them know what you said to the others. megamind fixes that.

It's a small CLI (`remember`) that searches conversation histories from Goose, Claude Code, Pi, Codex, and Gemini in parallel. Install it as an agent skill (SKILL.md) so any agent can recall what you've discussed across all of them.

```
$ remember -l 2 -k goose -k sandbox
# Memory Recall: keywords: [goose, sandbox], mode: ALL must match
Found 5 results across 5 sources in 50ms

## Claude Code (2 results, 38ms)
1. [2026-03-11 04:19] (session: /Users/micn/Development/sandpit)
   Here's a comprehensive overview of how these CLI agent tools handle hooks,
   security policies, and command interception...

## Pi (2 results, 49ms)
1. [2026-03-17 05:40] (session: /Users/micn/Development/sandpit)
   OK so here's what I'm thinking. sandpit wraps your agent with three
   invisible enforcement layers...
```

## Install

```bash
cargo build --release
cp target/release/remember ~/.local/bin/
```

### Agent Skill

Copy `SKILL.md` into your agent's skill directory so it knows how to use `remember`:

| Agent | Install |
|-------|---------|
| **Pi** | `mkdir -p ~/.pi/agent/skills/remember && cp SKILL.md ~/.pi/agent/skills/remember/` |
| **Codex** | `mkdir -p ~/.codex/skills/remember && cp SKILL.md ~/.codex/skills/remember/` |
| **Claude Code** | Reference from `.claude/settings.json` or `AGENTS.md` |
| **Goose** | Add to `.goose/skills/` or reference in config |

Or just put it wherever your agent reads skill files. The format is standard [Agent Skills](https://agentskills.io).

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
| **Goose** | `~/.local/share/goose/sessions/` (SQLite + JSONL) |
| **Claude Code** | `~/.claude/projects/*/` (JSONL) |
| **Pi** | `~/.pi/agent/sessions/*/` (JSONL) |
| **Codex** | `~/.codex/sessions/` (JSONL) |
| **Gemini** | `~/.gemini/tmp/*/chats/` (JSON) |

## Design

- Each source implements `MemorySource` trait — add new ones in `src/sources/`
- All sources queried concurrently via tokio
- Results cached in `~/.cache/remember/` with 5min TTL
- Read-only — never writes to source databases
