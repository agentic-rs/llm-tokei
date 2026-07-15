import alibabaDefinitions from "./models/alibaba.json" with { type: "json" };
import anthropicDefinitions from "./models/anthropic.json" with { type: "json" };
import cohereDefinitions from "./models/cohere.json" with { type: "json" };
import deepseekDefinitions from "./models/deepseek.json" with { type: "json" };
import googleDefinitions from "./models/google.json" with { type: "json" };
import metaDefinitions from "./models/meta.json" with { type: "json" };
import microsoftDefinitions from "./models/microsoft.json" with { type: "json" };
import minimaxDefinitions from "./models/minimax.json" with { type: "json" };
import mistralDefinitions from "./models/mistral.json" with { type: "json" };
import moonshotaiDefinitions from "./models/moonshotai.json" with { type: "json" };
import openaiDefinitions from "./models/openai.json" with { type: "json" };
import xaiDefinitions from "./models/xai.json" with { type: "json" };
import zaiDefinitions from "./models/zai.json" with { type: "json" };
import { listModelsDevSources } from "./models-dev-sources.js";

export { listModelsDevSources } from "./models-dev-sources.js";

export type ResolutionConfidence = "exact" | "normalized" | "heuristic" | "unknown";

export type ModelDefinition = {
  vendor: string;
  name?: string;
  aliases?: string[];
  rolling_aliases?: string[];
  family?: string;
  series?: string;
  is_rolling?: true;
  source_aliases?: Record<string, string[]>;
};

