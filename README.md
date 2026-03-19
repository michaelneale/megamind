# megamind

Fast CLI memory recall for agents. Searches conversation histories in parallel.

```
$ remember "goose perception" -l 5
# Memory Recall: "goose perception"
Found 16 results across 5 sources in 13ms
...
```

## Install

```bash
cargo build --release
cp target/release/remember ~/.local/bin/
```

### Pi Agent Skill

```bash
mkdir -p ~/.pi/agent/skills/remember
cp SKILL.md ~/.pi/agent/skills/remember/SKILL.md
```

Requires `remember` on your `$PATH`.

## Usage

```bash
remember "what was I working on yesterday"
remember -k rust -k sqlite
remember -k mesh --after 2026-01-01 --before 2026-02-01
remember "distributed systems" -k gossip -l 10
remember -f json "perception"
remember sources
remember clear-cache
```

Flags: `-k` keyword (repeatable), `--after`/`--before` date range, `-l` limit per source, `-f json` for machine output, `--any` for OR mode (default is AND).

## Data Sources

Auto-discovered — only available sources are queried.

| Source | Storage | What it searches |
|--------|---------|-----------------|
| **Goose** | `~/.local/share/goose/sessions/` | Conversation messages (SQLite + JSONL) |
| **Claude Code** | `~/.claude/projects/*/` (JSONL) | Session transcripts |
| **Pi** | `~/.pi/agent/sessions/*/` (JSONL) | Session transcripts |
| **Codex** | `~/.codex/sessions/` (JSONL) | Session transcripts |
| **Gemini** | `~/.gemini/tmp/*/chats/` (JSON) | Session transcripts |

## Design

- Each source implements `MemorySource` trait — add new ones in `src/sources/`
- All sources queried concurrently via tokio
- Results cached in `~/.cache/remember/` with 5min TTL
- Read-only — never writes to source databases
