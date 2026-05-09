#!/usr/bin/env bun
/// <reference types="bun-types" />
/*
 * extract-jsonl-schema.ts
 *
 * Stream-scan one or more (potentially large, optionally gzipped) JSONL files
 * and emit a TypeScript type declaration that captures the observed schema.
 *
 * Features:
 *   - Streaming line-by-line parsing (constant memory wrt file size).
 *   - Auto-detects gzip via .gz / .gzip extension.
 *   - Tracks optional vs nullable separately.
 *   - Detects tagged/discriminated unions on common keys (type/kind/tag/role/$type/event/op).
 *   - Recursively unifies array items; mixed-shape arrays become `oneOf` (TS union).
 *   - Auto-names variants from the discriminator value.
 *
 * Usage:
 *   bun run scripts/extract-jsonl-schema.ts [--out <path>] [--name <Root>] [--tag <key>] [files...]
 *   cat file.jsonl | bun run scripts/extract-jsonl-schema.ts > schema.ts
 */

// ---------------- CLI parsing ----------------

interface Args {
  out: string | null;
  name: string;
  tag: string | null;
  files: string[];
}

function parseArgs(argv: string[]): Args {
  const a: Args = { out: null, name: "Root", tag: null, files: [] };
  for (let i = 0; i < argv.length; i++) {
    const x = argv[i]!;
    if (x === "--out") a.out = argv[++i] ?? null;
    else if (x === "--name") a.name = argv[++i] ?? "Root";
    else if (x === "--tag") a.tag = argv[++i] ?? null;
    else if (x === "-h" || x === "--help") {
      printHelp();
      process.exit(0);
    } else if (x.startsWith("--")) {
      console.error(`unknown flag: ${x}`);
      process.exit(2);
    } else a.files.push(x);
  }
  return a;
}

function printHelp() {
  console.log(
    `Usage: bun run scripts/extract-jsonl-schema.ts [--out <path>] [--name <Root>] [--tag <key>] [files...]\n` +
      `\n` +
      `If no files are given, reads from stdin. .gz/.gzip files are auto-decompressed.\n`,
  );
}

// ---------------- Schema IR ----------------

type Prim = "string" | "number" | "boolean" | "null";

type Schema =
  | { k: "never" }
  | { k: "any" }
  | {
      k: "prim";
      types: Set<Prim>;
      literals?: Set<string>; // string literal tag values
      numLiterals?: Set<number>; // numeric literal tag values
      // String-value aggregates (only meaningful when 'string' is in `types`):
      seenString?: boolean;
      // AND-merged "all observed strings match this alias predicate":
      aliasOnly?: Map<string, boolean>;
      // OR-merged extra evidence flags (e.g., "any sample ended with '='"):
      aliasEvidence?: Map<string, boolean>;
      minLen?: number;
    }
  | { k: "array"; item: Schema }
  | { k: "object"; total: number; props: Map<string, { schema: Schema; present: number }> }
  | { k: "record"; key: string; value: Schema } // map; `key` is alias name ("Path", "Uuid", or "string")
  | { k: "union"; variants: Schema[] }; // anyOf fallback

const NEVER: Schema = { k: "never" };

const TAG_CANDIDATES = ["type", "kind", "tag", "role", "$type", "$mid", "event", "op"] as const;

// Dedup hints: [scopePrefix on the legacy old-chain (PascalCase), namePrefix without _hash].
// First entry: copilot-chat — Children_1 (kind:1) prompt-tsx tree nodes deep under
// CopilotChat.v -> Variant.metadata.toolCallResults{}.content[] -> $mid:23.value.node.children[].
const DEDUP_HINTS: readonly [string, string][] = [
  ["CopilotChat_1_V_Variant_Metadata_ToolCallResults_Value_Content", "Children_1"],
  ["CopilotChat_1_V_Variant_Metadata_ToolCallResults_Value_Content", "Children_2"],
];

function isPathLike(key: string): boolean {
  if (!key) return false;
  if (key.includes("/") || key.includes("\\")) return true;
  if (key.startsWith("~")) return true;
  return false;
}

function isPathLikeValue(s: string): boolean {
  if (!s) return false;
  if (s.startsWith("~/") || s.startsWith("./") || s.startsWith("../")) return true;
  if (s.includes("/") || s.includes("\\")) return true;
  return false;
}

// ---- Alias registry ----
// Order = render precedence (most specific first). Blob is handled separately.
interface AliasDef {
  name: string;
  predicate: (s: string) => boolean;
  evidence?: (s: string) => boolean; // if set, must OR-true at least once across samples
}

const ALIASES: AliasDef[] = [
  { name: "VscodeCallId", predicate: (s) => /^call_[A-Za-z0-9]+__vscode-\d+$/.test(s) },
  { name: "Uuid", predicate: (s) => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(s) },
  { name: "Sha256", predicate: (s) => /^[0-9a-f]{64}$/i.test(s) },
  { name: "Sha1", predicate: (s) => /^[0-9a-f]{40}$/i.test(s) },
  { name: "Semver", predicate: (s) => /^v?\d+\.\d+\.\d+(-[0-9A-Za-z.\-]+)?(\+[0-9A-Za-z.\-]+)?$/.test(s) },
  { name: "Email", predicate: (s) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(s) },
  {
    name: "IsoDate",
    predicate: (s) => /^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:?\d{2})?)?$/.test(s),
  },
  { name: "Url", predicate: (s) => /^[a-z][a-z0-9+.\-]*:\/\//i.test(s) },
  { name: "Path", predicate: isPathLikeValue },
  { name: "Hex", predicate: (s) => s.length >= 8 && s.length % 2 === 0 && /^[0-9a-f]+$/i.test(s) },
  {
    name: "Base64",
    predicate: (s) => s.length >= 24 && s.length % 4 === 0 && /^[A-Za-z0-9+/]+=*$/.test(s),
    evidence: (s) => s.endsWith("="),
  },
];

