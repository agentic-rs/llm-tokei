import { parse } from "smol-toml";
import { posix as path } from "node:path";

import type { PriceRecord } from "./types.js";

export type TomlValue =
  | string
  | number
  | boolean
  | Date
  | TomlValue[]
  | { [key: string]: TomlValue };

export type TomlDocument = { [key: string]: TomlValue };

export type ModelsDevDocument =
  | {
      kind: "toml";
      value: TomlDocument;
    }
  | {
      kind: "symlink";
      target: string;
    };

export type EndpointIdentity = {
  provider: string;
  model: string;
};

export type ResolvedDocument = {
  dependencies: Set<string>;
  missing?: true;
  value: TomlDocument;
};

const PROVIDER_MODEL_PATH = /^providers\/([^/]+)\/models\/(.+)\.toml$/;
const BASE_MODEL_PATH = /^models\/.+\.toml$/;

export function isModelsDevDocumentPath(path: string): boolean {
  return PROVIDER_MODEL_PATH.test(path) || BASE_MODEL_PATH.test(path);
}

export function endpointIdentity(path: string): EndpointIdentity | undefined {
  const match = path.match(PROVIDER_MODEL_PATH);
  if (!match) return undefined;
  return { provider: match[1], model: match[2] };
}

export function endpointKey(identity: EndpointIdentity): string {
  return `${identity.provider}\0${identity.model}`;
}

export function parseModelsDevDocument(pathname: string, source: string, mode: string): ModelsDevDocument {
  if (mode === "120000") {
    return { kind: "symlink", target: symlinkTarget(pathname, source) };
  }
  try {
    const value = parseWithHistoricalDuplicateRecovery(source);
    if (!isRecord(value)) {
      throw new Error("expected a TOML table");
    }
    return { kind: "toml", value };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`could not parse models.dev TOML ${pathname}: ${message}`);
  }
}

function parseWithHistoricalDuplicateRecovery(source: string): TomlDocument {
  try {
    return parsedDocument(source);
  } catch (error) {
    const repaired = removeRepeatedAssignments(source);
    if (repaired === source) throw error;
    return parsedDocument(repaired);
  }
}

function parsedDocument(source: string): TomlDocument {
  const value = parse(source);
  if (!isRecord(value)) {
    throw new Error("expected a TOML table");
  }
  return value;
}

/**
 * Some historical sync commits repeat a scalar assignment verbatim. TOML
 * correctly rejects those files, but removing an identical duplicate does not
 * invent a price or alter the resolved value. For non-price metadata only, a
 * conflicting duplicate is also removed so an unrelated invalid `limit` or
 * capability field cannot block price history. Price and inheritance conflicts
 * remain an error and therefore cannot be silently guessed.
 */
