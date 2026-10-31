// Strip AI-assistant referral tags from hyperlink targets. Ported from
// crates/lawlint-docx/src/links.rs.
//
// ChatGPT appends `utm_source=chatgpt.com` to every link it formats. In a
// `.docx` the URL lives in a relationship part (`word/_rels/*.rels`), not in
// the text the engine lints, so `core/no-ai-tracking-links` never sees it.
// This pass rewrites the `Target` of each external hyperlink relationship
// directly. Word does not track edits to relationship targets, so these
// changes cannot be redlines; the caller reports them instead.

import { type Package, strToBytes } from "./pkg";
import { escapeAttr, resolveCharRef, resolvePredefinedEntity } from "./xml";

/**
 * `utm_source` values that mark a link as AI-formatted, compared
 * case-insensitively. Keep in step with `builtin/rules/no-ai-tracking-links.md`.
 */
export const AI_UTM_SOURCES: readonly string[] = ["chatgpt.com", "openai.com", "openai"];

const HYPERLINK_REL =
  "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

export interface TrackedLink {
  /** Relationship part the link lives in, e.g. `word/_rels/document.xml.rels`. */
  part: string;
  url: string;
  cleaned: string;
}

/** `url` without its AI `utm_source` parameters, or `undefined` when it has none. */
export function stripAiTracking(url: string): string | undefined {
  const hash = url.indexOf("#");
  const rest = hash === -1 ? url : url.slice(0, hash);
  const fragment = hash === -1 ? "" : url.slice(hash);
  const q = rest.indexOf("?");
  if (q === -1) return undefined;
  const params = rest.slice(q + 1).split("&");
  const kept = params.filter((param) => !isAiTag(param));
  if (kept.length === params.length) return undefined;
  let out = rest.slice(0, q);
  if (kept.some((p) => p !== "")) out += `?${kept.join("&")}`;
  return out + fragment;
}

function isAiTag(param: string): boolean {
  const eq = param.indexOf("=");
  if (eq === -1) return false;
  const key = param.slice(0, eq);
  const value = param.slice(eq + 1).toLowerCase();
  return key.toLowerCase() === "utm_source" && AI_UTM_SOURCES.includes(value);
}

/** Every AI-tagged hyperlink target in the package, in part order. */
export function find(pkg: Package): TrackedLink[] {
  const found: TrackedLink[] = [];
  for (const part of relsParts(pkg)) {
    for (const target of hyperlinkTargets(pkg.getStr(part) ?? "")) {
      const cleaned = stripAiTracking(target.value);
      if (cleaned !== undefined) found.push({ part, url: target.value, cleaned });
    }
  }
  return found;
}

/** Rewrite every AI-tagged hyperlink target in place; returns the links changed. */
export function strip(pkg: Package): TrackedLink[] {
  const changed: TrackedLink[] = [];
  for (const part of relsParts(pkg)) {
    const xml = pkg.getStr(part) ?? "";
    let out = "";
    let cursor = 0;
    for (const target of hyperlinkTargets(xml)) {
      const cleaned = stripAiTracking(target.value);
      if (cleaned === undefined) continue;
      out += xml.slice(cursor, target.start) + escapeAttr(cleaned);
      cursor = target.end;
      changed.push({ part, url: target.value, cleaned });
    }
    if (cursor > 0) pkg.set(part, strToBytes(out + xml.slice(cursor)));
  }
  return changed;
}

function relsParts(pkg: Package): string[] {
  return pkg.parts
    .map((p) => p.name)
    .filter((name) => name.startsWith("word/_rels/") && name.endsWith(".rels"));
}

interface Target {
  value: string;
  start: number;
  end: number;
}

function hyperlinkTargets(xml: string): Target[] {
  const targets: Target[] = [];
  let from = 0;
  for (;;) {
    const open = xml.indexOf("<Relationship", from);
    if (open === -1) break;
    const close = xml.indexOf(">", open);
    if (close === -1) break;
    from = close + 1;
    const tag = xml.slice(open, close);
    // `<Relationships` is the root element, not a relationship.
    if (!/^\s/.test(tag.slice("<Relationship".length))) continue;
    const isHyperlink = attr(tag, "Type")?.raw === HYPERLINK_REL;
    const external = attr(tag, "TargetMode")?.raw === "External";
    if (!(isHyperlink && external)) continue;
    const target = attr(tag, "Target");
    if (target) {
      targets.push({
        value: unescapeAttr(target.raw),
        start: open + target.start,
        end: open + target.end,
      });
    }
  }
  return targets;
}

function attr(tag: string, name: string): { start: number; end: number; raw: string } | undefined {
  let from = 0;
  for (;;) {
    const at = tag.indexOf(name, from);
    if (at === -1) return undefined;
    from = at + name.length;
    const preceded = at > 0 && /\s/.test(tag[at - 1] ?? "");
    const rest = tag.slice(from).trimStart();
    if (!preceded || !rest.startsWith("=")) continue;
    const afterEq = rest.slice(1).trimStart();
    const quote = afterEq[0];
    if (quote !== '"' && quote !== "'") return undefined;
    const start = tag.length - afterEq.length + 1;
    const len = tag.slice(start).indexOf(quote);
    if (len === -1) return undefined;
    return { start, end: start + len, raw: tag.slice(start, start + len) };
  }
}

/** quick-xml `unescape`; an unknown entity leaves the raw value, as in Rust. */
function unescapeAttr(raw: string): string {
  let ok = true;
  const out = raw.replace(/&([^;&]*);/g, (whole, name: string) => {
    const ch = resolvePredefinedEntity(name) ?? resolveCharRef(name);
    if (ch === undefined) ok = false;
    return ch ?? whole;
  });
  return ok ? out : raw;
}