function buildAliasMaps(s: string): { only: Map<string, boolean>; ev: Map<string, boolean> } {
  const only = new Map<string, boolean>();
  const ev = new Map<string, boolean>();
  for (const a of ALIASES) {
    only.set(a.name, a.predicate(s));
    if (a.evidence) ev.set(a.name, a.evidence(s));
  }
  return { only, ev };
}

function detectKeyAlias(keys: string[]): string {
  // Pick the most specific alias from ALIASES that all keys satisfy (with evidence if required).
  for (const def of ALIASES) {
    if (!keys.every((k) => def.predicate(k))) continue;
    if (def.evidence && !keys.some((k) => def.evidence!(k))) continue;
    return def.name;
  }
  return "string";
}

function maybeRecordify(o: Schema & { k: "object" }): Schema {
  if (o.props.size < 2) return o;
  const keys = [...o.props.keys()];
  const key = detectKeyAlias(keys);
  if (key === "string") return o;
  // Collapse: fold all per-key schemas into a single value schema.
  let val: Schema = NEVER;
  for (const { schema } of o.props.values()) val = merge(val, schema);
  return { k: "record", key, value: val };
}

// ---------------- Build IR from JSON value ----------------

function fromValue(v: unknown, isTagField = false): Schema {
  if (v === null) return { k: "prim", types: new Set(["null"]) };
  const t = typeof v;
  if (t === "string") {
    const str = v as string;
    const { only, ev } = buildAliasMaps(str);
    const s: Schema = {
      k: "prim",
      types: new Set(["string"]),
      seenString: true,
      aliasOnly: only,
      aliasEvidence: ev,
      minLen: str.length,
    };
    if (isTagField) (s as any).literals = new Set([str]);
    return s;
  }
  if (t === "number") {
    const s: Schema = { k: "prim", types: new Set(["number"]) };
    if (isTagField) (s as any).numLiterals = new Set([v as number]);
    return s;
  }
  if (t === "boolean") return { k: "prim", types: new Set(["boolean"]) };
  if (Array.isArray(v)) {
    let item: Schema = NEVER;
    for (const it of v) item = merge(item, fromValue(it));
    return { k: "array", item };
  }
  if (t === "object") {
    const obj = v as Record<string, unknown>;
    const props = new Map<string, { schema: Schema; present: number }>();
    for (const [key, val] of Object.entries(obj)) {
      const tagField = (TAG_CANDIDATES as readonly string[]).includes(key);
      props.set(key, { schema: fromValue(val, tagField), present: 1 });
    }
    return maybeRecordify({ k: "object", total: 1, props });
  }
  return { k: "any" };
}

// ---------------- Merge ----------------

function merge(a: Schema, b: Schema): Schema {
  if (a.k === "never") return b;
  if (b.k === "never") return a;
  if (a.k === "any" || b.k === "any") return { k: "any" };

  // union: distribute
  if (a.k === "union") return mergeIntoUnion(a, b);
  if (b.k === "union") return mergeIntoUnion(b, a);

  if (a.k === "prim" && b.k === "prim") {
    const types = new Set<Prim>([...a.types, ...b.types]);
    let literals: Set<string> | undefined;
    if (a.literals || b.literals) {
      literals = new Set<string>([...(a.literals ?? []), ...(b.literals ?? [])]);
      if (literals.size > 64) literals = undefined; // bail on cardinality blow-up
    }
    let numLiterals: Set<number> | undefined;
    if (a.numLiterals || b.numLiterals) {
      numLiterals = new Set<number>([...(a.numLiterals ?? []), ...(b.numLiterals ?? [])]);
      if (numLiterals.size > 64) numLiterals = undefined;
    }
    const seenString = !!(a.seenString || b.seenString);
    const out: Schema = {
      k: "prim",
      types,
      ...(literals ? { literals } : {}),
      ...(numLiterals ? { numLiterals } : {}),
    };
    if (seenString) {
      out.seenString = true;
      // alias only flags: AND across observed string sources only.
      const only = new Map<string, boolean>();
      const ev = new Map<string, boolean>();
      for (const def of ALIASES) {
        const aa = a.seenString ? a.aliasOnly?.get(def.name) !== false : true;
        const bb = b.seenString ? b.aliasOnly?.get(def.name) !== false : true;
        only.set(def.name, aa && bb);
        if (def.evidence) {
          const ea = a.aliasEvidence?.get(def.name) === true;
          const eb = b.aliasEvidence?.get(def.name) === true;
          ev.set(def.name, ea || eb);
        }
      }
      out.aliasOnly = only;
      out.aliasEvidence = ev;
      const la = a.seenString && a.minLen !== undefined ? a.minLen : Number.POSITIVE_INFINITY;
      const lb = b.seenString && b.minLen !== undefined ? b.minLen : Number.POSITIVE_INFINITY;
      out.minLen = Math.min(la, lb);
    }
    return out;
  }

  if (a.k === "array" && b.k === "array") {
    return { k: "array", item: merge(a.item, b.item) };
  }

  if (a.k === "record" && b.k === "record") {
    const key = a.key === b.key ? a.key : "string";
    return { k: "record", key, value: merge(a.value, b.value) };
  }
  if (a.k === "record" && b.k === "object") return mergeRecordWithObject(a, b);
  if (b.k === "record" && a.k === "object") return mergeRecordWithObject(b, a);

  if (a.k === "object" && b.k === "object") {
    return mergeObjects(a, b);
  }

  // mismatched kinds → union
  return { k: "union", variants: [a, b] };
}