function removeRepeatedAssignments(source: string): string {
  const seen = new Map<string, string>();
  let table = "";
  let changed = false;
  const lines: string[] = [];

  for (const line of source.split(/(?<=\n)/)) {
    const trimmed = line.trim();
    const tableMatch = trimmed.match(/^\[\[?([^\]]+)\]?\]\s*(?:#.*)?$/);
    if (tableMatch) {
      table = tableMatch[1];
      lines.push(line);
      continue;
    }

    const assignment = line.match(/^\s*([A-Za-z0-9_.-]+)\s*=\s*(.*?)\s*$/);
    if (!assignment) {
      lines.push(line);
      continue;
    }
    const key = `${table}\0${assignment[1]}`;
    const value = assignment[2];
    const previous = seen.get(key);
    if (previous !== undefined && (previous === value || canDiscardConflictingMetadata(table, assignment[1]))) {
      changed = true;
      continue;
    }
    seen.set(key, value);
    lines.push(line);
  }

  return changed ? lines.join("") : source;
}

function canDiscardConflictingMetadata(table: string, key: string): boolean {
  return table !== "cost" && table !== "extends" && key !== "base_model" && key !== "base_model_omit";
}

/**
 * Resolves current `base_model` metadata inheritance and the historical
 * provider-to-provider `[extends]` form before extracting a route price.
 */
export class ModelsDevResolver {
  readonly #cache = new Map<string, ResolvedDocument>();

  constructor(
    private readonly documents: ReadonlyMap<string, ModelsDevDocument>,
    private readonly commitSha: string
  ) {}

  resolve(path: string): ResolvedDocument {
    return this.resolvePath(path, []);
  }

  private resolvePath(path: string, ancestors: string[]): ResolvedDocument {
    const cached = this.#cache.get(path);
    if (cached) return cached;
    if (ancestors.includes(path)) {
      throw new Error(
        `models.dev inheritance cycle in commit ${this.commitSha}: ${[...ancestors, path].join(" -> ")}`
      );
    }

    const source = this.documents.get(path);
    if (!source) {
      const resolved = {
        dependencies: new Set<string>(),
        missing: true as const,
        value: {}
      };
      this.#cache.set(path, resolved);
      return resolved;
    }

    const nextAncestors = [...ancestors, path];
    if (source.kind === "symlink") {
      if (!this.documents.has(source.target)) {
        const resolved = {
          dependencies: new Set([source.target]),
          missing: true as const,
          value: {}
        };
        this.#cache.set(path, resolved);
        return resolved;
      }
      const target = this.resolvePath(source.target, nextAncestors);
      const resolved = {
        dependencies: new Set([source.target, ...target.dependencies]),
        ...(target.missing ? { missing: true as const } : {}),
        value: cloneDocument(target.value)
      };
      this.#cache.set(path, resolved);
      return resolved;
    }

    const document = source.value;
    const dependencies = new Set<string>();
    let value: TomlDocument = {};

    const baseModel = document.base_model;
    if (baseModel !== undefined) {
      const basePath = baseModelPath(baseModel, path, this.commitSha);
      const base = this.resolvePath(basePath, nextAncestors);
      value = deepMerge(value, base.value);
      dependencies.add(basePath);
      for (const dependency of base.dependencies) dependencies.add(dependency);
    }

    const legacyExtends = legacyExtendsFrom(document, path, this.commitSha);
    if (legacyExtends) {
      const basePath = providerModelPath(legacyExtends.from, path, this.commitSha);
      const base = this.resolvePath(basePath, nextAncestors);
      value = deepMerge(value, base.value);
      dependencies.add(basePath);
      for (const dependency of base.dependencies) dependencies.add(dependency);
    }

    value = deepMerge(value, withoutInheritanceControls(document));
    applyOmissions(value, stringArray(document.base_model_omit, "base_model_omit", path, this.commitSha));
    applyOmissions(value, legacyExtends?.omit ?? []);

    const resolved = { dependencies, value };
    this.#cache.set(path, resolved);
    return resolved;
  }
}

export function priceFromResolvedDocument(
  identity: EndpointIdentity,
  value: TomlDocument,
  path: string,
  commitSha: string
): PriceRecord | undefined {
  const cost = value.cost;
  if (cost === undefined) return undefined;
  if (!isRecord(cost)) {
    throw new Error(`models.dev ${path} has a non-table cost in commit ${commitSha}`);
  }

  const input = requiredPrice(cost.input, "input", path, commitSha);
  const output = requiredPrice(cost.output, "output", path, commitSha);
  const optional = {
    reasoning: optionalPrice(cost.reasoning, "reasoning", path, commitSha),
    cache_read: optionalPrice(cost.cache_read ?? cost.inputCached ?? cost.input_cached, "cache_read", path, commitSha),
    cache_write: optionalPrice(cost.cache_write ?? cost.outputCached ?? cost.output_cached, "cache_write", path, commitSha),
    input_audio: optionalPrice(cost.input_audio ?? cost.audio_input, "input_audio", path, commitSha),
    output_audio: optionalPrice(cost.output_audio ?? cost.audio_output, "output_audio", path, commitSha)
  };
  return compactPrice({
    provider: identity.provider,
    model: identity.model,
    input,
    output,
    ...optional
  });
}

function baseModelPath(value: TomlValue, path: string, commitSha: string): string {
  if (typeof value !== "string" || !validModelReference(value)) {
    throw new Error(`models.dev ${path} has an invalid base_model in commit ${commitSha}`);
  }
  return `models/${value}.toml`;
}

function providerModelPath(value: string, path: string, commitSha: string): string {
  if (!validModelReference(value)) {
    throw new Error(`models.dev ${path} has an invalid extends.from in commit ${commitSha}`);
  }
  const [provider, ...model] = value.split("/");
  if (!provider || model.length === 0 || model.some((part) => !part)) {
    throw new Error(`models.dev ${path} has an invalid extends.from in commit ${commitSha}`);
  }
  return `providers/${provider}/models/${model.join("/")}.toml`;
}

function legacyExtendsFrom(
  document: TomlDocument,
  path: string,
  commitSha: string
): { from: string; omit: string[] } | undefined {
  const value = document.extends;
  if (value === undefined) return undefined;
  if (typeof value === "string") {
    return { from: value, omit: [] };
  }
  if (!isRecord(value) || typeof value.from !== "string") {
    throw new Error(`models.dev ${path} has an invalid extends table in commit ${commitSha}`);
  }
  return { from: value.from, omit: stringArray(value.omit, "extends.omit", path, commitSha) };
}

function withoutInheritanceControls(document: TomlDocument): TomlDocument {
  const value = cloneDocument(document);
  delete value.base_model;
  delete value.base_model_omit;
  delete value.extends;
  return value;
}

function applyOmissions(value: TomlDocument, paths: string[]): void {
  for (const path of paths) {
    const segments = path.split(".");
    let parent: TomlDocument | undefined = value;
    for (const segment of segments.slice(0, -1)) {
      if (!parent) break;
      const child: TomlValue | undefined = parent[segment];
      if (!isRecord(child)) {
        parent = undefined;
        break;
      }
      parent = child;
    }
    if (parent) delete parent[segments.at(-1)!];
  }
}

function stringArray(value: TomlValue | undefined, name: string, path: string, commitSha: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string" || !item)) {
    throw new Error(`models.dev ${path} has an invalid ${name} in commit ${commitSha}`);
  }
  return value.map((item) => item as string);
}

