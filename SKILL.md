---
name: remember
description: Search across agent conversation histories (Goose, Claude Code, Pi, Codex, Gemini) to recall past context. Use when you need to find something from previous sessions or look up past work.
---

# Remember

Fast CLI memory recall. Searches conversation histories from multiple agents in parallel.

## When to Use

- You need to recall what was discussed or worked on in previous sessions
- The user asks "what was I doing with X" or "remember when we..."
- You need context from past conversations across any agent

## Data Sources

Auto-discovered — only available ones are queried: Goose, Claude Code, Pi, Codex, Gemini.

## Usage

```bash
remember "what was I working on yesterday"
remember -k rust -k sqlite
remember -k mesh --after 2026-01-01 --before 2026-02-01
remember "distributed systems" -k gossip --after 2026-02-01 -l 10
remember "foo" -k bar --any
remember -f json "perception"
remember sources
```

### Key flags

- `-k <word>` — Keyword filter (repeatable)
- `--after YYYY-MM-DD` — Results after this date
- `--before YYYY-MM-DD` — Results before this date
- `-l <n>` — Max results per source (default: 20)
- `-f json` — JSON output
- `--any` — OR mode instead of AND

## Tips

- Use `-f json` when you need to parse results programmatically
- Combine free-text with `-k` keywords for precise filtering
- Add date ranges to narrow down recent vs. old results
- Results are cached for 5 minutes — identical queries return instantly
