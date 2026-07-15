# @tokn-ai/model-catalog

A versioned, dependency-free catalog for resolving reported LLM model names to
canonical names, vendors, families, and series. It deliberately does not ship
prices: rate data is provider-, region-, and billing-mode-specific.

`vendor` always identifies the model maker, not the route that serves it. For
example, Meta owns Llama while Azure, Bedrock, and OpenRouter may each expose
separate billable routes for it. The original records are a deliberately
duplicated snapshot of `llm-tokei`'s registry; vendor files can now evolve and
publish independently without changing the Rust CLI's bundled data path.

```ts
import { resolveModel } from "@tokn-ai/model-catalog";

resolveModel({ model: "anthropic/claude-sonnet-4-5-20250929" });
// { canonical_name: "claude-sonnet-4.5", family: "claude-sonnet",
//   series: "claude-4.5", vendor: "anthropic", confidence: "heuristic", ... }
```

For price lookup, retain the input `provider` and reported `model` ID; a
`canonical_name` is a route-neutral catalog identity, not a billing SKU. Use an
exact, non-rolling resolution as a guard before applying a rate keyed by the
provider, raw model ID, region, and billing mode. A record marked
`is_rolling: true` (such as a `-latest` selector) is useful for usage
aggregation, but it does not identify a fixed priceable release.

When a spelling is meaningful only for one serving route, the catalog keeps it
source-scoped rather than turning it into a global alias:

```ts
resolveModel({ provider: "azure", model: "phi-4-mini" });
// { canonical_name: "phi-4-mini-instruct", vendor: "microsoft", ... }

resolveModel({ model: "phi-4-mini" });
// { canonical_name: null, confidence: "unknown", matched_by: null }
```

This keeps provider-specific model IDs available for later pricing rules.

## Audit against models.dev

The development-only report fetches every current models.dev model record and
shows how the catalog resolves it. It does not modify the catalog.

```sh
pnpm models-dev:report
pnpm models-dev:report -- --official --unmatched
pnpm models-dev:report -- --json > models-dev-report.json
```

Use the JSON report to review aliases. An `unknown` result means the record is
not safely mapped; it may need a new canonical model instead of an alias.

`--official` uses the explicit vendor-to-source relationship exported by
`listModelsDevSources()`. It can include a scoped host source—for example, only
Phi records from Azure—without treating Azure itself as the model vendor.
