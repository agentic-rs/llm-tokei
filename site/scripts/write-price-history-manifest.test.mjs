import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import { writePriceHistoryManifest } from "./write-price-history-manifest.mjs";

const COMMIT_SHA = "a".repeat(40);
const GENERATOR_REVISION = "b".repeat(40);
const CATALOG_REVISION = "c".repeat(40);
const PRICE_CSV = Buffer.from("op,provider,model\n");
const FAMILIES_CSV = Buffer.from(
  "provider,model,canonical_name,family,release_date\n",
);

test("writes checksummed price and family artifacts for one source commit", async (t) => {
  const directory = mkdtempSync(path.join(tmpdir(), "price-history-manifest-"));
  t.after(() => rmSync(directory, { force: true, recursive: true }));
  const csvPath = path.join(directory, `changes.${COMMIT_SHA}.csv`);
  const familiesCsvPath = path.join(
    directory,
    `families.${COMMIT_SHA}.csv`,
  );
  writeFileSync(csvPath, PRICE_CSV);
  writeFileSync(familiesCsvPath, FAMILIES_CSV);

  const result = await writePriceHistoryManifest(
    {
      csv: csvPath,
      families_csv: familiesCsvPath,
      catalog_revision: CATALOG_REVISION,
      generator_revision: GENERATOR_REVISION,
      source_repository: "https://github.com/anomalyco/models.dev",
      source_ref: "dev",
    },
    new Date("2026-07-29T00:00:00.000Z"),
  );

  const expected = {
    schema_version: 1,
    catalog_revision: CATALOG_REVISION,
    generator_revision: GENERATOR_REVISION,
    generated_at: "2026-07-29T00:00:00.000Z",
    source_repository: "https://github.com/anomalyco/models.dev",
    source_ref: "dev",
    source_commit_sha: COMMIT_SHA,
    csv: {
      path: path.basename(csvPath),
      bytes: PRICE_CSV.length,
      sha256: sha256(PRICE_CSV),
    },
    families: {
      path: path.basename(familiesCsvPath),
      bytes: FAMILIES_CSV.length,
      sha256: sha256(FAMILIES_CSV),
    },
  };
  assert.deepEqual(result.manifest, expected);
  assert.equal(
    readFileSync(result.path, "utf8"),
    `${JSON.stringify(expected, null, 2)}\n`,
  );
});

test("rejects artifacts generated from different source commits", async (t) => {
  const directory = mkdtempSync(path.join(tmpdir(), "price-history-manifest-"));
  t.after(() => rmSync(directory, { force: true, recursive: true }));
  const csvPath = path.join(directory, `changes.${COMMIT_SHA}.csv`);
  const familiesCsvPath = path.join(
    directory,
    `families.${"c".repeat(40)}.csv`,
  );
  writeFileSync(csvPath, PRICE_CSV);
  writeFileSync(familiesCsvPath, FAMILIES_CSV);

  await assert.rejects(
    writePriceHistoryManifest({
      csv: csvPath,
      families_csv: familiesCsvPath,
      catalog_revision: CATALOG_REVISION,
      generator_revision: GENERATOR_REVISION,
      source_repository: "https://github.com/anomalyco/models.dev",
      source_ref: "dev",
    }),
    /price and family CSVs must use the same source commit/,
  );
});

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}
