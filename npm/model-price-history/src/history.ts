import {
  assertCompleteRepository,
  listChangedPaths,
  listFirstParentCommits,
  listTreeEntries,
  readFilesAtCommit,
  resolveCommit,
  type GitCommit
} from "./git.js";
import {
  endpointIdentity,
  endpointKey,
  isModelsDevDocumentPath,
  ModelsDevResolver,
  parseModelsDevDocument,
  priceFromResolvedDocument,
  type EndpointIdentity,
  type ModelsDevDocument
} from "./models-dev.js";
import type {
  DailyPriceSnapshot,
  DailySnapshotOptions,
  PriceChange,
  PriceProvenance,
  PriceRecord,
  PriceSnapshot,
  RepositoryOptions
} from "./types.js";

type EndpointState = {
  dependencies: Set<string>;
  identity: EndpointIdentity;
  path: string;
  price: PriceRecord | undefined;
};

type ScannerState = {
  documents: Map<string, ModelsDevDocument>;
  endpoints: Map<string, EndpointState>;
  endpoint_paths: Map<string, string>;
  reverse_dependencies: Map<string, Set<string>>;
};

type PendingChange = Omit<PriceChange, "sequence">;

type CommitFrame = {
  changes: PriceChange[];
  commit: GitCommit;
  state: ScannerState;
};

export function resolveHistoryCommit(options: RepositoryOptions): string {
  assertCompleteRepository(options.repository_path);
  return resolveCommit(options.repository_path, options.ref ?? "HEAD");
}

export async function* iteratePriceChanges(options: RepositoryOptions): AsyncGenerator<PriceChange> {
  for (const frame of iterateFrames(options)) {
    for (const change of frame.changes) yield change;
  }
}

export async function* iterateDailyPriceSnapshots(
  options: DailySnapshotOptions
): AsyncGenerator<DailyPriceSnapshot> {
  const today = utcDate(options.now ?? new Date());
  const yesterday = addDays(today, -1);
  let hasSeenPrice = false;
  let previousDay: string | undefined;
  let previousCommit: GitCommit | undefined;
  let previousPrices: PriceRecord[] = [];

  for (const frame of iterateFrames(options)) {
    const day = utcDate(new Date(frame.commit.ts));
    if (previousDay !== undefined && day < previousDay) {
      throw new Error(
        `first-parent committer timestamps decrease from ${previousDay} to ${day}; cannot create UTC daily snapshots`
      );
    }

    if (previousDay !== undefined && day > previousDay && previousCommit) {
      for (const date of datesBetween(previousDay, addDays(day, -1))) {
        if (date > yesterday) break;
        if (!hasSeenPrice) continue;
        yield {
          date,
          commit_sha: previousCommit.commit_sha,
          ts: previousCommit.ts,
          prices: previousPrices
        };
      }
    }

    previousDay = day;
    previousCommit = frame.commit;
    previousPrices = snapshotPrices(frame.state);
    if (previousPrices.length > 0) hasSeenPrice = true;
  }

  if (previousDay && previousCommit) {
    for (const date of datesBetween(previousDay, yesterday)) {
      if (!hasSeenPrice) continue;
      yield {
        date,
        commit_sha: previousCommit.commit_sha,
        ts: previousCommit.ts,
        prices: previousPrices
      };
    }
  }
}

export async function getLatestPriceSnapshot(options: RepositoryOptions): Promise<PriceSnapshot> {
  const commitSha = resolveHistoryCommit(options);
  const commit = listFirstParentCommits(options.repository_path, commitSha).at(-1);
  if (!commit) {
    throw new Error(`models.dev repository ${JSON.stringify(options.repository_path)} has no commits`);
  }
  const { state } = initializeState(options.repository_path, commit);
  return {
    commit_sha: commit.commit_sha,
    ts: commit.ts,
    prices: snapshotPrices(state)
  };
}

