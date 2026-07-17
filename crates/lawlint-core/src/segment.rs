//! Legal-aware sentence segmentation + tokenization. [agent A]
//!
//! The highest-value component. Must NOT split after legal abbreviations
//! (`v.`, `Id.`, `U.S.`, `Fed.`, `Cir.`, `Inc.`, month abbreviations, single
//! capital initials, ordinal reporters, enumerations, …). Split on `.?!`
//! followed by whitespace + a likely sentence start.
//!
//! Canonical test: `"See Roe v. Wade, 410 U.S. 113 (1973). The court held
//! that this applies."` → 2 sentences, first `is_citation == true`.

use std::sync::LazyLock;

use regex::Regex;

use crate::document::{Sentence, Token, TokenKind};
use crate::types::TextRange;

/// Abbreviations after which a period never ends a sentence. Compared
/// case-insensitively against `<word>.` (word = run of `[A-Za-z0-9.()'’-]`
/// before the candidate period).
const ABBREVIATIONS: &[&str] = &[
    // case style / citation
    "v.", "vs.", "id.", "ibid.", "no.", "nos.", "fed.", "r.", "civ.", "crim.", "p.", "proc.",
    "evid.", "cir.", "ct.", "cl.", "u.s.", "s.", "ed.", "l.", "rev.", "stat.", "reg.", "sec.",
    "art.", "para.", "pp.", "seq.",
    // latin
    "e.g.", "i.e.", "etc.", "cf.",
    // corporate
    "inc.", "corp.", "ltd.", "co.",
    // honorifics
    "mr.", "mrs.", "ms.", "dr.", "prof.", "hon.",
    // months
    "jan.", "feb.", "mar.", "apr.", "jun.", "jul.", "aug.", "sep.", "sept.", "oct.", "nov.",
    "dec.",
];

/// Ordinal reporters: `2d.`, `3d.`, `4th.`, `1st.`, `2nd.`, `3rd.`, …
static ORDINAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\d+(d|st|nd|rd|th)$").expect("ordinal regex"));

/// Enumeration labels at line starts: `1.`, `(a).`, `iv.`, `(iii).`, `B.` —
/// short digit runs, a single letter, or roman numerals, optionally
/// parenthesized. Deliberately excludes 4-digit runs so years still split.
static ENUM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\(?([0-9]{1,3}|[A-Za-z]|[ivxlcIVXLC]{1,3})\)?$").expect("enum regex")
});

/// Reporter pattern: `410 U.S. 113`.
static REPORTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+\s+[A-Z][A-Za-z.]{0,15}\s+\d+").expect("reporter regex"));

