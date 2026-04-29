# llm-tokei

A fast token-usage stats CLI for local LLM coding agents. Parses session
data on disk and prints aggregated input/output/cache/reasoning token totals
with optional cost estimates.

**Supported sources (v1):**
- **OpenAI Codex CLI** — `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`
- **OpenCode** — `~/.local/share/opencode/opencode.db` (SQLite)

## Install

```sh
cargo build --release
# binary at ./target/release/llm-tokei
```

## Quick usage

```sh
# Default: group by source × model, table output
llm-tokei

# Last 7 days, broken down per day
llm-tokei --since 7d --group-by source,date --date-bucket day

# Top 10 most expensive sessions in OpenCode
llm-tokei --source opencode --group-by session --sort cost --limit 10

# Per-project totals as JSON
llm-tokei --group-by project --format json
```

## Flags

| Flag | Description |
|------|-------------|
| `--source codex,opencode` | Subset of sources to scan |
| `--codex-dir <path>` | Override `~/.codex/sessions` |
| `--opencode-db <path>` | Override `~/.local/share/opencode/opencode.db` |
| `--since <when>` / `--until <when>` | RFC3339, `YYYY-MM-DD`, or relative (`7d`, `24h`, `2w`, `1mo`) |
| `--model <glob>` / `--provider <glob>` / `--cwd <glob>` | Filters |
| `--group-by source,model,provider,project,date,session` | Comma list, ordered |
| `--date-bucket day\|week\|month` | Date bucketing |
| `--format table\|json` | Output format |
| `--sort total\|input\|output\|cost\|date\|turns` | Sort key (desc; `--asc` to invert) |
| `--limit <N>` | Truncate rows |
| `--no-cost` | Hide cost column |
| `--pricing path.json` | Merge custom prices into bundled table |
| `--no-color` | Disable ANSI colors |
| `-v, --verbose` | Print parse stats to stderr |

## Pricing

Costs are derived from `data/prices.json` (USD per 1M tokens, keyed by
`provider/model` or just `model`). OpenCode's own embedded `cost` field is
used when present. Provide `--pricing <file.json>` to extend or override.

## Notes

- Codex CLI emits cumulative `token_count` events per turn; we use the
  **last** `total_token_usage` per session, falling back to summed
  `last_token_usage` deltas if the totals are absent.
- OpenCode token totals are per-assistant-message and aggregated per session.
- The OpenCode DB is opened read-only; safe to run while OpenCode is active.

## Roadmap

- Claude Code (`~/.claude/projects/**/*.jsonl`) source
- GitHub Copilot CLI source
- `--watch` live mode
- CSV / Markdown renderers
- Per-turn detail view
