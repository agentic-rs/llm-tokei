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
| `--sort total\|input\|output\|cost\|cost-base\|date\|turns` | Sort key (desc; `--asc` to invert). `cost` = multiplied. |
| `--limit <N>` | Truncate rows |
| `--no-cost` | Hide cost column |
| `--pricing path.json` | Merge custom prices into bundled table |
| `--no-color` | Disable ANSI colors |
| `-v, --verbose` | Print parse stats to stderr |

## Pricing

`data/prices.json` is split into two halves:

```jsonc
{
  "providers": {
    "github-copilot": {
      "multiplier": 1.0,                              // default for all models
      "models": {
        "claude-opus-4.7": { "multiplier": 10.0 },    // model-specific override
        "gpt-5":           { "multiplier": 0.0 }
      }
    }
  },
  "models": {
    "claude-opus-4.7": { "input": 15, "output": 75, "cache_read": 1.5, "cache_write": 18.75 },
    "gpt-5":           { "input": 1.25, "output": 10, "cache_read": 0.125, "cache_write": 0 },
    "github-copilot/gpt-5": { "input": 0, "output": 0, "cache_read": 0, "cache_write": 0 }
    // provider-prefixed key only needed when the price differs from the plain entry
  }
}
```

Lookup order:
- **Base price**: `models["provider/model"]` → `models["model"]` → `-`
- **Multiplier**: `providers[provider].models[model].multiplier` → `providers[provider].multiplier` → `1.0`

Two cost columns are reported:
- **`cost($)`** — base USD from token rates only.
- **`cost×mult($)`** — base × multiplier (e.g. Copilot premium-request weighting).

`--pricing path.json` merges into the bundled table (entries you supply win).

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
