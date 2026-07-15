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

test("can limit a report to official catalog vendors", () => {
  const report = createModelsDevReport(
    {
      openai: { models: { "gpt-5": {} } },
      ignored: { models: { "other-model": {} } }
    },
    { official: true }
  );

  assert.equal(report.provider_count, 1);
  assert.equal(report.model_count, 1);
  assert.equal(report.providers[0].provider, "openai");
});
