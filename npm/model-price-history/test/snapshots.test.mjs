import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  getLatestPriceSnapshot,
  iterateDailyPriceSnapshots,
  writeChangesCsv,
  writeDailySnapshotCsvs,
  writeIncrementalChangesCsv,
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
    daily.map((snapshot) => ({
      date: snapshot.date,
      commit_sha: snapshot.commit_sha,
      prices: snapshot.prices.length
    })),
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
    [`2024-03-01.${first}.csv`, `2024-03-02.${second}.csv`, `2024-03-03.${second}.csv`, `2024-03-04.${deleted}.csv`]
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

test("incremental changes are byte-identical to a complete rebuild", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write("providers/base/models/gpt.toml", `[cost]\ninput = 1\noutput = 2\n`);
  fixture.write("providers/alias/models/gpt.toml", `[extends]\nfrom = "base/gpt"\n`);
  fixture.write("providers/direct/models/gpt,mini.toml", `[cost]\ninput = 0.5\noutput = 1\n`);
  fixture.commit("add inherited prices", "2024-04-01T09:00:00Z");
  fixture.write("README.md", "# checkpoint\n");
  const checkpoint = fixture.commit("checkpoint metadata", "2024-04-02T09:00:00Z");

  const baseDirectory = mkdtempSync(path.join(tmpdir(), "model-price-history-base-"));
  const fullDirectory = mkdtempSync(path.join(tmpdir(), "model-price-history-full-"));
  const incrementalDirectory = mkdtempSync(path.join(tmpdir(), "model-price-history-incremental-"));
  const noOpDirectory = mkdtempSync(path.join(tmpdir(), "model-price-history-noop-"));
  for (const directory of [baseDirectory, fullDirectory, incrementalDirectory, noOpDirectory]) {
    t.after(() => rmSync(directory, { force: true, recursive: true }));
  }

  const base = await writeChangesCsv(
    {
      ref: checkpoint,
      repository_path: fixture.repository_path
    },
    baseDirectory
  );
  const noOp = await writeIncrementalChangesCsv(
    {
      base_commit_sha: checkpoint,
      base_csv_path: base.path,
      ref: checkpoint,
      repository_path: fixture.repository_path
    },
    noOpDirectory
  );
  assert.equal(readFileSync(noOp.path, "utf8"), readFileSync(base.path, "utf8"));

  fixture.write("providers/base/models/gpt.toml", `[cost]\ninput = 3\noutput = 4\n`);
  fixture.commit("change inherited prices", "2024-04-03T09:00:00Z");
  fixture.write("providers/direct/models/new.toml", `[cost]\ninput = 5\noutput = 6\n`);
  fixture.commit("add direct price", "2024-04-04T09:00:00Z");
  fixture.remove("providers/alias/models/gpt.toml");
  const head = fixture.commit("remove inherited route", "2024-04-05T09:00:00Z");

  const complete = await writeChangesCsv({ repository_path: fixture.repository_path }, fullDirectory);
  const incremental = await writeIncrementalChangesCsv(
    {
      base_commit_sha: checkpoint,
      base_csv_path: base.path,
      repository_path: fixture.repository_path
    },
    incrementalDirectory
  );

  assert.equal(incremental.commit_sha, head);
  assert.equal(readFileSync(incremental.path, "utf8"), readFileSync(complete.path, "utf8"));
});

test("runs incremental change generation through the CLI", async (t) => {
  const fixture = createFixtureRepository();
  t.after(() => fixture.cleanup());

  fixture.write("providers/openai/models/gpt.toml", `[cost]\ninput = 1\noutput = 2\n`);
  const checkpoint = fixture.commit("add price", "2024-04-10T09:00:00Z");

  const baseDirectory = mkdtempSync(path.join(tmpdir(), "model-price-history-cli-base-"));
  const outputDirectory = mkdtempSync(path.join(tmpdir(), "model-price-history-cli-update-"));
  const fallbackDirectory = mkdtempSync(path.join(tmpdir(), "model-price-history-cli-fallback-"));
  t.after(() => rmSync(baseDirectory, { force: true, recursive: true }));
  t.after(() => rmSync(outputDirectory, { force: true, recursive: true }));
  t.after(() => rmSync(fallbackDirectory, { force: true, recursive: true }));

  const base = await writeChangesCsv(
    {
      ref: checkpoint,
      repository_path: fixture.repository_path
    },
    baseDirectory
  );
  fixture.write("providers/openai/models/gpt.toml", `[cost]\ninput = 2\noutput = 3\n`);
  const head = fixture.commit("change price", "2024-04-11T09:00:00Z");

  const cli = new URL("../dist/cli.js", import.meta.url);
  const output = execFileSync(
    process.execPath,
    [
      fileURLToPath(cli),
      "changes",
      "--repo",
      fixture.repository_path,
      "--out-dir",
      outputDirectory,
      "--base-csv",
      base.path,
      "--base-commit",
      checkpoint
    ],
    { encoding: "utf8" }
  ).trim();

  assert.equal(output, path.join(outputDirectory, `changes.${head}.csv`));
  assert.match(readFileSync(output, "utf8"), /,2,openai,gpt,2,3,/);

  const invalidBase = path.join(baseDirectory, "invalid.csv");
  writeFileSync(invalidBase, "invalid\n", "utf8");
  const fallback = spawnSync(
    process.execPath,
    [
      fileURLToPath(cli),
      "changes",
      "--repo",
      fixture.repository_path,
      "--out-dir",
      fallbackDirectory,
      "--base-csv",
      invalidBase,
      "--base-commit",
      checkpoint
    ],
    { encoding: "utf8" }
  );
  assert.equal(fallback.status, 0, fallback.stderr);
  assert.match(fallback.stderr, /incremental checkpoint rejected/);
  assert.equal(readFileSync(fallback.stdout.trim(), "utf8"), readFileSync(output, "utf8"));
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
