# lawlint evaluation corpus

This document records the committed baseline for issue #33. The corpus keeps
human/AI pairs together through a deterministic 70/30 train/test split hashed
by `pair_id`. The train split is used by the CI regression gate; the held-out
test split is reserved for the milestone headline.

## Composition

`corpus.jsonl` contains 448 rows: 224 human and 224 AI.

| Dimension | Counts |
| --- | --- |
| Labels | human 224; ai 224 |
| Sources | caselaw-access-project 137; sec-edgar 70; govinfo 11; manual 12; foundry 218 |
| Registers | opinion 279; contract 144; statute 22; memo 3 |
| AI styles | naive 75; rule-evading 76; self-edit 73 |
| AI models | gpt-5.5 73; claude-opus-4-8 72; FW-GLM-5.1 73; manual 6 |

CAP opinions and govinfo statutes are public-domain text. SEC EDGAR
contracts are privately drafted instruments that are publicly filed/public
records; they are not characterized here as public domain. Generated AI rows
are committed evaluation content.

Auto-sourced rows are 100–500 words and carry source/date provenance. The
12 manual seed rows, including the issue's motivating examples, are
intentionally exempt from that floor rather than being distorted to reach it.

## Baseline

The score used by `auc()` is `100 - lint_score`, and AUC is
`P(AI is more AI-like than human)`. Thus 0.500 is chance, values above 0.500
are discriminative, and values below 0.500 are anti-discriminative.

### Train split (CI gate)

The train split contains 330 rows.

| Slice | AUC | Mean human-likeness: human | Mean human-likeness: AI |
| --- | ---: | ---: | ---: |
| Overall | 0.306 | 52.267 | 69.964 |
| Naive | 0.303 | 52.267 | 68.852 |
| Rule-evading | 0.354 | 52.267 | 65.276 |
| Self-edit | 0.255 | 52.267 | 76.226 |

### Held-out test split (headline)

The held-out test split contains 118 rows and is reported as a one-time
milestone measure:

| Slice | AUC | Mean human-likeness: human | Mean human-likeness: AI |
| --- | ---: | ---: | ---: |
| Overall | 0.367 | 57.475 | 69.678 |
| Naive | 0.398 | 57.475 | 67.476 |
| Rule-evading | 0.371 | 57.475 | 69.056 |
| Self-edit | 0.332 | 57.475 | 72.550 |

Both splits are printed by `report`; `baseline.json` stores the train-split
overall/slice AUCs and per-rule precision values.

The below-chance held-out result is the Goodhart finding: authentic legal prose
trips lexical rules more often than generated AI prose. Oxford commas,
sentence length, semicolons, legalese, and parenthetical asides are prominent
human-firing signals, while rule-evading and self-edit AI is specifically
clean of many of them. This is the ground-truth baseline that follow-on
layer-2 statistical rules must improve.

Weakly AI-leaning rules include `core/no-rule-of-three`,
`core/no-repetitive-openers`, and `core/no-marketing-language`; they have high
precision but low recall. Inferential rules
(`core/empty-hedge` and `core/padded-elaboration`) are not evaluated in this
harness because `evaluate()` runs without a judge.

The complete per-rule precision/recall/F1 table for both splits is printed by
`report`.

## Regression gate

The committed-corpus test evaluates the **train** split. It guards overall AUC,
each prompt-style AUC, and each non-inferential rule's precision. A regression
fails when a metric falls more than 0.03 below the committed baseline. The
precision guard is intentional: the headline problem is rules firing on human
prose, so the gate protects against a rule becoming more human-firing. It does
not use per-rule recall, which would punish the remediation this corpus is
intended to enable. The test adds approximately 49 seconds to the workspace
test run.

## Workflow

The JSONL schema is the `Sample` type in `crates/lawlint-eval/src/lib.rs`.
Human rows set `label: "human"`, actual `word_count`, source/register
provenance, and a unique `pair_id`; `split` remains unset so the pair hash
assigns it. Matched AI rows reuse that `pair_id`, set `label: "ai"`, and record
`model` and `prompt_style`.

Human-source appenders are feature-gated:

```text
cargo run -p lawlint-eval --features sourcing --bin source_cap
cargo run -p lawlint-eval --features sourcing --bin source_edgar
cargo run -p lawlint-eval --features sourcing --bin source_govinfo
```

AI generation uses Foundry and requires `AZURE_FOUNDRY_API_KEY` and
`AZURE_FOUNDRY_ENDPOINT`:

```text
cargo run -p lawlint-eval --features sourcing --bin generate_ai
```

The generator obtains an abstract 8–15-word topic label through a separate
summarization call and rejects generated outputs with substantial verbatim
source overlap. The final regeneration measured 0/224 AI rows (0.0%) opening
with the seed's first eight words and 0/224 (0.0%) containing the seed's first
ten words verbatim.

Generate the report and committed train baseline with:

```text
cargo run -p lawlint-eval --bin report
cargo run -p lawlint-eval --bin report -- --emit-baseline crates/lawlint-eval/corpus/baseline.json
```

CI runs `cargo test --workspace`, including the committed-corpus regression
test and its 0.03 tolerance band.
