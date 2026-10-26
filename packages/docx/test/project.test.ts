// Ported from crates/lawlint-docx/src/project.rs #[cfg(test)].

import { describe, expect, test } from "bun:test";
import { type Projection, project } from "../src/project";

const NS =
  '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>';

function proj(inner: string): Projection {
  return project(`${NS}${inner}</w:body></w:document>`);
}

function runSlice(p: Projection, i: number): string {
  // Runs' start/end are UTF-8 byte offsets; decode that byte range.
  const bytes = new TextEncoder().encode(p.text);
  return new TextDecoder().decode(bytes.subarray(p.runs[i].start, p.runs[i].end));
}

test("maps_run_byte_ranges_and_paragraph_breaks", () => {
  const p = proj("<w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t> world</w:t></w:r></w:p>");
  expect(p.text).toBe("Hello world\n\n");
  expect(p.runs.length).toBe(2);
  expect([p.runs[0].ordinal, p.runs[0].start, p.runs[0].end]).toEqual([0, 0, 5]);
  expect([p.runs[1].ordinal, p.runs[1].start, p.runs[1].end]).toEqual([1, 5, 11]);
  expect(runSlice(p, 1)).toBe(" world");
});

test("tabs_and_breaks_are_structural_gaps", () => {
  const p = proj("<w:p><w:r><w:tab/><w:t>a</w:t><w:br/><w:t>b</w:t></w:r></w:p>");
  expect(p.text).toBe("\ta\nb\n\n");
  expect(runSlice(p, 0)).toBe("a");
  expect(runSlice(p, 1)).toBe("b");
});

test("utf8_offsets_are_byte_indices", () => {
  const p = proj("<w:p><w:r><w:t>café</w:t></w:r><w:r><w:t>!</w:t></w:r></w:p>");
  // "café" is 5 bytes (é = 2), so the second run starts at byte 5.
  expect(p.runs[1].start).toBe(5);
  expect(runSlice(p, 1)).toBe("!");
});

test("empty_text_node_still_consumes_an_ordinal", () => {
  const p = proj("<w:p><w:r><w:t/></w:r><w:r><w:t>x</w:t></w:r></w:p>");
  expect(p.runs.length).toBe(2);
  expect(p.runs[0].start).toBe(p.runs[0].end); // empty span
  expect([p.runs[1].ordinal, p.runs[1].start]).toEqual([1, 0]);
});

test("xml_entities_are_unescaped", () => {
  const p = proj("<w:p><w:r><w:t>A &amp; B &lt; C</w:t></w:r></w:p>");
  expect(p.text).toBe("A & B < C\n\n");
  expect(runSlice(p, 0)).toBe("A & B < C");
});

test("existing_tracked_changes_contribute_their_ordinals", () => {
  // A run inside <w:ins> still holds a <w:t> and must keep counting so the
  // revise pass agrees with these ordinals.
  const p = proj(
    "<w:p><w:r><w:t>keep</w:t></w:r><w:ins><w:r><w:t>added</w:t></w:r></w:ins><w:r><w:t>tail</w:t></w:r></w:p>",
  );
  expect(p.text).toBe("keepaddedtail\n\n");
  expect(p.runs.length).toBe(3);
  expect(p.runs[2].ordinal).toBe(2);
  expect(runSlice(p, 2)).toBe("tail");
});