/// Case style: `Roe v. Wade` / `Roe vs. Wade`.
static CASE_STYLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z][\w.'’-]*\s+vs?\.\s+[A-Z]").expect("case-style regex"));

/// Citation signals; a sentence starting with one of these followed by a
/// case style is a citation sentence. `See also` must precede `See`.
const CITATION_SIGNALS: &[&str] = &["See also", "See", "But see", "Cf.", "Accord", "E.g.,"];

/// Split the block slice `range` of `source` into sentences (with tokens and
/// the `is_citation` heuristic). All ranges are absolute byte offsets into
/// `source`. [agent A]
pub fn split_sentences(source: &str, range: TextRange) -> Vec<Sentence> {
    let text = range.slice(source);
    let base = range.start;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut sentences: Vec<Sentence> = Vec::new();
    let mut start: Option<usize> = None; // relative byte offset of sentence start
    let mut i = 0usize;

    let push = |sentences: &mut Vec<Sentence>, s: usize, e: usize| {
        if e > s {
            let abs = TextRange { start: base + s, end: base + e };
            sentences.push(make_sentence(source, abs));
        }
    };

    while i < chars.len() {
        let (off, c) = chars[i];
        if start.is_none() {
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            start = Some(off);
        }
        if matches!(c, '.' | '!' | '?') {
            // Absorb a run of terminators, then trailing closers, then whitespace.
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j].1, '.' | '!' | '?') {
                j += 1;
            }
            let mut k = j;
            while k < chars.len() && matches!(chars[k].1, '"' | '\'' | '”' | '’' | ')' | ']') {
                k += 1;
            }
            let mut m = k;
            while m < chars.len() && chars[m].1.is_whitespace() {
                m += 1;
            }
            let end_rel = if k < chars.len() { chars[k].0 } else { text.len() };
            if m >= chars.len() {
                // Terminator run reaches end of block: always close.
                push(&mut sentences, start.take().unwrap_or(off), end_rel);
                i = m;
                continue;
            }
            let has_ws = m > k;
            let blocked = c == '.' && j == i + 1 && is_blocked_period(text, &chars, i);
            if has_ws && is_likely_sentence_start(chars[m].1) && !blocked {
                push(&mut sentences, start.take().unwrap_or(off), end_rel);
                i = m;
                continue;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    if let Some(s) = start {
        let trimmed = text[s..].trim_end().len();
        push(&mut sentences, s, s + trimmed);
    }
    sentences
}

fn make_sentence(source: &str, range: TextRange) -> Sentence {
    Sentence {
        range,
        tokens: tokenize(source, range),
        is_citation: is_citation_sentence(range.slice(source)),
    }
}

fn is_likely_sentence_start(c: char) -> bool {
    c.is_uppercase() || c.is_ascii_digit() || matches!(c, '"' | '“' | '‘' | '\'' | '(')
}

/// `chars[i]` is a lone `.`; decide whether a sentence break here is blocked
/// by the legal abbreviation rules.
fn is_blocked_period(text: &str, chars: &[(usize, char)], i: usize) -> bool {
    // Scan back over the word directly attached to the period.
    let mut idx = i;
    while idx > 0 {
        let ch = chars[idx - 1].1;
        if ch.is_alphanumeric() || matches!(ch, '.' | '(' | ')' | '\'' | '’' | '-') {
            idx -= 1;
        } else {
            break;
        }
    }
    if idx == i {
        return false; // no word attached (period after whitespace/punct)
    }
    let wstart = chars[idx].0;
    let word = &text[wstart..chars[i].0];

    // 1. Known abbreviation (case-insensitive, incl. multi-dot like U.S.).
    let lowered = format!("{}.", word.to_lowercase());
    if ABBREVIATIONS.contains(&lowered.as_str()) {
        return true;
    }
    // 2. Single capital initial: `J. Smith`.
    let mut wchars = word.chars();
    if let (Some(first), None) = (wchars.next(), wchars.next()) {
        if first.is_ascii_uppercase() {
            return true;
        }
    }
    // 3. Ordinal reporters: `2d.`, `3d.`, `4th.`.
    if ORDINAL_RE.is_match(word) {
        return true;
    }
    // 4. Enumerations at list/line starts: `1.`, `(a).`, `iv.`.
    if ENUM_RE.is_match(word) && at_line_start(text, wstart) {
        return true;
    }
    false
}

/// True when everything between the previous newline (or block start) and
/// `pos` is whitespace.
fn at_line_start(text: &str, pos: usize) -> bool {
    let line_begin = text[..pos].rfind('\n').map(|n| n + 1).unwrap_or(0);
    text[line_begin..pos].trim().is_empty()
}

/// Tokenize the slice `range` of `source` into Word / Number / Punct tokens
/// with absolute byte ranges. Words include `'`, `’`, and `-` interiors.
/// [agent A]
pub fn tokenize(source: &str, range: TextRange) -> Vec<Token> {
    let text = range.slice(source);
    let base = range.start;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let (off, c) = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_alphabetic() {
            let mut j = i + 1;
            while j < chars.len() {
                let ch = chars[j].1;
                if ch.is_alphanumeric() {
                    j += 1;
                } else if matches!(ch, '\'' | '’' | '-')
                    && j + 1 < chars.len()
                    && chars[j + 1].1.is_alphanumeric()
                {
                    j += 1; // interior apostrophe/hyphen: don't, café’s, e-mail
                } else {
                    break;
                }
            }
            let end = if j < chars.len() { chars[j].0 } else { text.len() };
            tokens.push(Token {
                range: TextRange { start: base + off, end: base + end },
                kind: TokenKind::Word,
            });
            i = j;
        } else if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() {
                let ch = chars[j].1;
                if ch.is_ascii_digit() {
                    j += 1;
                } else if matches!(ch, '.' | ',')
                    && j + 1 < chars.len()
                    && chars[j + 1].1.is_ascii_digit()
                {
                    j += 1; // interior separators: 1,000 / 3.5
                } else {
                    break;
                }
            }
            let end = if j < chars.len() { chars[j].0 } else { text.len() };
            tokens.push(Token {
                range: TextRange { start: base + off, end: base + end },
                kind: TokenKind::Number,
            });
            i = j;
        } else {
            tokens.push(Token {
                range: TextRange { start: base + off, end: base + off + c.len_utf8() },
                kind: TokenKind::Punct,
            });
            i += 1;
        }
    }
    tokens
}

