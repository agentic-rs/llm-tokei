# llm-tokei

A fast token-usage stats CLI for local LLM coding agents. Parses session
data on disk and prints aggregated input/output/cache/reasoning token totals
with optional cost estimates.

**Supported sources (v1):**
- **OpenAI Codex CLI** — `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`
- **OpenCode** — `~/.local/share/opencode/opencode.db` (SQLite)
- **Claude Code** — `~/.claude/projects/<encoded-cwd>/<session>.jsonl`
- **GitHub Copilot Chat** (VS Code / Insiders / VSCodium / Cursor) —
  `…/User/workspaceStorage/<workspace-id>/chatSessions/<session>.jsonl`

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
| `--source codex,opencode,claude,copilot` | Subset of sources to scan |
| `--codex-dir <path>` | Override `~/.codex/sessions` |
| `--opencode-db <path>` | Override `~/.local/share/opencode/opencode.db` |
| `--claude-dir <path>` | Override `$CLAUDE_HOME/projects` (or `~/.claude/projects`) |
| `--copilot-dir <path>` | Override VS Code `workspaceStorage` root (repeatable) |
| `--since <when>` / `--until <when>` | RFC3339, `YYYY-MM-DD`, or relative (`7d`, `24h`, `2w`, `1mo`) |
| `--model <glob>` / `--provider <glob>` / `--cwd <glob>` | Filters |
| `--group-by source,model,provider,project,date,session` | Comma list, ordered |
| `--date-bucket day\|week\|month` | Date bucketing |
| `--format table\|json` | Output format |
| `--sort total\|input\|output\|cost\|cost-base\|date\|turns` | Sort key (desc; `--asc` to invert). `cost` = multiplied. |
| `--limit <N>` | Truncate rows |
| `--no-cost` | Hide cost column |
| `--avg turn\|round\|session` | Show per-unit averages in table output |
| `--split-input` | Show uncached input (`input - cache_r`) as `input_u` |
| `--pricing path.json` | Merge custom prices into bundled table |
| `--no-color` | Disable ANSI colors |
| `-v, --verbose` | Print parse stats to stderr |

## Pricing

The bundled `data/prices.json` is **generated** from [models.dev](https://models.dev),
`data/models.json`, `data/providers.json`, and `data/prices.override.csv`.
Schema:

```jsonc
{
  "providers": {
    "github-copilot": {
      "included": true,                                 // default for all models in this provider
      "models": {
        "claude-opus-4.7": { "multiplier": 10.0, "included": false },
        "gpt-5":           { "multiplier": 0.0 }       // inherits included=true
      }
    }
  },
  "models": {
    "claude-opus-4.7": { "provider": "anthropic", "aliases": ["claude-opus-4-7"] },
    "gpt-5":           { "provider": "openai", "aliases": ["openai/gpt-5"] }
  },
  "prices": [
    { "provider": "anthropic", "model": "claude-opus-4.7", "name": "claude-opus-4-7",
      "input": 15, "output": 75, "cache_read": 1.5, "cache_write": 18.75 },
    { "provider": "openai", "model": "gpt-5", "input": 1.25, "output": 10, "cache_read": 0.125 }
  ]
}
```

Defaults / lookup order:
- Model IDs are canonicalized through `models[*].aliases` before grouping or pricing.
- **Base price**: exact `(provider, canonical_model)` row in `prices`, then the model's official provider row from `models[model].provider`, then no cost.
- **Multiplier**: `providers[P].models[M].multiplier` → `providers[P].multiplier` → `1.0`.
  Omitted on write when equal to the default `1.0` for providers (always emitted for models).
- **Included**: `providers[P].models[M].included` → `providers[P].included` → `false`.
  Omitted on write when equal to its default. When `included == true` the
  multiplied cost is forced to `$0` (covered by a subscription / plan), but the
  base cost is still computed using the official-provider price when available.

Two cost columns are reported:
- **`cost($)`** — base USD from token rates only (always shown).
- **`cost×mult($)`** — base × multiplier, or `0` if `included`.

### Token semantics

- `input` is the **full prompt total** (cached + uncached).
- `cache_r` is the cached portion of `input` (a subset, not additional).
- `cache_w` is cache-write tokens (billed separately at write rates).
- `total = input + output + reasoning + cache_w` (cache_read isn't double-counted).

### Refreshing models.dev data

```sh
cargo run --example fetch_prices
cargo build --release
```

`fetch_prices` only downloads `https://models.dev/api.json` and updates
`data/models.dev.csv`. `build.rs` generates the embedded `prices.json` under
Cargo's `OUT_DIR` during `cargo build` by merging `models.dev.csv` with the
hand-curated inputs. `data/prices.json` is not checked in.

Hand-curated inputs:
- `data/models.json` — canonical model names, official provider, aliases.
- `data/providers.json` — provider/model `included` and `multiplier` metadata.
- `data/prices.override.csv` — rate overrides/additions with the same columns as `models.dev.csv`.

If models.dev reports all token cost fields as `0` for a provider/model, the
build generator treats that row as `included` and omits it from `prices` so base
cost can fall back to the official provider's rate.

`--pricing path.json` merges into the bundled table at runtime (entries you
supply win).

## Notes

- Codex CLI emits cumulative `token_count` events per turn; we use the
  **last** `total_token_usage` per session, falling back to summed
  `last_token_usage` deltas if the totals are absent.
- OpenCode token totals are per-assistant-message and aggregated per session.
- The OpenCode DB is opened read-only; safe to run while OpenCode is active.
- Claude Code: `input` is summed across assistant turns as
  `input_tokens + cache_read_input_tokens + cache_creation_input_tokens`
  (matching the cached+uncached convention used by the other sources);
  `cache_w` corresponds to `cache_creation`.
- GitHub Copilot Chat: chat session files don't persist per-turn input/output
  token counts. `input` and `output` are **estimates** derived from the
  rendered prompt and response text length (~4 chars/token); `reasoning` is
  exact when the model emits `thinking.tokens`. Treat Copilot rows as
  approximate. Default discovery walks Code, Code - Insiders, VSCodium and
  Cursor user directories on Linux/macOS/Windows.

## Roadmap

- `--watch` live mode
- CSV / Markdown renderers
- Per-turn detail view
