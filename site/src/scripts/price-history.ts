import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";

const PRICE_FIELDS = [
  "input",
  "output",
  "reasoning",
  "cache_read",
  "cache_write",
  "input_audio",
  "output_audio",
] as const;

const DEFAULT_MODEL_COUNT = 5;

type PriceField = (typeof PRICE_FIELDS)[number];
type NumericPriceValues = Partial<Record<PriceField, number | null>>;
type MarkerKind = "change" | "remove" | "start";

type CatalogRoute = {
  event_count: number;
  fields: PriceField[];
  is_active: boolean;
  last_ts: number;
  model: string;
  provider: string;
};

type PriceManifest = {
  csv: {
    bytes: number;
    path: string;
    sha256: string;
  };
  generated_at: string;
  schema_version: number;
  source_commit_sha: string;
  source_ref: string;
  source_repository: string;
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

type LoadedMessage = {
  catalog: CatalogRoute[];
  event_count: number;
  route_count: number;
  type: "loaded";
};

type SeriesMessage = {
  request_id: number;
  routes: RouteSeries[];
  type: "series";
};

type ErrorMessage = {
  message: string;
  type: "error";
};

type WorkerMessage = ErrorMessage | LoadedMessage | SeriesMessage;

type ChangeMarker = {
  commit_sha: string;
  display_value: number;
  kind: MarkerKind;
  model: string;
  model_color: string;
  new_value: number | null;
  plot_ts: number;
  previous_value: number | null;
  ts: number;
};

type RenderedChart = {
  height: number;
  host: HTMLElement;
  plot: uPlot;
};

const FIELD_LABELS: Record<PriceField, string> = {
  input: "Input",
  output: "Output",
  reasoning: "Reasoning",
  cache_read: "Cache read",
  cache_write: "Cache write",
  input_audio: "Audio input",
  output_audio: "Audio output",
};

const app = requiredElement<HTMLElement>(document, "[data-price-app]");

const status = requiredElement<HTMLElement>(app, "[data-status]");
const controls = requiredElement<HTMLElement>(app, "[data-controls]");
const providerSelect = requiredElement<HTMLSelectElement>(
  app,
  "[data-provider]",
);
const modelLegend = requiredElement<HTMLElement>(app, "[data-model-legend]");
const modelFilter = requiredElement<HTMLInputElement>(
  app,
  "[data-model-filter]",
);
const modelOptions = requiredElement<HTMLElement>(app, "[data-model-options]");
const modelSummary = requiredElement<HTMLElement>(app, "[data-model-summary]");
const recentModelsButton = requiredElement<HTMLButtonElement>(
  app,
  "[data-recent-models]",
);
const eventfulModelsButton = requiredElement<HTMLButtonElement>(
  app,
  "[data-eventful-models]",
);
const fieldSet = requiredElement<HTMLFieldSetElement>(app, "[data-fields]");
const chartShell = requiredElement<HTMLElement>(app, "[data-chart-shell]");
const chartsContainer = requiredElement<HTMLElement>(app, "[data-charts]");
const routeLabel = requiredElement<HTMLElement>(app, "[data-route]");
const rangeLabel = requiredElement<HTMLElement>(app, "[data-range]");
const summary = requiredElement<HTMLElement>(app, "[data-summary]");
const emptyState = requiredElement<HTMLElement>(app, "[data-empty]");
const sourceCommit = requiredElement<HTMLAnchorElement>(
  app,
  "[data-source-commit]",
);
const eventCount = requiredElement<HTMLElement>(app, "[data-event-count]");
const routeCount = requiredElement<HTMLElement>(app, "[data-route-count]");
const downloadSize = requiredElement<HTMLElement>(app, "[data-download-size]");
const emptyTitle = requiredElement<HTMLElement>(app, "[data-empty-title]");
const emptyDetail = requiredElement<HTMLElement>(app, "[data-empty-detail]");

let asOfTs = 0;
let catalog: CatalogRoute[] = [];
let eventfulModelsOnly = false;
let latestRequestId = 0;
let manifest: PriceManifest | undefined;
let renderedCharts: RenderedChart[] = [];
let requestId = 0;
let resizeObserver: ResizeObserver | undefined;
let selectedFields: PriceField[] = ["input", "output"];
let selectedModels: string[] = [];
let visibleRoutes: CatalogRoute[] = [];
let worker: Worker | undefined;

for (const field of PRICE_FIELDS) {
  const label = document.createElement("label");
  label.className = "field-toggle";
  label.dataset.fieldLabel = field;
  const input = document.createElement("input");
  input.type = "checkbox";
  input.value = field;
  input.checked = field === "input" || field === "output";
  input.addEventListener("change", requestSeries);
  label.append(input, document.createTextNode(FIELD_LABELS[field]));
  fieldSet.append(label);
}

providerSelect.addEventListener("change", () => {
  selectProvider(providerSelect.value);
  requestSeries();
});
modelFilter.addEventListener("input", renderModelLegend);
eventfulModelsButton.addEventListener("click", () => {
  eventfulModelsOnly = !eventfulModelsOnly;
  eventfulModelsButton.setAttribute("aria-pressed", String(eventfulModelsOnly));
  if (eventfulModelsOnly) {
    const eventfulRoutes = visibleRoutes.filter(
      (route) => route.event_count > 1,
    );
    const eventfulModels = new Set(eventfulRoutes.map((route) => route.model));
    selectedModels = selectedModels.filter((model) =>
      eventfulModels.has(model),
    );
    if (selectedModels.length === 0) selectRecentModels(eventfulRoutes);
    updateAvailableFields();
    requestSeries();
  }
  renderModelLegend();
});
recentModelsButton.addEventListener("click", () => {
  selectRecentModels(filteredVisibleRoutes());
  renderModelLegend();
  updateAvailableFields();
  requestSeries();
});
window.addEventListener("beforeunload", () => {
  destroyCharts();
  worker?.terminate();
});

void loadPriceHistory();

async function loadPriceHistory(): Promise<void> {
  const dataBaseUrl = app.dataset.dataBaseUrl;
  if (!dataBaseUrl) throw new Error("price history data URL is missing");

  try {
    const manifestUrl = new URL(
      "manifest.json",
      new URL(dataBaseUrl, window.location.origin),
    );
    const response = await fetch(manifestUrl);
    if (!response.ok)
      throw new Error(`manifest request failed with HTTP ${response.status}`);
    manifest = (await response.json()) as PriceManifest;
    if (manifest.schema_version !== 1) {
      throw new Error(
        `unsupported price history schema ${manifest.schema_version}`,
      );
    }
    asOfTs = Date.parse(manifest.generated_at);
    if (!Number.isFinite(asOfTs)) {
      throw new Error(
        "price history manifest has an invalid generated_at value",
      );
    }
    setProvenance(manifest);

    worker = new Worker(
      new URL("../workers/price-history.ts", import.meta.url),
      {
        type: "module",
      },
    );
    worker.addEventListener("message", handleWorkerMessage);
    worker.addEventListener("error", (event) => showFailure(event.message));
    worker.postMessage({
      csv_url: new URL(manifest.csv.path, manifestUrl).toString(),
      type: "load",
    });
  } catch (error) {
    showFailure(error instanceof Error ? error.message : String(error));
  }
}

function handleWorkerMessage(event: MessageEvent<WorkerMessage>): void {
  const message = event.data;
  if (message.type === "error") {
    showFailure(message.message);
    return;
  }
  if (message.type === "loaded") {
    catalog = message.catalog;
    eventCount.textContent = formatCount(message.event_count);
    routeCount.textContent = formatCount(message.route_count);
    populateProviders();
    controls.hidden = false;
    modelLegend.hidden = false;
    chartShell.hidden = false;
    emptyState.hidden = true;
    status.textContent = `${formatCount(message.event_count)} changes indexed`;
    requestSeries();
    return;
  }
  if (message.request_id !== latestRequestId) return;
  renderComparison(message.routes, selectedFields);
}

function populateProviders(): void {
  const providers = Array.from(
    new Set(catalog.map((route) => route.provider)),
  ).sort();
  providerSelect.replaceChildren(
    ...providers.map((provider) => new Option(provider, provider)),
  );
  providerSelect.value = providers.includes("openai")
    ? "openai"
    : (providers[0] ?? "");
  selectProvider(providerSelect.value);
}

function selectProvider(provider: string): void {
  visibleRoutes = catalog
    .filter((route) => route.provider === provider)
    .sort(
      (left, right) =>
        Number(right.is_active) - Number(left.is_active) ||
        right.last_ts - left.last_ts ||
        left.model.localeCompare(right.model),
    );
  modelFilter.value = "";
  eventfulModelsOnly = false;
  eventfulModelsButton.setAttribute("aria-pressed", "false");
  selectRecentModels();
  renderModelLegend();
  updateAvailableFields();
}

function selectRecentModels(routes: CatalogRoute[] = visibleRoutes): void {
  selectedModels = routes
    .slice(0, DEFAULT_MODEL_COUNT)
    .map((route) => route.model);
}

function toggleModel(model: string): void {
  const selected = new Set(selectedModels);
  if (selected.has(model)) {
    selected.delete(model);
  } else {
    selected.add(model);
  }
  selectedModels = visibleRoutes
    .filter((route) => selected.has(route.model))
    .map((route) => route.model);
  renderModelLegend();
  updateAvailableFields();
  requestSeries();
}

function renderModelLegend(): void {
  const filteredRoutes = filteredVisibleRoutes();
  recentModelsButton.disabled = filteredRoutes.length === 0;
  recentModelsButton.title =
    filteredRoutes.length === visibleRoutes.length
      ? "Plot the five newest models for this provider"
      : "Plot the five newest models in the current filter";
  const query = modelFilter.value.trim();
  const listing =
    filteredRoutes.length === visibleRoutes.length
      ? `${visibleRoutes.length} total`
      : `${filteredRoutes.length} listed / ${visibleRoutes.length} total`;
  modelSummary.textContent =
    `${selectedModels.length} plotted · ${listing} · ` +
    "active models first, then newest updates";

  if (filteredRoutes.length === 0) {
    const message = document.createElement("span");
    message.className = "model-options__empty";
    message.textContent =
      query && eventfulModelsOnly
        ? "No models match both filters."
        : query
          ? "No model names match this filter."
          : "No models have more than one history event.";
    modelOptions.replaceChildren(message);
    return;
  }

  modelOptions.replaceChildren(
    ...filteredRoutes.map((route) => {
      const selected = selectedModels.includes(route.model);
      const color = modelColor(route.model);
      const button = document.createElement("button");
      button.type = "button";
      button.className = "model-option";
      button.style.setProperty("--model-color", color);
      button.setAttribute("aria-pressed", String(selected));
      button.setAttribute(
        "aria-label",
        `${selected ? "Hide" : "Show"} ${route.model}, ` +
          `${formatCount(route.event_count)} history ` +
          pluralize("event", route.event_count),
      );
      button.title =
        `${route.model} · ${route.is_active ? "active" : "removed"} · ` +
        `${formatCount(route.event_count)} ${pluralize("event", route.event_count)} · ` +
        `updated ${formatDate(route.last_ts)}`;

      const swatch = document.createElement("span");
      swatch.className = "model-option__swatch";
      const name = document.createElement("span");
      name.className = "model-option__name";
      name.textContent = route.model;
      const count = document.createElement("span");
      count.className = "model-option__count";
      count.textContent = formatCount(route.event_count);
      const state = document.createElement("span");
      state.className = "model-option__state";
      state.textContent = route.is_active
        ? formatShortDate(route.last_ts)
        : "removed";
      button.append(swatch, name, count, state);
      button.addEventListener("click", () => toggleModel(route.model));
      return button;
    }),
  );
}

function filteredVisibleRoutes(): CatalogRoute[] {
  const query = modelFilter.value.trim().toLocaleLowerCase("en-US");
  return visibleRoutes.filter(
    (route) =>
      (!eventfulModelsOnly || route.event_count > 1) &&
      (!query || route.model.toLocaleLowerCase("en-US").includes(query)),
  );
}

function updateAvailableFields(): void {
  const selectedRoutes = visibleRoutes.filter((route) =>
    selectedModels.includes(route.model),
  );
  const fieldRoutes =
    selectedRoutes.length > 0 ? selectedRoutes : visibleRoutes;
  const available = new Set(fieldRoutes.flatMap((route) => route.fields));
  for (const label of fieldSet.querySelectorAll<HTMLElement>(
    "[data-field-label]",
  )) {
    const field = label.dataset.fieldLabel as PriceField;
    const input = requiredElement<HTMLInputElement>(label, "input");
    input.disabled = !available.has(field);
    label.dataset.unavailable = String(input.disabled);
    if (input.disabled) input.checked = false;
  }

  if (checkedFields().length > 0) return;
  const fallback = (["input", "output", ...PRICE_FIELDS] as PriceField[]).find(
    (field) => available.has(field),
  );
  const input = fallback
    ? fieldSet.querySelector<HTMLInputElement>(`input[value="${fallback}"]`)
    : undefined;
  if (input) input.checked = true;
}

function requestSeries(): void {
  if (!worker) return;
  if (selectedModels.length === 0) {
    destroyCharts();
    chartsContainer.replaceChildren(createEmptyChartMessage());
    routeLabel.textContent = `${providerSelect.value} / no models shown`;
    rangeLabel.textContent = "";
    summary.textContent =
      "Click a model in the legend to show its price history.";
    status.textContent = "No models shown";
    return;
  }
  selectedFields = checkedFields();
  if (selectedFields.length === 0) {
    status.textContent = "Choose at least one price dimension";
    return;
  }

  latestRequestId = ++requestId;
  routeLabel.textContent = `${providerSelect.value} / ${selectedModels.length} ${pluralize("model", selectedModels.length)}`;
  status.textContent = "Rendering comparison…";
  worker.postMessage({
    fields: selectedFields,
    models: selectedModels,
    provider: providerSelect.value,
    request_id: latestRequestId,
    type: "series",
  });
}

function checkedFields(): PriceField[] {
  return Array.from(
    fieldSet.querySelectorAll<HTMLInputElement>("input:checked"),
  ).map((input) => input.value as PriceField);
}

function renderComparison(routes: RouteSeries[], fields: PriceField[]): void {
  destroyCharts();

  const allChanges = routes.flatMap((route) => route.changes);
  const activeRouteCount = routes.filter((route) => route.is_active).length;
  const eventTotal = routes.reduce(
    (total, route) => total + route.event_count,
    0,
  );
  if (allChanges.length === 0) {
    chartsContainer.replaceChildren(createEmptyChartMessage());
    rangeLabel.textContent = "";
    summary.textContent =
      "No scalar price values are available for this selection.";
    status.textContent = "No recorded price changes for this selection";
    return;
  }

  const firstTs = Math.min(...allChanges.map((change) => change.ts));
  const lastPlotTs = Math.max(
    asOfTs,
    ...allChanges.map((change) => change.plot_ts),
  );
  const segmentEndTimestamps = allChanges
    .filter((change) =>
      change.changed_fields.some(
        (field) =>
          change.previous_values[field] != null && change.values[field] == null,
      ),
    )
    .map((change) => change.plot_ts - 1);
  const xValues = Array.from(
    new Set([
      ...allChanges.map((change) => change.plot_ts),
      ...segmentEndTimestamps,
      lastPlotTs,
    ]),
  ).sort((left, right) => left - right);

  for (const [index, field] of fields.entries()) {
    renderMetricChart(field, routes, xValues, index === fields.length - 1);
  }
  observeChartSizes();

  rangeLabel.textContent = `${formatDate(firstTs)} – ${formatDate(lastPlotTs)} · dataset as of`;
  summary.textContent =
    `${formatCount(eventTotal)} source ${pluralize("event", eventTotal)} across ` +
    `${routes.length} ${pluralize("model", routes.length)}. ${activeRouteCount} active ` +
    `${pluralize("route", activeRouteCount)} extend to ${formatDate(lastPlotTs)}.`;
  status.textContent = `${formatCount(eventTotal)} route ${pluralize("change", eventTotal)} compared`;
}

function renderMetricChart(
  field: PriceField,
  routes: RouteSeries[],
  xValues: number[],
  isLast: boolean,
): void {
  const panel = document.createElement("article");
  panel.className = "metric-chart";
  const heading = document.createElement("div");
  heading.className = "metric-chart__heading";
  const titleGroup = document.createElement("div");
  const title = document.createElement("h3");
  title.textContent = FIELD_LABELS[field];
  const unit = document.createElement("span");
  unit.textContent = "USD / 1M tokens";
  titleGroup.append(title, unit);
  const detail = document.createElement("p");
  detail.className = "metric-chart__detail";
  setDefaultMarkerDetail(detail);
  heading.append(titleGroup, detail);

  const host = document.createElement("div");
  host.className = "metric-chart__plot";
  host.setAttribute("role", "group");
  host.setAttribute(
    "aria-label",
    `${FIELD_LABELS[field]} historical price comparison for ${routes
      .map((route) => route.model)
      .join(", ")}`,
  );
  panel.append(heading, host);
  chartsContainer.append(panel);

  const markers = buildMarkers(routes, field);
  const data = buildAlignedData(routes, field, xValues);
  const height = isLast ? 260 : 240;
  const plot = new uPlot(
    createPlotOptions(routes, markers, detail, host.clientWidth, height),
    data,
    host,
  );
  renderedCharts.push({ height, host, plot });
}

function createPlotOptions(
  routes: RouteSeries[],
  markers: ChangeMarker[],
  detail: HTMLElement,
  width: number,
  height: number,
): uPlot.Options {
  const markerElements: Array<{
    element: HTMLButtonElement;
    marker: ChangeMarker;
  }> = [];

  const positionMarkers = (plot: uPlot): void => {
    for (const { element, marker } of markerElements) {
      const left = plot.valToPos(marker.plot_ts, "x");
      const top = plot.valToPos(marker.display_value, "y");
      const visible =
        left >= 0 &&
        left <= plot.over.clientWidth &&
        top >= 0 &&
        top <= plot.over.clientHeight;
      element.hidden = !visible;
      element.style.left = `${left}px`;
      element.style.top = `${top}px`;
    }
  };

  const addMarkers = (plot: uPlot): void => {
    for (const marker of markers) {
      const element = createMarkerElement(marker, detail);
      plot.over.append(element);
      markerElements.push({ element, marker });
    }
    positionMarkers(plot);
  };

  return {
    axes: [
      {
        font: "11px SFMono-Regular, Consolas, monospace",
        grid: { stroke: "#ded5c6", width: 1 },
        size: 42,
        space: 90,
        stroke: "#74786f",
        ticks: { stroke: "#b9ad9b", width: 1 },
        values: (_plot, splits) => splits.map((value) => formatAxisDate(value)),
      },
      {
        font: "11px SFMono-Regular, Consolas, monospace",
        grid: { stroke: "#ded5c6", width: 1 },
        size: 68,
        space: 42,
        stroke: "#74786f",
        ticks: { stroke: "#b9ad9b", width: 1 },
        values: (_plot, splits) =>
          splits.map((value) => `$${formatPrice(value)}`),
      },
    ],
    cursor: {
      drag: { setScale: false, x: false, y: false },
      points: { show: false },
      sync: {
        key: "price-history-comparison",
        scales: ["x", null],
        setSeries: false,
      },
      y: false,
    },
    height,
    hooks: {
      draw: [positionMarkers],
      ready: [addMarkers],
    },
    legend: { show: false },
    ms: 1,
    padding: [12, 16, 0, 0],
    scales: {
      x: { time: true },
      y: {
        range: (_plot, _minimum, maximum) => {
          const safeMaximum =
            Number.isFinite(maximum) && maximum > 0 ? maximum : 1;
          return [0, niceMaximum(safeMaximum)];
        },
      },
    },
    series: [
      {},
      ...routes.map((route) => ({
        label: route.model,
        paths: uPlot.paths.stepped?.({
          align: 1,
          alignGaps: 1,
          ascDesc: true,
        }),
        points: { show: false },
        spanGaps: false,
        stroke: modelColor(route.model),
        width: 2.25,
      })),
    ],
    width: Math.max(1, width),
  };
}

function buildAlignedData(
  routes: RouteSeries[],
  field: PriceField,
  xValues: number[],
): uPlot.AlignedData {
  const series = routes.map((route) => {
    const changes = new Map(
      route.changes.map((change) => [
        change.plot_ts,
        change.values[field] ?? null,
      ]),
    );
    let currentValue: number | null = null;
    return xValues.map((ts) => {
      if (changes.has(ts)) currentValue = changes.get(ts) ?? null;
      return currentValue;
    });
  });
  return [xValues, ...series] as uPlot.AlignedData;
}

function buildMarkers(
  routes: RouteSeries[],
  field: PriceField,
): ChangeMarker[] {
  return routes.flatMap((route) =>
    route.changes.flatMap((change) => {
      if (!change.changed_fields.includes(field)) return [];
      const previousValue = change.previous_values[field] ?? null;
      const newValue = change.values[field] ?? null;
      if (previousValue === null && newValue === null) return [];
      const kind: MarkerKind =
        newValue === null
          ? "remove"
          : previousValue === null
            ? "start"
            : "change";
      return [
        {
          commit_sha: change.commit_sha,
          display_value: newValue ?? previousValue ?? 0,
          kind,
          model: route.model,
          model_color: modelColor(route.model),
          new_value: newValue,
          plot_ts: change.plot_ts,
          previous_value: previousValue,
          ts: change.ts,
        },
      ];
    }),
  );
}

function createMarkerElement(
  marker: ChangeMarker,
  detail: HTMLElement,
): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.className = `change-marker change-marker--${marker.kind}`;
  element.style.setProperty("--marker-color", marker.model_color);
  element.setAttribute("aria-label", markerAriaLabel(marker));
  if (marker.kind === "remove") element.textContent = "×";
  element.addEventListener("mouseenter", () => setMarkerDetail(detail, marker));
  element.addEventListener("focus", () => setMarkerDetail(detail, marker));
  element.addEventListener("mouseleave", () => setDefaultMarkerDetail(detail));
  element.addEventListener("blur", () => setDefaultMarkerDetail(detail));
  return element;
}