function mergeIntoUnion(u: Schema & { k: "union" }, x: Schema): Schema {
  // Flatten union+union by merging each variant of x in turn.
  if (x.k === "union") {
    let acc: Schema = u;
    for (const v of x.variants) acc = acc.k === "union" ? mergeIntoUnion(acc, v) : merge(acc, v);
    return acc;
  }
  // Try to merge x into a compatible variant; otherwise add.
  const out: Schema[] = [];
  let placed = false;
  for (const v of u.variants) {
    if (!placed && compatibleForMerge(v, x)) {
      const merged = merge(v, x);
      // If merging produced a fresh union, splice its variants in (flatten).
      if (merged.k === "union") {
        out.push(...merged.variants);
      } else {
        out.push(merged);
      }
      placed = true;
    } else out.push(v);
  }
  if (!placed) out.push(x);
  // Dedupe trivially-equal object variants by tag identity.
  return out.length === 1 ? out[0]! : { k: "union", variants: out };
}

function compatibleForMerge(a: Schema, b: Schema): boolean {
  if (a.k !== b.k) return false;
  if (a.k === "object" && b.k === "object") {
    // Use raw literal sets per tag key (regardless of presence): if either side
    // has a tag literal at any TAG_CANDIDATE key, both sides must agree on it,
    // otherwise these are different variants and must NOT be merged.
    for (const key of TAG_CANDIDATES) {
      const sa = a.props.get(key)?.schema as any;
      const sb = b.props.get(key)?.schema as any;
      const litsA: Set<string | number> | undefined =
        (sa?.literals as Set<string> | undefined) ?? (sa?.numLiterals as Set<number> | undefined);
      const litsB: Set<string | number> | undefined =
        (sb?.literals as Set<string> | undefined) ?? (sb?.numLiterals as Set<number> | undefined);
      if (!litsA && !litsB) continue;
      // One side has tag literals here, the other doesn't → incompatible.
      if (!litsA || !litsB) return false;
      // Both have literal sets → must overlap (i.e., share at least one tag value).
      let overlap = false;
      for (const v of litsA) if (litsB.has(v)) { overlap = true; break; }
      if (!overlap) return false;
    }
    return true;
  }
  return true;
}

function pickTagLiteral(o: Schema & { k: "object" }): { key: string; value: string | number } | null {
  for (const key of TAG_CANDIDATES) {
    const p = o.props.get(key);
    if (!p || p.present !== o.total) continue;
    if (p.schema.k !== "prim") continue;
    const types = p.schema.types;
    if (types.size !== 1) continue;
    if (types.has("string")) {
      const lits = p.schema.literals;
      if (!lits || lits.size !== 1) continue;
      return { key, value: [...lits][0]! };
    }
    if (types.has("number")) {
      const lits = p.schema.numLiterals;
      if (!lits || lits.size !== 1) continue;
      return { key, value: [...lits][0]! };
    }
  }
  return null;
}

function pickTagKey(o: Schema & { k: "object" }): string | null {
  for (const key of TAG_CANDIDATES) {
    const p = o.props.get(key);
    if (!p || p.present !== o.total) continue;
    if (p.schema.k !== "prim") continue;
    const types = p.schema.types;
    if (types.size !== 1) continue;
    if (types.has("string") && p.schema.literals && p.schema.literals.size > 0) return key;
    if (types.has("number") && p.schema.numLiterals && p.schema.numLiterals.size > 0) return key;
  }
  return null;
}

function mergeRecordWithObject(rec: Schema & { k: "record" }, obj: Schema & { k: "object" }): Schema {
  // If the object's keys are also all path-like, fold its values into the record's value.
  const allPath = obj.props.size === 0 || [...obj.props.keys()].every(isPathLike);
  if (allPath) {
    let v: Schema = rec.value;
    for (const { schema } of obj.props.values()) v = merge(v, schema);
    // Re-derive key alias considering the union of keys observed so far via this object.
    const objKeyAlias = obj.props.size === 0 ? rec.key : detectKeyAlias([...obj.props.keys()]);
    const key = rec.key === objKeyAlias ? rec.key : "string";
    return { k: "record", key, value: v };
  }
  // Mixed: keep both shapes via union.
  return { k: "union", variants: [rec, obj] };
}

