# @tokn-ai/model-catalog

A versioned, dependency-free catalog for resolving reported LLM model names to
canonical names, vendors, families, and series. It deliberately does not ship
prices: rate data is provider-, region-, and billing-mode-specific.

```ts
import { resolveModel } from "@tokn-ai/model-catalog";

resolveModel({ model: "anthropic/claude-sonnet-4-5-20250929" });
// { canonical_name: "claude-sonnet-4.5", family: "claude-sonnet",
//   series: "claude-4.5", vendor: "anthropic", confidence: "heuristic", ... }
```

Use only `confidence: "exact"` results for strict billing. `heuristic` results
are useful for usage aggregation and should be explicitly accepted by callers.