function* iterateFrames(options: RepositoryOptions): Generator<CommitFrame> {
  const commitSha = resolveHistoryCommit(options);
  const commits = listFirstParentCommits(options.repository_path, commitSha);
  if (commits.length === 0) {
    throw new Error(`models.dev repository ${JSON.stringify(options.repository_path)} has no commits`);
  }

  let state: ScannerState | undefined;
  let sequence = 0;

  for (const commit of commits) {
    let pending: PendingChange[];
    if (!state) {
      const initialized = initializeState(options.repository_path, commit);
      state = initialized.state;
      pending = initialized.changes;
    } else {
      pending = applyCommit(options.repository_path, state, commit);
    }
    const changes = pending
      .sort(comparePendingChanges)
      .map((change) => ({ ...change, sequence: (sequence += 1) }) as PriceChange);
    yield { changes, commit, state };
  }
}

function initializeState(repositoryPath: string, commit: GitCommit): { changes: PendingChange[]; state: ScannerState } {
  const entries = listTreeEntries(repositoryPath, commit.commit_sha)
    .filter((entry) => isModelsDevDocumentPath(entry.path))
    .sort((left, right) => compareText(left.path, right.path));
  const paths = entries.map((entry) => entry.path);
  const modes = new Map(entries.map((entry) => [entry.path, entry.mode]));
  const contents = readFilesAtCommit(repositoryPath, commit.commit_sha, paths);
  const documents = new Map<string, ModelsDevDocument>();
  for (const path of paths) {
    documents.set(path, parseModelsDevDocument(path, contents.get(path)!, modes.get(path)!));
  }

  const state: ScannerState = {
    documents,
    endpoints: new Map(),
    endpoint_paths: new Map(),
    reverse_dependencies: new Map()
  };
  const resolver = new ModelsDevResolver(state.documents, commit.commit_sha);
  const changes: PendingChange[] = [];

  for (const path of paths) {
    const identity = endpointIdentity(path);
    if (!identity) continue;
    const endpoint = resolveEndpoint(identity, path, resolver, commit);
    state.endpoints.set(endpointKey(identity), endpoint);
    state.endpoint_paths.set(endpointKey(identity), path);
    addReverseDependencies(state.reverse_dependencies, endpointKey(identity), endpoint.dependencies);
    if (endpoint.price) {
      changes.push(upsert(endpoint.price, provenance(commit)));
    }
  }

  return { changes, state };
}

function applyCommit(repositoryPath: string, state: ScannerState, commit: GitCommit): PendingChange[] {
  const changes = listChangedPaths(repositoryPath, commit.parent_commit_sha!, commit.commit_sha).filter(({ path }) =>
    isModelsDevDocumentPath(path)
  );
  if (changes.length === 0) return [];

  const affected = new Set<string>();
  for (const { path } of changes) {
    for (const endpoint of state.reverse_dependencies.get(path) ?? []) affected.add(endpoint);
    const identity = endpointIdentity(path);
    if (identity) affected.add(endpointKey(identity));
  }

  const writePaths = [...new Set(changes
    .filter((change) => change.op === "write")
    .map((change) => change.path)
    .sort(compareText))];
  const modes = new Map(
    listTreeEntries(repositoryPath, commit.commit_sha, writePaths).map((entry) => [entry.path, entry.mode])
  );
  const contents = readFilesAtCommit(repositoryPath, commit.commit_sha, writePaths);
  for (const change of changes) {
    if (change.op === "delete") {
      state.documents.delete(change.path);
    } else {
      const mode = modes.get(change.path);
      if (!mode) {
        throw new Error(`models.dev document ${change.path} is missing in commit ${commit.commit_sha}`);
      }
      state.documents.set(change.path, parseModelsDevDocument(change.path, contents.get(change.path)!, mode));
    }
  }

  for (const change of changes) {
    const identity = endpointIdentity(change.path);
    if (!identity) continue;
    const key = endpointKey(identity);
    if (change.op === "delete") {
      state.endpoint_paths.delete(key);
    } else {
      state.endpoint_paths.set(key, change.path);
    }
  }

  const resolver = new ModelsDevResolver(state.documents, commit.commit_sha);
  const pending: PendingChange[] = [];
  for (const key of [...affected].sort(compareText)) {
    const previous = state.endpoints.get(key);
    const path = state.endpoint_paths.get(key);
    if (!path) {
      if (previous?.price) {
        pending.push(remove(previous.price, provenance(commit)));
      }
      if (previous) {
        removeReverseDependencies(state.reverse_dependencies, key, previous.dependencies);
        state.endpoints.delete(key);
      }
      continue;
    }

    const identity = endpointIdentity(path);
    if (!identity) {
      throw new Error(`models.dev endpoint path ${path} became invalid in commit ${commit.commit_sha}`);
    }
    const endpoint = resolveEndpoint(identity, path, resolver, commit);
    removeReverseDependencies(state.reverse_dependencies, key, previous?.dependencies ?? new Set());
    addReverseDependencies(state.reverse_dependencies, key, endpoint.dependencies);

    if (!samePrice(previous?.price, endpoint.price)) {
      if (endpoint.price) {
        pending.push(upsert(endpoint.price, provenance(commit)));
      } else if (previous?.price) {
        pending.push(remove(previous.price, provenance(commit)));
      }
    }
    state.endpoints.set(key, endpoint);
  }

  return pending;
}

