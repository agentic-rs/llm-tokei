export const PRICE_FIELDS = [
  "input",
  "output",
  "reasoning",
  "cache_read",
  "cache_write",
  "input_audio",
  "output_audio"
] as const;

export type PriceField = (typeof PRICE_FIELDS)[number];

export type PriceRecord = {
  provider: string;
  model: string;
  input: string;
  output: string;
  reasoning?: string;
  cache_read?: string;
  cache_write?: string;
  input_audio?: string;
  output_audio?: string;
};

export type PriceProvenance = {
  ts: string;
  commit_sha: string;
};

export type PriceChange =
  | (PriceRecord &
      PriceProvenance & {
        op: "upsert";
        sequence: number;
      })
  | (PriceProvenance & {
      op: "delete";
      sequence: number;
      provider: string;
      model: string;
    });

export type RepositoryOptions = {
  repository_path: string;
  ref?: string;
};

export type DailySnapshotOptions = RepositoryOptions & {
  now?: Date;
};

export type PriceSnapshot = PriceProvenance & {
  prices: PriceRecord[];
};

export type DailyPriceSnapshot = PriceSnapshot & {
  date: string;
};

export type WrittenChanges = {
  commit_sha: string;
  path: string;
};

export type WrittenSnapshot = PriceSnapshot & {
  path: string;
};

export type WrittenDailySnapshot = PriceProvenance & {
  date: string;
  path: string;
};
