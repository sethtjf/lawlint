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

### Intent split (#38)

Every rule carries an `intent: style | detection` tag (default `detection`).
Style rules keep linting and reporting diagnostics exactly as before, but the
human-likeness score aggregates **detection-intent rules only** — so the
AUC below measures detection rules alone. The original whole-ruleset baseline
(train AUC 0.306, below chance) is preserved in git history; the retune
reclassified the anti-discriminative lexical rules as style and folded the
absolute `core/no-em-dash` rule (train precision 0.065) into the rate-based
`core/no-em-dash-overuse`.

### Layer-2 statistical rules (#37)

Two document-level statistical rules ship on top of the intent split, each a
single per-document flag (`metric` + `threshold` + `direction` in YAML;
computations in `engines/statistical.rs`):

| Rule | Metric | Flag | Train AUC added alone |
| --- | --- | --- | ---: |
| `core/uniform-sentence-rhythm` | sentence-length variance (burstiness) | below 105 | 0.697 → 0.858 |
| `core/triad-overuse` | "A, B, and C" constructions per 1000 words | above 2 | 0.697 → 0.845 |

Thresholds were tuned on the train split only
(`cargo run -p lawlint-eval --bin tune_statistical`); each shipped metric had
to individually raise train AUC over the #38 baseline (0.697). Three candidate
metrics from the issue were measured and **dropped**:

- **cadence autocorrelation** — raw value AUC 0.490 on train (chance); its
  apparent flag gain was a penalty-scaling artifact (the tuner's always-fire
  control shows +0.013 from any universal flag).
- **paired-adjective rate** — raw value AUC 0.561; the only thresholds that
  gained AUC fired on 79% of human train documents (precision 0.55), and
  honest thresholds (above the human p90) lost AUC outright.
- **repeated-opener density** — individually +0.030, but adding it to the
  shipped pair *lowered* the combined train AUC from 0.913 to 0.890; its
  human false fires reorder pairs the other two rules already separate.

Non-inferential rules and their intents, with train-split precision:

| Rule | Intent | Train precision | Fired (TP+FP) |
| --- | --- | ---: | ---: |
| `core/no-hedging` | detection | 1.000 | 1 |
| `core/no-ai-cliches` | detection | 0.972 | 36 |
| `core/uniform-sentence-rhythm` | detection | 0.941 | 101 |
| `core/triad-overuse` | detection | 0.770 | 161 |
| `core/no-marketing-language` | detection | 0.972 | 36 |
| `core/no-not-only` | detection | 0.972 | 36 |
| `core/no-doublets` | detection | 0.949 | 39 |
| `core/no-rule-of-three` | detection | 0.917 | 12 |
| `core/no-repetitive-openers` | detection | 0.658 | 79 |
| `core/no-empty-emphasis` | detection | 0.500 | 2 |
| `core/no-passive-overuse` | detection | 0.500 | 2 |
| `core/no-em-dash-overuse` | detection | 0.000 | 3 |
| `core/no-robotic-transitions` | detection | — | 0 |
| `core/no-sycophantic-openers` | detection | — | 0 |
| `core/no-throat-clearing` | detection | — | 0 |
| `core/no-legalese` | style | 0.453 | 128 |
| `core/no-parenthetical-asides` | style | 0.358 | 109 |
| `core/sentence-length` | style | 0.330 | 224 |
| `core/oxford-comma` | style | 0.323 | 161 |
| `core/no-semicolons` | style | 0.238 | 130 |
| `core/no-en-dash` | style | 0.000 | 5 |

