// Markdown block structure via micromark + mdast (pulldown-cmark parity).
// Ranges are UTF-8 byte offsets into the source. Code blocks keep their full
// container range; text blocks are built from inline node ranges so markers
// stay outside the linted range.

import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmTableFromMarkdown } from "mdast-util-gfm-table";
import { gfmTable } from "micromark-extension-gfm-table";
// Minimal mdast node shape (avoids a runtime import of the types package).
interface RootContent {
  type: string;
  children?: RootContent[];
  spread?: boolean;
  position?: { start: { offset: number }; end: { offset: number } };
}
interface Parent {
  children?: RootContent[];
}
import type { BlockKind } from "./document";
import type { TextRange } from "./types";
import { type Src, wsTrim, wsTrimEnd, wsTrimStart } from "./utf8";

type Container = "item" | "quote";

function classify(containers: Container[]): BlockKind {
  const last = containers[containers.length - 1];
  if (last === "item") return "listitem";
  if (last === "quote") return "blockquote";
  return "paragraph";
}

type Open = { kind: BlockKind; start: number; end: number } | null;

function flush(current: { v: Open }, blocks: [BlockKind, TextRange][]) {
  if (current.v && current.v.end > current.v.start) {
    blocks.push([current.v.kind, { start: current.v.start, end: current.v.end }]);
  }
  current.v = null;
}

function extend(current: { v: Open }, containers: Container[], range: TextRange) {
  if (current.v) {
    current.v.start = Math.min(current.v.start, range.start);
    current.v.end = Math.max(current.v.end, range.end);
  } else {
    current.v = { kind: classify(containers), start: range.start, end: range.end };
  }
}

// Inline node types that accumulate into the current text block.
const INLINE = new Set([
  "text",
  "inlineCode",
  "html",
  "break",
  "emphasis",
  "strong",
  "delete",
  "link",
  "linkReference",
  "image",
  "imageReference",
  "footnoteReference",
]);

function byteRange(
  src: Src,
  node: { position?: { start: { offset: number }; end: { offset: number } } },
): TextRange | null {
  const p = node.position;
  if (!p) return null;
  return { start: src.byte(p.start.offset), end: src.byte(p.end.offset) };
}

/** Walk an mdast node's children. */
function kids(node: RootContent | Parent): RootContent[] {
  return node.children ?? [];
}

/** Emit pseudo-events for an inline node subtree (positions cover the element). */
function inlineExtend(node: RootContent, src: Src, current: { v: Open }, containers: Container[]) {
  if (INLINE.has(node.type)) {
    const r = byteRange(src, node);
    if (r) extend(current, containers, r);
    return;
  }
  for (const k of kids(node)) inlineExtend(k, src, current, containers);
}

function visit(
  node: RootContent,
  src: Src,
  blocks: [BlockKind, TextRange][],
  current: { v: Open },
  containers: Container[],
  inTightItem: boolean,
) {
  switch (node.type) {
    case "heading": {
      flush(current, blocks);
      current.v = { kind: "heading", start: Number.MAX_SAFE_INTEGER, end: 0 };
      for (const k of kids(node)) inlineExtend(k, src, current, containers);
      flush(current, blocks);
      return;
    }
    case "paragraph": {
      // pulldown-cmark emits no Paragraph for tight list items: the inline
      // run opens an implicit block classified by the container context.
      flush(current, blocks);
      current.v = { kind: classify(containers), start: Number.MAX_SAFE_INTEGER, end: 0 };
      for (const k of kids(node)) inlineExtend(k, src, current, containers);
      flush(current, blocks);
      return;
    }
    case "list": {
      flush(current, blocks);
      for (const k of kids(node))
        visit(k, src, blocks, current, containers, (node as { spread?: boolean }).spread === false);
      flush(current, blocks);
      return;
    }
    case "listItem": {
      flush(current, blocks);
      containers.push("item");
      for (const k of kids(node)) {
        if (k.type === "paragraph" && inTightItem) {
          // Tight item: no Paragraph event — inline content opens an
          // implicit block classified by the container stack.
          for (const kk of kids(k)) inlineExtend(kk, src, current, containers);
          flush(current, blocks);
        } else {
          visit(k, src, blocks, current, containers, inTightItem);
        }
      }
      flush(current, blocks);
      containers.pop();
      return;
    }
    case "blockquote": {
      flush(current, blocks);
      containers.push("quote");
      for (const k of kids(node)) visit(k, src, blocks, current, containers, inTightItem);
      flush(current, blocks);
      containers.pop();
      return;
    }
    case "code": {
      flush(current, blocks);
      const r = byteRange(src, node);
      if (r) {
        // pulldown-cmark starts an *indented* code block's range at the
        // first content character, past the leading indentation.
        const fenced =
          src.text.startsWith("```", src.u16(r.start)) ||
          src.text.startsWith("~~~", src.u16(r.start));
        if (!fenced) {
          let u16 = src.u16(r.start);
          while (u16 < src.text.length && (src.text[u16] === " " || src.text[u16] === "\t"))
            u16 += 1;
          r.start = src.byte(u16);
          // pulldown includes the trailing line terminator.
          const e16 = src.u16(r.end);
          if (e16 < src.text.length && src.text[e16] === "\n") r.end = src.byte(e16 + 1);
        }
        blocks.push(["codeblock", r]);
      }
      return;
    }
    case "table": {
      // GFM tables are not prose: swallowed whole, like HTML blocks.
      flush(current, blocks);
      return;
    }
    case "html":
    case "thematicBreak":
    case "yaml":
    case "definition":
    case "footnoteDefinition": {
      flush(current, blocks);
      return;
    }
    default: {
      inlineExtend(node, src, current, containers);
      return;
    }
  }
}

export function markdownBlocks(src: Src): [BlockKind, TextRange][] {
  const tree = fromMarkdown(src.text, {
    extensions: [gfmTable()],
    mdastExtensions: [gfmTableFromMarkdown()],
  });
  const blocks: [BlockKind, TextRange][] = [];
  const current: { v: Open } = { v: null };
  const containers: Container[] = [];
  for (const node of tree.children)
    visit(node as unknown as RootContent, src, blocks, current, containers, false);
  flush(current, blocks);
  return blocks;
}

/** Plain-text fallback: paragraphs split on blank lines, trimmed. */
export function plainBlocks(src: Src): [BlockKind, TextRange][] {
  const blocks: [BlockKind, TextRange][] = [];
  const text = src.text;
  let pos = 0; // u16 pos of line start
  let blockStart: number | null = null; // byte
  let blockEnd = 0; // byte
  // Iterate lines with their terminators, like split_inclusive('\n').
  let idx = 0;
  while (idx <= text.length) {
    const nl = text.indexOf("\n", idx);
    const lineEnd = nl === -1 ? text.length : nl + 1;
    if (nl === -1 && idx === text.length) break;
    const line = text.slice(idx, lineEnd);
    const lineStart = idx;
    idx = lineEnd;
    pos = lineStart; // u16
    if (wsTrim(line).length === 0) {
      if (blockStart !== null) {
        blocks.push(["paragraph", { start: blockStart, end: blockEnd }]);
        blockStart = null;
      }
      continue;
    }
    const contentStart = lineStart + (line.length - wsTrimStart(line).length);
    const contentEnd = lineStart + wsTrimEnd(line).length;
    if (blockStart === null) blockStart = src.byte(contentStart);
    blockEnd = src.byte(contentEnd);
  }
  void pos;
  if (blockStart !== null) {
    blocks.push(["paragraph", { start: blockStart, end: blockEnd }]);
  }
  return blocks;
}
