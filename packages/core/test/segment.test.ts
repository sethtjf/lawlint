// Ported from crates/lawlint-core/src/segment.rs #[cfg(test)].

import { describe, expect, test } from "bun:test";
import type { Sentence } from "../src/document";
import { isCitationSentence, splitSentences, tokenize } from "../src/segment";
import { Src } from "../src/utf8";
import { wsTrim } from "../src/utf8";

function sentences(text: string): Sentence[] {
  return splitSentences(text, 0, text);
}
function sentenceTexts(text: string): string[] {
  const src = new Src(text);
  return sentences(text).map((s) => src.slice(s.range.start, s.range.end));
}
function slice(text: string, r: { start: number; end: number }): string {
  return new Src(text).slice(r.start, r.end);
}

// ---- §12 segmentation corpus -------------------------------------------

test("canonical_roe_v_wade", () => {
  const text = "See Roe v. Wade, 410 U.S. 113 (1973). The court held that this applies.";
  const s = sentences(text);
  expect(s.length).toBe(2);
  expect(s[0].isCitation).toBe(true);
  expect(s[1].isCitation).toBe(false);
  expect(slice(text, s[0].range)).toBe("See Roe v. Wade, 410 U.S. 113 (1973).");
  expect(slice(text, s[1].range)).toBe("The court held that this applies.");
});

describe("segmentation_corpus", () => {
  const corpus: [string, number][] = [
    ["See Roe v. Wade, 410 U.S. 113 (1973). The court held that this applies.", 2],
    ["Fed. R. Civ. P. 12(b)(6) governs motions to dismiss. The standard is well known.", 2],
    ["Mr. Smith met Dr. Jones on Jan. 5, 2020. They discussed the case.", 2],
    ["Id. at 5. The reasoning was sound.", 2],
    ["1. The first claim fails.\n2. The second claim also fails.", 2],
    ["(a). Each party shall bear its own costs.", 1],
    ["The court, per the 4th. Circuit, affirmed. We agree.", 2],
    ["See Miranda v. Arizona, 384 U.S. 436, 444 (1966); see also Dickerson v. United States.", 1],
    ["J. Smith argued the case. The court agreed with J. Smith.", 2],
    ["E.g., Brown v. Board of Educ., 347 U.S. 483 (1954). This is settled law.", 2],
    ["Was the verdict just? Absolutely not! The appeal was denied.", 3],
    ["See U.S. v. Nixon, 418 U.S. 683 (1974). Executive privilege is not absolute.", 2],
    ["Cf. Katz v. United States, 389 U.S. 347 (1967). But see Olmstead.", 2],
    ["The agreement was signed by Acme Co. Ltd. It became effective immediately.", 1],
    ["Filing is due Oct. 1, 2024. Do not miss the deadline.", 2],
    ["The motion was denied. No. 12-345 was closed.", 2],
    ["Plaintiff relies on Fed. R. Evid. 702. Daubert governs admissibility.", 2],
    ["Accord Smith v. Jones, 12 F.3d 88 (2d Cir. 1993). The rule is settled.", 2],
    [
      "The seminal case is Brown, 347 U.S. 483, 495 (1954), which changed everything. Its impact endures.",
      2,
    ],
    ["Section 1983 provides a remedy. See 42 U.S.C. § 1988 for fees.", 2],
    ["It was—frankly—wrong. The court said so.", 2],
    ["The café’s brief was filed. “Justice delayed is justice denied.” The motion followed.", 3],
  ];
  for (const [text, expected] of corpus) {
    test(`count ${expected}: ${text.slice(0, 40)}`, () => {
      const got = sentences(text);
      expect(got.length).toBe(expected);
      for (const s of got) {
        const sl = slice(text, s.range);
        expect(sl.length).toBeGreaterThan(0);
        expect(sl).toBe(wsTrim(sl));
      }
    });
  }
});

describe("corpus_citation_flags", () => {
  const cases: [string, boolean][] = [
    ["See Roe v. Wade, 410 U.S. 113 (1973).", true],
    ["Cf. Katz v. United States, 389 U.S. 347 (1967).", true],
    ["Accord Smith v. Jones, 12 F.3d 88 (2d Cir. 1993).", true],
    ["E.g., Brown v. Board of Educ., 347 U.S. 483 (1954).", true],
    ["But see Olmstead v. United States, for the older rule.", true],
    ["See also Terry v. Ohio, on reasonable suspicion.", true],
    ["The seminal case is Brown, 347 U.S. 483, 495 (1954), which changed everything.", true],
    ["The court held that this applies.", false],
    ["But see Olmstead.", false],
    ["Seeing the writing on the wall, they settled.", false],
    ["The standard is well known.", false],
  ];
  for (const [text, expected] of cases) {
    test(text.slice(0, 50), () => {
      expect(isCitationSentence(text)).toBe(expected);
    });
  }
});