`core/no-en-dash` joined the style set on its train-split check: all five
firings land on human prose — typography lint, not an authorship signal.
`core/no-em-dash-overuse` stays detection per the #38 decision; its three
train firings are all human, so it remains a watch item (unchanged by the
layer-2 work in #37, which added rules rather than retuning it).

### Train split (CI gate)

The train split contains 330 rows.

| Slice | AUC | Mean human-likeness: human | Mean human-likeness: AI |
| --- | ---: | ---: | ---: |
| Overall | 0.913 | 96.261 | 78.576 |
| Naive | 0.853 | 96.261 | 87.907 |
| Rule-evading | 0.947 | 96.261 | 72.121 |
| Self-edit | 0.938 | 96.261 | 76.132 |

### Held-out test split (headline)

The held-out test split contains 118 rows:

| Slice | AUC | Mean human-likeness: human | Mean human-likeness: AI |
| --- | ---: | ---: | ---: |
| Overall | 0.873 | 95.017 | 77.271 |
| Naive | 0.786 | 95.017 | 86.619 |
| Rule-evading | 0.942 | 95.017 | 70.222 |
| Self-edit | 0.903 | 95.017 | 73.800 |

Both splits are printed by `report`; `baseline.json` stores the train-split
overall/slice AUCs and per-rule precision values for every rule that fired.

The naive slice was the layer-2 target: naive generations trip few lexical
rules at these text lengths, so before #37 that slice sat at 0.578 train /
0.589 test. The two document-level statistical rules moved it to 0.853 train /
0.786 test — rhythm and triad rate are properties of the generation itself,
not of any word list a prompt can name.
Inferential rules (`core/empty-hedge` and `core/padded-elaboration`)
are not evaluated in this harness because `evaluate()` runs without a judge;
they are measured by the opt-in judged harness below.

The complete per-rule precision/recall/F1 table for both splits, labeled by
intent, is printed by `report`.

## Regression gate

The committed-corpus test evaluates the **train** split. It guards overall AUC,
each prompt-style AUC, and each non-inferential rule's precision. A regression
fails when a metric falls more than 0.03 below the committed baseline. The
precision guard is intentional: the headline problem is rules firing on human
prose, so the gate protects against a rule becoming more human-firing. It does
not use per-rule recall, which would punish the remediation this corpus is
intended to enable. Rules with zero support on the current run (never fired on
either label) are omitted from the precision map and skipped by
`check_regression` — 0/0 precision is not a measurement, so narrowing a rule
to silence on train cannot false-fail the gate. The test adds approximately
49 seconds to the workspace test run.

## Tier-3 judged evaluation (#39)

The lexical harness above never measures the two inferential rules. The
opt-in judged harness does: `judged_report` runs the real judge pipeline
(`lint_full` — chunk planning, grounding, confidence floor, scope mask) over
the **train split** and reports per-rule precision/recall/F1 for
`core/empty-hedge` and `core/padded-elaboration` plus the
**verdict-discipline rate** from #39. It is feature-gated and never part of
the CI gate: judge runs are slow and backend-dependent.

```text
cargo run --release -p lawlint-eval --features judged --bin judged_report
cargo run --release -p lawlint-eval --features judged --bin judged_report -- \
  --model "anthropic:claude-sonnet-4-5,foundry:gpt-5.5" --limit 20
```

`--model` is repeatable or comma-separated and takes the same specs as
`lawlint --judge` (`anthropic:<model>`, `openai:<base-url>#<model>`,
`foundry:<deployment>`; hosted keys resolve environment-first, then the
`lawlint init` credential store). A backend that cannot run — missing key,
unreachable endpoint — fails with a per-backend message while the others still
print results. `--limit N` truncates to the first N train samples for smoke
runs.

Findings are cached in the shared judge disk cache
(`~/.cache/lawlint/judge/`), including a sibling entry per chunk holding the
findings the verdict-polarity guard dropped, so reruns are cheap and the
discipline stats survive cache hits. Chunks whose responses stay malformed
after core's retry are never cached and are re-attempted on every run.

### Local-Qwen baseline (train split, 330 samples) — historical

**This is the measurement that removed in-process inference in 0.9.** The
embedded backend is gone; the numbers are kept because they are the evidence,
and because anyone tempted to serve a similarly small model behind `openai:`
should expect the same results. Reproducing this row is no longer possible with
a shipped lawlint.

Backend `local:Qwen/Qwen2.5-1.5B-Instruct-GGUF` (q4_k_m), prompt
version 2 (post-#39-Part-1 polarity guard + clean-chunk `[]` example).
First run 948 s on an Apple-silicon laptop; cached rerun 521 s (the residual
time is the 38 persistently malformed chunks being retried).

| Rule | Precision | Recall | F1 | TP | FP | FN |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `core/empty-hedge` | 0.235 | 0.073 | 0.111 | 12 | 39 | 153 |
| `core/padded-elaboration` | 0.000 | 0.000 | 0.000 | 0 | 1 | 165 |

Pipeline stats: 330 chunks, 38 failed closed (malformed after retry), 112 of
418 guard-surviving findings grounded, 306 discarded as
ungroundable/foreign-rule.

Verdict discipline (parse layer, target <5% post-guard; pre-guard baseline
on the motivating document was ~50%):

| Metric | Value |
| --- | ---: |
| Model-emitted findings | 423 |
| Dropped by polarity guard | 5 |
| Raw negation-emission rate | **1.2%** |
| Guard survivors on human samples | 180 |
| — of which verdict-negations (broad heuristic) | 0 |
| Surviving-negation rate on human samples | **0.0%** |

Both rates are under the 5% target: the Part 1 guard plus prompt v2 solved
the negative-verdict failure mode. The surviving-negation heuristic
(`lawlint_eval::judged::looks_negational`) is deliberately wider than the
parse-time guard but *verdict-directed* — the rubrics themselves phrase
violations with negations ("hedges a claim without saying what is
uncertain"), so a bag-of-negation-words match would count nearly every
rubric-echoing explanation as a false alarm.

Verdict discipline is now the solved half; detection quality is not. The
1.5B model's F1 on both inferential rules is far below every shipped lexical
detection rule, and its false positives on human prose are rubric echoes
flagged confidently. On these numbers the local judge does not support
turning tier 3 on by default.

These measurements first drove the #50 cloud-first decision — hosted providers
preselected everywhere, unconfigured AI features erroring with init guidance
rather than silently downloading a model — and then, in 0.9, the removal of
in-process inference altogether. A tier that finds 7% of real hedges while
confidently emitting 39 false ones is worse than an absent tier, because the
score looks earned. Running privately is now served by pointing
`openai:<base-url>#<model>` at a locally-run OpenAI-compatible server, which
needs no API key and can serve a far larger model than lawlint could bundle.

### Hosted backends (pending keys)

The #39 comparison across the #41 model catalog needs hosted credentials not
present on the measurement machine. Placeholder — run
`judged_report --model <spec>` once keys exist:

| Backend | Precision (EH) | Recall (EH) | Precision (PE) | Recall (PE) | Negation-emission | Surviving-negation |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `anthropic:` (catalog) | — | — | — | — | — | — |
| `openai:` (catalog) | — | — | — | — | — | — |
| `foundry:` (catalog) | — | — | — | — | — | — |

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
