// Document tree + parsing. Every range is a UTF-8 byte offset.

import { markdownBlocks, plainBlocks } from "./markdown";
import { splitSentences } from "./segment";
import type { TextRange } from "./types";
import { Src } from "./utf8";

// Serde names are all lowercase in the JSON output.
export type BlockKind = "paragraph" | "heading" | "listitem" | "blockquote" | "codeblock";

export interface Block {
  kind: BlockKind;
  range: TextRange;
  sentences: Sentence[];
}

export interface Sentence {
  range: TextRange;
  tokens: Token[];
  isCitation: boolean;
}

export interface Token {
  range: TextRange;
  kind: "word" | "number" | "punct";
}

export interface Document {
  blocks: Block[];
  /** Decoded source text (the full input). */
  source: Src;
}

export function parse(source: Src | string, markdown: boolean): Document {
  const src = typeof source === "string" ? new Src(source) : source;
  const raw = markdown ? markdownBlocks(src) : plainBlocks(src);
  const blocks: Block[] = raw.map(([kind, range]) => {
    const sentences =
      kind === "codeblock"
        ? []
        : splitSentences(src.slice(range.start, range.end), range.start, src.text);
    return { kind, range, sentences };
  });
  return { blocks, source: src };
}
