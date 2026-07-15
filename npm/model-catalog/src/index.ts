import alibabaDefinitions from "./models/alibaba.json" with { type: "json" };
import anthropicDefinitions from "./models/anthropic.json" with { type: "json" };
import deepseekDefinitions from "./models/deepseek.json" with { type: "json" };
import googleDefinitions from "./models/google.json" with { type: "json" };
import minimaxDefinitions from "./models/minimax.json" with { type: "json" };
import openaiDefinitions from "./models/openai.json" with { type: "json" };
import zaiDefinitions from "./models/zai.json" with { type: "json" };

export type ResolutionConfidence = "exact" | "normalized" | "heuristic" | "unknown";

export type ModelDefinition = {
  provider: string;
  name?: string;
  aliases?: string[];
  family?: string;
  series?: string;
};

export type KnownModel = {
  canonical_name: string;
  vendor: string;
  family: string;
  series: string;
};

export type ResolvedModel = KnownModel & {
  confidence: Exclude<ResolutionConfidence, "unknown">;
  matched_by: string;
};

export type UnknownModel = {
  canonical_name: null;
  confidence: "unknown";
  matched_by: null;
};

export type Resolution = ResolvedModel | UnknownModel;

export type ResolveModelInput = {
  model: string | null | undefined;
  provider?: string | null | undefined;
};

type ModelCatalog = Record<string, ModelDefinition>;

const catalog = mergeVendorCatalogs({
  alibaba: alibabaDefinitions,
  anthropic: anthropicDefinitions,
  deepseek: deepseekDefinitions,
  google: googleDefinitions,
  minimax: minimaxDefinitions,
  openai: openaiDefinitions,
  zai: zaiDefinitions
});
const aliases = new Map<string, string>();

for (const [canonicalName, definition] of Object.entries(catalog)) {
  for (const alias of [canonicalName, definition.name, ...(definition.aliases ?? [])]) {
    if (!alias) continue;
    const normalized = normalize(alias);
    const previous = aliases.get(normalized);
    if (previous && previous !== canonicalName) {
      throw new Error(`model alias ${JSON.stringify(alias)} maps to both ${previous} and ${canonicalName}`);
    }
    aliases.set(normalized, canonicalName);
  }
}

/** Resolve a reported model identifier without inventing a priceable model for unknown input. */
export function resolveModel(input: ResolveModelInput): Resolution {
  if (!input.model) return unknown();

  const reported = normalize(input.model);
  if (!reported) return unknown();

  const exact = aliases.get(reported);
  if (exact) {
    const wasNormalized = input.model !== reported;
    return resolved(exact, wasNormalized ? "normalized" : "exact", wasNormalized ? "normalization" : "catalog_alias");
  }

  let candidate = reported;
  for (let pass = 0; pass < heuristicRules.length; pass += 1) {
    let changed = false;
    for (const rule of heuristicRules) {
      const next = rule.apply(candidate);
      if (next === candidate) continue;
      candidate = next;
      changed = true;
      const canonicalName = aliases.get(candidate);
      if (canonicalName) return resolved(canonicalName, "heuristic", rule.name);
    }
    if (!changed) break;
  }

  return unknown();
}

export function getModel(canonicalName: string): KnownModel | undefined {
  const canonical = aliases.get(normalize(canonicalName));
  return canonical ? knownModel(canonical) : undefined;
}

export function listModels(): KnownModel[] {
  return Object.keys(catalog).sort().map(knownModel);
}

function mergeVendorCatalogs(vendorCatalogs: Record<string, ModelCatalog>): ModelCatalog {
  const merged: ModelCatalog = {};
  for (const [vendor, definitions] of Object.entries(vendorCatalogs)) {
    for (const [canonicalName, definition] of Object.entries(definitions)) {
      if (definition.provider !== vendor) {
        throw new Error(`model ${canonicalName} belongs in ${definition.provider}.json, not ${vendor}.json`);
      }
      if (merged[canonicalName]) {
        throw new Error(`canonical model ${canonicalName} appears in multiple vendor catalogs`);
      }
      merged[canonicalName] = definition;
    }
  }
  return merged;
}

