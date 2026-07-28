#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";

const options = parseOptions(process.argv.slice(2));
const csvPath = path.resolve(requiredOption(options, "csv"));
const generatorRevision = requiredOption(options, "generator_revision");
const sourceRepository = requiredOption(options, "source_repository");
const sourceRef = requiredOption(options, "source_ref");
const match = path.basename(csvPath).match(/^changes\.([0-9a-f]{40,64})\.csv$/);

if (!match) {
  throw new Error(
    `CSV filename does not contain a full source commit: ${csvPath}`,
  );
}
if (!/^[0-9a-f]{40,64}$/.test(generatorRevision)) {
  throw new Error(`--generator-revision must be a full Git object ID`);
}

const bytes = await readFile(csvPath);
const csvStat = await stat(csvPath);
const manifest = {
  schema_version: 1,
  generator_revision: generatorRevision,
  generated_at: new Date().toISOString(),
  source_repository: sourceRepository,
  source_ref: sourceRef,
  source_commit_sha: match[1],
  csv: {
    path: path.basename(csvPath),
    bytes: csvStat.size,
    sha256: createHash("sha256").update(bytes).digest("hex"),
  },
};
const manifestPath = path.join(path.dirname(csvPath), "manifest.json");

await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
process.stdout.write(`${manifestPath}\n`);

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
