# megamind

Cross-agent memory. If you use multiple coding agents, none of them know what you said to the others. megamind fixes that.

It's a small CLI (`remember`) that searches conversation histories from Goose, Claude Code, Pi, Codex, Gemini, Amp, and OpenCode in parallel. Drop the included `SKILL.md` into `~/.skills/` and any agent that supports [Agent Skills](https://agentskills.io) can recall what you've discussed across all of them.

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

## Install

```bash
cargo build --release
cp target/release/remember ~/.local/bin/
mkdir -p ~/.skills/remember && cp SKILL.md ~/.skills/remember/
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

## Adding Sources

Implement the `MemorySource` trait in `src/sources/`, register in `mod.rs`.