function mergeObjects(
  a: Schema & { k: "object" },
  b: Schema & { k: "object" },
): Schema {
  // Tagged-union detection: if both have the same tag key and any variant has a
  // distinct singleton value, build a union instead of merging.
  const keyA = pickTagKey(a);
  const keyB = pickTagKey(b);
  if (USER_TAG_KEY || (keyA && keyA === keyB)) {
    const key = USER_TAG_KEY ?? keyA!;
    const propA = a.props.get(key)?.schema as any;
    const propB = b.props.get(key)?.schema as any;
    const litsA: Set<string | number> | undefined =
      (propA?.literals as Set<string> | undefined) ?? (propA?.numLiterals as Set<number> | undefined);
    const litsB: Set<string | number> | undefined =
      (propB?.literals as Set<string> | undefined) ?? (propB?.numLiterals as Set<number> | undefined);
    if (litsA && litsB) {
      // If literal sets are disjoint AND each has 1 value → clear different variants.
      const interSize = [...litsA].filter((x) => litsB.has(x)).length;
      if (interSize === 0) {
        return { k: "union", variants: [a, b] };
      }
    }
  }

  const props = new Map<string, { schema: Schema; present: number }>();
  const total = a.total + b.total;
  const allKeys = new Set<string>([...a.props.keys(), ...b.props.keys()]);
  for (const k of allKeys) {
    const pa = a.props.get(k);
    const pb = b.props.get(k);
    if (pa && pb) {
      props.set(k, { schema: merge(pa.schema, pb.schema), present: pa.present + pb.present });
    } else if (pa) {
      props.set(k, { schema: pa.schema, present: pa.present });
    } else if (pb) {
      props.set(k, { schema: pb.schema, present: pb.present });
    }
  }
  return maybeRecordify({ k: "object", total, props });
}

let USER_TAG_KEY: string | null = null;

// ---------------- Streaming line reader ----------------

async function* lines(stream: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const dec = new TextDecoder("utf-8");
  let buf = "";
  // @ts-ignore - async iteration over streams
  for await (const chunk of stream as any) {
    buf += dec.decode(chunk, { stream: true });
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      yield line;
    }
  }
  buf += dec.decode();
  if (buf.length) yield buf;
}

function openSource(path: string): ReadableStream<Uint8Array> {
  const f = Bun.file(path);
  let s: ReadableStream<Uint8Array> = f.stream();
  if (path.endsWith(".gz") || path.endsWith(".gzip")) {
    // @ts-ignore - DecompressionStream is global in Bun/modern runtimes
    s = s.pipeThrough(new DecompressionStream("gzip"));
  }
  return s;
}

async function ingest(stream: ReadableStream<Uint8Array>, source: string, state: { schema: Schema }) {
  let n = 0;
  for await (const raw of lines(stream)) {
    const line = raw.trim();
    if (!line) continue;
    n++;
    try {
      const v = JSON.parse(line);
      state.schema = merge(state.schema, fromValue(v));
    } catch (e) {
      console.error(`[${source}:${n}] JSON parse error: ${(e as Error).message}`);
    }
  }
  console.error(`[${source}] processed ${n} lines`);
}

// ---------------- Emit TypeScript ----------------

function pascal(s: string): string {
  return s
    .replace(/[^a-zA-Z0-9]+/g, " ")
    .trim()
    .split(/\s+/)
    .map((w) => (w ? w[0]!.toUpperCase() + w.slice(1) : ""))
    .join("") || "Variant";
}

function isSafeIdent(k: string): boolean {
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(k);
}

interface DeclEntry {
  name: string;
  body: string;
  doc: string[];
  oldChain: string;
  ir: Schema & { k: "object" };
}

interface EmitCtx {
  decls: DeclEntry[];
  used: Set<string>;
  aliases: Set<string>; // e.g., "Path", "Blob"
  objCache: Map<Schema, string>; // IR-ref -> emitted hoisted type name
}

interface PathCtx {
  old: string; // PascalCase chain (only used for hashing)
  pretty: string; // human path: "EventMsg.payload -> ExecCommandEnd.parsed_cmd[] -> ListFiles"
  field: string; // most recent JSON field name encountered ("" if none yet)
}

function uniqueName(ctx: EmitCtx, base: string): string {
  let n = base;
  let i = 2;
  while (ctx.used.has(n)) n = `${base}_${i++}`;
  ctx.used.add(n);
  return n;
}

function child(parent: string, seg: string): string {
  if (!parent) return seg;
  return `${parent}_${seg}`;
}

function descendField(p: PathCtx, key: string): PathCtx {
  return {
    old: child(p.old, pascal(key)),
    pretty: p.pretty ? `${p.pretty}.${key}` : key,
    field: key,
  };
}

function descendVariant(p: PathCtx, tagValue: string | number): PathCtx {
  const sv = String(tagValue);
  const seg = pascal(sv);
  return {
    old: child(p.old, seg),
    pretty: p.pretty ? `${p.pretty} -> ${sv}` : sv,
    field: p.field,
  };
}

function descendArray(p: PathCtx): PathCtx {
  return { old: child(p.old, "Item"), pretty: p.pretty + "[]", field: p.field };
}

function descendRecord(p: PathCtx): PathCtx {
  return { old: child(p.old, "Value"), pretty: p.pretty + "{}", field: p.field };
}

function descendVariantFallback(p: PathCtx): PathCtx {
  return {
    old: child(p.old, "Variant"),
    pretty: p.pretty ? `${p.pretty} -> Variant` : "Variant",
    field: p.field,
  };
}

function sha8(s: string): string {
  // Synchronous SHA-256, first 8 hex chars.
  // Bun has node:crypto.
  // Lazily imported once.
  return _sha8(s);
}
let _crypto: typeof import("node:crypto") | null = null;
function _sha8(s: string): string {
  if (!_crypto) _crypto = require("node:crypto");
  return _crypto.createHash("sha256").update(s).digest("hex").slice(0, 8);
}

function variantTypeName(p: PathCtx, leaf: string): string {
  const hash = sha8(child(p.old, leaf));
  const leafSeg = pascal(leaf);
  const fieldSeg = p.field ? pascal(p.field) : "";
  const raw = fieldSeg ? `${fieldSeg}_${leafSeg}_${hash}` : `${leafSeg}_${hash}`;
  return /^[0-9]/.test(raw) ? `$${raw}` : raw;
}