function setMarkerDetail(detail: HTMLElement, marker: ChangeMarker): void {
  const valueText =
    marker.kind === "start"
      ? `became available at $${formatPrice(marker.new_value ?? 0)}`
      : marker.kind === "remove"
        ? `was removed from $${formatPrice(marker.previous_value ?? 0)}`
        : `changed from $${formatPrice(marker.previous_value ?? 0)} to ` +
          `$${formatPrice(marker.new_value ?? 0)}`;
  const sourceLink = document.createElement("a");
  sourceLink.href = manifest
    ? `${manifest.source_repository}/commit/${marker.commit_sha}`
    : "#";
  sourceLink.textContent = marker.commit_sha.slice(0, 8);
  sourceLink.target = "_blank";
  sourceLink.rel = "noreferrer";
  detail.replaceChildren(
    document.createTextNode(
      `${marker.model} ${valueText} · ${formatDateTime(marker.ts)} · `,
    ),
    sourceLink,
  );
}

function setDefaultMarkerDetail(detail: HTMLElement): void {
  detail.textContent = "Hover or focus a marker to inspect its source change.";
}

function markerAriaLabel(marker: ChangeMarker): string {
  if (marker.kind === "start") {
    return (
      `${marker.model} became available at $${formatPrice(marker.new_value ?? 0)} ` +
      `on ${formatDateTime(marker.ts)}`
    );
  }
  if (marker.kind === "remove") {
    return (
      `${marker.model} was removed from $${formatPrice(marker.previous_value ?? 0)} ` +
      `on ${formatDateTime(marker.ts)}`
    );
  }
  return (
    `${marker.model} changed from $${formatPrice(marker.previous_value ?? 0)} ` +
    `to $${formatPrice(marker.new_value ?? 0)} on ${formatDateTime(marker.ts)}`
  );
}

