import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import { restorePriceHistoryBase } from "./restore-price-history-base.mjs";

const COMMIT_SHA = "a".repeat(40);
const GENERATOR_REVISION = "b".repeat(40);
const CSV = Buffer.from(
  "op,ts,commit_sha,sequence,provider,model,input,output,reasoning,cache_read,cache_write,input_audio,output_audio\n",
);

test("restores a compatible deployed change stream", async (t) => {
  const outputDirectory = mkdtempSync(
    path.join(tmpdir(), "price-history-base-"),
  );
  t.after(() => rmSync(outputDirectory, { force: true, recursive: true }));
  const manifest = createManifest();
  const result = await restorePriceHistoryBase(
    createOptions(outputDirectory),
    createFetch(manifest, CSV),
  );

  assert.equal(result.available, true);
  assert.equal(result.base_commit_sha, COMMIT_SHA);
  assert.deepEqual(readFileSync(result.base_csv_path), CSV);
});

test("rejects an incompatible generator revision", async (t) => {
  const outputDirectory = mkdtempSync(
    path.join(tmpdir(), "price-history-base-"),
  );
  t.after(() => rmSync(outputDirectory, { force: true, recursive: true }));
  const manifest = {
    ...createManifest(),
    generator_revision: "c".repeat(40),
  };
  const result = await restorePriceHistoryBase(
    createOptions(outputDirectory),
    createFetch(manifest, CSV),
  );

  assert.deepEqual(result, {
    available: false,
    reason: "generator revision changed",
  });
});

test("rejects a change stream with the wrong checksum", async (t) => {
  const outputDirectory = mkdtempSync(
    path.join(tmpdir(), "price-history-base-"),
  );
  t.after(() => rmSync(outputDirectory, { force: true, recursive: true }));
  const manifest = {
    ...createManifest(),
    prices: {
      ...createManifest().prices,
      path: `changes.${"d".repeat(64)}.csv`,
      sha256: "d".repeat(64),
    },
  };
  const result = await restorePriceHistoryBase(
    createOptions(outputDirectory),
    createFetch(manifest, CSV),
  );

  assert.deepEqual(result, {
    available: false,
    reason: "CSV checksum mismatch",
  });
});

function createManifest() {
  return {
    schema_version: 2,
    catalog_revision: "c".repeat(40),
    generator_revision: GENERATOR_REVISION,
    generated_at: "2026-07-28T00:00:00.000Z",
    source_repository: "https://github.com/anomalyco/models.dev",
    source_ref: "dev",
    source_commit_sha: COMMIT_SHA,
    prices: {
      path: `changes.${createHash("sha256").update(CSV).digest("hex")}.csv`,
      latest_path: "changes.csv",
      bytes: CSV.length,
      sha256: createHash("sha256").update(CSV).digest("hex"),
    },
    families: {
      path: `families.${createHash("sha256").update("").digest("hex")}.csv`,
      latest_path: "families.csv",
      bytes: 0,
      sha256: createHash("sha256").update("").digest("hex"),
    },
  };
}

function createOptions(outputDirectory) {
  return {
    generator_revision: GENERATOR_REVISION,
    manifest_url:
      "https://agentic.tokn-ai.dev/llm-tokei/models/manifest.json",
    output_directory: outputDirectory,
    source_ref: "dev",
    source_repository: "https://github.com/anomalyco/models.dev",
  };
}

function createFetch(manifest, csv) {
  return async (url) => {
    if (url.pathname.endsWith("/manifest.json")) {
      return new Response(JSON.stringify(manifest), {
        headers: { "content-type": "application/json" },
      });
    }
    return new Response(csv, {
      headers: { "content-length": String(csv.length) },
    });
  };
}
