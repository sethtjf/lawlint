// Golden-parity replay: rebuild every docx case in parity/golden/docx.jsonl
// and compare the output package entry-by-entry against the golden .docx.

import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { type Diagnostic, lint } from "@lawlint/core";
import { unzipSync } from "fflate";
import { type ReviseOptions, applyTrackedChanges, extract } from "../src/index";

const ROOT = join(import.meta.dir, "../../..");
const GOLDEN_DIR = join(ROOT, "parity/golden/docx");

const encoder = new TextEncoder();

interface GoldenCase {
  id: string;
  input: {
    source: { fixture: string };
    options: {
      author: string;
      date: string;
      includeAiRewrites: boolean;
      annotateFindings: boolean;
      diagnostics?: string;
    };
  };
  output: {
    extract?: string;
    diagnosticCount?: number;
    applied: number;
    annotated: number;
    skipped: number;
    sha256: string;
    outputFile: string;
  };
}

function sha256(b: Uint8Array): string {
  return createHash("sha256").update(b).digest("hex");
}

function fixturePath(fixture: string): string {
  return join(
    ROOT,
    fixture === "../../crates/lawlint-docx/tests/fixtures/sample.docx"
      ? "crates/lawlint-docx/tests/fixtures/sample.docx"
      : join("parity/fixtures", fixture),
  );
}

function entries(bytes: Uint8Array): [string, Uint8Array][] {
  return Object.entries(unzipSync(bytes)).filter(([n]) => !n.endsWith("/"));
}

/** Compare two packages: same entry names in order, same uncompressed bytes. */
function expectSamePackage(actual: Uint8Array, golden: Uint8Array, label: string): void {
  const a = entries(actual);
  const g = entries(golden);
  expect(
    a.map(([n]) => n),
    `${label}: entry name list`,
  ).toEqual(g.map(([n]) => n));
  for (let i = 0; i < a.length; i++) {
    const [name, ad] = a[i];
    const [, gd] = g[i];
    if (!Buffer.from(ad).equals(Buffer.from(gd))) {
      let off = 0;
      while (off < Math.min(ad.length, gd.length) && ad[off] === gd[0 + off]) off++;
      const ctx = (b: Uint8Array) =>
        new TextDecoder().decode(b.subarray(Math.max(0, off - 100), off + 100));
      throw new Error(
        `${label}: part ${name} differs at offset ${off}\nactual:   ...${ctx(ad)}...\ngolden: ...${ctx(gd)}...`,
      );
    }
  }
}

const cases = readFileSync(join(ROOT, "parity/golden/docx.jsonl"), "utf8")
  .split("\n")
  .filter((l) => l.trim() !== "")
  .map((l) => JSON.parse(l) as GoldenCase);

function manualDiagnostics(id: string, text: string): Diagnostic[] {
  // Mirrors docx_surface in parity_capture.rs: one diagnostic for the first
  // occurrence of the needle at its UTF-8 byte offset.
  const needle = id === "cross-run-fix" ? "cease and desist" : "delve";
  const textBytes = encoder.encode(text);
  const needleBytes = encoder.encode(needle);
  let start = -1;
  outer: for (let i = 0; i + needleBytes.length <= textBytes.length; i++) {
    for (let j = 0; j < needleBytes.length; j++) {
      if (textBytes[i + j] !== needleBytes[j]) continue outer;
    }
    start = i;
    break;
  }
  if (start === -1) return [];
  const end = start + needleBytes.length;
  return [
    {
      ruleId: "parity/manual-fix",
      severity: "warning",
      tier: "static",
      intent: "style",
      span: { start, end },
      message: "Parity tracked-change fixture.",
      line: 1,
      column: 1,
      excerpt: needle,
      suggestion: "examined",
      fix: {
        edits: [{ range: { start, end }, replacement: "examined" }],
        applicability: "machine-applicable",
      },
    },
  ];
}

describe("docx golden parity", () => {
  for (const c of cases) {
    test(c.id, () => {
      const bytes = readFileSync(fixturePath(c.input.source.fixture));
      const text = extract(bytes);
      const isLinted = c.input.options.diagnostics === "lint-default";

      const diagnostics = isLinted
        ? lint(text).diagnostics
        : manualDiagnostics(c.id.replace("docx-", ""), text);

      const opts: ReviseOptions = {
        author: c.input.options.author,
        date: c.input.options.date,
        includeAiRewrites: c.input.options.includeAiRewrites,
        annotateFindings: c.input.options.annotateFindings,
      };
      const revised = applyTrackedChanges(bytes, diagnostics, opts);

      if (c.output.extract !== undefined) {
        expect(text).toBe(c.output.extract);
      }
      if (c.output.diagnosticCount !== undefined) {
        expect(diagnostics.length).toBe(c.output.diagnosticCount);
      }
      expect(revised.applied).toBe(c.output.applied);
      expect(revised.annotated).toBe(c.output.annotated);
      expect(revised.skipped).toBe(c.output.skipped);

      const golden = readFileSync(join(ROOT, "parity", c.output.outputFile));

      // Nothing-applied fast path: the output must be the input bytes.
      if (c.output.sha256 === sha256(bytes)) {
        expect(revised.bytes).toEqual(bytes);
      } else {
        expectSamePackage(revised.bytes, golden, c.id);
      }

      // The output must always round-trip through extract.
      expect(() => extract(revised.bytes)).not.toThrow();
    });
  }
});
