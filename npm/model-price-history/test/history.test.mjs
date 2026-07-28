import assert from "node:assert/strict";
import { test } from "node:test";

import { getLatestPriceSnapshot, iteratePriceChanges } from "../dist/index.js";
import { createFixtureRepository } from "./fixture-repository.mjs";

test("emits deterministic scalar price change events and preserves zero", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write(
    "providers/openai/models/gpt.toml",
    `name = "GPT"\n\n[cost]\ninput = 1.00\noutput = 2.00\n`
  );
  const initial = fixture.commit("add gpt", "2024-01-01T09:00:00Z");

  fixture.write(
    "providers/openai/models/gpt.toml",
    `name = "Renamed GPT"\ntemperature = true\ntemperature = true\n\n[cost]\ninput = 1.0\noutput = 2\n\n[limit]\noutput = 10\noutput = 20\n`
  );
  fixture.commit("rename gpt", "2024-01-01T12:00:00Z");

  fixture.write(
    "providers/openai/models/gpt.toml",
    `name = "Renamed GPT"\n\n[cost]\ninput = 1.5\noutput = 2\ncache_read = 0.5\n`
  );
  fixture.write(
    "providers/openrouter/models/openai/gpt.toml",
    `[cost]\ninput = 0\noutput = 3\n`
  );
  const changed = fixture.commit("change routes", "2024-01-02T09:00:00Z");

  fixture.remove("providers/openai/models/gpt.toml");
  const removed = fixture.commit("remove gpt", "2024-01-03T09:00:00Z");

  const changes = [];
  for await (const change of iteratePriceChanges({ repository_path: fixture.repository_path })) {
    changes.push(change);
  }

  assert.deepEqual(
    changes.map((change) => ({
      cache_read: "cache_read" in change ? change.cache_read : undefined,
      commit_sha: change.commit_sha,
      input: "input" in change ? change.input : undefined,
      model: change.model,
      op: change.op,
      output: "output" in change ? change.output : undefined,
      provider: change.provider,
      sequence: change.sequence,
      ts: change.ts
    })),
    [
      {
        cache_read: undefined,
        commit_sha: initial,
        input: "1",
        model: "gpt",
        op: "upsert",
        output: "2",
        provider: "openai",
        sequence: 1,
        ts: "2024-01-01T09:00:00.000Z"
      },
      {
        cache_read: "0.5",
        commit_sha: changed,
        input: "1.5",
        model: "gpt",
        op: "upsert",
        output: "2",
        provider: "openai",
        sequence: 2,
        ts: "2024-01-02T09:00:00.000Z"
      },
      {
        cache_read: undefined,
        commit_sha: changed,
        input: "0",
        model: "openai/gpt",
        op: "upsert",
        output: "3",
        provider: "openrouter",
        sequence: 3,
        ts: "2024-01-02T09:00:00.000Z"
      },
      {
        cache_read: undefined,
        commit_sha: removed,
        input: undefined,
        model: "gpt",
        op: "delete",
        output: undefined,
        provider: "openai",
        sequence: 4,
        ts: "2024-01-03T09:00:00.000Z"
      }
    ]
  );

  const latest = await getLatestPriceSnapshot({ repository_path: fixture.repository_path });
  assert.equal(latest.commit_sha, removed);
  assert.deepEqual(latest.prices, [
    {
      input: "0",
      model: "openai/gpt",
      output: "3",
      provider: "openrouter"
    }
  ]);
});

