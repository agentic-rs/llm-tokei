const PAGE_SIZE = 100;

export {};

type CsvArtifact = {
  bytes: number;
  latest_path: string;
  path: string;
  sha256: string;
};

type ModelsManifest = {
  catalog_revision: string;
  families: CsvArtifact;
  generated_at: string;
  prices: CsvArtifact;
  schema_version: number;
  source_commit_sha: string;
  source_repository: string;
};

type FamilyRoute = {
  canonical_name: string;
  family: string;
  model: string;
  provider: string;
  release_date: string;
};

const app = requiredElement<HTMLElement>(document, "[data-family-app]");
const status = requiredElement<HTMLElement>(app, "[data-status]");
const controls = requiredElement<HTMLElement>(app, "[data-controls]");
const queryInput = requiredElement<HTMLInputElement>(app, "[data-query]");
const providerSelect = requiredElement<HTMLSelectElement>(
  app,
  "[data-provider]",
);
const familySelect = requiredElement<HTMLSelectElement>(app, "[data-family]");
const resolvedInput = requiredElement<HTMLInputElement>(app, "[data-resolved]");
const tableShell = requiredElement<HTMLElement>(app, "[data-table-shell]");
const tableRows = requiredElement<HTMLTableSectionElement>(app, "[data-rows]");
const tableFooter = requiredElement<HTMLElement>(app, "[data-table-footer]");
const summary = requiredElement<HTMLElement>(app, "[data-summary]");
const previousButton = requiredElement<HTMLButtonElement>(
  app,
  "[data-previous]",
);
const nextButton = requiredElement<HTMLButtonElement>(app, "[data-next]");
const pageLabel = requiredElement<HTMLElement>(app, "[data-page]");
const emptyState = requiredElement<HTMLElement>(app, "[data-empty]");
const emptyDetail = requiredElement<HTMLElement>(app, "[data-empty-detail]");
const sourceCommit = requiredElement<HTMLAnchorElement>(
  app,
  "[data-source-commit]",
);
const routeCount = requiredElement<HTMLElement>(app, "[data-route-count]");
const generated = requiredElement<HTMLElement>(app, "[data-generated]");
const download = requiredElement<HTMLAnchorElement>(app, "[data-download]");
const downloadSize = requiredElement<HTMLElement>(
  app,
  "[data-download-size]",
);

let currentPage = 1;
let routes: FamilyRoute[] = [];

for (const input of [queryInput, providerSelect, familySelect, resolvedInput]) {
  input.addEventListener("input", () => {
    currentPage = 1;
    render();
  });
}
previousButton.addEventListener("click", () => {
  currentPage -= 1;
  render();
  tableShell.scrollTo({ top: 0 });
});
nextButton.addEventListener("click", () => {
  currentPage += 1;
  render();
  tableShell.scrollTo({ top: 0 });
});

void loadFamilies();

async function loadFamilies(): Promise<void> {
  const dataBaseUrl = app.dataset.dataBaseUrl;
  if (!dataBaseUrl) throw new Error("model family data URL is missing");

  try {
    const manifestUrl = new URL(
      "manifest.json",
      new URL(dataBaseUrl, window.location.origin),
    );
    const manifestResponse = await fetch(manifestUrl);
    if (!manifestResponse.ok) {
      throw new Error(
        `manifest request failed with HTTP ${manifestResponse.status}`,
      );
    }
    const manifest = (await manifestResponse.json()) as ModelsManifest;
    if (manifest.schema_version !== 2) {
      throw new Error(
        `unsupported model data schema ${manifest.schema_version}`,
      );
    }

    const csvUrl = new URL(manifest.families.path, manifestUrl);
    const csvResponse = await fetch(csvUrl);
    if (!csvResponse.ok) {
      throw new Error(
        `model families request failed with HTTP ${csvResponse.status}`,
      );
    }
    routes = parseFamilyRoutes(await csvResponse.text());
    setFilters();
    setProvenance(manifest, csvUrl);
    controls.hidden = false;
    tableShell.hidden = false;
    tableFooter.hidden = false;
    emptyState.hidden = true;
    status.textContent = `${formatCount(routes.length)} routes indexed`;
    render();
  } catch (error) {
    showFailure(error instanceof Error ? error.message : String(error));
  }
}

function parseFamilyRoutes(csv: string): FamilyRoute[] {
  const rows = parseCsv(csv);
  const header: string[] | undefined = rows.next().value;
  if (!header) throw new Error("model families CSV is empty");

  const columns = new Map<string, number>(
    header.map((name: string, index: number) => [name, index]),
  );
  for (const required of [
    "provider",
    "model",
    "canonical_name",
    "family",
    "release_date",
  ]) {
    if (!columns.has(required)) {
      throw new Error(`model families CSV is missing ${required}`);
    }
  }

  const parsed: FamilyRoute[] = [];
  for (let next = rows.next(); !next.done; next = rows.next()) {
    const row = next.value;
    if (row.length === 1 && row[0] === "") continue;
    parsed.push({
      canonical_name: cell(row, columns, "canonical_name"),
      family: cell(row, columns, "family"),
      model: cell(row, columns, "model"),
      provider: cell(row, columns, "provider"),
      release_date: cell(row, columns, "release_date"),
    });
  }
  return parsed;
}

