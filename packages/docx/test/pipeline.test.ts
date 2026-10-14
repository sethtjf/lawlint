// Ported from crates/lawlint-docx/tests/pipeline.rs.

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { lint } from "@lawlint/core";
import { unzipSync } from "fflate";
import { applyTrackedChanges, defaultReviseOptions, extract } from "../src/index";

const FIXTURE = readFileSync(
  join(import.meta.dir, "../../../crates/lawlint-docx/tests/fixtures/sample.docx"),
);

function part(bytes: Uint8Array, name: string): Uint8Array | undefined {
  return unzipSync(bytes)[name];
}

function names(bytes: Uint8Array): string[] {
  return Object.keys(unzipSync(bytes)).filter((n) => !n.endsWith("/"));
}

test("extracts_projected_text", () => {
  const text = extract(FIXTURE);
  expect(text).toContain("Motion to Dismiss");
  expect(text).toContain("pursuant to the schedule");
  // Paragraph separators keep block segmentation meaningful.
  expect(text).toContain("\n\n");
});

test("writes_tracked_changes_and_preserves_all_other_parts", () => {
  const text = extract(FIXTURE);
  const result = lint(text);
  expect(
    result.diagnostics.some((d) => d.fix !== undefined),
    "fixture should surface at least one machine-applicable fix",
  ).toBe(true);

  const opts = {
    author: "unit-test",
    date: "2020-01-01T00:00:00Z",
    includeAiRewrites: true,
    annotateFindings: true,
  };
  const out = applyTrackedChanges(FIXTURE, result.diagnostics, opts);
  expect(out.applied).toBeGreaterThanOrEqual(1);

  const doc = new TextDecoder().decode(part(out.bytes, "word/document.xml"));
  expect(doc).toContain("<w:ins");
  expect(doc).toContain("<w:del");
  expect(doc).toContain("<w:commentReference");

  const comments = new TextDecoder().decode(part(out.bytes, "word/comments.xml"));
  expect(comments).toContain('w:author="unit-test"');

  // comments.xml is declared and related.
  const ct = new TextDecoder().decode(part(out.bytes, "[Content_Types].xml"));
  expect(ct).toContain("/word/comments.xml");
  const rels = new TextDecoder().decode(part(out.bytes, "word/_rels/document.xml.rels"));
  expect(rels).toContain("comments.xml");

  // The only new part is comments.xml; every original part survives.
  const before = new Set(names(FIXTURE));
  const after = new Set(names(out.bytes));
  for (const name of before) {
    expect(after.has(name), `dropped original part: ${name}`).toBe(true);
  }
  const added = [...after].filter((n) => !before.has(n));
  expect(added).toEqual(["word/comments.xml"]);

  // Unrelated parts are byte-identical (no lossy round-trip).
  for (const name of ["word/theme/theme1.xml", "word/styles.xml", "docProps/thumbnail.jpeg"]) {
    expect(part(out.bytes, name), `part changed unexpectedly: ${name}`).toEqual(
      part(FIXTURE, name),
    );
  }
});

test("no_applicable_fixes_returns_input_unchanged", () => {
  // A doc with no diagnostics: apply is a no-op passthrough.
  const out = applyTrackedChanges(FIXTURE, [], defaultReviseOptions());
  expect(out.applied).toBe(0);
  expect(out.bytes).toEqual(FIXTURE);
});