/// Citation heuristic: reporter pattern `\d+\s+[A-Z][A-Za-z.]{0,15}\s+\d+`
/// or a citation signal (`See`, `See also`, `Cf.`, `Accord`, `But see`,
/// `E.g.,`) followed by case-style `X v. Y`. [agent A]
pub fn is_citation_sentence(sentence_text: &str) -> bool {
    let t = sentence_text
        .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '"' | '“' | '‘' | '\''));
    if REPORTER_RE.is_match(t) {
        return true;
    }
    for signal in CITATION_SIGNALS {
        if let Some(rest) = t.strip_prefix(signal) {
            if rest.starts_with(|c: char| c.is_whitespace()) && CASE_STYLE_RE.is_match(rest) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(text: &str) -> TextRange {
        TextRange { start: 0, end: text.len() }
    }

    fn sentences(text: &str) -> Vec<Sentence> {
        split_sentences(text, whole(text))
    }

    fn sentence_texts(text: &str) -> Vec<String> {
        sentences(text).iter().map(|s| s.range.slice(text).to_string()).collect()
    }

    // ---- §12 segmentation corpus: ≥15 tricky legal cases -----------------

    #[test]
    fn canonical_roe_v_wade() {
        let text = "See Roe v. Wade, 410 U.S. 113 (1973). The court held that this applies.";
        let s = sentences(text);
        assert_eq!(s.len(), 2, "expected 2 sentences in {text:?}");
        assert!(s[0].is_citation, "first sentence must be a citation");
        assert!(!s[1].is_citation);
        assert_eq!(s[0].range.slice(text), "See Roe v. Wade, 410 U.S. 113 (1973).");
        assert_eq!(s[1].range.slice(text), "The court held that this applies.");
    }

    #[test]
    fn segmentation_corpus() {
        // (text, expected sentence count)
        let corpus: &[(&str, usize)] = &[
            // 1. Canonical case (also asserted in detail above).
            ("See Roe v. Wade, 410 U.S. 113 (1973). The court held that this applies.", 2),
            // 2. Rule-citation abbreviation chain.
            ("Fed. R. Civ. P. 12(b)(6) governs motions to dismiss. The standard is well known.", 2),
            // 3. Honorifics and month abbreviation.
            ("Mr. Smith met Dr. Jones on Jan. 5, 2020. They discussed the case.", 2),
            // 4. Id. cite with pin.
            ("Id. at 5. The reasoning was sound.", 2),
            // 5. Numbered enumeration at line starts.
            ("1. The first claim fails.\n2. The second claim also fails.", 2),
            // 6. Parenthesized enumeration at block start.
            ("(a). Each party shall bear its own costs.", 1),
            // 7. Ordinal reporter with trailing period.
            ("The court, per the 4th. Circuit, affirmed. We agree.", 2),
            // 8. Long cite with semicolon: no split.
            ("See Miranda v. Arizona, 384 U.S. 436, 444 (1966); see also Dickerson v. United States.", 1),
            // 9. Single capital initials.
            ("J. Smith argued the case. The court agreed with J. Smith.", 2),
            // 10. E.g. signal with Educ. abbreviation.
            ("E.g., Brown v. Board of Educ., 347 U.S. 483 (1954). This is settled law.", 2),
            // 11. Question and exclamation terminators.
            ("Was the verdict just? Absolutely not! The appeal was denied.", 3),
            // 12. U.S. as a party name.
            ("See U.S. v. Nixon, 418 U.S. 683 (1974). Executive privilege is not absolute.", 2),
            // 13. Cf. signal + But see follow-up.
            ("Cf. Katz v. United States, 389 U.S. 347 (1967). But see Olmstead.", 2),
            // 14. Corporate abbreviations swallow the boundary (documented tradeoff).
            ("The agreement was signed by Acme Co. Ltd. It became effective immediately.", 1),
            // 15. Month abbreviation + year boundary (years DO split).
            ("Filing is due Oct. 1, 2024. Do not miss the deadline.", 2),
            // 16. No. as docket-number abbreviation.
            ("The motion was denied. No. 12-345 was closed.", 2),
            // 17. Evidence-rule cite ending in a bare number.
            ("Plaintiff relies on Fed. R. Evid. 702. Daubert governs admissibility.", 2),
            // 18. Accord signal with 2d Cir. parenthetical.
            ("Accord Smith v. Jones, 12 F.3d 88 (2d Cir. 1993). The rule is settled.", 2),
            // 19. Reporter cite embedded mid-sentence.
            ("The seminal case is Brown, 347 U.S. 483, 495 (1954), which changed everything. Its impact endures.", 2),
            // 20. Statute reference, See without case style: still splits fine.
            ("Section 1983 provides a remedy. See 42 U.S.C. § 1988 for fees.", 2),
            // 21. Em dashes inside a sentence (multi-byte).
            ("It was—frankly—wrong. The court said so.", 2),
            // 22. Curly quotes: closing quote stays with its sentence.
            ("The café’s brief was filed. “Justice delayed is justice denied.” The motion followed.", 3),
        ];
        for (text, expected) in corpus {
            let got = sentences(text);
            assert_eq!(
                got.len(),
                *expected,
                "wrong sentence count for {text:?}: got {:?}",
                sentence_texts(text)
            );
            // Every range must slice cleanly and be trimmed.
            for s in &got {
                let sl = s.range.slice(text);
                assert!(!sl.is_empty());
                assert_eq!(sl, sl.trim(), "sentence not trimmed: {sl:?}");
            }
        }
    }

    #[test]
    fn corpus_citation_flags() {
        let cases: &[(&str, bool)] = &[
            ("See Roe v. Wade, 410 U.S. 113 (1973).", true),
            ("Cf. Katz v. United States, 389 U.S. 347 (1967).", true),
            ("Accord Smith v. Jones, 12 F.3d 88 (2d Cir. 1993).", true),
            ("E.g., Brown v. Board of Educ., 347 U.S. 483 (1954).", true),
            ("But see Olmstead v. United States, for the older rule.", true),
            ("See also Terry v. Ohio, on reasonable suspicion.", true),
            ("The seminal case is Brown, 347 U.S. 483, 495 (1954), which changed everything.", true),
            ("The court held that this applies.", false),
            ("But see Olmstead.", false), // signal without a case style
            ("Seeing the writing on the wall, they settled.", false), // "See" prefix of a word
            ("The standard is well known.", false),
        ];
        for (text, expected) in cases {
            assert_eq!(is_citation_sentence(text), *expected, "citation flag wrong for {text:?}");
        }
    }

    // ---- splitting details ----------------------------------------------

    #[test]
    fn no_terminator_yields_one_sentence() {
        let s = sentences("Hello world");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].range.slice("Hello world"), "Hello world");
    }

    #[test]
    fn whitespace_only_yields_no_sentences() {
        assert!(sentences("   \n\t  ").is_empty());
        assert!(sentences("").is_empty());
    }

    #[test]
    fn lowercase_continuation_does_not_split() {
        // "no." style abbreviations aside, a lowercase next word blocks a split.
        let s = sentences("The rule (see supra p. 4) still applies. done deal it was");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn split_ranges_are_subranges_of_input() {
        let text = "  Padded start. Padded end.  ";
        let s = sentences(text);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].range.slice(text), "Padded start.");
        assert_eq!(s[1].range.slice(text), "Padded end.");
    }

    #[test]
    fn non_zero_base_offsets_are_absolute() {
        let source = "IGNORED PREFIX. The claim fails. It is dismissed. SUFFIX";
        // Range covering only the middle two sentences.
        let start = source.find("The").unwrap();
        let end = source.find(" SUFFIX").unwrap();
        let s = split_sentences(source, TextRange { start, end });
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].range.slice(source), "The claim fails.");
        assert_eq!(s[1].range.slice(source), "It is dismissed.");
        for sent in &s {
            for t in &sent.tokens {
                assert!(sent.range.contains(&t.range), "token outside its sentence");
            }
        }
    }

    #[test]
    fn multibyte_utf8_ranges_slice_exact_text() {
        let text = "The café’s motion—filed früh—was granted. “Naïve” objections followed.";
        let s = sentences(text);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].range.slice(text), "The café’s motion—filed früh—was granted.");
        assert_eq!(s[1].range.slice(text), "“Naïve” objections followed.");
        for sent in &s {
            for t in &sent.tokens {
                // Slicing must not panic (char-boundary safe) and be non-empty.
                assert!(!t.range.slice(text).is_empty());
            }
        }
        // Word token with multi-byte apostrophe interior.
        let words: Vec<&str> = s[0]
            .tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Word)
            .map(|t| t.range.slice(text))
            .collect();
        assert!(words.contains(&"café’s"), "words: {words:?}");
        assert!(words.contains(&"früh"));
    }

    // ---- tokenizer -------------------------------------------------------

    #[test]
    fn tokenize_words_numbers_punct() {
        let text = "Don't cite 410 U.S. 113 (1973)—ever.";
        let toks = tokenize(text, whole(text));
        let render: Vec<(TokenKind, &str)> =
            toks.iter().map(|t| (t.kind, t.range.slice(text))).collect();
        assert_eq!(
            render,
            vec![
                (TokenKind::Word, "Don't"),
                (TokenKind::Word, "cite"),
                (TokenKind::Number, "410"),
                (TokenKind::Word, "U"),
                (TokenKind::Punct, "."),
                (TokenKind::Word, "S"),
                (TokenKind::Punct, "."),
                (TokenKind::Number, "113"),
                (TokenKind::Punct, "("),
                (TokenKind::Number, "1973"),
                (TokenKind::Punct, ")"),
                (TokenKind::Punct, "—"),
                (TokenKind::Word, "ever"),
                (TokenKind::Punct, "."),
            ]
        );
    }

    #[test]
    fn tokenize_interior_hyphens_apostrophes_and_number_separators() {
        let text = "well-known e-mail costs 1,000.50 dollars-";
        let toks = tokenize(text, whole(text));
        let render: Vec<(TokenKind, &str)> =
            toks.iter().map(|t| (t.kind, t.range.slice(text))).collect();
        assert_eq!(
            render,
            vec![
                (TokenKind::Word, "well-known"),
                (TokenKind::Word, "e-mail"),
                (TokenKind::Word, "costs"),
                (TokenKind::Number, "1,000.50"),
                (TokenKind::Word, "dollars"),
                (TokenKind::Punct, "-"), // trailing hyphen is not interior
            ]
        );
    }

    #[test]
    fn tokenize_empty_and_whitespace() {
        assert!(tokenize("", whole("")).is_empty());
        assert!(tokenize("  \n ", whole("  \n ")).is_empty());
    }

    #[test]
    fn tokens_reproduce_source_via_ranges() {
        let text = "Judge Peña—“wise”—ruled 2-1 on §1983 claims.";
        for t in tokenize(text, whole(text)) {
            // ranges must be valid char boundaries in the ORIGINAL source
            let _ = t.range.slice(text);
            assert!(t.range.end > t.range.start);
        }
    }

    // ---- abbreviation internals -----------------------------------------

    #[test]
    fn ordinal_reporters_block_splits() {
        assert_eq!(sentences("The 2d. Circuit and the 3d. Circuit agree.").len(), 1);
        assert_eq!(sentences("The 1st. filing was late. It was rejected.").len(), 2);
    }

    #[test]
    fn enumeration_only_blocks_at_line_start() {
        // Mid-line "5." splits when followed by a capital.
        assert_eq!(sentences("The answer is 5. Nothing more follows here.").len(), 2);
        // Line-start "5." is an enumeration label.
        assert_eq!(sentences("5. The fifth claim fails entirely.").len(), 1);
    }

    #[test]
    fn year_in_parens_still_splits() {
        // "(1954)" must not be treated as an enumeration even at line start.
        let s = sentences("(1954). Brown was decided then.");
        assert_eq!(s.len(), 2);
    }
}
