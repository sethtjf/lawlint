# Style mining (`lawlint learn`)

This document describes how `lawlint learn` turns a body of source writing
into rules that edit agent drafts toward that writing. The core decision is to
mine from a *contrast*: the configured model drafts its own version of sampled
source passages, so each pair holds content constant and differs only in
style. Every mined rule must then pass gates in both directions. It may never
flag the source corpus, and (for mined and calibrated rules) it must flag at
least one AI draft. The result is a rule package that covers mechanics,
phrasing, rhythm, and, through AI-judge rubrics, tone, cohesion, clarity, and
reasoning, plus a `STYLE.md` brief that agents read before drafting.

## Pipeline

| Stage | Module | Model calls | Output |
| --- | --- | --- | --- |
| Pass 1: absence statistics | `learn.rs` | 0 | "never does X" phrase rules, sentence-length cap |
| Style profile | `learn/profile.rs` | 0 | `StyleProfile`, calibrated statistical rules, `transitions-never-used` |
| Contrast set | `learn/contrast.rs` | `--contrast` (default 6) | `ContrastPair { source, original, draft }` |
| Hard mining | `learn.rs` lenses | 4 (+ repair) | phrase / leading rules |
| Soft mining | `learn/soft.rs` lenses | 4 | inferential rules, `intent: style` |
| Self-consistency gate | `learn.rs` | 0 | drops hard rules that fire on the corpus |
| Discrimination check | `learn/contrast.rs` | 0 | drops mined/calibrated rules that fire on no draft |
| Judge gate | `learn/soft.rs` | 2 per soft rule | drops soft rules the judge applies to the corpus, or never to drafts |
| Brief | `learn/guide.rs` | 0 | `STYLE.md`, `profile.json` |

All new rules use existing engines and metrics, so `lawlint-core` and the
TypeScript port are unchanged and golden parity is unaffected.

## Style profile

`profile::measure(&[(&str, bool)]) -> StyleProfile` runs over the full corpus
and, separately, over the contrast drafts. It records:

- words per sentence and sentences per paragraph as `Distribution { n, mean, min, p10, median, p90, max }`;
- per-1,000-word rates for dashes, semicolons, colons, parentheticals, question and exclamation marks, first and second person, contractions, and approximate passive and nominalization counts;
- sentence-initial connectives in use, and the AI-favored connectives the corpus never opens with;
- the per-passage distribution of each document-level metric in `engines::statistical::Metric`, measured on the same 2,000-character chunks the gate lints.

Calibrated rules set their threshold just past the corpus's own extreme
(`rhythm-floor` below 80% of the flattest passage; `cadence-ceiling`,
`opener-repetition-ceiling`, `triad-ceiling`, and `paired-phrase-ceiling`
above the maximum plus a margin). They need at least four measurable passages
and are dropped by the discrimination check if no draft crosses the line.

## Contrast set

`contrast::select_sources` picks passages of 80+ words spread evenly across
the corpus. The drafting prompt asks the model to write the same substance
fresh, at the same length, without reusing sentences. `validate_draft` rejects
empty drafts, drafts under 40% of the source length, and drafts that copy more
than half their sentences, because a copied draft would teach the miner that
the corpus's own sentences are machine style.

Drafts use the backend's default sampling rather than temperature 0, since
the point is the model's natural drafting voice.

## Soft rules

Four lenses (tone and register, transitions and cohesion, clarity and
concision, reasoning and structure) each propose up to three inferential rules
as JSON. `soft::soft_candidate` enforces grounding before any judge call:

- each pass example must appear verbatim in the corpus after whitespace, quote, and case normalization, and at least three must survive;
- flag examples found in the corpus are removed, and at least three must remain.

`soft::gate` then runs each rule through `lawlint_core::lint_full` with the
real judge, once over six corpus passages and once over the concatenated
drafts. Keep requires zero corpus findings, zero failed chunks, and (with a
contrast set) at least one draft finding.

## Tradeoffs

- **More text leaves the machine.** Defaults add about 12,000 characters of contrast passages and 12,000 characters of judge-gate passages to the existing 16,000-character sample. `--contrast 0 --no-soft` restores the old footprint.
- **Cost.** Defaults add 6 drafting calls, 4 soft-mining calls, and up to 24 judge calls (12 soft rules × 2).
- **Weak contrast evidence.** "Fires on at least one of six drafts" is a low bar. It removes dead rules but does not measure precision on AI text.
- **Soft self-fire coverage is partial.** Hard rules are checked against every corpus passage; soft rules against six, because judge calls cost money.
- **Strict gates cost recall.** A noisy judge that flags one corpus sentence drops a good rule. This follows the existing posture: a weak model costs recall, not correctness.

## Open questions

1. Pass-2 hard rules keep the loader's default `intent: detection` and so move the human-likeness score. Soft and calibrated rules are `style`. Should mined hard rules also default to `style`?
2. Drafts come from the mining model. Production agents may use a different model; a `--contrast-model` flag would let the contrast set match the model whose drafts the package will edit.
3. Should the discrimination check move from "at least one draft" to a minimum hit rate once `--contrast` is large enough to estimate one?
4. Should the user be able to mine against an existing corpus of AI drafts (for example, past agent output) instead of generating one?
