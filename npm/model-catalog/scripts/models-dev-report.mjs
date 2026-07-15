import { pathToFileURL } from "node:url";

import { listModelsDevSources, resolveModel } from "../dist/index.js";

const MODELS_DEV_URL = "https://models.dev/api.json";
const confidenceLevels = ["exact", "normalized", "heuristic", "unknown"];
export function createModelsDevReport(api, options = {}) {
  const requestedProviders = new Set((options.providers ?? []).map((provider) => provider.trim().toLowerCase()));
  const officialSources = options.official ? sourcesByProvider(listModelsDevSources()) : undefined;
  const filterProviders = requestedProviders.size > 0 || officialSources !== undefined;
  const providers = [];

  for (const [provider, providerData] of Object.entries(api).sort(([left], [right]) => left.localeCompare(right))) {
    if (filterProviders && requestedProviders.size > 0 && !requestedProviders.has(provider)) continue;
    const sourceFilters = officialSources?.get(provider);
    if (officialSources && !sourceFilters) continue;
    const models = Object.entries(providerData?.models ?? {})
      .filter(([sourceModel]) => !sourceFilters || sourceFilters.some((source) => sourceMatches(source, sourceModel)))
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([sourceModel, sourceData]) => ({
        source_model: sourceModel,
        display_name: typeof sourceData?.name === "string" ? sourceData.name : null,
        resolution: resolveModel({ provider, model: sourceModel })
      }));
    providers.push({
      provider,
      counts: countByConfidence(models),
      models
    });
  }

  const counts = emptyCounts();
  for (const provider of providers) {
    for (const confidence of confidenceLevels) {
      counts[confidence] += provider.counts[confidence];
    }
  }

  return {
    provider_count: providers.length,
    model_count: providers.reduce((total, provider) => total + provider.models.length, 0),
    counts,
    providers
  };
}

function sourcesByProvider(sources) {
  const result = new Map();
  for (const source of sources) {
    const providerSources = result.get(source.provider) ?? [];
    providerSources.push(source);
    result.set(source.provider, providerSources);
  }
  return result;
}

function sourceMatches(source, sourceModel) {
  return !source.model_prefix || sourceModel.startsWith(source.model_prefix);
}

function countByConfidence(models) {
  const counts = emptyCounts();
  for (const model of models) {
    counts[model.resolution.confidence] += 1;
  }
  return counts;
}

function emptyCounts() {
  return Object.fromEntries(confidenceLevels.map((confidence) => [confidence, 0]));
}

function parseArguments(args) {
  const options = { format: "summary", official: false, providers: [], unmatched: false };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--") {
      continue;
    } else if (argument === "--help" || argument === "-h") {
      options.help = true;
    } else if (argument === "--json") {
      options.format = "json";
    } else if (argument === "--official") {
      options.official = true;
    } else if (argument === "--unmatched") {
      options.unmatched = true;
    } else if (argument === "--provider" && args[index + 1]) {
      options.providers.push(args[index + 1]);
      index += 1;
    } else {
      throw new Error(`unknown or incomplete argument: ${argument}`);
    }
  }
  if (options.format === "json" && options.unmatched) {
    throw new Error("--json and --unmatched cannot be combined");
  }
  return options;
}

function usage() {
  return `Usage: pnpm models-dev:report -- [options]

Fetch every models.dev record and resolve it against this catalog.

Options:
  --json                 Emit every record as JSON.
  --official             Limit records to curated models.dev sources for catalog vendors.
  --provider <id>        Limit records to a models.dev provider; repeatable.
  --unmatched            Print every unmatched provider/model pair.
  --help, -h             Show this help.
`;
}

function printSummary(report) {
  console.log(`models.dev: ${report.provider_count} providers, ${report.model_count} model records`);
  console.log(`exact=${report.counts.exact} normalized=${report.counts.normalized} heuristic=${report.counts.heuristic} unknown=${report.counts.unknown}`);
  for (const provider of report.providers) {
    const { counts } = provider;
    console.log(`${provider.provider}\texact=${counts.exact}\tnormalized=${counts.normalized}\theuristic=${counts.heuristic}\tunknown=${counts.unknown}`);
  }
}

function printUnmatched(report) {
  for (const provider of report.providers) {
    for (const model of provider.models) {
      if (model.resolution.confidence === "unknown") {
        console.log(`${provider.provider}/${model.source_model}`);
      }
    }
  }
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  if (options.help) {
    console.log(usage());
    return;
  }

  const response = await fetch(MODELS_DEV_URL);
  if (!response.ok) {
    throw new Error(`models.dev returned ${response.status}`);
  }
  const report = {
    source: MODELS_DEV_URL,
    fetched_at: new Date().toISOString(),
    ...createModelsDevReport(await response.json(), options)
  };

  if (options.format === "json") {
    console.log(JSON.stringify(report, null, 2));
  } else if (options.unmatched) {
    printUnmatched(report);
  } else {
    printSummary(report);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(`models.dev report: ${error.message}`);
    process.exit(1);
  });
}
