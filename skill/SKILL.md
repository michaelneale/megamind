---
name: remember
description: Search across agent conversation histories (Goose, Claude Code, Pi, Codex, Gemini, Amp, OpenCode) to recall past context. Use when you need to find something from previous sessions, look up past work, or the user asks "remember when" or "what was I doing with X".
---

# Remember

Fast CLI memory recall. Searches conversation histories from multiple coding agents in parallel.

## Prerequisites

The `remember` binary must be on your PATH. Install it:

```bash
# macOS Apple Silicon
mkdir -p ~/.local/bin && curl -fsSL https://github.com/michaelneale/megamind/releases/latest/download/remember-darwin-arm64.tar.gz | tar xz -C ~/.local/bin

# or from source
cargo install --git https://github.com/michaelneale/megamind.git
```

Verify: `remember sources` should list available sources.

## Usage

```bash
remember "what was the auth approach we discussed"
remember -k rust -k sqlite
remember -k deploy --after 2026-01-01 --before 2026-02-01
remember "distributed systems" -k gossip --after 2026-02-01 -l 10
remember "foo" -k bar --any
remember -f json "perception"
remember sources
```

### Key Flags

- `-k <word>` — Keyword filter (repeatable)
- `--after YYYY-MM-DD` — Results after this date
- `--before YYYY-MM-DD` — Results before this date
- `-l <n>` — Max results per source (default: 20)
- `-f json` — JSON output (useful for programmatic parsing)
- `--any` — OR mode instead of AND (default is AND)

## Tips

- Use `-f json` when you need to parse results programmatically
- Combine free-text with `-k` keywords for precise filtering
- Add date ranges to narrow down recent vs. old results
- Results are cached for 5 minutes — identical queries return instantly
- `remember sources` shows which agent histories are available on this machine
