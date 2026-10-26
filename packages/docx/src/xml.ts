// Minimal XML tokenizer used in place of quick-xml's Reader. Events carry
// their raw source slice so passthrough emission is byte-identical to the
// input, matching what quick-xml 0.41's Reader→Writer round-trip produces.

import { DocxError } from "./index";

export type XmlEvent =
  | { kind: "start"; name: string; raw: string }
  | { kind: "end"; name: string; raw: string }
  | { kind: "empty"; name: string; raw: string }
  | { kind: "text"; raw: string }
  | { kind: "generalRef"; name: string; raw: string }
  | { kind: "cdata"; raw: string; content: string }
  | { kind: "comment"; raw: string }
  | { kind: "pi"; raw: string }
  | { kind: "decl"; raw: string }
  | { kind: "doctype"; raw: string };

/** Local name of a qualified tag, like quick-xml's `local_name()`. */
export function local(name: string): string {
  const i = name.lastIndexOf(":");
  return i === -1 ? name : name.slice(i + 1);
}

const malformed = (msg: string) => new DocxError("malformed", msg);

function scanTag(xml: string, from: number): number {
  // Index one past the `>` that ends the tag starting at `from`, honoring
  // quoted attribute values (a `>` may legally appear inside them).
  let i = from;
  let quote = "";
  while (i < xml.length) {
    const c = xml[i];
    if (quote) {
      if (c === quote) quote = "";
    } else if (c === '"' || c === "'") {
      quote = c;
    } else if (c === ">") {
      return i + 1;
    }
    i++;
  }
  throw malformed("unclosed tag");
}

const NAME_END = new Set([" ", "\t", "\r", "\n", "/", ">", "?"]);

function tagName(xml: string, from: number): string {
  let i = from;
  while (i < xml.length && !NAME_END.has(xml[i])) i++;
  return xml.slice(from, i);
}

const REF = /^&(?:#x[0-9a-fA-F]+|#\d+|[A-Za-z_:][\w.-]*);/;

export function tokenize(xml: string): XmlEvent[] {
  const events: XmlEvent[] = [];
  const stack: string[] = [];
  let i = 0;
  const n = xml.length;

  while (i < n) {
    const c = xml[i];
    if (c === "<") {
      if (xml.startsWith("<!--", i)) {
        const end = xml.indexOf("-->", i + 4);
        if (end === -1) throw malformed("unclosed comment");
        events.push({ kind: "comment", raw: xml.slice(i, end + 3) });
        i = end + 3;
        continue;
      }
      if (xml.startsWith("<![CDATA[", i)) {
        const end = xml.indexOf("]]>", i + 9);
        if (end === -1) throw malformed("unclosed CDATA");
        events.push({
          kind: "cdata",
          raw: xml.slice(i, end + 3),
          content: xml.slice(i + 9, end),
        });
        i = end + 3;
        continue;
      }
      if (xml.startsWith("<?", i)) {
        const end = xml.indexOf("?>", i + 2);
        if (end === -1) throw malformed("unclosed processing instruction");
        const raw = xml.slice(i, end + 2);
        const name = tagName(xml, i + 2).toLowerCase();
        events.push({ kind: name === "xml" ? "decl" : "pi", raw });
        i = end + 2;
        continue;
      }
      if (xml.startsWith("<!", i)) {
        const end = scanTag(xml, i + 2);
        events.push({ kind: "doctype", raw: xml.slice(i, end) });
        i = end;
        continue;
      }
      if (xml.startsWith("</", i)) {
        const name = tagName(xml, i + 2);
        const end = scanTag(xml, i + 2);
        const expected = stack.pop();
        if (expected !== name) {
          throw malformed(
            expected === undefined
              ? `ill-formed document: unexpected closing tag </${name}>`
              : `ill-formed document: expected </${expected}>, but </${name}> was found`,
          );
        }
        events.push({ kind: "end", name, raw: xml.slice(i, end) });
        i = end;
        continue;
      }
      const name = tagName(xml, i + 1);
      if (!name) throw malformed("ill-formed tag");
      const end = scanTag(xml, i + 1);
      const raw = xml.slice(i, end);
      if (raw.endsWith("/>")) {
        events.push({ kind: "empty", name, raw });
      } else {
        stack.push(name);
        events.push({ kind: "start", name, raw });
      }
      i = end;
      continue;
    }
    if (c === "&") {
      const m = REF.exec(xml.slice(i, i + 64));
      if (!m) throw malformed(`illegal character '&' at offset ${i}`);
      const raw = m[0];
      events.push({ kind: "generalRef", name: raw.slice(1, -1), raw });
      i += raw.length;
      continue;
    }
    let j = xml.indexOf("<", i);
    const amp = xml.indexOf("&", i);
    if (j === -1 || (amp !== -1 && amp < j)) j = amp;
    if (j === -1) j = n;
    events.push({ kind: "text", raw: xml.slice(i, j) });
    i = j;
  }

  if (stack.length > 0) {
    throw malformed(`ill-formed document: unclosed tag <${stack[stack.length - 1]}>`);
  }
  return events;
}

const PREDEFINED: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
};