test("resolves historical extends without making base-model metadata a price event", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write("providers/base/models/alpha.toml", `[cost]\ninput = 1\noutput = 2\n`);
  fixture.write("providers/wrapper/models/alpha.toml", `[extends]\nfrom = "base/alpha"\n`);
  fixture.write("models/openai/gpt.toml", `name = "GPT"\n`);
  fixture.write(
    "providers/edge/models/openai/gpt.toml",
    `base_model = "openai/gpt"\n\n[cost]\ninput = 3\noutput = 4\n`
  );
  fixture.commit("add inherited routes", "2024-02-01T09:00:00Z");

  fixture.write("providers/base/models/alpha.toml", `[cost]\ninput = 5\noutput = 2\n`);
  const baseChanged = fixture.commit("change base route", "2024-02-02T09:00:00Z");

  fixture.write("models/openai/gpt.toml", `name = "GPT renamed"\n`);
  fixture.commit("change model metadata", "2024-02-03T09:00:00Z");

  const changes = [];
  for await (const change of iteratePriceChanges({ repository_path: fixture.repository_path })) {
    changes.push(change);
  }

  const updates = changes.filter((change) => change.commit_sha === baseChanged);
  assert.deepEqual(
    updates.map((change) => [change.provider, change.model, change.op, "input" in change ? change.input : undefined]),
    [
      ["base", "alpha", "upsert", "5"],
      ["wrapper", "alpha", "upsert", "5"]
    ]
  );
  assert.equal(changes.filter((change) => change.ts === "2024-02-03T09:00:00.000Z").length, 0);
});

test("keeps local prices when historical base_model metadata is missing", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write(
    "providers/opencode/models/minimax-m3.toml",
    `base_model = "minimax/minimax-m3"\n\n[cost]\ninput = 0.3\noutput = 1.2\ncache_read = 0.06\n`
  );
  fixture.commit("add route with missing base metadata", "2024-02-15T09:00:00Z");

  const changes = [];
  for await (const change of iteratePriceChanges({ repository_path: fixture.repository_path })) {
    changes.push(change);
  }

  assert.deepEqual(
    changes.map((change) => ({
      cache_read: "cache_read" in change ? change.cache_read : undefined,
      input: "input" in change ? change.input : undefined,
      model: change.model,
      output: "output" in change ? change.output : undefined,
      provider: change.provider
    })),
    [
      {
        cache_read: "0.06",
        input: "0.3",
        model: "minimax-m3",
        output: "1.2",
        provider: "opencode"
      }
    ]
  );
});

test("resolves Git symlinked provider models and ignores broken links", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write("providers/base/models/gpt.toml", `[cost]\ninput = 1\noutput = 2\n`);
  fixture.symlink("providers/alias/models/gpt.toml", "../../base/models/gpt.toml");
  fixture.symlink("providers/broken/models/gpt.toml", "../../missing/models/gpt.toml");
  fixture.commit("add aliases", "2024-05-01T09:00:00Z");

  fixture.write("providers/base/models/gpt.toml", `[cost]\ninput = 2\noutput = 3\n`);
  const changed = fixture.commit("change base", "2024-05-02T09:00:00Z");

  const changes = [];
  for await (const change of iteratePriceChanges({ repository_path: fixture.repository_path })) {
    changes.push(change);
  }

  assert.deepEqual(
    changes
      .filter((change) => change.commit_sha === changed)
      .map((change) => [change.provider, change.model, change.op, "input" in change ? change.input : undefined]),
    [
      ["alias", "gpt", "upsert", "2"],
      ["base", "gpt", "upsert", "2"]
    ]
  );
  assert.equal(changes.some((change) => change.provider === "broken"), false);
});

test("normalizes historical cached price field names", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write(
    "providers/anthropic/models/claude.toml",
    `[cost]\ninput = 3\noutput = 15\ninputCached = 3.75\noutputCached = 0.3\n`
  );
  fixture.commit("add legacy cache price", "2024-06-01T09:00:00Z");

  const changes = [];
  for await (const change of iteratePriceChanges({ repository_path: fixture.repository_path })) {
    changes.push(change);
  }

  assert.equal(changes.length, 1);
  assert.equal(changes[0].cache_read, "3.75");
  assert.equal(changes[0].cache_write, "0.3");
});

test("uses code-unit ordering for deterministic event sequences", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  for (const provider of ["z", "ä", "a"]) {
    fixture.write(`providers/${provider}/models/gpt.toml`, `[cost]\ninput = 1\noutput = 2\n`);
  }
  fixture.commit("add locale-sensitive providers", "2024-07-01T09:00:00Z");

  const changes = [];
  for await (const change of iteratePriceChanges({ repository_path: fixture.repository_path })) {
    changes.push(change);
  }

  assert.deepEqual(
    changes.map((change) => [change.sequence, change.provider]),
    [
      [1, "a"],
      [2, "z"],
      [3, "ä"]
    ]
  );
});
