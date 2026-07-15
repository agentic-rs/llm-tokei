import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  getLatestPriceSnapshot,
  iterateDailyPriceSnapshots,
  writeChangesCsv,
  writeDailySnapshotCsvs,
  writeLatestSnapshotCsv
} from "../dist/index.js";
import { createFixtureRepository } from "./fixture-repository.mjs";

test("creates completed UTC daily snapshots and skips today", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write("providers/openai/models/gpt.toml", `[cost]\ninput = 1\noutput = 2\n`);
  const first = fixture.commit("add gpt", "2024-03-01T09:00:00Z");
  fixture.write("providers/openai/models/gpt.toml", `[cost]\ninput = 2\noutput = 3\n`);
  const second = fixture.commit("change gpt", "2024-03-02T09:00:00Z");
  fixture.remove("providers/openai/models/gpt.toml");
  const deleted = fixture.commit("remove gpt", "2024-03-04T09:00:00Z");
  fixture.write("README.md", "# fixture\n");
  const head = fixture.commit("metadata today", "2024-03-05T09:00:00Z");

  const options = {
    now: new Date("2024-03-05T12:00:00Z"),
    repository_path: fixture.repository_path
  };
  const daily = [];
  for await (const snapshot of iterateDailyPriceSnapshots(options)) daily.push(snapshot);

  assert.deepEqual(
    daily.map((snapshot) => ({ date: snapshot.date, commit_sha: snapshot.commit_sha, prices: snapshot.prices.length })),
    [
      { date: "2024-03-01", commit_sha: first, prices: 1 },
      { date: "2024-03-02", commit_sha: second, prices: 1 },
      { date: "2024-03-03", commit_sha: second, prices: 1 },
      { date: "2024-03-04", commit_sha: deleted, prices: 0 }
    ]
  );

  const outputDirectory = mkdtempSync(path.join(tmpdir(), "model-price-history-output-"));
  t.after(() => rmSync(outputDirectory, { force: true, recursive: true }));
  const changes = await writeChangesCsv(options, outputDirectory);
  const writtenDaily = await writeDailySnapshotCsvs(options, outputDirectory);
  const latest = await writeLatestSnapshotCsv(options, outputDirectory);

  const firstChanges = readFileSync(changes.path, "utf8");
  await writeChangesCsv(options, outputDirectory);
  assert.equal(readFileSync(changes.path, "utf8"), firstChanges);

  assert.equal(path.basename(changes.path), `changes.${head}.csv`);
  assert.equal(path.basename(latest.path), `latest.${head}.csv`);
  assert.deepEqual(
    writtenDaily.map((snapshot) => path.basename(snapshot.path)),
    [
      `2024-03-01.${first}.csv`,
      `2024-03-02.${second}.csv`,
      `2024-03-03.${second}.csv`,
      `2024-03-04.${deleted}.csv`
    ]
  );
  assert.deepEqual(
    writtenDaily.map((snapshot) => ({ date: snapshot.date, ts: snapshot.ts })),
    [
      { date: "2024-03-01", ts: "2024-03-01T09:00:00.000Z" },
      { date: "2024-03-02", ts: "2024-03-02T09:00:00.000Z" },
      { date: "2024-03-03", ts: "2024-03-02T09:00:00.000Z" },
      { date: "2024-03-04", ts: "2024-03-04T09:00:00.000Z" }
    ]
  );
  assert.equal("prices" in writtenDaily[0], false);
  assert.deepEqual(
    readdirSync(outputDirectory).sort(),
    [
      `2024-03-01.${first}.csv`,
      `2024-03-02.${second}.csv`,
      `2024-03-03.${second}.csv`,
      `2024-03-04.${deleted}.csv`,
      `changes.${head}.csv`,
      `latest.${head}.csv`
    ].sort()
  );
  assert.match(readFileSync(changes.path, "utf8"), /^op,ts,commit_sha,sequence,provider,model,input,output/);
  assert.equal(
    readFileSync(latest.path, "utf8"),
    "ts,provider,model,input,output,reasoning,cache_read,cache_write,input_audio,output_audio\n"
  );
  assert.equal(
    readFileSync(path.join(outputDirectory, `2024-03-03.${second}.csv`), "utf8"),
    "ts,provider,model,input,output,reasoning,cache_read,cache_write,input_audio,output_audio\n" +
      "2024-03-02T09:00:00.000Z,openai,gpt,2,3,,,,,\n"
  );
  assert.equal((await getLatestPriceSnapshot(options)).commit_sha, head);
});

test("runs the CLI against a local repository", (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());
  fixture.write("providers/openai/models/gpt.toml", `[cost]\ninput = 1\noutput = 2\n`);
  const head = fixture.commit("add gpt", "2024-04-01T09:00:00Z");

  const outputDirectory = mkdtempSync(path.join(tmpdir(), "model-price-history-cli-"));
  t.after(() => rmSync(outputDirectory, { force: true, recursive: true }));
  const cli = new URL("../dist/cli.js", import.meta.url);
  const output = execFileSync(
    process.execPath,
    [fileURLToPath(cli), "latest", "--repo", fixture.repository_path, "--out-dir", outputDirectory],
    { encoding: "utf8" }
  ).trim();

  assert.equal(output, path.join(outputDirectory, `latest.${head}.csv`));
  assert.match(readFileSync(output, "utf8"), /2024-04-01T09:00:00.000Z,openai,gpt,1,2/);

  const help = execFileSync(process.execPath, [fileURLToPath(cli), "daily", "--help"], { encoding: "utf8" });
  assert.match(help, /model-price-history daily/);
});

test("keeps header-only daily snapshots after same-day price removal", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write("providers/openai/models/gpt.toml", `[cost]\ninput = 1\noutput = 2\n`);
  fixture.commit("add gpt", "2024-05-01T09:00:00Z");
  fixture.remove("providers/openai/models/gpt.toml");
  const removed = fixture.commit("remove gpt", "2024-05-01T10:00:00Z");
  fixture.write("README.md", "# fixture\n");
  const metadata = fixture.commit("metadata", "2024-05-02T09:00:00Z");

  const daily = [];
  for await (const snapshot of iterateDailyPriceSnapshots({
    now: new Date("2024-05-03T12:00:00Z"),
    repository_path: fixture.repository_path
  })) {
    daily.push(snapshot);
  }

  assert.deepEqual(
    daily.map((snapshot) => ({
      commit_sha: snapshot.commit_sha,
      date: snapshot.date,
      prices: snapshot.prices.length
    })),
    [
      { commit_sha: removed, date: "2024-05-01", prices: 0 },
      { commit_sha: metadata, date: "2024-05-02", prices: 0 }
    ]
  );
});