function render(s: Schema, ctx: EmitCtx, p: PathCtx): string {
  switch (s.k) {
    case "never":
      return "never";
    case "any":
      return "unknown";
    case "prim": {
      const parts: string[] = [];
      const lits = s.literals;
      const nlits = s.numLiterals;
      const hasString = s.types.has("string");
      const hasNumber = s.types.has("number");
      const onlyStringNonNull = hasString && !hasNumber && !s.types.has("boolean");
      if (hasString && lits && lits.size > 0) {
        for (const l of [...lits].sort()) parts.push(JSON.stringify(l));
      } else if (hasString) {
        let stringRepr = "string";
        if (onlyStringNonNull && s.seenString) {
          let picked: string | null = null;
          for (const def of ALIASES) {
            if (s.aliasOnly?.get(def.name) !== true) continue;
            if (def.evidence && s.aliasEvidence?.get(def.name) !== true) continue;
            picked = def.name;
            break;
          }
          if (picked) {
            ctx.aliases.add(picked);
            stringRepr = picked;
          } else if (s.minLen !== undefined && s.minLen > 50) {
            ctx.aliases.add("Blob");
            stringRepr = "Blob";
          }
        }
        parts.push(stringRepr);
      }
      if (hasNumber && nlits && nlits.size > 0) {
        for (const l of [...nlits].sort((x, y) => x - y)) parts.push(String(l));
      } else if (hasNumber) {
        parts.push("number");
      }
      if (s.types.has("boolean")) parts.push("boolean");
      if (s.types.has("null")) parts.push("null");
      return parts.length ? parts.join(" | ") : "never";
    }
    case "array": {
      const inner = render(s.item, ctx, descendArray(p));
      return needsParens(s.item) ? `Array<${inner}>` : `${inner}[]`;
    }
    case "record": {
      const inner = render(s.value, ctx, descendRecord(p));
      if (s.key !== "string") ctx.aliases.add(s.key);
      return `Record<${s.key}, ${inner}>`;
    }
    case "union": {
      const parts: string[] = [];
      for (const v of s.variants) {
        if (v.k === "object") {
          const cached = ctx.objCache.get(v);
          if (cached) {
            parts.push(cached);
            continue;
          }
          const tag = pickTagLiteral(v);
          const sub = tag ? descendVariant(p, tag.value) : descendVariantFallback(p);
          const baseName = tag ? variantTypeName(p, String(tag.value)) : variantTypeName(p, "Variant");
          const variantName = uniqueName(ctx, baseName);
          ctx.objCache.set(v, variantName);
          emitNamedObject(v, ctx, variantName, sub);
          parts.push(variantName);
        } else {
          parts.push(render(v, ctx, descendVariantFallback(p)));
        }
      }
      // Dedupe by reference (canonical-merged IRs may appear multiple times).
      const seen = new Set<string>();
      const uniq = parts.filter((x) => (seen.has(x) ? false : (seen.add(x), true)));
      return uniq.join(" | ");
    }
    case "object": {
      return renderObjectInline(s, ctx, p);
    }
  }
}

function needsParens(s: Schema): boolean {
  return (
    s.k === "union" ||
    (s.k === "prim" && (s.types.size + (s.literals?.size ?? 0) + (s.numLiterals?.size ?? 0)) > 1)
  );
}

function renderObjectInline(o: Schema & { k: "object" }, ctx: EmitCtx, p: PathCtx): string {
  if (o.props.size === 0) return "Record<string, unknown>";
  const lines: string[] = ["{"];
  const keys = [...o.props.keys()].sort();
  for (const key of keys) {
    const { schema, present } = o.props.get(key)!;
    const optional = present < o.total ? "?" : "";
    const t = render(schema, ctx, descendField(p, key));
    const safe = isSafeIdent(key) ? key : JSON.stringify(key);
    lines.push(`  ${safe}${optional}: ${indent(t)};`);
  }
  lines.push("}");
  return lines.join("\n");
}

function emitNamedObject(o: Schema & { k: "object" }, ctx: EmitCtx, name: string, p: PathCtx): void {
  const body = renderObjectInline(o, ctx, p);
  const doc = p.pretty ? [p.pretty] : [];
  // oldChain is the PARENT old-chain, used for hint matching.
  ctx.decls.push({ name, body, doc, oldChain: p.old, ir: o });
}

function indent(s: string): string {
  return s.replace(/\n/g, "\n  ");
}

function parseDecl(decl: string): { doc: string[]; name: string; body: string } | null {
  // doc may be either single-line `/** path */` or multi-line `/**\n * a\n * b\n */`.
  const m = decl.match(/^(?:(\/\*\*[\s\S]*?\*\/)\n)?export type ([\w$]+) = ([\s\S]+);\s*$/);
  if (!m) return null;
  const [, rawDoc, name, body] = m;
  let doc: string[] = [];
  if (rawDoc) {
    const inner = rawDoc.replace(/^\/\*\*\s*/, "").replace(/\s*\*\/$/, "");
    if (inner.includes("\n")) {
      for (const line of inner.split("\n")) {
        const t = line.replace(/^\s*\*\s?/, "").trim();
        if (t) doc.push(t);
      }
    } else {
      const t = inner.trim();
      if (t) doc.push(t);
    }
  }
  return { doc, name: name!, body: body! };
}

function formatDoc(paths: string[]): string {
  if (paths.length === 0) return "";
  if (paths.length === 1) return `/** ${paths[0]} */\n`;
  return `/**\n${paths.map((p) => ` * ${p}`).join("\n")}\n */\n`;
}

