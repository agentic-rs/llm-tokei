#!/usr/bin/env node

import {
  writeChangesCsv,
  writeDailySnapshotCsvs,
  writeIncrementalChangesCsv,
  writeLatestSnapshotCsv,
  writeModelFamiliesCsv
} from "./index.js";

type Command = "changes" | "daily" | "families" | "latest";

export async function run(argv: readonly string[] = process.argv.slice(2)): Promise<void> {
  const [command, ...rest] = argv;
  if (command === "--help" || command === "-h" || !command) {
    printUsage();
    return;
  }
  if (!isCommand(command)) {
    throw new Error(`unknown command ${JSON.stringify(command)}`);
  }
  if (rest.includes("--help") || rest.includes("-h")) {
    printUsage();
    return;
  }

  const options = parseOptions(rest);
  if (!options.repository_path) throw new Error("--repo is required");
  if (!options.output_directory) throw new Error("--out-dir is required");

  const input = { repository_path: options.repository_path, ref: options.ref };
  if (command === "changes") {
    const hasBaseCsv = options.base_csv_path !== undefined;
    const hasBaseCommit = options.base_commit_sha !== undefined;
    if (hasBaseCsv !== hasBaseCommit) {
      throw new Error("--base-csv and --base-commit must be provided together");
    }

    let result;
    if (options.base_csv_path && options.base_commit_sha) {
      try {
        result = await writeIncrementalChangesCsv(
          {
            ...input,
            base_commit_sha: options.base_commit_sha,
            base_csv_path: options.base_csv_path
          },
          options.output_directory
        );
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        process.stderr.write(
          `model-price-history: incremental checkpoint rejected (${message}); rebuilding complete history\n`
        );
        result = await writeChangesCsv(input, options.output_directory);
      }
    } else {
      result = await writeChangesCsv(input, options.output_directory);
    }
    process.stdout.write(`${result.path}\n`);
    return;
  }
  if (options.base_csv_path || options.base_commit_sha) {
    throw new Error("--base-csv and --base-commit are only valid for changes");
  }
  if (command === "daily") {
    for (const result of await writeDailySnapshotCsvs(input, options.output_directory)) {
      process.stdout.write(`${result.path}\n`);
    }
    return;
  }
  if (command === "families") {
    const result = await writeModelFamiliesCsv(input, options.output_directory);
    process.stdout.write(`${result.path}\n`);
    return;
  }
  const result = await writeLatestSnapshotCsv(input, options.output_directory);
  process.stdout.write(`${result.path}\n`);
}

function isCommand(value: string): value is Command {
  return value === "changes" || value === "daily" || value === "families" || value === "latest";
}

function parseOptions(argv: readonly string[]): {
  base_commit_sha?: string;
  base_csv_path?: string;
  output_directory?: string;
  ref?: string;
  repository_path?: string;
} {
  const options: {
    base_commit_sha?: string;
    base_csv_path?: string;
    output_directory?: string;
    ref?: string;
    repository_path?: string;
  } = {};
  for (let index = 0; index < argv.length; index += 1) {
    const option = argv[index];
    const value = argv[index + 1];
    if (option === "--repo") {
      options.repository_path = requiredOptionValue(option, value);
    } else if (option === "--ref") {
      options.ref = requiredOptionValue(option, value);
    } else if (option === "--out-dir") {
      options.output_directory = requiredOptionValue(option, value);
    } else if (option === "--base-csv") {
      options.base_csv_path = requiredOptionValue(option, value);
    } else if (option === "--base-commit") {
      options.base_commit_sha = requiredOptionValue(option, value);
    } else {
      throw new Error(`unknown option ${JSON.stringify(option)}`);
    }
    index += 1;
  }
  return options;
}

function requiredOptionValue(option: string, value: string | undefined): string {
  if (!value || value.startsWith("--")) throw new Error(`${option} requires a value`);
  return value;
}

function printUsage(): void {
  process.stdout.write(
    `Usage:\n\n  model-price-history changes --repo <models.dev> --out-dir <directory> [--ref <ref>] [--base-csv <csv> --base-commit <sha>]\n  model-price-history daily --repo <models.dev> --out-dir <directory> [--ref <ref>]\n  model-price-history families --repo <models.dev> --out-dir <directory> [--ref <ref>]\n  model-price-history latest --repo <models.dev> --out-dir <directory> [--ref <ref>]\n`
  );
}

run().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`model-price-history: ${message}\n`);
  process.exitCode = 1;
});
