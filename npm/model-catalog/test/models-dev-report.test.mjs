import assert from "node:assert/strict";
import test from "node:test";

import { createModelsDevReport } from "../scripts/models-dev-report.mjs";

test("reports a resolution for every models.dev model record", () => {
  const report = createModelsDevReport({
    openai: {
      models: {
        "gpt-5": { name: "GPT-5" },
        "gpt-5-chat-latest": { name: "GPT-5 Chat" },
        "unknown-model": { name: "Unknown" }
      }
    },
    ignored: {
      models: {
        "other-model": { name: "Other" }
      }
    }
  });

  assert.equal(report.provider_count, 2);
  assert.equal(report.model_count, 4);
  assert.deepEqual(report.counts, {
    exact: 2,
    normalized: 0,
    heuristic: 0,
    unknown: 2
  });
  assert.equal(report.providers[0].models[0].source_model, "other-model");
  assert.equal(report.providers[1].models[1].resolution.canonical_name, "gpt-5");
});

test("can limit a report to configured catalog source records", () => {
  const report = createModelsDevReport(
    {
      openai: { models: { "gpt-5": {} } },
      azure: { models: { "gpt-5": {}, "phi-4-mini": {} } },
      "github-models": { models: { "microsoft/phi-4-mini-instruct": {}, unrelated: {} } },
      ignored: { models: { "other-model": {} } }
    },
    { official: true }
  );

  assert.equal(report.provider_count, 3);
  assert.equal(report.model_count, 3);
  assert.deepEqual(report.providers.map((provider) => provider.provider), ["azure", "github-models", "openai"]);
  assert.equal(report.providers[0].models[0].resolution.canonical_name, "phi-4-mini-instruct");
  assert.equal(report.providers[1].models[0].resolution.confidence, "exact");
});