function substAll(s: string, rename: Map<string, string>): string {
  let out = s;
  for (const [from, to] of rename) {
    if (from === to) continue;
    out = out.replace(new RegExp(`(?<!\\w)${from.replace(/\$/g, "\\$")}(?!\\w)`, "g"), to);
  }
  return out;
}

interface HoistMeta {
  ir: Schema & { k: "object" };
  oldChain: string; // == sub.old at the point emitNamedObject would store it
  leaf: string; // tag string or "Variant"
  field: string; // p.field at parent
  pretty: string;
}

function collectHoists(s: Schema, p: PathCtx, out: HoistMeta[], seen: Set<Schema>): void {
  if (seen.has(s)) return;
  seen.add(s);
  switch (s.k) {
    case "array":
      collectHoists(s.item, descendArray(p), out, seen);
      return;
    case "record":
      collectHoists(s.value, descendRecord(p), out, seen);
      return;
    case "object":
      for (const [k, { schema }] of s.props) collectHoists(schema, descendField(p, k), out, seen);
      return;
    case "union":
      for (const v of s.variants) {
        if (v.k === "object") {
          const tag = pickTagLiteral(v);
          const sub = tag ? descendVariant(p, tag.value) : descendVariantFallback(p);
          const leaf = tag ? String(tag.value) : "Variant";
          out.push({ ir: v, oldChain: sub.old, leaf, field: p.field, pretty: sub.pretty });
          for (const [k, { schema }] of v.props) collectHoists(schema, descendField(sub, k), out, seen);
        } else {
          collectHoists(v, descendVariantFallback(p), out, seen);
        }
      }
      return;
  }
}

function namePrefixOf(h: HoistMeta): string {
  const fieldSeg = h.field ? pascal(h.field) : "";
  const leafSeg = pascal(h.leaf);
  const raw = fieldSeg ? `${fieldSeg}_${leafSeg}` : leafSeg;
  return /^[0-9]/.test(raw) ? `$${raw}` : raw;
}

function applyHintsOnIR(root: Schema, rootName: string): Map<Schema, Schema> {
  const hoists: HoistMeta[] = [];
  collectHoists(root, { old: rootName, pretty: "", field: "" }, hoists, new Set());

  const canonicalFor = new Map<Schema, Schema>();

  for (const [scope, namePrefix] of DEDUP_HINTS) {
    const candidates = hoists.filter((h) => h.oldChain.startsWith(scope) && namePrefixOf(h) === namePrefix);
    if (candidates.length < 2) continue;

    // Adjacency: edge iff schemas share ≥1 prop name.
    const n = candidates.length;
    const adj: Set<number>[] = Array.from({ length: n }, () => new Set());
    for (let a = 0; a < n; a++) {
      const ka = new Set(candidates[a]!.ir.props.keys());
      for (let b = a + 1; b < n; b++) {
        let shared = false;
        for (const k of candidates[b]!.ir.props.keys()) if (ka.has(k)) { shared = true; break; }
        if (shared) {
          adj[a]!.add(b);
          adj[b]!.add(a);
        }
      }
    }
    const seen = new Set<number>();
    for (let root = 0; root < n; root++) {
      if (seen.has(root)) continue;
      const comp: number[] = [];
      const stack = [root];
      while (stack.length) {
        const x = stack.pop()!;
        if (seen.has(x)) continue;
        seen.add(x);
        comp.push(x);
        for (const y of adj[x]!) if (!seen.has(y)) stack.push(y);
      }
      if (comp.length < 2) continue;
      comp.sort((x, y) => {
        const dx = candidates[x]!, dy = candidates[y]!;
        if (dx.oldChain.length !== dy.oldChain.length) return dx.oldChain.length - dy.oldChain.length;
        return dx.pretty < dy.pretty ? -1 : dx.pretty > dy.pretty ? 1 : 0;
      });
      const canon = candidates[comp[0]!]!;
      // Merge IRs into the canonical IR (mutates canon.ir.props by reassign).
      let mergedIr: Schema = canon.ir;
      for (let k = 1; k < comp.length; k++) mergedIr = merge(mergedIr, candidates[comp[k]!]!.ir);
      if (mergedIr.k !== "object") continue;
      // Mutate canon.ir to take the merged props/total so its identity remains stable.
      (canon.ir as any).props = mergedIr.props;
      (canon.ir as any).total = mergedIr.total;
      // Map non-canonicals to canonical IR.
      for (let k = 1; k < comp.length; k++) {
        canonicalFor.set(candidates[comp[k]!]!.ir, canon.ir);
      }
      // Cycle detection inside canon.
      cycleSplice(canon.ir, canonicalFor);
    }
  }
  return canonicalFor;
}

// Walk `canon`'s substructure and substitute any nested object IR sharing the same
// (tagKey, tagValue) with canon ref. Mutates union.variants in place and updates
// `canonicalFor` so other tree references rewrite to canon too.
function cycleSplice(canon: Schema & { k: "object" }, canonicalFor: Map<Schema, Schema>): void {
  const canonTag = pickTagLiteral(canon);
  if (!canonTag) return;
  const stack: Schema[] = [canon];
  const visited = new Set<Schema>([canon]);
  while (stack.length) {
    const cur = stack.pop()!;
    if (cur.k === "object") {
      for (const p of cur.props.values()) {
        if (visited.has(p.schema)) continue;
        visited.add(p.schema);
        stack.push(p.schema);
      }
    } else if (cur.k === "array") {
      if (!visited.has(cur.item)) { visited.add(cur.item); stack.push(cur.item); }
    } else if (cur.k === "record") {
      if (!visited.has(cur.value)) { visited.add(cur.value); stack.push(cur.value); }
    } else if (cur.k === "union") {
      for (let vi = 0; vi < cur.variants.length; vi++) {
        const v = cur.variants[vi]!;
        if (v.k === "object" && v !== canon) {
          const tag = pickTagLiteral(v);
          if (tag && tag.key === canonTag.key && tag.value === canonTag.value) {
            canonicalFor.set(v, canon);
            cur.variants[vi] = canon;
            continue;
          }
        }
        if (!visited.has(v)) { visited.add(v); stack.push(v); }
      }
    }
  }
}