function resolveEndpoint(
  identity: EndpointIdentity,
  path: string,
  resolver: ModelsDevResolver,
  commit: GitCommit
): EndpointState {
  const resolved = resolver.resolve(path);
  return {
    dependencies: new Set([path, ...resolved.dependencies]),
    identity,
    path,
    price: priceFromResolvedDocument(identity, resolved.value, path, commit.commit_sha)
  };
}

function snapshotPrices(state: ScannerState): PriceRecord[] {
  return [...state.endpoints.values()]
    .flatMap((endpoint) => {
      if (!endpoint.price) return [];
      return [{ ...endpoint.price }];
    })
    .sort((left, right) => compareIdentity(left, right));
}

function samePrice(left: PriceRecord | undefined, right: PriceRecord | undefined): boolean {
  if (!left || !right) return left === right;
  return (
    left.provider === right.provider &&
    left.model === right.model &&
    left.input === right.input &&
    left.output === right.output &&
    left.reasoning === right.reasoning &&
    left.cache_read === right.cache_read &&
    left.cache_write === right.cache_write &&
    left.input_audio === right.input_audio &&
    left.output_audio === right.output_audio
  );
}

function provenance(commit: GitCommit): { ts: string; commit_sha: string } {
  return { ts: commit.ts, commit_sha: commit.commit_sha };
}

function upsert(price: PriceRecord, origin: PriceProvenance): PendingChange {
  return { ...price, ...origin, op: "upsert" };
}

function remove(price: PriceRecord, origin: PriceProvenance): PendingChange {
  return { provider: price.provider, model: price.model, ...origin, op: "delete" };
}

function addReverseDependencies(
  reverseDependencies: Map<string, Set<string>>,
  endpoint: string,
  dependencies: ReadonlySet<string>
): void {
  for (const dependency of dependencies) {
    const dependents = reverseDependencies.get(dependency) ?? new Set<string>();
    dependents.add(endpoint);
    reverseDependencies.set(dependency, dependents);
  }
}

function removeReverseDependencies(
  reverseDependencies: Map<string, Set<string>>,
  endpoint: string,
  dependencies: ReadonlySet<string>
): void {
  for (const dependency of dependencies) {
    const dependents = reverseDependencies.get(dependency);
    if (!dependents) continue;
    dependents.delete(endpoint);
    if (dependents.size === 0) reverseDependencies.delete(dependency);
  }
}

function comparePendingChanges(left: PendingChange, right: PendingChange): number {
  return compareIdentity(left, right) || compareText(left.op, right.op);
}

function compareIdentity(
  left: Pick<PriceRecord, "provider" | "model">,
  right: Pick<PriceRecord, "provider" | "model">
): number {
  return compareText(left.provider, right.provider) || compareText(left.model, right.model);
}

function compareText(left: string, right: string): number {
  if (left === right) return 0;
  return left < right ? -1 : 1;
}

function utcDate(value: Date): string {
  return value.toISOString().slice(0, 10);
}

function addDays(date: string, amount: number): string {
  const value = new Date(`${date}T00:00:00.000Z`);
  value.setUTCDate(value.getUTCDate() + amount);
  return utcDate(value);
}

function* datesBetween(start: string, end: string): Generator<string> {
  for (let date = start; date <= end; date = addDays(date, 1)) {
    yield date;
  }
}
