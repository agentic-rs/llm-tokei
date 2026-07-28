import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { getModelFamilySnapshot, writeModelFamiliesCsv } from "../dist/index.js";
import { createFixtureRepository } from "./fixture-repository.mjs";

test("maps every historical provider route through model-catalog", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write("models/openai/gpt-5.toml", `release_date = "2025-08-07"\n`);
  fixture.write("providers/openai/models/gpt-5.toml", `base_model = "openai/gpt-5"\n`);
  fixture.write(
    "providers/azure/models/phi-4-mini.toml",
    `family = "wrong-route-family"\nrelease_date = "2024-02"\n[cost]\ninput = "not-a-price"\n`
  );
  fixture.write(
    "providers/anthropic/models/claude-sonnet-4-5-20250929.toml",
    `family = "wrong-removed-family"\nrelease_date = "2024-10-22"\n`
  );
  fixture.write(
    "providers/openai/models/gpt-5-chat-latest.toml",
    `family = "wrong-rolling-family"\nrelease_date = "2025-08-07"\n`
  );
  fixture.write(
    "providers/unknown/models/future,model.toml",
    `family = "invented-upstream-family"\nrelease_date = "2026-01"\n`
  );
  fixture.commit("add family mappings", "2024-01-01T09:00:00Z");

  fixture.write("models/openai/gpt-5.toml", `release_date = "2025-08-08"\n`);
  fixture.write(
    "providers/azure/models/phi-4-mini.toml",
    `family = "another-wrong-family"\nrelease_date = "2024-02-15"\n`
  );
  fixture.commit("correct release dates", "2024-01-02T09:00:00Z");

  fixture.remove("providers/anthropic/models/claude-sonnet-4-5-20250929.toml");
  fixture.commit("remove old route", "2024-01-03T09:00:00Z");

  fixture.remove("providers/openai/models/gpt-5-chat-latest.toml");
  fixture.commit("temporarily remove route", "2024-01-04T09:00:00Z");
  fixture.write(
    "providers/openai/models/gpt-5-chat-latest.toml",
    `family = "still-wrong-after-restore"\nrelease_date = "2025-09-01"\n`
  );
  const head = fixture.commit("restore route", "2024-01-05T09:00:00Z");

  const snapshot = getModelFamilySnapshot({ repository_path: fixture.repository_path });

  assert.equal(snapshot.commit_sha, head);
  assert.deepEqual(snapshot.mappings, [
    {
      canonical_name: "claude-sonnet-4.5",
      family: "claude-sonnet",
      model: "claude-sonnet-4-5-20250929",
      provider: "anthropic",
      release_date: "2024-10-22"
    },
    {
      canonical_name: "phi-4-mini-instruct",
      family: "phi-4-mini",
      model: "phi-4-mini",
      provider: "azure",
      release_date: "2024-02-15"
    },
    {
      canonical_name: "gpt-5",
      family: "gpt-5",
      model: "gpt-5",
      provider: "openai",
      release_date: "2025-08-08"
    },
    {
      canonical_name: "gpt-5",
      family: "gpt-5",
      model: "gpt-5-chat-latest",
      provider: "openai",
      release_date: "2025-09-01"
    },
    {
      model: "future,model",
      provider: "unknown",
      release_date: "2026-01"
    }
  ]);

  const outputDirectory = mkdtempSync(path.join(tmpdir(), "model-family-mapping-"));
  t.after(() => rmSync(outputDirectory, { force: true, recursive: true }));
  const result = await writeModelFamiliesCsv(
    { repository_path: fixture.repository_path },
    outputDirectory
  );

  assert.equal(path.basename(result.path), `families.${head}.csv`);
  assert.equal(
    readFileSync(result.path, "utf8"),
    "provider,model,canonical_name,family,release_date\n" +
      "anthropic,claude-sonnet-4-5-20250929,claude-sonnet-4.5,claude-sonnet,2024-10-22\n" +
      "azure,phi-4-mini,phi-4-mini-instruct,phi-4-mini,2024-02-15\n" +
      "openai,gpt-5,gpt-5,gpt-5,2025-08-08\n" +
      "openai,gpt-5-chat-latest,gpt-5,gpt-5,2025-09-01\n" +
      'unknown,"future,model",,,2026-01\n'
  );
});

test("runs family mapping generation through the CLI", (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());
  fixture.write(
    "providers/openai/models/gpt-5.toml",
    `family = "wrong-upstream-family"\nrelease_date = "2025-08-07"\n`
  );
  const head = fixture.commit("add family", "2024-02-01T09:00:00Z");

  const outputDirectory = mkdtempSync(path.join(tmpdir(), "model-family-mapping-cli-"));
  t.after(() => rmSync(outputDirectory, { force: true, recursive: true }));
  const cli = new URL("../dist/cli.js", import.meta.url);
  const output = execFileSync(
    process.execPath,
    [fileURLToPath(cli), "families", "--repo", fixture.repository_path, "--out-dir", outputDirectory],
    { encoding: "utf8" }
  ).trim();

  assert.equal(output, path.join(outputDirectory, `families.${head}.csv`));
  assert.match(
    readFileSync(output, "utf8"),
    /^provider,model,canonical_name,family,release_date\nopenai,gpt-5,gpt-5,gpt-5,2025-08-07\n$/
  );
});

test("ignores invalid historical release dates and accepts later corrections", (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());
  fixture.write(
    "providers/openai/models/gpt-5.toml",
    `release_date = "2025-16-09"\n`
  );
  fixture.commit("add invalid historical date", "2024-03-01T09:00:00Z");
  fixture.write(
    "providers/openai/models/gpt-5.toml",
    `release_date = "2025-09-16"\n`
  );
  fixture.commit("correct historical date", "2024-03-02T09:00:00Z");

  assert.deepEqual(
    getModelFamilySnapshot({ repository_path: fixture.repository_path }).mappings,
    [
      {
        canonical_name: "gpt-5",
        family: "gpt-5",
        model: "gpt-5",
        provider: "openai",
        release_date: "2025-09-16"
      }
    ]
  );
});