function resolved(canonicalName: string, confidence: ResolvedModel["confidence"], matchedBy: string): ResolvedModel {
  return { ...knownModel(canonicalName), confidence, matched_by: matchedBy };
}

function knownModel(canonicalName: string): KnownModel {
  const definition = catalog[canonicalName];
  return {
    canonical_name: canonicalName,
    vendor: definition.provider,
    family: definition.family ?? inferFamily(canonicalName),
    series: definition.series ?? inferSeries(canonicalName)
  };
}

function unknown(): UnknownModel {
  return { canonical_name: null, confidence: "unknown", matched_by: null };
}

function normalize(value: string): string {
  return value.trim().toLowerCase();
}

const heuristicRules = [
  { name: "date_suffix", apply: stripDateSuffix },
  { name: "mode_suffix", apply: stripModeSuffix },
  { name: "variant_suffix", apply: stripVariantSuffix },
  { name: "provider_prefix", apply: stripProviderPrefix },
  { name: "slash_prefix", apply: stripSlashPrefix },
  { name: "version_separator", apply: normalizeVersionSeparator }
];

function stripDateSuffix(value: string): string {
  const withoutDefault = value.replace(/@default$/, "");
  return withoutDefault
    .replace(/(?:-|@)\d{8}$/, "")
    .replace(/-\d{4}-\d{2}-\d{2}$/, "")
    .replace(/-\d{6}$/, "");
}

function stripModeSuffix(value: string): string {
  return value.replace(/(?::thinking|-thinking|-think|-fast)$/, "");
}

function stripVariantSuffix(value: string): string {
  return value.replace(/(?:-chat-latest|-latest|-chat|-preview)$/, "");
}

function stripProviderPrefix(value: string): string {
  return value.replace(/^(?:zai-org|anthropic|openai|copilot|google|zai|deepseek|alibaba|minimax)-/, "");
}

function stripSlashPrefix(value: string): string {
  const slash = value.indexOf("/");
  return slash === -1 || slash === value.length - 1 ? value : value.slice(slash + 1);
}

function normalizeVersionSeparator(value: string): string {
  for (const candidate of versionSeparatorCandidates(value)) {
    if (aliases.has(candidate)) return candidate;
  }
  return value;
}

function versionSeparatorCandidates(value: string): string[] {
  return [...value].flatMap((character, index) => {
    if (character !== "-" || !/\d/.test(value[index - 1] ?? "") || !/\d/.test(value[index + 1] ?? "")) return [];
    return `${value.slice(0, index)}.${value.slice(index + 1)}`;
  });
}

function inferFamily(canonicalName: string): string {
  const claude = canonicalName.match(/^(claude-(?:haiku|sonnet|opus))(?:-|$)/);
  if (claude) return claude[1];
  const gemini = canonicalName.match(/^(gemini-[\d.]+-(?:flash(?:-lite)?|pro))(?:-|$)/);
  if (gemini) return gemini[1];
  const gpt = canonicalName.match(/^(gpt-\d+(?:\.\d+)?)(?:-|$)/);
  if (gpt) return gpt[1];
  const reasoning = canonicalName.match(/^openai-(o\d+)(?:-|$)/);
  if (reasoning) return reasoning[1];
  return canonicalName.replace(/(?:-(?:mini|nano|pro|flash|lite|turbo|codex|chat|reasoner|highspeed))$/, "");
}

function inferSeries(canonicalName: string): string {
  const claude = canonicalName.match(/^claude-(?:haiku|sonnet|opus)-(\d+(?:\.\d+)?)/);
  if (claude) return `claude-${claude[1]}`;
  const gpt = canonicalName.match(/^(gpt-\d+(?:\.\d+)?)/);
  if (gpt) return gpt[1];
  const gemini = canonicalName.match(/^(gemini-\d+(?:\.\d+)?)/);
  if (gemini) return gemini[1];
  const reasoning = canonicalName.match(/^openai-(o\d+)/);
  if (reasoning) return reasoning[1];
  return inferFamily(canonicalName);
}
