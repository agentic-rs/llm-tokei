# Usage Guide

`llm-tokei` reads local coding-agent session data and prints aggregated usage.
It does not call provider APIs for your session history.

## Basic Shape

```sh
llm-tokei [OPTIONS]
llm-tokei dump [OPTIONS] [FILES]...
```

The default report scans all discovered sources, groups by `source,model`, sorts
by `total` descending, and prints a table with cost columns.

```sh
llm-tokei
```

## Periods

Use period shortcuts for common windows.

| Flag | Meaning |
| --- | --- |
| `--24h` or `--period 24h` | Rolling last 24 hours |
| `--7d` or `--period 7d` | Rolling last 7 days |
| `--1m` or `--period 1m` | Rolling last 30 days |
| `--today` or `--period today` | Local midnight today through now |
| `--week` or `--period week` | Start of this local week through now |
| `--month` or `--period month` | Start of this local month through now |

Examples:

```sh
llm-tokei --24h
llm-tokei --week --group-by date,source
llm-tokei --period 1m --sort cost --limit 20
```

`--since` and `--until` provide explicit filters. They accept RFC3339 datetimes,
`YYYY-MM-DD`, and relative expressions such as `24h`, `7d`, `2w`, and `1mo`.

```sh
llm-tokei --since 2026-05-01 --until 2026-05-12
llm-tokei --since 12h --model 'gpt-*'
```

If both `--period` and `--since` are supplied, `--since` wins.

## Grouping

Use `--group-by` with a comma-separated list.

Available dimensions:

| Dimension | Description |
| --- | --- |
| `source` | Agent/source name |
| `model` | Canonicalized model name |
| `provider` | Provider ID when known |
| `project` | Project name or cwd basename |
| `date` | Date bucket label |
| `session` | Session ID, shortened in tables |

Examples:

```sh
llm-tokei --group-by project,source,model
llm-tokei --group-by session,source,model --sort cost --limit 10
llm-tokei --month --group-by date,source --date-bucket day
llm-tokei --group-by date,project --date-bucket week
```

`--date-bucket` supports `day`, `week`, and `month` when grouping by `date`.

## Filters

Filters reduce the records included in the report.

```sh
llm-tokei --model 'claude-*'
llm-tokei --provider openai
llm-tokei --cwd '*/work/project-*'
llm-tokei --source codex,claude
```

Glob filters are matched against the relevant field. Model filters also check
canonicalized aliases where pricing metadata knows them.

## Output

### Table

Table output is the default.

```sh
llm-tokei --format table
```

Useful table flags:

| Flag | Description |
| --- | --- |
| `-h`, `--human` | Compact usage numbers, for example `5.0M` |
| `--bytes` | Show `input` and `output` in bytes instead of tokens |
| `--split-input` | Show uncached input as `input_u` |
| `--avg turn\|round\|session` | Show per-unit averages for usage columns |
| `--table-width <N>` | Fit output to a fixed width |
| `--no-fit` | Disable automatic table fitting |
| `--no-color` | Disable ANSI colors |
| `--no-cost` | Hide cost columns |
| `--cost actual\|mixed\|official` | Select cost mode (default: `mixed`) |
| `--cost-per <dimension>` | Add top cost split columns, for example by provider |

By default, table numbers are exact and comma-separated. With `--human`, usage
columns use `K`, `M`, `B`, and `T` units and keep one decimal when a unit is
shown, such as `5.0M`. Count columns (`turns`, `rounds`, `sessions`) and cost
columns stay exact.

When color is enabled, human-readable values whose unit is smaller than the
largest unit in that column are gray. This makes scale differences easier to
scan without changing the number.

When a table has to fit a target width, lower-priority statistic columns are
hidden first and a `hidden columns:` footer is printed. Grouping columns remain
visible and long values are truncated if needed.

### JSON

JSON output is intended for scripts.

```sh
llm-tokei --format json --group-by source,model,project
```