function requiredPrice(value: TomlValue | undefined, name: string, path: string, commitSha: string): string {
  const price = optionalPrice(value, name, path, commitSha);
  if (price === undefined) {
    throw new Error(`models.dev ${path} has no ${name} price in commit ${commitSha}`);
  }
  return price;
}

function optionalPrice(value: TomlValue | undefined, name: string, path: string, commitSha: string): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    throw new Error(`models.dev ${path} has an invalid ${name} price in commit ${commitSha}`);
  }
  return decimal(value);
}

function decimal(value: number): string {
  const text = String(value);
  if (!/[eE]/.test(text)) return text;

  const [coefficient, exponentText] = text.toLowerCase().split("e");
  const exponent = Number(exponentText);
  const sign = coefficient.startsWith("-") ? "-" : "";
  const unsigned = sign ? coefficient.slice(1) : coefficient;
  const [whole, fraction = ""] = unsigned.split(".");
  const digits = `${whole}${fraction}`.replace(/^0+(?=\d)/, "");
  const decimalIndex = whole.length + exponent;

  if (decimalIndex <= 0) return `${sign}0.${"0".repeat(-decimalIndex)}${digits}`;
  if (decimalIndex >= digits.length) return `${sign}${digits}${"0".repeat(decimalIndex - digits.length)}`;
  return `${sign}${digits.slice(0, decimalIndex)}.${digits.slice(decimalIndex)}`;
}

function compactPrice(value: PriceRecord): PriceRecord {
  return Object.fromEntries(Object.entries(value).filter(([, field]) => field !== undefined)) as PriceRecord;
}

function validModelReference(value: string): boolean {
  return !value.startsWith("/") && !value.endsWith("/") && !value.split("/").some((part) => !part || part === "." || part === "..");
}

function symlinkTarget(pathname: string, source: string): string {
  const target = source.trim();
  if (!target || target.includes("\0") || path.isAbsolute(target)) {
    throw new Error(`models.dev symlink ${pathname} has an invalid target`);
  }
  const resolved = path.normalize(path.join(path.dirname(pathname), target));
  if (resolved === ".." || resolved.startsWith("../")) {
    throw new Error(`models.dev symlink ${pathname} escapes the repository`);
  }
  return resolved;
}

function deepMerge(base: TomlDocument, override: TomlDocument): TomlDocument {
  const value = cloneDocument(base);
  for (const [key, overrideValue] of Object.entries(override)) {
    const baseValue = value[key];
    value[key] = isRecord(baseValue) && isRecord(overrideValue) ? deepMerge(baseValue, overrideValue) : cloneValue(overrideValue);
  }
  return value;
}

function cloneDocument(value: TomlDocument): TomlDocument {
  return cloneValue(value) as TomlDocument;
}

function cloneValue(value: TomlValue): TomlValue {
  if (Array.isArray(value)) return value.map(cloneValue);
  if (value instanceof Date) return new Date(value);
  if (isRecord(value)) {
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, cloneValue(child)]));
  }
  return value;
}

function isRecord(value: unknown): value is TomlDocument {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value) && !(value instanceof Date);
}
