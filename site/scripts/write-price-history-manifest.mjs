#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export async function writePriceHistoryManifest(options, now = new Date()) {
  const csvPath = path.resolve(options.csv);
  const familiesCsvPath = path.resolve(options.families_csv);
  if (path.dirname(csvPath) !== path.dirname(familiesCsvPath)) {
    throw new Error("price and family CSVs must use the same output directory");
  }
  if (!/^[0-9a-f]{40,64}$/.test(options.catalog_revision)) {
    throw new Error("--catalog-revision must be a full Git object ID");
  }
  if (!/^[0-9a-f]{40,64}$/.test(options.generator_revision)) {
    throw new Error("--generator-revision must be a full Git object ID");
  }
  if (Number.isNaN(now.getTime())) {
    throw new Error("manifest generation date is invalid");
  }

  const [pricesSource, familiesSource] = await Promise.all([
    inspectArtifact(csvPath, "changes"),
    inspectArtifact(familiesCsvPath, "families"),
  ]);
  if (pricesSource.source_commit_sha !== familiesSource.source_commit_sha) {
    throw new Error("price and family CSVs must use the same source commit");
  }
  const [prices, families] = await Promise.all([
    publishArtifact(pricesSource, "changes"),
    publishArtifact(familiesSource, "families"),
  ]);

  const manifest = {
    schema_version: 2,
    catalog_revision: options.catalog_revision,
    generator_revision: options.generator_revision,
    generated_at: now.toISOString(),
    source_repository: options.source_repository,
    source_ref: options.source_ref,
    source_commit_sha: pricesSource.source_commit_sha,
    prices,
    families,
  };
  const manifestPath = path.join(path.dirname(csvPath), "manifest.json");
  await writeFile(
    manifestPath,
    `${JSON.stringify(manifest, null, 2)}\n`,
    "utf8",
  );
  return { manifest, path: manifestPath };
}

async function inspectArtifact(filePath, name) {
  const match = path
    .basename(filePath)
    .match(new RegExp(`^${name}\\.([0-9a-f]{40,64})\\.csv$`));
  if (!match) {
    throw new Error(
      `${name} CSV filename does not contain a full source commit: ${filePath}`,
    );
  }
  const bytes = await readFile(filePath);
  return {
    bytes,
    file_path: filePath,
    source_commit_sha: match[1],
    sha256: createHash("sha256").update(bytes).digest("hex"),
  };
}

async function publishArtifact(source, name) {
  const directory = path.dirname(source.file_path);
  const immutableName = `${name}.${source.sha256}.csv`;
  const latestName = `${name}.csv`;
  await Promise.all([
    writeFile(path.join(directory, immutableName), source.bytes),
    writeFile(path.join(directory, latestName), source.bytes),
  ]);
  await rm(source.file_path);
  return {
    path: immutableName,
    latest_path: latestName,
    bytes: source.bytes.length,
    sha256: source.sha256,
  };
}

function parseOptions(argv) {
  const parsed = {};
  for (let index = 0; index < argv.length; index += 2) {
    const option = argv[index];
    const value = argv[index + 1];
    if (!option?.startsWith("--") || !value) {
      throw new Error(`invalid option near ${JSON.stringify(option)}`);
    }
    parsed[option.slice(2).replaceAll("-", "_")] = value;
  }
  return parsed;
}

function requiredOption(options, name) {
  const value = options[name];
  if (!value) throw new Error(`--${name.replaceAll("_", "-")} is required`);
  return value;
}

async function run(argv = process.argv.slice(2)) {
  const options = parseOptions(argv);
  const result = await writePriceHistoryManifest({
    csv: requiredOption(options, "csv"),
    families_csv: requiredOption(options, "families_csv"),
    catalog_revision: requiredOption(options, "catalog_revision"),
    generator_revision: requiredOption(options, "generator_revision"),
    source_repository: requiredOption(options, "source_repository"),
    source_ref: requiredOption(options, "source_ref"),
  });
  process.stdout.write(`${result.path}\n`);
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  await run();
}
