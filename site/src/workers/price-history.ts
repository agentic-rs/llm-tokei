const PRICE_FIELDS = [
  "input",
  "output",
  "reasoning",
  "cache_read",
  "cache_write",
  "input_audio",
  "output_audio",
] as const;

type PriceField = (typeof PRICE_FIELDS)[number];

type PriceValues = Partial<Record<PriceField, string>>;
type NumericPriceValues = Partial<Record<PriceField, number | null>>;

type RouteChange = {
  commit_sha: string;
  op: "delete" | "upsert";
  ts: number;
  values: PriceValues;
};

type RouteHistory = {
  changes: RouteChange[];
  fields: Set<PriceField>;
  last_ts: number;
  model: string;
  provider: string;
};

type SeriesChange = {
  changed_fields: PriceField[];
  commit_sha: string;
  op: "delete" | "upsert";
  plot_ts: number;
  previous_values: NumericPriceValues;
  ts: number;
  values: NumericPriceValues;
};

type RouteSeries = {
  changes: SeriesChange[];
  event_count: number;
  is_active: boolean;
  model: string;
  provider: string;
};

type LoadMessage = {
  type: "load";
  csv_url: string;
};

type SeriesMessage = {
  type: "series";
  fields: PriceField[];
  models: string[];
  provider: string;
  request_id: number;
};

type IncomingMessage = LoadMessage | SeriesMessage;

const routes = new Map<string, RouteHistory>();

self.addEventListener("message", (event: MessageEvent<IncomingMessage>) => {
  void handleMessage(event.data).catch((error: unknown) => {
    self.postMessage({
      type: "error",
      message: error instanceof Error ? error.message : String(error),
    });
  });
});

async function handleMessage(message: IncomingMessage): Promise<void> {
  if (message.type === "load") {
    await loadCsv(message.csv_url);
    return;
  }
  if (message.type === "series") {
    sendSeries(message);
  }
}

async function loadCsv(csvUrl: string): Promise<void> {
  const response = await fetch(csvUrl);
  if (!response.ok) {
    throw new Error(
      `price history request failed with HTTP ${response.status}`,
    );
  }

  const csv = await response.text();
  const rows = parseCsv(csv);
  const header: string[] | undefined = rows.next().value;
  if (!header) throw new Error("price history CSV is empty");

  const columns = new Map<string, number>(
    header.map((name: string, index: number) => [name, index]),
  );
  for (const required of [
    "op",
    "ts",
    "commit_sha",
    "provider",
    "model",
    "input",
    "output",
  ]) {
    if (!columns.has(required))
      throw new Error(`price history CSV is missing ${required}`);
  }

  routes.clear();
  let eventCount = 0;
  for (let next = rows.next(); !next.done; next = rows.next()) {
    const row = next.value;
    if (row.length === 1 && row[0] === "") continue;
    const provider = cell(row, columns, "provider");
    const model = cell(row, columns, "model");
    const op = cell(row, columns, "op");
    const ts = Date.parse(cell(row, columns, "ts"));
    const commitSha = cell(row, columns, "commit_sha");
    if (
      !provider ||
      !model ||
      !commitSha ||
      (op !== "upsert" && op !== "delete") ||
      !Number.isFinite(ts)
    ) {
      throw new Error(`invalid price history row ${eventCount + 2}`);
    }

    const key = routeKey(provider, model);
    const route = routes.get(key) ?? {
      changes: [],
      fields: new Set<PriceField>(),
      last_ts: ts,
      model,
      provider,
    };
    const values: PriceValues = {};
    if (op === "upsert") {
      for (const field of PRICE_FIELDS) {
        const value = cell(row, columns, field);
        if (value !== "") {
          values[field] = value;
          route.fields.add(field);
        }
      }
    }
    route.changes.push({ commit_sha: commitSha, op, ts, values });
    route.last_ts = Math.max(route.last_ts, ts);
    routes.set(key, route);
    eventCount += 1;
  }

  const catalog = Array.from(routes.values())
    .map((route) => ({
      provider: route.provider,
      model: route.model,
      fields: PRICE_FIELDS.filter((field) => route.fields.has(field)),
      last_ts: route.last_ts,
    }))
    .sort(
      (left, right) =>
        left.provider.localeCompare(right.provider) ||
        left.model.localeCompare(right.model),
    );

  self.postMessage({
    type: "loaded",
    catalog,
    event_count: eventCount,
    route_count: catalog.length,
  });
}

function sendSeries(message: SeriesMessage): void {
  const selectedModels = new Set(message.models);
  const selectedRoutes = Array.from(routes.values())
    .filter(
      (route) =>
        route.provider === message.provider && selectedModels.has(route.model),
    )
    .sort(
      (left, right) =>
        message.models.indexOf(left.model) -
        message.models.indexOf(right.model),
    )
    .map((route) => buildRouteSeries(route, message.fields));

  self.postMessage({
    type: "series",
    request_id: message.request_id,
    routes: selectedRoutes,
  });
}

function buildRouteSeries(
  route: RouteHistory,
  fields: PriceField[],
): RouteSeries {
  let previousPlotTs = Number.NEGATIVE_INFINITY;
  const state: NumericPriceValues = {};
  const changes: SeriesChange[] = [];

  for (const change of route.changes) {
    const plotTs = Math.max(change.ts, previousPlotTs + 1);
    previousPlotTs = plotTs;
    const previousValues: NumericPriceValues = {};
    const values: NumericPriceValues = {};
    const changedFields: PriceField[] = [];

    for (const field of fields) {
      const previousValue = state[field] ?? null;
      const value =
        change.op === "delete" ? null : parsePrice(change.values[field]);
      previousValues[field] = previousValue;
      values[field] = value;
      state[field] = value;
      if (previousValue !== value) changedFields.push(field);
    }

    if (changedFields.length > 0) {
      changes.push({
        changed_fields: changedFields,
        commit_sha: change.commit_sha,
        op: change.op,
        plot_ts: plotTs,
        previous_values: previousValues,
        ts: change.ts,
        values,
      });
    }
  }

  return {
    changes,
    event_count: route.changes.length,
    is_active: route.changes.at(-1)?.op !== "delete",
    model: route.model,
    provider: route.provider,
  };
}

function parsePrice(value: string | undefined): number | null {
  if (value === undefined || value === "") return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function* parseCsv(csv: string): Generator<string[]> {
  let cell = "";
  let quoted = false;
  let row: string[] = [];

  for (let index = 0; index < csv.length; index += 1) {
    const char = csv[index];
    if (quoted) {
      if (char === '"' && csv[index + 1] === '"') {
        cell += '"';
        index += 1;
      } else if (char === '"') {
        quoted = false;
      } else {
        cell += char;
      }
    } else if (char === '"' && cell.length === 0) {
      quoted = true;
    } else if (char === ",") {
      row.push(cell);
      cell = "";
    } else if (char === "\n") {
      row.push(cell.endsWith("\r") ? cell.slice(0, -1) : cell);
      yield row;
      cell = "";
      row = [];
    } else {
      cell += char;
    }
  }

  if (quoted) throw new Error("price history CSV ends inside a quoted cell");
  if (cell !== "" || row.length > 0) {
    row.push(cell.endsWith("\r") ? cell.slice(0, -1) : cell);
    yield row;
  }
}

function cell(
  row: string[],
  columns: Map<string, number>,
  name: string,
): string {
  const index = columns.get(name);
  return index === undefined ? "" : (row[index] ?? "");
}

function routeKey(provider: string, model: string): string {
  return `${provider}\u0000${model}`;
}