// Auto-detect recursive types: any object IR with a tag literal that contains
// (transitively) another object IR with the same tag literal under the same field
// is treated as one recursive type. Operates after hint merge so it sees its IR tree.
function applyAutoRecursive(root: Schema, rootName: string): Map<Schema, Schema> {
  const hoists: HoistMeta[] = [];
  collectHoists(root, { old: rootName, pretty: "", field: "" }, hoists, new Set());

  const canonicalFor = new Map<Schema, Schema>();

  // Bucket by (field, tagKey, tagValue).
  const buckets = new Map<string, HoistMeta[]>();
  for (const h of hoists) {
    const tag = pickTagLiteral(h.ir);
    if (!tag) continue;
    const key = `${h.field}\x00${tag.key}\x00${String(tag.value)}`;
    const arr = buckets.get(key) ?? [];
    arr.push(h);
    buckets.set(key, arr);
  }

  for (const [, group] of buckets) {
    if (group.length < 2) continue;

    // Build directed reachability: edge a→b if b.ir is reachable from a.ir's substructure.
    const n = group.length;
    const reach: boolean[][] = Array.from({ length: n }, () => Array(n).fill(false));
    for (let i = 0; i < n; i++) {
      const targets = new Set<Schema>();
      for (let j = 0; j < n; j++) if (i !== j) targets.add(group[j]!.ir);
      if (targets.size === 0) continue;
      const found = new Set<Schema>();
      const stack: Schema[] = [];
      const seen = new Set<Schema>([group[i]!.ir]);
      // Seed children of i.ir (don't count i itself).
      pushChildren(group[i]!.ir, stack, seen);
      while (stack.length) {
        const cur = stack.pop()!;
        if (targets.has(cur)) found.add(cur);
        pushChildren(cur, stack, seen);
      }
      for (let j = 0; j < n; j++) if (i !== j && found.has(group[j]!.ir)) reach[i]![j] = true;
    }

    // Undirected reachability closure → connected components.
    const adj: Set<number>[] = Array.from({ length: n }, () => new Set());
    for (let i = 0; i < n; i++)
      for (let j = i + 1; j < n; j++)
        if (reach[i]![j] || reach[j]![i]) {
          adj[i]!.add(j);
          adj[j]!.add(i);
        }
    const seen = new Set<number>();
    for (let r = 0; r < n; r++) {
      if (seen.has(r)) continue;
      const comp: number[] = [];
      const stack = [r];
      while (stack.length) {
        const x = stack.pop()!;
        if (seen.has(x)) continue;
        seen.add(x);
        comp.push(x);
        for (const y of adj[x]!) if (!seen.has(y)) stack.push(y);
      }
      if (comp.length < 2) continue;
      // Canonical = shortest oldChain.
      comp.sort((x, y) => {
        const dx = group[x]!, dy = group[y]!;
        if (dx.oldChain.length !== dy.oldChain.length) return dx.oldChain.length - dy.oldChain.length;
        return dx.pretty < dy.pretty ? -1 : dx.pretty > dy.pretty ? 1 : 0;
      });
      const canon = group[comp[0]!]!;
      let mergedIr: Schema = canon.ir;
      for (let k = 1; k < comp.length; k++) mergedIr = merge(mergedIr, group[comp[k]!]!.ir);
      if (mergedIr.k !== "object") continue;
      (canon.ir as any).props = mergedIr.props;
      (canon.ir as any).total = mergedIr.total;
      for (let k = 1; k < comp.length; k++) {
        canonicalFor.set(group[comp[k]!]!.ir, canon.ir);
      }
      cycleSplice(canon.ir, canonicalFor);
    }
  }
  return canonicalFor;
}

function pushChildren(s: Schema, stack: Schema[], seen: Set<Schema>): void {
  switch (s.k) {
    case "object":
      for (const p of s.props.values()) {
        if (seen.has(p.schema)) continue;
        seen.add(p.schema);
        stack.push(p.schema);
      }
      return;
    case "array":
      if (!seen.has(s.item)) { seen.add(s.item); stack.push(s.item); }
      return;
    case "record":
      if (!seen.has(s.value)) { seen.add(s.value); stack.push(s.value); }
      return;
    case "union":
      for (const v of s.variants) {
        if (seen.has(v)) continue;
        seen.add(v);
        stack.push(v);
      }
      return;
  }
}