function setFilters(): void {
  const providers = uniqueSorted(
    routes.map((route) => route.provider).filter(Boolean),
  );
  providerSelect.append(
    ...providers.map((provider) => new Option(provider, provider)),
  );

  const families = uniqueSorted(
    routes.map((route) => route.family).filter(Boolean),
  );
  familySelect.append(
    ...families.map((family) => new Option(family, family)),
  );
}

function render(): void {
  const query = queryInput.value.trim().toLocaleLowerCase();
  const provider = providerSelect.value;
  const family = familySelect.value;
  const filtered = routes.filter((route) => {
    if (provider && route.provider !== provider) return false;
    if (family && route.family !== family) return false;
    if (resolvedInput.checked && (!route.canonical_name || !route.family)) {
      return false;
    }
    if (!query) return true;
    return [
      route.provider,
      route.model,
      route.canonical_name,
      route.family,
      route.release_date,
    ].some((value) => value.toLocaleLowerCase().includes(query));
  });

  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  currentPage = Math.min(Math.max(1, currentPage), pageCount);
  const start = (currentPage - 1) * PAGE_SIZE;
  const pageRows = filtered.slice(start, start + PAGE_SIZE);
  tableRows.replaceChildren(...pageRows.map(renderRow));

  const first = filtered.length === 0 ? 0 : start + 1;
  const last = Math.min(start + PAGE_SIZE, filtered.length);
  summary.textContent =
    filtered.length === routes.length
      ? `Showing ${formatCount(first)}–${formatCount(last)} of ${formatCount(routes.length)} routes`
      : `Showing ${formatCount(first)}–${formatCount(last)} of ${formatCount(filtered.length)} matching routes`;
  pageLabel.textContent = `Page ${formatCount(currentPage)} of ${formatCount(pageCount)}`;
  previousButton.disabled = currentPage === 1;
  nextButton.disabled = currentPage === pageCount;
}

function renderRow(route: FamilyRoute): HTMLTableRowElement {
  const row = document.createElement("tr");
  row.append(textCell(route.provider));

  const modelCell = document.createElement("td");
  const link = document.createElement("a");
  const pricesUrl = new URL(
    "../prices/",
    new URL(window.location.href),
  );
  pricesUrl.searchParams.set("provider", route.provider);
  pricesUrl.searchParams.set("model", route.model);
  link.href = pricesUrl.toString();
  link.textContent = route.model;
  link.title = "Open this route in price history";
  modelCell.append(link);
  row.append(modelCell);

  row.append(
    textCell(route.canonical_name),
    textCell(route.family),
    textCell(route.release_date),
  );
  return row;
}

function textCell(value: string): HTMLTableCellElement {
  const element = document.createElement("td");
  element.textContent = value || "—";
  if (!value) element.className = "muted-cell";
  return element;
}

function setProvenance(
  manifest: ModelsManifest,
  csvUrl: URL,
): void {
  sourceCommit.textContent = manifest.source_commit_sha.slice(0, 12);
  sourceCommit.href =
    `${manifest.source_repository}/commit/${manifest.source_commit_sha}`;
  routeCount.textContent = formatCount(routes.length);
  const generatedDate = new Date(manifest.generated_at);
  generated.textContent = Number.isNaN(generatedDate.valueOf())
    ? manifest.generated_at
    : new Intl.DateTimeFormat(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(generatedDate);
  download.href = csvUrl.toString();
  downloadSize.textContent = formatBytes(manifest.families.bytes);
}

function showFailure(message: string): void {
  status.textContent = /HTTP 404\b/.test(message)
    ? "Dataset unavailable"
    : "Model families failed to load";
  emptyDetail.textContent = /HTTP 404\b/.test(message)
    ? "This build does not contain the generated model dataset."
    : "Reload the page to try again. The browser console contains the technical error.";
  controls.hidden = true;
  tableShell.hidden = true;
  tableFooter.hidden = true;
  emptyState.hidden = false;
  console.warn(`Model families: ${message}`);
}

function* parseCsv(csv: string): Generator<string[]> {
  let field = "";
  let inQuotes = false;
  let row: string[] = [];

  for (let index = 0; index < csv.length; index += 1) {
    const character = csv[index];
    if (inQuotes) {
      if (character === '"' && csv[index + 1] === '"') {
        field += '"';
        index += 1;
      } else if (character === '"') {
        inQuotes = false;
      } else {
        field += character;
      }
      continue;
    }

    if (character === '"') {
      inQuotes = true;
    } else if (character === ",") {
      row.push(field);
      field = "";
    } else if (character === "\n") {
      row.push(field.endsWith("\r") ? field.slice(0, -1) : field);
      yield row;
      row = [];
      field = "";
    } else {
      field += character;
    }
  }

  if (inQuotes) throw new Error("model families CSV has an unterminated quote");
  if (field || row.length > 0) {
    row.push(field.endsWith("\r") ? field.slice(0, -1) : field);
    yield row;
  }
}

function cell(
  row: string[],
  columns: Map<string, number>,
  name: string,
): string {
  return row[columns.get(name) ?? -1] ?? "";
}

function uniqueSorted(values: string[]): string[] {
  return Array.from(new Set(values)).sort((left, right) =>
    left.localeCompare(right),
  );
}

function formatCount(value: number): string {
  return new Intl.NumberFormat().format(value);
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}

function requiredElement<T extends Element>(
  root: ParentNode,
  selector: string,
): T {
  const element = root.querySelector<T>(selector);
  if (!element) throw new Error(`missing element ${selector}`);
  return element;
}