JSON keeps raw numeric values even when `--human` would affect table output.
With `--bytes`, only the JSON `input` and `output` fields switch to bytes.

## Sorting And Limits

```sh
llm-tokei --sort total
llm-tokei --sort input --asc
llm-tokei --sort cost --limit 10
```

Sort keys: `total`, `input`, `output`, `cost`, `date`, and `turns`.

## Token Semantics

Table and JSON rows include these fields:

| Field | Meaning |
| --- | --- |
| `input` | Displayed prompt total: uncached input plus cache reads and writes |
| `output` | Assistant output tokens |
| `reasoning` | Reasoning output tokens when available |
| `cache_r` / `cache_read` | Cached-read prompt tokens |
| `cache_w` / `cache_write` | Cache-write prompt tokens |
| `total` | `input + output + reasoning`, token-based |
| `turns` | API/model turns |
| `rounds` | User-initiated prompt rounds |
| `sessions` | Distinct sessions in the group |

`--split-input` changes table `input` to uncached input only and labels it
`input_u`.

`--bytes` changes only `input` and `output` to bytes. `reasoning`, cache fields,
`total`, and pricing remain token-based.

Some sources provide exact token counts. Some sources require estimates because
the local session files do not persist full token accounting. Estimated values
are marked with `~` in table output and `*_estimated` booleans in JSON output.

## Config

`llm-tokei` loads defaults from `~/.config/llm-tokei.toml` when present, or from
`$XDG_CONFIG_HOME/llm-tokei.toml` if `XDG_CONFIG_HOME` is set.
CLI flags always override config values.

```toml
format = "table"
cost = "mixed"
group-by = ["source", "model"]
human = true
period = "month"
source = ["codex", "opencode"]
```

Use a custom config file or disable config loading:

```sh
llm-tokei --config ./llm-tokei.toml
llm-tokei --no-config
```

Save structured defaults from CLI args:

```sh
llm-tokei config args "--cost official --group-by provider --human"
llm-tokei config list
llm-tokei config args --reset
llm-tokei --cost actual --group-by source,model --save-default
llm-tokei --no-default
```

`config args "..."` and `--save-default` both parse normal main CLI flags and
write the corresponding structured TOML keys. `--save-default` then continues
to run the command normally. `--no-default` skips applying saved config defaults
for one run.

Config keys mirror the main CLI flags using kebab-case names, for example
`date-bucket`, `table-width`, `cost-per`, `codex-dir`, and `copilot-cli-dir`.
Subcommand-specific options are not read from config.

## Pricing

