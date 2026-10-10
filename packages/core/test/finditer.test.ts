// find_iter parity — expectations derived from real Rust regex::Regex::find_iter
// (a throwaway bin was run and the outputs hard-coded here), plus the \z
// regression and the density capture-span fix.

import { describe, expect, test } from "bun:test";
import { lintWith } from "../src/index";
import { compileRustRegex, rustFindIter } from "../src/regex";
import { RuleSet } from "../src/registry";

function spans(pattern: string, text: string): [number, number][] {
  return [...rustFindIter(compileRustRegex(pattern), text)].map((m) => [
    m.index,
    m.index + m[0].length,
  ]);
}

// Each case: pattern, text, and the exact (start,end) list Rust produced.
const RUST_CASES: [string, string, [number, number][]][] = [
  [
    "(?:^|delve)",
    "we delve into delve twice delve",
    [
      [0, 0],
      [3, 8],
      [14, 19],
      [26, 31],
    ],
  ],
  [
    "a*",
    "xaxx",
    [
      [0, 0],
      [1, 2],
      [3, 3],
      [4, 4],
    ],
  ],
  [
    "\\b",
    "xaxx",
    [
      [0, 0],
      [4, 4],
    ],
  ],
  [
    "x*",
    "xaxx",
    [
      [0, 1],
      [2, 4],
    ],
  ],
  [
    "a*",
    "🦀a🦀",
    [
      [0, 0],
      [2, 3],
      [5, 5],
    ],
  ], // UTF-16 indices; Rust gave byte offsets (0,0),(4,5),(9,9)
  [
    "",
    "ab",
    [
      [0, 0],
      [1, 1],
      [2, 2],
    ],
  ],
  [
    "x|",
    "xyx",
    [
      [0, 1],
      [2, 3],
    ],
  ],
  [
    "(?:^|delve)",
    "delve delve",
    [
      [0, 0],
      [6, 11],
    ],
  ],
  [
    "x+",
    "xaxx",
    [
      [0, 1],
      [2, 4],
    ],
  ],
  // Not portable: `(?:^|delve)*` on "delve x" is (0,0),(1,1)..(7,7) in Rust
  // — Rust ends the * repeat at the empty ^ iteration — but JS takes the
  // non-empty alternative and gives (0,5). That is a match-semantics
  // difference no find_iter shim can fix; no rule uses a nullable-
  // alternation repeat.
];

describe("rustFindIter matches Rust find_iter", () => {
  for (const [pat, text, expected] of RUST_CASES) {
    test(`${JSON.stringify(pat)} on ${JSON.stringify(text)}`, () => {
      expect(spans(pat, text)).toEqual(expected);
    });
  }
});

test("empty matches no longer stop iteration", () => {
  // Regression: the old zero-width guard broke at the first empty match, so
  // `(?:^|delve)` reported only (0,0) and missed every real `delve`.
  const got = spans("(?:^|delve)", "a delve and delve");
  expect(got).toEqual([
    [0, 0],
    [2, 7],
    [12, 17],
  ]);
});

test("\\z does not match before a trailing newline", () => {
  // Rust \z is end-of-input only; a plain JS $ would match "foo\n" at 0..3.
  expect(compileRustRegex(String.raw`foo\z`).test("foo\n")).toBe(false);
  expect(compileRustRegex(String.raw`foo\z`).test("foo")).toBe(true);
});

test("density first span uses capture-group indices, not a text search", () => {
  // Group 1's text `x` also appears earlier inside the match `xxx`, so an
  // indexOf fallback would highlight 0..1 instead of 2..3.
  const yaml = `---\nid: dens\nengine: density\nthreshold: 0\npatterns: ["x{2,}(x)"]\n---\n`;
  const rs = RuleSet.fromSources("t", [["d.md", yaml]]);
  const result = lintWith("axxx", {}, rs);
  expect(result.diagnostics.length).toBe(1);
  const span = result.diagnostics[0].span;
  // The match is `xxx` at 1..4; group 1 is its last x at byte 3..4. An
  // indexOf(m[1]) fallback would report 1..2, the first x in the match.
  expect("axxx".slice(span.start, span.end)).toBe("x");
  expect(span).toEqual({ start: 3, end: 4 });
});