// ---- splitting details ---------------------------------------------------

test("no_terminator_yields_one_sentence", () => {
  const s = sentences("Hello world");
  expect(s.length).toBe(1);
  expect(slice("Hello world", s[0].range)).toBe("Hello world");
});

test("whitespace_only_yields_no_sentences", () => {
  expect(sentences("   \n\t  ")).toHaveLength(0);
  expect(sentences("")).toHaveLength(0);
});

test("lowercase_continuation_does_not_split", () => {
  const s = sentences("The rule (see supra p. 4) still applies. done deal it was");
  expect(s.length).toBe(1);
});

test("split_ranges_are_subranges_of_input", () => {
  const text = "  Padded start. Padded end.  ";
  const s = sentences(text);
  expect(s.length).toBe(2);
  expect(slice(text, s[0].range)).toBe("Padded start.");
  expect(slice(text, s[1].range)).toBe("Padded end.");
});

test("non_zero_base_offsets_are_absolute", () => {
  const source = "IGNORED PREFIX. The claim fails. It is dismissed. SUFFIX";
  const src = new Src(source);
  const start = src.byte(source.indexOf("The"));
  const end = src.byte(source.indexOf(" SUFFIX"));
  const s = splitSentences(src.slice(start, end), start, source);
  expect(s.length).toBe(2);
  expect(src.slice(s[0].range.start, s[0].range.end)).toBe("The claim fails.");
  expect(src.slice(s[1].range.start, s[1].range.end)).toBe("It is dismissed.");
  for (const sent of s) {
    for (const t of sent.tokens) {
      expect(t.range.start).toBeGreaterThanOrEqual(sent.range.start);
      expect(t.range.end).toBeLessThanOrEqual(sent.range.end);
    }
  }
});

test("multibyte_utf8_ranges_slice_exact_text", () => {
  const text = "The café’s motion—filed früh—was granted. “Naïve” objections followed.";
  const s = sentences(text);
  expect(s.length).toBe(2);
  expect(slice(text, s[0].range)).toBe("The café’s motion—filed früh—was granted.");
  expect(slice(text, s[1].range)).toBe("“Naïve” objections followed.");
  for (const sent of s) {
    for (const t of sent.tokens) {
      expect(slice(text, t.range).length).toBeGreaterThan(0);
    }
  }
  const words = s[0].tokens.filter((t) => t.kind === "word").map((t) => slice(text, t.range));
  expect(words).toContain("café’s");
  expect(words).toContain("früh");
});

// ---- tokenizer -----------------------------------------------------------

test("tokenize_words_numbers_punct", () => {
  const text = "Don't cite 410 U.S. 113 (1973)—ever.";
  const toks = tokenize(text, 0);
  const render = toks.map((t) => [t.kind, slice(text, t.range)]);
  expect(render).toEqual([
    ["word", "Don't"],
    ["word", "cite"],
    ["number", "410"],
    ["word", "U"],
    ["punct", "."],
    ["word", "S"],
    ["punct", "."],
    ["number", "113"],
    ["punct", "("],
    ["number", "1973"],
    ["punct", ")"],
    ["punct", "—"],
    ["word", "ever"],
    ["punct", "."],
  ]);
});

test("tokenize_interior_hyphens_apostrophes_and_number_separators", () => {
  const text = "well-known e-mail costs 1,000.50 dollars-";
  const toks = tokenize(text, 0);
  const render = toks.map((t) => [t.kind, slice(text, t.range)]);
  expect(render).toEqual([
    ["word", "well-known"],
    ["word", "e-mail"],
    ["word", "costs"],
    ["number", "1,000.50"],
    ["word", "dollars"],
    ["punct", "-"],
  ]);
});

test("tokenize_empty_and_whitespace", () => {
  expect(tokenize("", 0)).toHaveLength(0);
  expect(tokenize("  \n ", 0)).toHaveLength(0);
});

test("tokens_reproduce_source_via_ranges", () => {
  const text = "Judge Peña—“wise”—ruled 2-1 on §1983 claims.";
  for (const t of tokenize(text, 0)) {
    void slice(text, t.range);
    expect(t.range.end).toBeGreaterThan(t.range.start);
  }
});

// ---- abbreviation internals ----------------------------------------------

test("ordinal_reporters_block_splits", () => {
  expect(sentences("The 2d. Circuit and the 3d. Circuit agree.").length).toBe(1);
  expect(sentences("The 1st. filing was late. It was rejected.").length).toBe(2);
});

test("enumeration_only_blocks_at_line_start", () => {
  expect(sentences("The answer is 5. Nothing more follows here.").length).toBe(2);
  expect(sentences("5. The fifth claim fails entirely.").length).toBe(1);
});

test("year_in_parens_still_splits", () => {
  const s = sentences("(1954). Brown was decided then.");
  expect(s.length).toBe(2);
});