function createEmptyChartMessage(): HTMLElement {
  const message = document.createElement("p");
  message.className = "chart-empty";
  message.textContent = "No price values to plot";
  return message;
}

function observeChartSizes(): void {
  resizeObserver = new ResizeObserver(() => {
    for (const chart of renderedCharts) {
      const width = Math.max(1, chart.host.clientWidth);
      if (Math.abs(chart.plot.width - width) > 1) {
        chart.plot.setSize({ height: chart.height, width });
      }
    }
  });
  for (const chart of renderedCharts) resizeObserver.observe(chart.host);
}

function destroyCharts(): void {
  resizeObserver?.disconnect();
  resizeObserver = undefined;
  for (const chart of renderedCharts) chart.plot.destroy();
  renderedCharts = [];
  chartsContainer.replaceChildren();
}

function setProvenance(priceManifest: PriceManifest): void {
  sourceCommit.textContent = priceManifest.source_commit_sha.slice(0, 12);
  sourceCommit.href = `${priceManifest.source_repository}/commit/${priceManifest.source_commit_sha}`;
  downloadSize.textContent = formatBytes(priceManifest.csv.bytes);
}

function showFailure(message: string): void {
  destroyCharts();
  const datasetMissing = /request failed with HTTP 404\b/.test(message);
  status.textContent = datasetMissing
    ? "Dataset unavailable"
    : "Price history failed to load";
  emptyTitle.textContent = datasetMissing
    ? "Price history is not available in this build."
    : "Price history could not be loaded.";
  emptyDetail.textContent = datasetMissing
    ? "The scheduled Pages build generates it without committing the CSV to Git."
    : "Reload the page to try again. The browser console contains the technical error.";
  controls.hidden = true;
  modelLegend.hidden = true;
  chartShell.hidden = true;
  emptyState.hidden = false;
  console.warn(`Price history: ${message}`);
}