export function resolvePredefinedEntity(name: string): string | undefined {
  return PREDEFINED[name];
}

/** quick-xml's `resolve_char_ref`: `&#N;`/`&#xH;` → the char, else undefined. */
export function resolveCharRef(name: string): string | undefined {
  let cp: number;
  if (name.startsWith("#x") || name.startsWith("#X")) {
    cp = Number.parseInt(name.slice(2), 16);
  } else if (name.startsWith("#")) {
    cp = Number.parseInt(name.slice(1), 10);
  } else {
    return undefined;
  }
  if (!Number.isFinite(cp) || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) {
    return undefined;
  }
  return String.fromCodePoint(cp);
}

/** quick-xml's `BytesText::new` escaping (verified: all five characters). */
export function escapeText(s: string): string {
  let out = "";
  for (const c of s) {
    switch (c) {
      case "&":
        out += "&amp;";
        break;
      case "<":
        out += "&lt;";
        break;
      case ">":
        out += "&gt;";
        break;
      case "'":
        out += "&apos;";
        break;
      case '"':
        out += "&quot;";
        break;
      default:
        out += c;
    }
  }
  return out;
}

/** quick-xml's `push_attribute` escaping inside double quotes. */
export function escapeAttr(s: string): string {
  return escapeText(s);
}

/**
 * Rust `replacen(needle, replacement, 1)`: first occurrence, replacement is
 * literal (String.replace would interpret `$&`, `$'`, `$$`, …).
 */
export function replaceFirst(haystack: string, needle: string, replacement: string): string {
  const at = haystack.indexOf(needle);
  if (at === -1) return haystack;
  return haystack.slice(0, at) + replacement + haystack.slice(at + needle.length);
}

const XMLNS_ATTR = /xmlns(?::([\w.-]+))?\s*=\s*(?:"([^"]*)"|'([^']*)')/g;

/** The xmlns prefix→URI bindings declared on one tag's raw slice ("" = default). */
export function parseXmlnsBindings(raw: string): Map<string, string> {
  const b = new Map<string, string>();
  for (const m of raw.matchAll(XMLNS_ATTR)) b.set(m[1] ?? "", (m[2] ?? m[3]) || "");
  return b;
}

/**
 * Choose a prefix for `uri` given a scope stack (outermost first; the
 * innermost binding of each prefix wins). If some prefix's innermost binding
 * is `uri`, reuse it — preferring `w`, then others innermost-first — with no
 * declaration. Otherwise pick a prefix unbound at every level (`w`, else the
 * first free of `lw`, `lw1`, …) and return the ` xmlns:P="uri"` declaration
 * to emit on generated top-level elements.
 */
export function choosePrefix(
  scopes: readonly Map<string, string>[],
  uri: string,
): { prefix: string; decl: string } {
  const seen = new Set<string>();
  const qualifying: string[] = [];
  for (let i = scopes.length - 1; i >= 0; i--) {
    for (const [p, u] of scopes[i]) {
      if (p === "" || seen.has(p)) continue;
      seen.add(p);
      if (u === uri) qualifying.push(p);
    }
  }
  const bound = qualifying.includes("w") ? "w" : qualifying[0];
  if (bound !== undefined) return { prefix: bound, decl: "" };
  let prefix = "w";
  if (seen.has(prefix)) {
    prefix = "lw";
    for (let i = 1; seen.has(prefix); i++) prefix = `lw${i}`;
  }
  return { prefix, decl: ` xmlns:${prefix}="${uri}"` };
}

/** The stack of live xmlns bindings while streaming a document's events. */
export class NsScope {
  private stack: Map<string, string>[] = [new Map()];

  /** Push the bindings declared on a start tag's raw slice. */
  push(raw: string): void {
    this.stack.push(parseXmlnsBindings(raw));
  }

  pop(): void {
    this.stack.pop();
  }

  /** Resolve `uri` in the current scope; see `choosePrefix`. */
  resolve(uri: string): { prefix: string; decl: string } {
    return choosePrefix(this.stack, uri);
  }
}
