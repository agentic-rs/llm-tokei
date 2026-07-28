import { randomUUID } from "node:crypto";
import { createWriteStream } from "node:fs";
import { mkdir, readFile, rename, rm } from "node:fs/promises";
import path from "node:path";
import { finished } from "node:stream/promises";

import {
  changeCsvHeader,
  changeCsvLastSequence,
  changeCsvLine,
  modelFamilyCsvLines,
  snapshotCsvLines
} from "./csv.js";
import {
  getLatestPriceSnapshot,
  iterateDailyPriceSnapshots,
  iteratePriceChanges,
  iteratePriceChangesSince,
  resolveHistoryCommit
} from "./history.js";
import { getModelFamilySnapshot } from "./families.js";
import type {
  DailySnapshotOptions,
  IncrementalChangesOptions,
  RepositoryOptions,
  WrittenChanges,
  WrittenDailySnapshot,
  WrittenModelFamilies,
  WrittenSnapshot
} from "./types.js";

export async function writeChangesCsv(options: RepositoryOptions, outputDirectory: string): Promise<WrittenChanges> {
  const commitSha = resolveHistoryCommit(options);
  const path = pathJoin(outputDirectory, changesFilename(commitSha));
  await mkdir(outputDirectory, { recursive: true });
  await writeAtomically(path, changeLines({ ...options, ref: commitSha }));
  return { commit_sha: commitSha, path };
}

export async function writeIncrementalChangesCsv(
  options: IncrementalChangesOptions,
  outputDirectory: string
): Promise<WrittenChanges> {
  const commitSha = resolveHistoryCommit(options);
  const baseCsv = await readFile(options.base_csv_path, "utf8");
  const sequence = changeCsvLastSequence(baseCsv);
  const path = pathJoin(outputDirectory, changesFilename(commitSha));
  await mkdir(outputDirectory, { recursive: true });
  await writeAtomically(
    path,
    incrementalChangeLines(
      {
        repository_path: options.repository_path,
        ref: commitSha
      },
      {
        commit_sha: options.base_commit_sha,
        sequence
      },
      baseCsv
    )
  );
  return { commit_sha: commitSha, path };
}

export async function writeDailySnapshotCsvs(
  options: DailySnapshotOptions,
  outputDirectory: string
): Promise<WrittenDailySnapshot[]> {
  const commitSha = resolveHistoryCommit(options);
  const frozen = { ...options, ref: commitSha };
  await mkdir(outputDirectory, { recursive: true });
  const snapshots: WrittenDailySnapshot[] = [];

  for await (const snapshot of iterateDailyPriceSnapshots(frozen)) {
    const path = pathJoin(outputDirectory, dailySnapshotFilename(snapshot.date, snapshot.commit_sha));
    await writeAtomically(path, snapshotCsvLines(snapshot));
    snapshots.push({
      commit_sha: snapshot.commit_sha,
      date: snapshot.date,
      path,
      ts: snapshot.ts
    });
  }

  return snapshots;
}

export async function writeLatestSnapshotCsv(
  options: RepositoryOptions,
  outputDirectory: string
): Promise<WrittenSnapshot> {
  const commitSha = resolveHistoryCommit(options);
  const snapshot = await getLatestPriceSnapshot({ ...options, ref: commitSha });
  const path = pathJoin(outputDirectory, latestSnapshotFilename(snapshot.commit_sha));
  await mkdir(outputDirectory, { recursive: true });
  await writeAtomically(path, snapshotCsvLines(snapshot));
  return { ...snapshot, path };
}

export async function writeModelFamiliesCsv(
  options: RepositoryOptions,
  outputDirectory: string
): Promise<WrittenModelFamilies> {
  const commitSha = resolveHistoryCommit(options);
  const snapshot = getModelFamilySnapshot({ ...options, ref: commitSha });
  const path = pathJoin(outputDirectory, modelFamiliesFilename(snapshot.commit_sha));
  await mkdir(outputDirectory, { recursive: true });
  await writeAtomically(path, modelFamilyCsvLines(snapshot.mappings));
  return { commit_sha: snapshot.commit_sha, path };
}

export function changesFilename(commitSha: string): string {
  return `changes.${commitSha}.csv`;
}

export function dailySnapshotFilename(date: string, commitSha: string): string {
  return `${date}.${commitSha}.csv`;
}

export function latestSnapshotFilename(commitSha: string): string {
  return `latest.${commitSha}.csv`;
}

export function modelFamiliesFilename(commitSha: string): string {
  return `families.${commitSha}.csv`;
}

async function* changeLines(options: RepositoryOptions): AsyncGenerator<string> {
  yield changeCsvHeader();
  for await (const change of iteratePriceChanges(options)) yield changeCsvLine(change);
}

async function* incrementalChangeLines(
  options: RepositoryOptions,
  checkpoint: {
    commit_sha: string;
    sequence: number;
  },
  baseCsv: string
): AsyncGenerator<string> {
  yield baseCsv;
  for await (const change of iteratePriceChangesSince(options, checkpoint)) {
    yield changeCsvLine(change);
  }
}

async function writeLines(filePath: string, lines: AsyncIterable<string> | Iterable<string>): Promise<void> {
  const stream = createWriteStream(filePath, { encoding: "utf8" });
  const completion = finished(stream);
  void completion.catch(() => undefined);
  try {
    for await (const line of lines) {
      if (!stream.write(line)) await waitForDrain(stream);
    }
    stream.end();
    await completion;
  } catch (error) {
    stream.destroy();
    await completion.catch(() => undefined);
    throw error;
  }
}

async function waitForDrain(stream: ReturnType<typeof createWriteStream>): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const cleanup = (): void => {
      stream.off("drain", onDrain);
      stream.off("error", onError);
    };
    const onDrain = (): void => {
      cleanup();
      resolve();
    };
    const onError = (error: Error): void => {
      cleanup();
      reject(error);
    };

    stream.once("drain", onDrain);
    stream.once("error", onError);
  });
}

async function writeAtomically(filePath: string, lines: AsyncIterable<string> | Iterable<string>): Promise<void> {
  const temporaryPath = `${filePath}.${process.pid}.${randomUUID()}.tmp`;
  try {
    await writeLines(temporaryPath, lines);
    await rename(temporaryPath, filePath);
  } catch (error) {
    await rm(temporaryPath, { force: true });
    throw error;
  }
}

function pathJoin(outputDirectory: string, filename: string): string {
  return path.resolve(outputDirectory, filename);
}
