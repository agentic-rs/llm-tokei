import assert from "node:assert/strict";
import test from "node:test";

import { getModel, listModels, listModelsDevSources, resolveModel } from "../dist/index.js";

test("loads the complete vendor-split catalog", () => {
  const models = listModels();
  assert.equal(models.length, 247);
  assert.deepEqual([...new Set(models.map((model) => model.vendor))].sort(), [
    "alibaba",
    "anthropic",
    "cohere",
    "deepseek",
    "google",
    "meta",
    "microsoft",
    "minimax",
    "mistral",
    "moonshotai",
    "openai",
    "xai",
    "zai"
  ]);
});

test("resolves explicit aliases", () => {
  assert.deepEqual(resolveModel({ model: "o1" }), {
    canonical_name: "openai-o1",
    vendor: "openai",
    family: "o1",
    series: "o1",
    confidence: "exact",
    matched_by: "catalog_alias"
  });
});

test("reports normalization separately from alias resolution", () => {
  const model = resolveModel({ model: " GPT-5 " });
  assert.equal(model.canonical_name, "gpt-5");
  assert.equal(model.confidence, "normalized");
  assert.equal(model.matched_by, "normalization");
});

test("resolves current official models.dev aliases exactly", () => {
  for (const [model, canonicalName] of [
    ["claude-opus-4-5-20251101", "claude-opus-4.5"],
    ["gemini-3.1-pro-preview", "gemini-3.1-pro"],
    ["gpt-4o-2024-08-06", "gpt-4o"],
    ["gpt-5.3-chat-latest", "gpt-5.3-chat"]
  ]) {
    const resolution = resolveModel({ model });
    assert.equal(resolution.canonical_name, canonicalName);
    assert.equal(resolution.confidence, "exact");
  }
});

test("resolves representative first-wave vendor models", () => {
  for (const [model, canonicalName] of [
    ["grok-4-5", "grok-4.5"],
    ["mistral-large-2512", "mistral-large-2512"],
    ["llama-3.3-70b-instruct", "llama-3.3-70b-instruct"],
    ["command-a-03-2025", "command-a-03-2025"],
    ["kimi-k2.7-code", "kimi-k2.7-code"]
  ]) {
    const resolution = resolveModel({ model });
    assert.equal(resolution.canonical_name, canonicalName);
    assert.equal(resolution.confidence, "exact");
  }
});

test("uses source-specific aliases without making them global", () => {
  assert.deepEqual(resolveModel({ provider: "azure", model: "phi-4-mini" }), {
    canonical_name: "phi-4-mini-instruct",
    vendor: "microsoft",
    family: "phi-4-mini",
    series: "phi-4",
    confidence: "exact",
    matched_by: "source_alias"
  });
  assert.equal(
    resolveModel({ provider: "github-models", model: "microsoft/phi-4-multimodal-instruct" }).canonical_name,
    "phi-4-multimodal-instruct"
  );
  assert.equal(resolveModel({ model: "phi-4-mini" }).canonical_name, null);
  assert.equal(resolveModel({ model: "microsoft/phi-4-multimodal-instruct" }).canonical_name, null);
  assert.equal(resolveModel({ model: "github-models/microsoft/phi-4-mini-instruct" }).canonical_name, null);
  assert.equal(resolveModel({ provider: "openrouter", model: "phi-4-mini" }).canonical_name, null);
  assert.equal(
    resolveModel({ provider: "llama", model: "cerebras-llama-4-maverick-17b-128e-instruct" }).canonical_name,
    "llama-4-maverick-17b-128e-instruct"
  );
  assert.equal(resolveModel({ model: "cerebras-llama-4-maverick-17b-128e-instruct" }).canonical_name, null);
});

test("marks mutable selectors as rolling", () => {
  assert.equal(getModel("gemini-flash-latest")?.is_rolling, true);
  assert.equal(getModel("mistral-large-latest")?.is_rolling, true);
  assert.equal(getModel("mistral-large-2512")?.is_rolling, undefined);
  assert.equal(getModel("gpt-5-chat-latest")?.is_rolling, true);
  assert.equal(resolveModel({ model: "gpt-5-chat-latest" }).is_rolling, true);
  assert.equal(resolveModel({ model: "openai/gpt-5-chat-latest" }).is_rolling, true);
});

test("keeps provider route IDs distinct from canonical model identity", () => {
  const sourceModel = "cerebras-llama-4-maverick-17b-128e-instruct";
  const resolution = resolveModel({ provider: "llama", model: sourceModel });
  assert.equal(resolution.canonical_name, "llama-4-maverick-17b-128e-instruct");
  assert.notEqual(resolution.canonical_name, sourceModel);
});

test("lists the explicit models.dev source relationship", () => {
  assert.deepEqual(
    listModelsDevSources().filter((source) => source.vendor === "microsoft"),
    [
      { vendor: "microsoft", provider: "azure", model_prefix: "phi-" },
      { vendor: "microsoft", provider: "azure-cognitive-services", model_prefix: "phi-" },
      { vendor: "microsoft", provider: "github-models", model_prefix: "microsoft/phi-" }
    ]
  );
});

test("marks normalized model names as heuristic", () => {
  const model = resolveModel({ model: "anthropic/claude-sonnet-4-5-20250929" });
  assert.equal(model.canonical_name, "claude-sonnet-4.5");
  assert.equal(model.confidence, "heuristic");
  assert.equal(model.family, "claude-sonnet");
  assert.equal(model.series, "claude-4.5");
});

test("does not make unknown names priceable", () => {
  assert.deepEqual(resolveModel({ model: "future-model-xyz" }), {
    canonical_name: null,
    confidence: "unknown",
    matched_by: null
  });
});

test("gets known model metadata by canonical name", () => {
  assert.deepEqual(getModel("gpt-5.1-codex-max")?.canonical_name, "gpt-5.1-codex");
});
