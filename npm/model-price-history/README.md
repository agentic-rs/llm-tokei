# @tokn-ai/model-price-history

Generate reproducible provider-route price history from a local
[models.dev](https://github.com/anomalyco/models.dev) Git repository.

The package keys a price by the serving `provider` and raw `model` ID. It does
not depend on `@tokn-ai/model-catalog`: a canonical model name is not a billing
SKU, and the same underlying model may have a different price on another
provider route.

All price fields are USD per million tokens. Blank CSV cells mean the source did
not specify that price dimension; `0` is preserved as an explicit price.

## Install

```sh
pnpm add @tokn-ai/model-price-history
```

The package does not clone or fetch models.dev. Pass a complete local checkout;
shallow repositories are rejected because they cannot produce a complete price
history.

## CLI

Each command resolves `--ref` to an immutable commit before it starts. Use an
output directory so generated filenames retain that source commit.

```sh
model-price-history changes --repo ../models.dev --ref dev --out-dir ./prices
model-price-history daily --repo ../models.dev --ref dev --out-dir ./prices/daily
model-price-history latest --repo ../models.dev --ref dev --out-dir ./prices
```

### Change history

`changes` writes one append-friendly event stream:

```text
changes.<resolved_commit_sha>.csv
```

```csv
op,ts,commit_sha,sequence,provider,model,input,output,reasoning,cache_read,cache_write,input_audio,output_audio
upsert,2026-07-15T10:12:00.000Z,abc123...,42,openai,gpt-5,1.25,10,10,0.125,,,
```

`ts` is the Git committer time in UTC, never the time the command ran.
`sequence` is the deterministic first-parent replay order, so consumers must
not rely on timestamp order alone. An `upsert` carries the complete scalar
price state; a `delete` has blank price fields and means that price record is
no longer resolved at that commit.

### Daily snapshots

`daily` writes one full CSV per completed UTC day:

```text
2026-07-14.<snapshot_commit_sha>.csv
```

The filename carries the snapshot commit. Each row includes that commit's UTC
committer time as `ts`; `snapshot_date` and `as_of_commit_sha` are not
duplicated into every row:

```csv
ts,provider,model,input,output,reasoning,cache_read,cache_write,input_audio,output_audio
2026-07-14T23:00:00.000Z,openai,gpt-5,1.25,10,10,0.125,,,
```

Daily state carries forward across no-change days. It never creates a snapshot
for the current UTC date, because that day is not complete. A file can be
header-only when every price record was removed.

### Latest snapshot

`latest` writes the resolved tip state directly from its Git tree:

```text
latest.<resolved_commit_sha>.csv
```

Unlike daily output, latest may represent a commit made today.
Its rows use the same `ts` column, sourced from the resolved tip commit.

## Library

```ts
import {
  getLatestPriceSnapshot,
  iteratePriceChanges,
  writeDailySnapshotCsvs
} from "@tokn-ai/model-price-history";

for await (const change of iteratePriceChanges({
  repository_path: "../models.dev",
  ref: "dev"
})) {
  console.log(change.provider, change.model, change.input, change.output);
}

const latest = await getLatestPriceSnapshot({
  repository_path: "../models.dev",
  ref: "dev"
});

await writeDailySnapshotCsvs(
  { repository_path: "../models.dev", ref: "dev" },
  "./prices/daily"
);
```

## Source compatibility

The scanner reads versioned source TOMLs rather than generated API JSON. It
resolves current `base_model` metadata links, historical provider `[extends]`
links, and Git symlinked model files before comparing prices. It normalizes the
historical `inputCached` and `outputCached` names to `cache_read` and
`cache_write`.

Version 1 intentionally emits only the scalar base `[cost]` fields listed in
the CSV headers. Context tiers and `context_over_200k` are ignored for now; a
tier-only change does not create a price event.
