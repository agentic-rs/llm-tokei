import { randomUUID } from "node:crypto";
import { createWriteStream } from "node:fs";
import { mkdir, rename, rm } from "node:fs/promises";
import path from "node:path";
import { finished } from "node:stream/promises";

import { changeCsvHeader, changeCsvLine, snapshotCsvLines } from "./csv.js";
import {
  getLatestPriceSnapshot,
  iterateDailyPriceSnapshots,
  iteratePriceChanges,
  resolveHistoryCommit
} from "./history.js";
import type {
  DailySnapshotOptions,
  RepositoryOptions,
  WrittenChanges,
  WrittenDailySnapshot,
  WrittenSnapshot
} from "./types.js";

export async function writeChangesCsv(
  options: RepositoryOptions,
  outputDirectory: string
): Promise<WrittenChanges> {
  const commitSha = resolveHistoryCommit(options);
  const path = pathJoin(outputDirectory, changesFilename(commitSha));
  await mkdir(outputDirectory, { recursive: true });
  await writeAtomically(path, changeLines({ ...options, ref: commitSha }));
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

export function changesFilename(commitSha: string): string {
  return `changes.${commitSha}.csv`;
}

export function dailySnapshotFilename(date: string, commitSha: string): string {
  return `${date}.${commitSha}.csv`;
}

export function latestSnapshotFilename(commitSha: string): string {
  return `latest.${commitSha}.csv`;
}

async function* changeLines(options: RepositoryOptions): AsyncGenerator<string> {
  yield changeCsvHeader();
  for await (const change of iteratePriceChanges(options)) yield changeCsvLine(change);
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

async function writeAtomically(
  filePath: string,
  lines: AsyncIterable<string> | Iterable<string>
): Promise<void> {
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