Bundled prices are generated from [models.dev](https://models.dev) plus local
metadata under `data/`.

One cost column is reported:

| Column | Meaning |
| --- | --- |
| `cost($)` / `cost` | USD in the selected cost mode |

Cost modes:

| Mode | Meaning |
| --- | --- |
| `actual` | Provider-specific pricing with multipliers; included providers/models cost `$0` |
| `mixed` | Default. Provider-specific pricing, but included providers/models fall back to official model rates |
| `official` | Official model-provider rates only, ignoring the source provider |

Examples:

```sh
llm-tokei --cost actual
llm-tokei --cost mixed --cost-per provider
llm-tokei --cost official --group-by model --sort cost
```

`--cost-per <dimension>` appends the top 3 cost contributors for a dimension as
extra table columns. Table headers use the split value directly and truncate it
to 10 characters. JSON output includes a `cost_per` object with full keys.

Runtime pricing overrides are complete JSON pricing files. Exactly one pricing
file is active: explicit `--pricing`, otherwise the update cache when present,
otherwise bundled prices.

```sh
llm-tokei --pricing ./pricing.json
```

Example override:

```json
{
  "providers": {
    "github-copilot": {
      "included": true,
      "models": {
        "claude-opus-4.7": { "multiplier": 10.0, "included": false }
      }
    }
  },
  "models": {
    "gpt-5": { "provider": "openai", "aliases": ["openai/gpt-5"] }
  },
  "prices": [
    {
      "provider": "openai",
      "model": "gpt-5",
      "input": 1.25,
      "output": 10.0,
      "cache_read": 0.125,
      "cache_write": null
    }
  ]
}
```

Pricing lookup uses canonical model aliases before grouping and costing.
Provider-specific lookup tries the exact `(provider, model)` row. Official lookup
uses the model's official provider mapping. Multipliers and included status can
be set per provider and overridden per model.

## Sources

### Codex CLI

Default root:

```text
$CODEX_HOME/sessions
~/.codex/sessions
```

Override:

```sh
llm-tokei --source codex --codex-dir /path/to/sessions
```

Codex sessions are JSONL rollout files. Cumulative token snapshots are converted
to per-turn deltas. Response item text is used for byte-mode input/output.

### OpenCode

Default database:

```text
$OPENCODE_DATA_DIR/opencode.db
$XDG_DATA_HOME/opencode/opencode.db
~/.local/share/opencode/opencode.db
```

Override:

```sh
llm-tokei --source opencode --opencode-db /path/to/opencode.db
```

The database is opened read-only.

### Claude Code

Default root:

```text
$CLAUDE_HOME/projects
~/.claude/projects
```

Override:

```sh
llm-tokei --source claude --claude-dir /path/to/projects
```

Claude usage fields map raw `input_tokens` to uncached input,
`cache_read_input_tokens` to cache reads, and cache creation fields to cache
writes.

### GitHub Copilot Chat

Default discovery scans VS Code, Code Insiders, VSCodium, and Cursor
`workspaceStorage` directories.

Override or add roots:

```sh
llm-tokei --source copilot --copilot-dir /path/to/workspaceStorage
```

Copilot Chat does not always persist exact input/output token counts in chat
session files. `llm-tokei` estimates input/output from rendered text length and
uses exact reasoning tokens when `thinking.tokens` is present. Shutdown metrics
from transcript files are preferred when available.

### GitHub Copilot CLI

Default root:

```text
~/.copilot/session-state
```

Override:

```sh
llm-tokei --source copilot-cli --copilot-cli-dir /path/to/session-state
```

Shutdown metrics are used when present. Otherwise usage is estimated from event
content.

## Cache

By default, parsed records are cached under your OS cache directory as
`llm-tokei.db`. The cache is keyed by source file path and modification time.

Use `--no-cache` to force a full re-parse:

```sh
llm-tokei --no-cache
```

Verbose mode prints cache stats:

```sh
llm-tokei -v
```

## Dumping Sessions

The `dump` subcommand emits replayable user-side JSONL message streams.

```sh
llm-tokei dump --codex ~/.codex/sessions/2026/05/12/rollout-example.jsonl
llm-tokei dump --codex --out ./dumped-codex
llm-tokei dump --copilot --out ./dumped-copilot
```

Use exactly one dump source flag: `--codex` or `--copilot`.

Without `--out`, output is written to stdout with comment headers when multiple
files are dumped. With `--out`, one `<session-id>.jsonl` file is written per
session.

## Maintenance Notes

Bundled pricing data is generated during build.

```sh
cargo run --example fetch_prices
cargo build --release
```

Generate the README showcase SVG from live CLI output:

```sh
cargo run --example gen-showcase -- --args "--24h --group-by source,model" --out docs/assets/showcase.svg
cargo run --example gen-showcase -- --args "--cost-per provider --cost official --month -h" --out docs/assets/showcase.svg
```

Hand-curated pricing inputs:

| File | Purpose |
| --- | --- |
| `data/models.json` | Canonical model names, official providers, aliases |
| `data/providers.json` | Provider/model included and multiplier metadata |
| `data/prices.override.csv` | Rate overrides and additions |

If models.dev reports all token cost fields as `0` for a provider/model, the
generator treats that row as included and omits it from base prices so cost can
fall back to the model's official provider rate.
