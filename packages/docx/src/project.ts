// Project `word/document.xml` into a flat plain-text string that the lawlint
// engine can lint, recording where each `<w:t>` run text lands in that
// string. All recorded ranges are UTF-8 byte offsets.
//
// The projection is deterministic: the tracked-change writer re-runs the
// exact same builder so diagnostic byte offsets line up with the runs they
// came from.

import { DocxError } from "./index";
import { type XmlEvent, local, resolveCharRef, resolvePredefinedEntity, tokenize } from "./xml";

/** One `<w:t>` text node and the byte range it occupies in the projected text. */
export interface RunText {
  /** Document-order index of the `<w:t>` element (0-based). */
  ordinal: number;
  start: number;
  end: number;
}

export interface Projection {
  text: string;
  /** Byte length of `text` — runs' start/end index into this. */
  byteLen: number;
  runs: RunText[];
}

const encoder = new TextEncoder();
const lossy = new TextDecoder();

export function project(documentXml: string): Projection {
  const events = tokenize(documentXml);

  let text = "";
  let byteLen = 0;
  const runs: RunText[] = [];
  let ordinal = 0;

  // State while inside a <w:t>…</w:t>.
  let inT = false;
  let tStart = 0;

  const push = (chunk: string) => {
    text += chunk;
    byteLen += encoder.encode(chunk).length;
  };

  for (const ev of events) {
    switch (ev.kind) {
      case "start":
        if (local(ev.name) === "t") {
          inT = true;
          tStart = byteLen;
        }
        break;
      case "end":
        if (local(ev.name) === "t") {
          if (inT) {
            runs.push({ ordinal, start: tStart, end: byteLen });
            ordinal++;
            inT = false;
          }
        } else if (local(ev.name) === "p") {
          push("\n\n");
        }
        break;
      case "empty":
        switch (local(ev.name)) {
          case "tab":
            push("\t");
            break;
          case "br":
          case "cr":
            push("\n");
            break;
          // An empty <w:t/> still consumes an ordinal so counts align.
          case "t":
            runs.push({ ordinal, start: byteLen, end: byteLen });
            ordinal++;
            break;
        }
        break;
      case "text":
        // The tokenizer emits entity refs as separate generalRef events, so
        // this raw text is already unescaped.
        if (inT) push(ev.raw);
        break;
      case "generalRef":
        if (inT) {
          const ch = resolveCharRef(ev.name);
          if (ch !== undefined) {
            push(ch);
          } else {
            const value = resolvePredefinedEntity(ev.name);
            if (value !== undefined) {
              push(value);
            } else {
              throw new DocxError("malformed", `unsupported XML entity &${ev.name};`);
            }
          }
        }
        break;
      case "cdata":
        if (inT) push(lossy.decode(encoder.encode(ev.content)));
        break;
    }
  }

  return { text, byteLen, runs };
}
