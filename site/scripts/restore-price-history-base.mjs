#!/usr/bin/env node

import { createHash } from "node:crypto";
import { appendFile, mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const MAX_CSV_BYTES = 512 * 1024 * 1024;

export async function restorePriceHistoryBase(
  options,
  fetchImplementation = fetch,
) {
  try {
    const manifestUrl = new URL(options.manifest_url);
    const manifestResponse = await fetchImplementation(manifestUrl, {
      signal: AbortSignal.timeout(15_000),
    });
    if (!manifestResponse.ok) {
      throw new Error(
        `manifest request failed with HTTP ${manifestResponse.status}`,
      );
    }

    const manifest = await manifestResponse.json();
    validateManifest(manifest, options);
    const csvUrl = new URL(manifest.prices.path, manifestUrl);
    if (csvUrl.origin !== manifestUrl.origin) {
      throw new Error("manifest CSV must use the manifest origin");
    }

    const csvResponse = await fetchImplementation(csvUrl, {
      signal: AbortSignal.timeout(30_000),
    });
    if (!csvResponse.ok) {
      throw new Error(`CSV request failed with HTTP ${csvResponse.status}`);
    }
    const contentLength = Number(csvResponse.headers.get("content-length"));
    if (Number.isFinite(contentLength) && contentLength > MAX_CSV_BYTES) {
      throw new Error(`CSV exceeds the ${MAX_CSV_BYTES}-byte safety limit`);
    }

    const bytes = Buffer.from(await csvResponse.arrayBuffer());
    if (bytes.length !== manifest.prices.bytes) {
      throw new Error(
        `CSV size mismatch: expected ${manifest.prices.bytes}, received ${bytes.length}`,
      );
    }
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    if (sha256 !== manifest.prices.sha256) {
      throw new Error("CSV checksum mismatch");
    }

    const outputDirectory = path.resolve(options.output_directory);
    const csvPath = path.join(outputDirectory, manifest.prices.path);
    await mkdir(outputDirectory, { recursive: true });
    await writeFile(csvPath, bytes);
    return {
      available: true,
      base_commit_sha: manifest.source_commit_sha,
      base_csv_path: csvPath,
    };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return {
      available: false,
      reason: message,
    };
  }
}

function validateManifest(manifest, options) {
  if (!isObject(manifest)) throw new Error("manifest is not an object");
  if (manifest.schema_version !== 2) {
    throw new Error(`unsupported manifest schema ${manifest.schema_version}`);
  }
  if (manifest.generator_revision !== options.generator_revision) {
    throw new Error("generator revision changed");
  }
  if (manifest.source_repository !== options.source_repository) {
    throw new Error("source repository changed");
  }
  if (manifest.source_ref !== options.source_ref) {
    throw new Error("source ref changed");
  }
  if (
    typeof manifest.source_commit_sha !== "string" ||
    !/^[0-9a-f]{40,64}$/.test(manifest.source_commit_sha)
  ) {
    throw new Error("manifest source commit is invalid");
  }
  if (!isObject(manifest.prices))
    throw new Error("manifest CSV metadata is missing");

  if (
    !Number.isSafeInteger(manifest.prices.bytes) ||
    manifest.prices.bytes < 0 ||
    manifest.prices.bytes > MAX_CSV_BYTES
  ) {
    throw new Error("manifest CSV size is invalid");
  }
  if (
    typeof manifest.prices.sha256 !== "string" ||
    !/^[0-9a-f]{64}$/.test(manifest.prices.sha256)
  ) {
    throw new Error("manifest CSV checksum is invalid");
  }
  if (manifest.prices.path !== `changes.${manifest.prices.sha256}.csv`) {
    throw new Error("manifest CSV path does not match its checksum");
  }
}

function isObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function run(argv = process.argv.slice(2)) {
  const options = parseOptions(argv);
  const result = await restorePriceHistoryBase({
    generator_revision: requiredOption(options, "generator_revision"),
    manifest_url: requiredOption(options, "manifest_url"),
    output_directory: requiredOption(options, "output_directory"),
    source_ref: requiredOption(options, "source_ref"),
    source_repository: requiredOption(options, "source_repository"),
  });

  if (result.available) {
    process.stderr.write(
      `price history: resuming from ${result.base_commit_sha}\n`,
    );
  } else {
    process.stderr.write(
      `price history: no reusable deployment (${result.reason}); rebuilding complete history\n`,
    );
  }

  const outputs = result.available
    ? {
        available: "true",
        base_commit_sha: result.base_commit_sha,
        base_csv_path: result.base_csv_path,
      }
    : {
        available: "false",
        base_commit_sha: "",
        base_csv_path: "",
      };
  const githubOutput = process.env.GITHUB_OUTPUT;
  if (githubOutput) {
    await appendFile(
      githubOutput,
      Object.entries(outputs)
        .map(([name, value]) => `${name}=${value}\n`)
        .join(""),
      "utf8",
    );
  } else {
    process.stdout.write(`${JSON.stringify(outputs)}\n`);
  }
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

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  await run();
}
