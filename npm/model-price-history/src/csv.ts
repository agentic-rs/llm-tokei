import type { PriceChange, PriceRecord, PriceSnapshot } from "./types.js";

const PRICE_COLUMNS = [
  "provider",
  "model",
  "input",
  "output",
  "reasoning",
  "cache_read",
  "cache_write",
  "input_audio",
  "output_audio"
] as const;

const CHANGE_COLUMNS = ["op", "ts", "commit_sha", "sequence", ...PRICE_COLUMNS] as const;
const SNAPSHOT_COLUMNS = ["ts", ...PRICE_COLUMNS] as const;

export function changeCsvHeader(): string {
  return csvLine(CHANGE_COLUMNS);
}

export function snapshotCsvHeader(): string {
  return csvLine(SNAPSHOT_COLUMNS);
}

export function changeCsvLine(change: PriceChange): string {
  return csvLine([
    change.op,
    change.ts,
    change.commit_sha,
    change.sequence,
    change.provider,
    change.model,
    "input" in change ? change.input : undefined,
    "output" in change ? change.output : undefined,
    "reasoning" in change ? change.reasoning : undefined,
    "cache_read" in change ? change.cache_read : undefined,
    "cache_write" in change ? change.cache_write : undefined,
    "input_audio" in change ? change.input_audio : undefined,
    "output_audio" in change ? change.output_audio : undefined
  ]);
}

export function* snapshotCsvLines(snapshot: PriceSnapshot): Generator<string> {
  yield snapshotCsvHeader();
  for (const price of snapshot.prices) yield snapshotCsvLine(snapshot.ts, price);
}

export function snapshotCsvLine(ts: string, price: PriceRecord): string {
  return csvLine([
    ts,
    price.provider,
    price.model,
    price.input,
    price.output,
    price.reasoning,
    price.cache_read,
    price.cache_write,
    price.input_audio,
    price.output_audio
  ]);
}

function csvLine(values: readonly (string | number | undefined)[]): string {
  return `${values.map(csvCell).join(",")}\n`;
}

function csvCell(value: string | number | undefined): string {
  if (value === undefined) return "";
  const text = String(value);
  return /[",\n\r]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}