export type KnownModel = {
  canonical_name: string;
  vendor: string;
  family: string;
  series: string;
  is_rolling?: true;
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
type AliasMatch = { canonical_name: string; is_rolling: boolean };

const catalog = mergeVendorCatalogs({
  alibaba: alibabaDefinitions,
  anthropic: anthropicDefinitions,
  cohere: cohereDefinitions,
  deepseek: deepseekDefinitions,
  google: googleDefinitions,
  meta: metaDefinitions,
  microsoft: microsoftDefinitions,
  minimax: minimaxDefinitions,
  mistral: mistralDefinitions,
  moonshotai: moonshotaiDefinitions,
  openai: openaiDefinitions,
  xai: xaiDefinitions,
  zai: zaiDefinitions
});
assertModelsDevSourceCoverage(catalog);

const aliases = new Map<string, AliasMatch>();
const sourceAliases = new Map<string, Map<string, AliasMatch>>();
const sourceAliasNames = new Set<string>();

for (const [canonicalName, definition] of Object.entries(catalog)) {
  const isRolling = definition.is_rolling === true;
  addAliases(
    aliases,
    [canonicalName, definition.name, ...(definition.aliases ?? [])],
    canonicalName,
    "model alias",
    isRolling
  );
  addAliases(aliases, definition.rolling_aliases ?? [], canonicalName, "rolling model alias", true);
  for (const [provider, providerAliases] of Object.entries(definition.source_aliases ?? {})) {
    const normalizedProvider = normalize(provider);
    const aliasesForProvider = sourceAliases.get(normalizedProvider) ?? new Map<string, AliasMatch>();
    addAliases(aliasesForProvider, providerAliases, canonicalName, `source alias for ${provider}`, isRolling);
    for (const alias of providerAliases) {
      sourceAliasNames.add(normalize(alias));
    }
    sourceAliases.set(normalizedProvider, aliasesForProvider);
  }
}

for (const alias of sourceAliasNames) {
  if (aliases.has(alias)) {
    throw new Error(`source-scoped alias ${JSON.stringify(alias)} cannot also be a global model alias`);
  }
}

function addAliases(
  target: Map<string, AliasMatch>,
  values: Array<string | undefined>,
  canonicalName: string,
  label: string,
  isRolling: boolean
): void {
  for (const alias of values) {
    if (!alias) continue;
    const normalized = normalize(alias);
    const previous = target.get(normalized);
    if (previous && (previous.canonical_name !== canonicalName || previous.is_rolling !== isRolling)) {
      throw new Error(`${label} ${JSON.stringify(alias)} maps to both ${previous.canonical_name} and ${canonicalName}`);
    }
    target.set(normalized, { canonical_name: canonicalName, is_rolling: isRolling });
  }
}

/**
 * Resolve a reported model identifier without inventing a priceable model for unknown input.
 * Keep the input provider and model ID for provider-specific price lookup; canonical_name is
 * a route-neutral catalog identity, not a billing SKU.
 */
export function resolveModel(input: ResolveModelInput): Resolution {
  if (!input.model) return unknown();

  const reported = normalize(input.model);
  if (!reported) return unknown();

  const provider = input.provider ? normalize(input.provider) : "";
  const sourceMatch = provider ? sourceAliases.get(provider)?.get(reported) : undefined;
  if (sourceMatch) {
    const wasNormalized = input.model !== reported;
    return resolved(
      sourceMatch.canonical_name,
      wasNormalized ? "normalized" : "exact",
      wasNormalized ? "normalization" : "source_alias",
      sourceMatch.is_rolling
    );
  }
  if (sourceAliasNames.has(reported)) return unknown();

  const exact = aliases.get(reported);
  if (exact) {
    const wasNormalized = input.model !== reported;
    return resolved(
      exact.canonical_name,
      wasNormalized ? "normalized" : "exact",
      wasNormalized ? "normalization" : "catalog_alias",
      exact.is_rolling
    );
  }

  let candidate = reported;
  for (let pass = 0; pass < heuristicRules.length; pass += 1) {
    let changed = false;
    for (const rule of heuristicRules) {
      const next = rule.apply(candidate);
      if (next === candidate) continue;
      candidate = next;
      changed = true;
      if (sourceAliasNames.has(candidate)) return unknown();
      const match = aliases.get(candidate);
      if (match) return resolved(match.canonical_name, "heuristic", rule.name, match.is_rolling);
    }
    if (!changed) break;
  }

  return unknown();
}

export function getModel(canonicalName: string): KnownModel | undefined {
  const canonical = aliases.get(normalize(canonicalName));
  return canonical ? withRolling(knownModel(canonical.canonical_name), canonical.is_rolling) : undefined;
}

export function listModels(): KnownModel[] {
  return Object.keys(catalog).sort().map(knownModel);
}

function mergeVendorCatalogs(vendorCatalogs: Record<string, Record<string, unknown>>): ModelCatalog {
  const merged: ModelCatalog = {};
  for (const [vendor, definitions] of Object.entries(vendorCatalogs)) {
    for (const [canonicalName, definition] of Object.entries(definitions)) {
      validateDefinition(canonicalName, definition, vendor);
      if (merged[canonicalName]) {
        throw new Error(`canonical model ${canonicalName} appears in multiple vendor catalogs`);
      }
      merged[canonicalName] = definition;
    }
  }
  return merged;
}

function validateDefinition(canonicalName: string, definition: unknown, vendor: string): asserts definition is ModelDefinition {
  if (!canonicalName.trim()) {
    throw new Error(`model in ${vendor}.json has an empty canonical name`);
  }
  if (!definition || typeof definition !== "object" || Array.isArray(definition)) {
    throw new Error(`model ${canonicalName} in ${vendor}.json must be an object`);
  }
  const rawDefinition = definition as Record<string, unknown>;
  if (rawDefinition.vendor !== vendor) {
    throw new Error(`model ${canonicalName} belongs in ${rawDefinition.vendor}.json, not ${vendor}.json`);
  }
  for (const field of ["name", "family", "series"] as const) {
    const value = rawDefinition[field];
    if (value !== undefined && (typeof value !== "string" || !value.trim())) {
      throw new Error(`model ${canonicalName} has an invalid ${field}`);
    }
  }
  validateStringArray(rawDefinition.aliases, `model ${canonicalName} aliases`);
  validateStringArray(rawDefinition.rolling_aliases, `model ${canonicalName} rolling aliases`);
  if (rawDefinition.is_rolling !== undefined && rawDefinition.is_rolling !== true) {
    throw new Error(`model ${canonicalName} is_rolling must be true when set`);
  }
  const sourceAliases = rawDefinition.source_aliases;
  if (sourceAliases !== undefined && (!sourceAliases || typeof sourceAliases !== "object" || Array.isArray(sourceAliases))) {
    throw new Error(`model ${canonicalName} source_aliases must be an object`);
  }
  for (const [provider, aliases] of Object.entries(sourceAliases ?? {})) {
    if (!provider.trim()) {
      throw new Error(`model ${canonicalName} has an empty source alias provider`);
    }
    validateStringArray(aliases, `model ${canonicalName} source aliases for ${provider}`);
  }
}

function validateStringArray(values: unknown, label: string): void {
  if (values === undefined) return;
  if (!Array.isArray(values) || values.some((value) => typeof value !== "string" || !value.trim())) {
    throw new Error(`${label} must be non-empty strings`);
  }
}

function assertModelsDevSourceCoverage(modelCatalog: ModelCatalog): void {
  const sourceVendors = new Set(listModelsDevSources().map((source) => source.vendor));
  for (const vendor of new Set(Object.values(modelCatalog).map((definition) => definition.vendor))) {
    if (!sourceVendors.has(vendor)) {
      throw new Error(`vendor ${vendor} has no configured models.dev source`);
    }
  }
}

function resolved(
  canonicalName: string,
  confidence: ResolvedModel["confidence"],
  matchedBy: string,
  isRolling: boolean
): ResolvedModel {
  return { ...withRolling(knownModel(canonicalName), isRolling), confidence, matched_by: matchedBy };
}

function withRolling(model: KnownModel, isRolling: boolean): KnownModel {
  return isRolling ? { ...model, is_rolling: true } : model;
}

function knownModel(canonicalName: string): KnownModel {
  const definition = catalog[canonicalName];
  const model: KnownModel = {
    canonical_name: canonicalName,
    vendor: definition.vendor,
    family: definition.family ?? inferFamily(canonicalName),
    series: definition.series ?? inferSeries(canonicalName)
  };
  return definition.is_rolling ? { ...model, is_rolling: true } : model;
}

function unknown(): UnknownModel {
  return { canonical_name: null, confidence: "unknown", matched_by: null };
}

function normalize(value: string): string {
  return value.trim().toLowerCase();
}

const heuristicRules = [
  { name: "provider_prefix", apply: stripProviderPrefix },
  { name: "slash_prefix", apply: stripSlashPrefix },
  { name: "date_suffix", apply: stripDateSuffix },
  { name: "mode_suffix", apply: stripModeSuffix },
  { name: "variant_suffix", apply: stripVariantSuffix },
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
  return value.replace(/(?:-latest|-chat|-preview)$/, "");
}

function stripProviderPrefix(value: string): string {
  return value.replace(/^(?:zai-org|anthropic|openai|copilot|google|zai|deepseek|alibaba|minimax|xai|mistral|meta|cohere|moonshotai-cn|moonshotai|microsoft)-/, "");
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
