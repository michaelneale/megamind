# remember

A fast CLI memory recall tool for agents. Searches across multiple conversation histories and perception data sources in parallel.

```
$ remember "goose perception" -l 5
# Memory Recall: "goose perception"
Found 16 results across 4 sources in 13ms
...
```

## Install

```bash
cargo build --release
cp target/release/remember ~/.local/bin/  # or wherever you keep binaries
```

## Usage

```bash
# Free-text query
remember "what was I working on yesterday"

# Keyword search
remember -k rust -k sqlite

# Date range
remember -k mesh --after 2026-01-01 --before 2026-02-01

# Combine everything
remember "distributed systems" -k gossip -k mesh --after 2026-02-01 -l 10

# JSON output (for programmatic use by agents)
remember -f json "perception"

# List available data sources
remember sources

# Clear result cache
remember clear-cache
```

## Data Sources

| Source | Storage | What it searches |
|--------|---------|-----------------|
| **Goose** | `~/.local/share/goose/sessions/sessions.db` (SQLite) | All goose conversation messages |
| **Claude Code** | `~/.claude/projects/*/` (JSONL files) | Claude Code session transcripts |
| **Pi** | `~/.pi/agent/sessions/*/` (JSONL files) | Pi agent session transcripts |
| **Perception** | `~/Library/Application Support/GoosePerception/perception.sqlite` | Screen OCR, voice transcripts, insights, mood/emotion data |

Sources are auto-discovered — only available sources are queried.

## Architecture

```
┌─────────────┐
│  CLI (clap)  │
└──────┬──────┘
       │
┌──────▼──────┐     ┌─────────┐
│ RecallEngine ├────►│  Cache   │  SHA256-keyed, 5min TTL
└──────┬──────┘     └─────────┘
       │
       │ parallel fan-out (tokio + futures::join_all)
       │
  ┌────┴────┬────────┬──────────┐
  ▼         ▼        ▼          ▼
┌────┐  ┌───────┐  ┌───┐  ┌──────────┐
│Goose│  │Claude │  │ Pi│  │Perception│
│(SQL)│  │(JSONL)│  │(J)│  │  (SQL)   │
└────┘  └───────┘  └───┘  └──────────┘
```

- **Modular**: Each source implements the `MemorySource` trait
- **Parallel**: All sources are queried concurrently via tokio
- **Cached**: Results are cached by query hash (in-memory + file-backed, 5min TTL)
- **Fast**: Release build searches 100K+ messages across 4 sources in ~13ms
- **Read-only**: Never writes to any source database

## Adding New Sources

Implement the `MemorySource` trait:

```rust
#[async_trait]
pub trait MemorySource: Send + Sync {
    fn name(&self) -> &str;
    fn is_available(&self) -> bool;
    async fn search(&self, query: &RecallQuery) -> anyhow::Result<SourceResults>;
}
```

Then register it in `src/sources/mod.rs` → `discover_sources()`.

## Cache

Results are cached in `~/.cache/remember/` with a 5-minute TTL. Cache keys are SHA256 hashes of normalized query parameters (search terms sorted + date range + limit).

Identical or similar queries within the TTL window return instantly from cache.