function rewriteIR(s: Schema, canonicalFor: Map<Schema, Schema>, seen: Set<Schema>): Schema {
  const replaced = (canonicalFor.get(s) as Schema | undefined) ?? s;
  if (seen.has(replaced)) return replaced;
  seen.add(replaced);
  switch (replaced.k) {
    case "array":
      replaced.item = rewriteIR(replaced.item, canonicalFor, seen);
      return replaced;
    case "record":
      replaced.value = rewriteIR(replaced.value, canonicalFor, seen);
      return replaced;
    case "object":
      for (const p of replaced.props.values()) p.schema = rewriteIR(p.schema, canonicalFor, seen);
      return replaced;
    case "union": {
      const newVariants: Schema[] = [];
      const refSeen = new Set<Schema>();
      for (const v of replaced.variants) {
        const w = rewriteIR(v, canonicalFor, seen);
        if (refSeen.has(w)) continue;
        refSeen.add(w);
        newVariants.push(w);
      }
      replaced.variants = newVariants;
      return replaced;
    }
    default:
      return replaced;
  }
}

function dedupeDecls(decls: DeclEntry[], rootRendered: string): { decls: DeclEntry[]; root: string } {
  let parsed = decls.map((d) => ({ name: d.name, body: d.body, doc: [...d.doc] }));

  while (true) {
    type Item = (typeof parsed)[number] & { prefix: string; hash: string };
    const items: Item[] = parsed.map((d) => {
      const m = d.name.match(/^(.*)_([0-9a-f]{8})$/);
      const prefix = m ? m[1]! : d.name;
      const hash = m ? m[2]! : "";
      return { ...d, prefix, hash };
    });

    const groups = new Map<string, Item[]>();
    for (const it of items) {
      if (!it.hash) continue;
      const key = `${it.prefix}\x00${it.body}`;
      const arr = groups.get(key) ?? [];
      arr.push(it);
      groups.set(key, arr);
    }

    const rename = new Map<string, string>();
    const mergedDocs = new Map<string, string[]>();
    let changed = false;
    for (const [, group] of groups) {
      if (group.length < 2) continue;
      changed = true;
      group.sort((a, b) => (a.hash < b.hash ? -1 : a.hash > b.hash ? 1 : 0));
      const canon = group[0]!;
      const allDocs: string[] = [];
      for (const m of group) for (const p of m.doc) if (!allDocs.includes(p)) allDocs.push(p);
      mergedDocs.set(canon.name, allDocs);
      for (let i = 1; i < group.length; i++) rename.set(group[i]!.name, canon.name);
    }
    if (!changed) {
      const out: DeclEntry[] = parsed.map((p) => {
        const orig = decls.find((d) => d.name === p.name);
        return {
          name: p.name,
          body: p.body,
          doc: p.doc,
          oldChain: orig?.oldChain ?? "",
          ir: orig?.ir ?? ({ k: "object", total: 0, props: new Map() } as any),
        };
      });
      return { decls: out, root: rootRendered };
    }

    const next: typeof parsed = [];
    for (const d of parsed) {
      if (rename.has(d.name)) continue;
      const newDoc = mergedDocs.get(d.name) ?? d.doc;
      next.push({ name: d.name, body: substAll(d.body, rename), doc: newDoc });
    }
    parsed = next;
    rootRendered = substAll(rootRendered, rename);
  }
}

function emit(root: Schema, rootName: string): string {
  // Phase 1a: hint-driven IR merging (DEDUP_HINTS).
  const hintCanonical = applyHintsOnIR(root, rootName);
  root = rewriteIR(root, hintCanonical, new Set());
  // Phase 1b: auto-detect recursive types (same tag literal under same field, ancestor-reachable).
  const autoCanonical = applyAutoRecursive(root, rootName);
  root = rewriteIR(root, autoCanonical, new Set());
  // Phase 2: render with IR-ref cache so shared canonical IRs emit once.
  const ctx: EmitCtx = { decls: [], used: new Set([rootName]), aliases: new Set(), objCache: new Map() };
  const renderedRaw = render(root, ctx, { old: rootName, pretty: "", field: "" });
  // Phase 3: structural dedupe of identical bodies sharing the same name prefix.
  const afterDedup = dedupeDecls(ctx.decls, renderedRaw);
  const renderedFinal = afterDedup.root;
  const declStrings = afterDedup.decls.map((d) => `${formatDoc(d.doc)}export type ${d.name} = ${d.body};`);
  const header = `// AUTO-GENERATED by scripts/extract-jsonl-schema.ts — do not edit by hand.\n`;
  const aliasDecls: string[] = [];
  for (const def of ALIASES) {
    if (ctx.aliases.has(def.name)) aliasDecls.push(`export type ${def.name} = string;`);
  }
  if (ctx.aliases.has("Blob")) aliasDecls.push("export type Blob = string;");
  const aliasBlock = aliasDecls.length ? aliasDecls.join("\n") + "\n\n" : "";
  return (
    header +
    aliasBlock +
    declStrings.join("\n\n") +
    (declStrings.length ? "\n\n" : "") +
    `export type ${rootName} = ${renderedFinal};\n`
  );
}

// ---------------- Main ----------------

async function main() {
  const args = parseArgs(process.argv.slice(2));
  USER_TAG_KEY = args.tag;
  const state = { schema: NEVER as Schema };

  if (args.files.length === 0) {
    // @ts-ignore - process.stdin works as ReadableStream-compatible in Bun via Readable.toWeb
    const { Readable } = await import("node:stream");
    const webStream = Readable.toWeb(process.stdin) as ReadableStream<Uint8Array>;
    await ingest(webStream, "<stdin>", state);
  } else {
    for (const f of args.files) {
      await ingest(openSource(f), f, state);
    }
  }

  const ts = emit(state.schema, args.name);
  if (args.out) {
    await Bun.write(args.out, ts);
    console.error(`wrote ${args.out}`);
  } else {
    process.stdout.write(ts);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
