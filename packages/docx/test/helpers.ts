// Namespace validation used across the docx tests: tokenize, keep a scope
// stack, resolve every prefixed element and attribute name, and fail on any
// unbound prefix — plus fail when a WordprocessingML local name resolves to
// any namespace other than W_NS (honoring the default namespace for
// unprefixed names).

import { local, parseXmlnsBindings, tokenize } from "../src/xml";

export const W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

const WORD_LOCALS = new Set([
  "r",
  "t",
  "delText",
  "ins",
  "del",
  "commentRangeStart",
  "commentRangeEnd",
  "commentReference",
  "comment",
]);

const ATTR_NAME = /([A-Za-z_][\w.-]*(?::[\w.-]+)?)\s*=/g;
const QUOTED = /"[^"]*"|'[^']*'/g;

export function assertWordNamespaces(xml: string): void {
  const scope: Map<string, string>[] = [new Map()];
  const lookup = (prefix: string): string | undefined => {
    for (let i = scope.length - 1; i >= 0; i--) {
      const uri = scope[i].get(prefix);
      if (uri !== undefined) return uri;
    }
    return undefined;
  };

  for (const ev of tokenize(xml)) {
    if (ev.kind === "start" || ev.kind === "empty") {
      scope.push(parseXmlnsBindings(ev.raw));
      const colon = ev.name.indexOf(":");
      const prefix = colon === -1 ? undefined : ev.name.slice(0, colon);
      const uri = lookup(prefix ?? "");
      if (prefix !== undefined && uri === undefined) {
        throw new Error(`unbound prefix "${prefix}" on <${ev.name}>`);
      }
      if (WORD_LOCALS.has(local(ev.name)) && (uri ?? "") !== W_NS) {
        throw new Error(`<${ev.name}> resolves to "${uri ?? ""}", expected ${W_NS}`);
      }
      // Attributes: ignore their values, then check every prefixed name is
      // bound (`xml:` is implicitly bound; xmlns declarations are not refs).
      const tagEnd = ev.raw.indexOf(ev.name) + ev.name.length;
      const attrs = ev.raw.slice(tagEnd).replace(QUOTED, '""');
      for (const m of attrs.matchAll(ATTR_NAME)) {
        const a = m[1];
        if (a === "xmlns" || a.startsWith("xmlns:")) continue;
        const ai = a.indexOf(":");
        if (ai !== -1) {
          const ap = a.slice(0, ai);
          if (ap === "xml") continue;
          if (lookup(ap) === undefined) {
            throw new Error(`unbound attribute prefix "${ap}" on <${ev.name}>`);
          }
        }
      }
      if (ev.kind === "empty") scope.pop();
    } else if (ev.kind === "end") {
      scope.pop();
    }
  }
}
