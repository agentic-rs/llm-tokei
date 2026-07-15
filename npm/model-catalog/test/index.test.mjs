import assert from "node:assert/strict";
import test from "node:test";

import { getModel, listModels, resolveModel } from "../dist/index.js";

test("loads the complete vendor-split catalog", () => {
  const models = listModels();
  assert.equal(models.length, 163);
  assert.deepEqual([...new Set(models.map((model) => model.vendor))].sort(), [
    "alibaba",
    "anthropic",
    "deepseek",
    "google",
    "minimax",
    "openai",
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
