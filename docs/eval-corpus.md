# lawlint evaluation corpus

This document records the committed baseline for issue #33. The corpus is
provenance-preserving: human rows are public-domain excerpts from CAP, SEC
EDGAR, and govinfo; generated rows retain the human row's `pair_id`, register,
date/era metadata, target length, model, and prompt style.

## Composition

`corpus.jsonl` contains 448 rows: 224 human and 224 AI. The deterministic
70/30 train/test split hashes `pair_id`, so each human/AI pair is always in the
same split.

| Dimension | Counts |
| --- | --- |
| Labels | human 224; ai 224 |
| Sources | caselaw-access-project 137; sec-edgar 70; govinfo 11; manual 12; foundry 218 |
| Registers | opinion 279; contract 144; statute 22; memo 3 |
| AI styles | naive 75; rule-evading 76; self-edit 73 |
| AI models | gpt-5.5 77; claude-opus-4-8 70; FW-GLM-5.1 71; six manual AI rows have no model |

Human passages are 100–500 words, carry source/date provenance, and have
unique passage-level pair IDs. The sourced CAP opinions, SEC exhibits, and
govinfo statutes are public-domain text; the AI rows are committed generated
evaluation content.

## Baseline

Metrics below are from the test split (118 rows). The score used by `auc()` is
`100 - lint_score`, and AUC is `P(AI is more AI-like than human)`.

| Slice | AUC | Mean human lint score | Mean AI lint score |
| --- | ---: | ---: | ---: |
| Overall | 0.385 | 57.475 | 68.119 |
| Naive | 0.464 | 57.475 | 61.429 |
| Rule-evading | 0.334 | 57.475 | 72.889 |
| Self-edit | 0.349 | 57.475 | 70.850 |

Interpretation: 0.500 is chance, values above 0.500 are discriminative, and
values below 0.500 are anti-discriminative. This baseline is below chance:
authentic legal prose trips the lexical rules more often than the generated
AI prose, especially on semicolons, Oxford commas, long sentences, legalese,
and parenthetical asides. Rule-evading and self-edit AI is specifically clean
of many of those signals. This is the Goodhart result predicted by the issue,
and is the ground-truth baseline that the layer-2 statistical rules must
improve.

The weakest but AI-leaning signals include `core/no-rule-of-three`
(precision 0.833, recall 0.085), `core/no-repetitive-openers` (0.667, 0.237),
and `core/no-marketing-language` (0.909, 0.169): they have high precision but
low recall. Human-heavy signals include `core/sentence-length` (precision
0.380, recall 0.508), `core/oxford-comma` (0.400, 0.373),
`core/no-semicolons` (0.283, 0.220), `core/no-legalese` (0.405, 0.254), and
`core/no-parenthetical-asides` (0.400, 0.237).

The complete committed per-rule precision/recall/F1 table is printed by
`report`; `baseline.json` stores the overall/slice AUCs and per-rule recall.

## Workflow

The JSONL schema is the `Sample` type in `crates/lawlint-eval/src/lib.rs`.
Human rows should set `label: "human"`, actual `word_count`, source/register
provenance, and a unique `pair_id`; leave `split` unset so the pair hash
assigns it. Matched AI rows reuse that `pair_id`, set `label: "ai"`, and record
`model` and `prompt_style`.

Human-source appenders are feature-gated:

```text
cargo run -p lawlint-eval --features sourcing --bin source_cap
cargo run -p lawlint-eval --features sourcing --bin source_edgar
cargo run -p lawlint-eval --features sourcing --bin source_govinfo
```

AI generation uses Foundry and requires `AZURE_FOUNDRY_API_KEY` (and
`AZURE_FOUNDRY_ENDPOINT`):

```text
cargo run -p lawlint-eval --features sourcing --bin generate_ai
```

Generate the report and regenerate the committed baseline with:

```text
cargo run -p lawlint-eval --bin report
cargo run -p lawlint-eval --bin report -- --emit-baseline crates/lawlint-eval/corpus/baseline.json
```

CI runs `cargo test --workspace`, including the committed-corpus regression
test. The test uses a 0.03 tolerance band and fails when a rule change drops
overall or slice AUC, or any per-rule recall, more than 0.03 below the
committed baseline. This protects future improvements from silently
regressing.