function requiredElement<T extends Element>(
  root: ParentNode,
  selector: string,
): T {
  const element = root.querySelector<T>(selector);
  if (!element) throw new Error(`missing element ${selector}`);
  return element;
}

function modelColor(model: string): string {
  let hash = 2166136261;
  for (let index = 0; index < model.length; index += 1) {
    hash ^= model.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return `hsl(${(hash >>> 0) % 360} 58% 36%)`;
}

function niceMaximum(value: number): number {
  if (value <= 0) return 1;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const normalized = value / magnitude;
  const nice =
    normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10;
  return nice * magnitude;
}

function formatPrice(value: number): string {
  return value.toLocaleString("en-US", {
    maximumFractionDigits: value < 1 ? 4 : 2,
  });
}

function formatAxisDate(ts: number): string {
  return new Intl.DateTimeFormat("en-US", {
    month: "short",
    timeZone: "UTC",
    year: "2-digit",
  }).format(ts);
}

function formatDate(ts: number): string {
  return new Intl.DateTimeFormat("en-US", {
    day: "numeric",
    month: "short",
    timeZone: "UTC",
    year: "numeric",
  }).format(ts);
}

function formatShortDate(ts: number): string {
  return new Intl.DateTimeFormat("en-US", {
    day: "numeric",
    month: "short",
    timeZone: "UTC",
  }).format(ts);
}

function formatDateTime(ts: number): string {
  return new Intl.DateTimeFormat("en-US", {
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    month: "short",
    timeZone: "UTC",
    timeZoneName: "short",
    year: "numeric",
  }).format(ts);
}

function formatCount(value: number): string {
  return value.toLocaleString("en-US");
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KiB`;
  return `${(value / 1024 ** 2).toFixed(1)} MiB`;
}

function pluralize(word: string, count: number): string {
  return count === 1 ? word : `${word}s`;
}
