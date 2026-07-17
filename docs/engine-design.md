# lawlint Rules Engine v2 — Design Contract

This is the authoritative contract for the greenfield rewrite of `lawlint-core`.
Implementation agents: transcribe types verbatim; where this doc and skeleton code
disagree, the skeleton files in the repo are the source of truth.

## 0. Goals & invariants

1. Three tiers: **static** (regex/token), **statistical** (whole-document), **inferential** (LLM judge).
2. **A diagnostic from the judge is indistinguishable downstream from a diagnostic from a regex.** No consumer branches on tier.
3. **Declarative-first**: rules are YAML data loaded at runtime. Built-ins ship as a bundled package using the same loader. Non-engineers author rules without rebuilding. Programmatic `Rule` trait is the escape hatch.
4. Spans are **byte offsets into original source**, always. Line/column (UTF-16 columns, 1-based) derived only at finalize.
5. Rule IDs are namespaced `package/name`, stable forever. Legacy flat ids resolve via alias.
6. Judge findings that cannot be **grounded** to a source span do not exist.
7. Core stays inference-agnostic and wasm-safe. Judge backends live in `crates/lawlint-judge` (native only).
8. Scoring (0–100 human-likeness) is preserved: deterministic from tiers 1–2; tier-3 contributes confidence-weighted points only above a floor, severity capped at Warning.

## 1. Workspace

Existing: `crates/lawlint-core`, `crates/lawlint-cli`, `crates/lawlint-wasm`, `apps/desktop/src-tauri`.
New (phase 2): `crates/lawlint-judge`.

Workspace deps to add to root `Cargo.toml`: `serde_yaml = "0.9"`, `thiserror = "2"`, `strsim = "0.11"`, `sha2 = "0.10"`, `include_dir = "0.7"`, `pulldown-cmark = { version = "0.12", default-features = false }`. Core also gains runtime `serde_json` (judge JSON parsing).

`lawlint-core` module layout & ownership (agents own ONLY their files):

```
crates/lawlint-core/src/
  lib.rs        # public API, re-exports          [skeleton, integration]
  types.rs      # core data model                  [skeleton — complete]
  error.rs      # thiserror error types            [skeleton — complete]
  config.rs     # LintOptions                      [skeleton — complete]
  rule.rs       # Rule trait, RuleMeta, Ctx        [skeleton — complete]
  document.rs   # document tree types + builder    [types: skeleton; parsing: agent A]
  segment.rs    # legal-aware segmentation         [agent A]
  markdown.rs   # markdown structure via pulldown  [agent A]
  engines/
    mod.rs      #                                  [skeleton]
    phrase.rs   # phrase engine (+ leading?)       [agent B]
    leading.rs  # sentence-opener engine           [agent B]
    density.rs  # density engine                   [agent C]
    statistical.rs # metric engine                 [agent C]
  loader.rs     # YAML parse + validation          [agent D]
  registry.rs   # RuleSet, packages, aliases       [agent D]
  judge.rs      # tier-3 pipeline (plan/run/ground/cache), Judge trait, MockJudge [agent F]
  dispatch.rs   # single-pass dispatcher, scope mask, suppression [integration]
  scoring.rs    # finalize: line/col/excerpt, stats, score        [integration]
crates/lawlint-core/builtin/
  style.yaml    # package manifest {name: core}    [agent E]
  rules/*.yaml  # 22 built-in rules                [agent E]
```

## 2. Core types (`types.rs`) — verbatim

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange { pub start: usize, pub end: usize } // byte offsets, original source
impl TextRange {
    pub fn slice<'a>(&self, text: &'a str) -> &'a str { &text[self.start..self.end] }
    pub fn contains(&self, other: &TextRange) -> bool { other.start >= self.start && other.end <= self.end }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity { Error, Warning, #[serde(alias = "info")] Suggestion }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier { Static, Statistical, Inferential }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope { Prose, Text, All }
// Prose: paragraph + list-item sentences, excluding citation sentences.
// Text:  Prose + headings + block quotes + citation sentences. (built-in default)
// All:   everything including code blocks.

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(pub String); // "package/name", e.g. "core/no-em-dash"

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub rule_id: RuleId,
    pub severity: Severity,
    pub tier: Tier,
    pub span: TextRange,
    pub message: String,
    pub line: usize,                 // 1-based; filled by finalize
    pub column: usize,               // 1-based, UTF-16 code units; filled by finalize
    #[serde(skip_serializing_if = "Option::is_none")] pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")] pub end_column: Option<usize>,
    pub excerpt: String,             // trimmed source line; filled by finalize
    #[serde(skip_serializing_if = "Option::is_none")] pub suggestion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub weight: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")] pub confidence: Option<f32>, // tier-3 only
    #[serde(skip_serializing_if = "Option::is_none")] pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fix { pub edits: Vec<Edit>, pub applicability: Applicability }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit { pub range: TextRange, pub replacement: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Applicability { MachineApplicable, MaybeIncorrect }
// Tier-3 fixes are ALWAYS MaybeIncorrect. Only MachineApplicable participates in --fix.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub word_count: usize,
    pub sentence_count: usize,
    pub score: i32, // 0..=100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintResult {
    pub diagnostics: Vec<Diagnostic>,
    pub stats: Stats,
    #[serde(skip_serializing_if = "Option::is_none")] pub judge: Option<JudgeStats>,
}
```

## 3. Document model (`document.rs` types — verbatim; parsing = agent A)

```rust
pub struct Document { pub blocks: Vec<Block> }
pub struct Block { pub kind: BlockKind, pub range: TextRange, pub sentences: Vec<Sentence> }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind { Paragraph, Heading, ListItem, BlockQuote, CodeBlock }
pub struct Sentence { pub range: TextRange, pub tokens: Vec<Token>, pub is_citation: bool }
pub struct Token { pub range: TextRange, pub kind: TokenKind }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind { Word, Number, Punct }

pub fn parse(source: &str, markdown: bool) -> Document;
```

- `markdown=false`: blocks are paragraphs split on blank lines, all `Paragraph`.
- `markdown=true`: use `pulldown-cmark` with `OffsetIter` to classify blocks (heading, list item, block quote, fenced/indented code). Code blocks get **no sentences** (never linted except Scope::All).
- Every node's range indexes the ORIGINAL source. Never normalize text in place.

### Segmentation (`segment.rs`) — the highest-value component

Legal-aware sentence splitting. Must NOT split after: `v.`, `vs.`, `Id.`, `Ibid.`, `No.`, `Nos.`, `Fed.`, `R.`, `Civ.`, `Crim.`, `P.`, `Proc.`, `Evid.`, `Cir.`, `Ct.`, `Cl.`, `U.S.`, `S.`, `Ed.`, `L.`, `Rev.`, `Stat.`, `Reg.`, `Sec.`, `Art.`, `art.`, `cl.`, `para.`, `pp.`, `p.`, `e.g.`, `i.e.`, `etc.`, `cf.`, `seq.`, `Inc.`, `Corp.`, `Ltd.`, `Co.`, `Mr.`, `Mrs.`, `Ms.`, `Dr.`, `Prof.`, `Hon.`, month abbreviations, single capital initials (`J. Smith`), ordinal reporters (`2d.`, `3d.`, `4th.`), enumerations like `(a).`/`1.` at list starts.
Split on `.?!` followed by whitespace + likely sentence start. Tokenize: words (incl. `'`, `’`, `-` interiors), numbers, punctuation, all with byte ranges.
Citation heuristic: mark `is_citation` when a sentence matches reporter pattern `\d+\s+[A-Z][A-Za-z.]{0,15}\s+\d+` or begins with a citation signal (`See`, `See also`, `Cf.`, `Accord`, `But see`, `E.g.,`) followed by a case-style `X v. Y`.
Canonical test: `"See Roe v. Wade, 410 U.S. 113 (1973). The court held that this applies."` → 2 sentences, first `is_citation == true`.

## 4. Rule trait (`rule.rs` — skeleton-complete, verbatim)

```rust
#[derive(Debug, Clone, Default)]
pub struct Interests { pub tokens: bool, pub sentences: bool, pub blocks: bool, pub document_exit: bool }

#[derive(Debug, Clone, Serialize)]
pub struct RuleMeta {
    pub id: RuleId, pub tier: Tier, pub scope: Scope, pub severity: Severity,
    pub description: String, pub docs_url: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub rationale: Option<String>,
    pub examples: Vec<RuleExample>, // { bad: String, good: String }
}

pub struct Report { // what engines emit; dispatcher stamps id/severity/tier, finalize adds pos
    pub span: TextRange, pub message: String,
    pub suggestion: Option<String>, pub weight: Option<u32>, pub fix: Option<Fix>,
}

pub struct Ctx<'a> {
    pub source: &'a str,
    pub word_count: usize,          // scope-aware prose word count (see §8)
    // internal: sink Vec<Report>, threshold override for current rule
}
impl<'a> Ctx<'a> {
    pub fn report(&mut self, r: Report);
    pub fn threshold(&self, default: f64) -> f64; // options.thresholds[rule id or alias]
}

pub trait Rule: Send + Sync {
    fn meta(&self) -> &RuleMeta;
    fn interests(&self) -> Interests;
    fn check_token(&mut self, _t: &Token, _ctx: &mut Ctx) {}
    fn check_sentence(&mut self, _s: &Sentence, _ctx: &mut Ctx) {}
    fn check_block(&mut self, _b: &Block, _ctx: &mut Ctx) {}
    fn on_document_exit(&mut self, _doc: &Document, _ctx: &mut Ctx) {}
    fn rubric(&self) -> Option<&RubricFragment> { None } // tier-3 rules only
}
```

Rules are **stateful, instantiated fresh per lint run** — `RuleSet` stores parsed
`RuleDef`s and `instantiate()`s `Vec<Box<dyn Rule>>` each run.

## 5. Engines

Each engine is a struct implementing `Rule`, constructed from a `RuleDef`.

- **phrase** (agent B): list of `{ regex, message?, suggestion?, fix? }`. Interest: blocks. Run each regex on `block.range.slice(source)`; report at absolute offsets. Optional `allow_context: { pattern, window }`: expand match by `window` bytes each side (clamped to char boundaries); if pattern matches the expanded slice, skip (used by `no-en-dash` for numeric ranges `1994–2001`). A `fix` string on an item makes a `MachineApplicable` single-edit Fix.
- **leading** (agent B): needle list. Interest: sentences. Case-insensitive match of any needle at sentence start → report needle span. Always the rule's configured severity (built-ins: error).
- **density** (agent C): one regex + `threshold` (matches per 1000 words). Interest: blocks + document_exit. Accumulate matches; at exit, fire only if `count/words*1000 > threshold` (threshold overridable via `Ctx::threshold`). Emit ONE report at first match span with `weight = ceil(count - threshold*words/1000).max(1) as u32` and message suffixed `" (N occurrences in M words)"`. **This formula is parity-critical.**
- **statistical** (agent C): `metric` +`params`:
  - `sentence-length`: params `max_words` (default 45, overridable via thresholds). Per sentence: count Word+Number tokens; over max → report sentence span.
  - `repetitive-openers`: params `run_length` (default 3). Track consecutive sentences (within a block) sharing the same lowercased first word token; on reaching run_length → report the run's last sentence span; reset after firing.
  Extension point: metric enum is non-exhaustive; unknown metric = load error.
- **inferential** (agent F consumes): no runtime check; carries a `RubricFragment`.

## 6. YAML rule format (`loader.rs`, agent D)

Package = directory: `style.yaml` (`name`, `version`, optional `description`) + `rules/*.yaml`, one rule per file. Built-in package embedded via `include_dir!` from `crates/lawlint-core/builtin/`.

```yaml
id: no-em-dash               # full id becomes <package>/<id>
engine: phrase               # phrase | leading | density | statistical | inferential
scope: text                  # prose | text | all   (default: text)
severity: error              # error | warning | suggestion (accepts legacy "info")
description: "Em dashes are a hallmark of AI-generated prose."
rationale: "..."             # optional
docs: "..."                  # optional; defaults to https://lawlint.com/rules/<id>
message: "Avoid em dashes"   # default message
examples:                    # optional; list of {bad, good}
  - { bad: "It was—frankly—wrong.", good: "It was, frankly, wrong." }
patterns:                    # phrase/leading/density; bare string or object
  - "—"
  - { pattern: "(?i)\\bdelve\\b", message: "…", suggestion: "examine", fix: "examine" }
allow_context: { pattern: '\d\s?–\s?\d', window: 8 }   # optional, phrase only
threshold: 8                 # density only: matches per 1000 words
metric: sentence-length      # statistical only
params: { max_words: 45 }    # statistical only
granularity: sentence        # inferential only: sentence | paragraph | document
rubric: >-                   # inferential only
  Flag hedges that carry no information about actual uncertainty. ...
flag_examples: ["...", "...", "..."]   # inferential: >= 3 required
pass_examples: ["...", "...", "..."]   # inferential: >= 3 required
```

Derived tier: phrase/leading → static; density/statistical → statistical; inferential → inferential.

**Validation is a product feature.** Errors must carry file path, field, given value, and valid alternatives in plain English, e.g.
`builtin/rules/no-em-dash.yaml: severity: "high" is not a severity — use error, warning, or suggestion`.
Rules: inferential requires rubric + ≥3 flag + ≥3 pass examples; inferential severity > warning is an error; density requires threshold + exactly one pattern; statistical requires a known metric; regexes must compile (report the regex error, never panic); duplicate ids within a package are an error.

### Registry (`registry.rs`, agent D)

```rust
pub struct RuleSet { /* defs + alias map */ }
impl RuleSet {
    pub fn built_in() -> RuleSet;                          // embedded package
    pub fn load_dir(path: &Path) -> Result<RuleSet, LoadError>;
    pub fn merge(&mut self, other: RuleSet) -> Result<(), LoadError>; // id collisions error
    pub fn resolve(&self, id_or_alias: &str) -> Option<&RuleId>;
    pub fn instantiate(&self, options: &LintOptions) -> Vec<Box<dyn Rule>>; // enable/disable/severity applied here
    pub fn metas(&self) -> Vec<&RuleMeta>;
}
```

Aliases: bare `name` resolves to `pkg/name` when unambiguous (legacy flat ids keep working in `enable`/`disable`/`severity`/`thresholds`/suppression). Ambiguity is a config error, silently preferring nothing.

## 7. Tier-3 pipeline (`judge.rs`, agent F)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubricFragment {
    pub rule: RuleId, pub severity: Severity, pub granularity: Granularity,
    pub rubric: String, pub flag_examples: Vec<String>, pub pass_examples: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity { Sentence, Paragraph, Document }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeRequest {
    pub chunk_range: TextRange, pub chunk_text: String,
    pub rules: Vec<RuleId>, pub prompt: String,
    pub cache_key_base: String, // sha256(chunk_text + rubric_set_hash + PROMPT_VERSION)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeFinding {
    pub rule: String, pub quote: String, pub explanation: String,
    pub confidence: f32, pub suggested_rewrite: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeStats {
    pub chunks: usize, pub cache_hits: usize, pub chunks_failed: usize,
    pub grounded: usize, pub hallucinated: std::collections::HashMap<String, usize>, // per rule
}

pub trait Judge: Send + Sync {
    fn evaluate(&self, req: &JudgeRequest) -> Result<Vec<JudgeFinding>, JudgeError>;
    fn model_id(&self) -> &str;
}
pub trait JudgeCache: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<JudgeFinding>>;
    fn put(&self, key: &str, findings: &[JudgeFinding]);
}
pub const PROMPT_VERSION: &str = "1";

pub fn plan_judge(doc: &Document, source: &str, rules: &[&RubricFragment]) -> Vec<JudgeRequest>;
pub fn run_judge(judge: &dyn Judge, cache: Option<&dyn JudgeCache>, reqs: &[JudgeRequest],
                 source: &str) -> (Vec<(JudgeRequest, JudgeFinding, TextRange)>, JudgeStats);
```

- **Chunking**: merge consecutive prose blocks up to ~1200 chars per chunk; one request per chunk carrying ALL sentence+paragraph-granularity rubrics (one call per chunk, not per rule). Document-granularity rubrics get one whole-document request.
- **Prompt**: rubrics with flag/pass examples + chunk text + instruction to return a JSON array of findings with verbatim `quote`s, matching the `JudgeFinding` schema exactly. Full cache key = `sha256(cache_key_base + model_id)`.
- **Run**: on malformed JSON, retry once; then fail the chunk closed (zero findings) and count `chunks_failed`.
- **Grounding** (`default_quote_ground`): (1) exact substring match of `quote` within the chunk; (2) fuzzy: best same-length char window by `strsim::normalized_levenshtein`, floor **0.9**; (3) discard + increment `hallucinated[rule]`. A finding that cannot be grounded does not exist.
- Findings whose rule isn't in the request's rule list are discarded (counted hallucinated). Confidence clamped to [0,1].
- `MemoryCache` provided in core; disk cache is the CLI's concern.
- `MockJudge` (scripted findings) in core for tests — the engine must be fully testable with zero inference.

## 8. Dispatch, scoring, suppression (integration agent)

**Dispatcher** (`dispatch.rs`): one traversal. Instantiate rules (enable/disable/severity from options applied in `instantiate`). Walk blocks → sentences → tokens; call subscribed rules whose `scope` admits the node (Prose/Text/All + citation exclusion); then `on_document_exit` for all. Collect `Report`s per rule → stamp `rule_id/severity/tier`. **Scope masking is enforced here, not in engines**: any report whose span falls outside the rule's scope mask is dropped.

**Suppression** (in dispatch): case-insensitive scan of source lines; `lawlint-disable-next-line [ids…]` (bare or inside `<!-- -->` / `//`) suppresses on the next non-blank line; `lawlint-disable [ids…]` … `lawlint-enable [ids…]` block-scoped; `lawlint-disable-file` at top. No ids = all rules. Ids resolve through aliases.

**Scoring** (`scoring.rs`) — parity-critical:
- `word_count`: regex `(?u)\b[\w'’-]+\b` over source with code-block ranges blanked.
- `sentence_count`: total Document sentences.
- Points: Error=5, Warning=3, Suggestion=1; × `weight` (default 1); tier-3 additionally × `confidence`.
- Tier-3 findings below the confidence floor (default **0.6**, `options.judge.floor`) are dropped; surviving tier-3 severity = `min(rule severity, Warning)`.
- `density = penalty / max(words,1) * 1000`; `score = round(100 * exp(-density/100)).clamp(0,100)`.
- **Golden parity** (from old test suite, must hold): mild hedging text → weight 2, score 55; heavy hedging → weight 11, score 4.

`finalize(source, diagnostics, doc) -> LintResult`: sort by span start, fill line/column/end_line/end_column (UTF-16 columns via `partition_point` over line starts) and `excerpt` (trimmed line).

**Public API** (`lib.rs`):
```rust
pub fn lint(text: &str, options: &LintOptions) -> LintResult;               // built-ins, tiers 1-2
pub fn lint_with(text: &str, options: &LintOptions, rules: &RuleSet) -> LintResult;
pub fn lint_full(text: &str, options: &LintOptions, rules: &RuleSet,
                 judge: &dyn Judge, cache: Option<&dyn JudgeCache>) -> LintResult; // + tier 3
// host-driven tier-3 for wasm: plan_judge / run_judge / ground exposed publicly
pub fn apply_fixes(text: &str, diagnostics: &[Diagnostic]) -> String; // MachineApplicable, non-overlapping, span order, single pass
```

**LintOptions** (`config.rs`, skeleton-complete):
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LintOptions {
    pub enable: Option<Vec<String>>, pub disable: Option<Vec<String>>,
    pub severity: Option<HashMap<String, Severity>>,
    pub thresholds: Option<HashMap<String, f64>>,
    pub markdown: Option<bool>,
    pub rule_dirs: Option<Vec<String>>,   // consumed by CLI/desktop, ignored by core lint()
    pub judge: Option<JudgeOptions>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JudgeOptions { pub enabled: Option<bool>, pub model: Option<String>, pub floor: Option<f32> }
```

## 9. Built-in package (agent E) — 22 rules

Recover exact regex pattern lists, messages, suggestions, and thresholds from the
old implementation: `git show HEAD:crates/lawlint-core/src/lib.rs` (functions
`built_in_rules`, `phrase`, `density`, `leading` and the bespoke rules). Port
faithfully — patterns byte-for-byte where possible. All scope `text` unless noted.

| id (core/…) | engine | severity | params |
|---|---|---|---|
| no-ai-cliches | phrase | warning | |
| no-legalese | phrase | warning | |
| no-not-only | phrase | warning | |
| no-doublets | phrase | suggestion | |
| no-marketing-language | phrase | error | |
| no-em-dash | phrase | error | pattern `—` |
| no-en-dash | phrase | error | pattern `–`, allow_context numeric range |
| no-semicolons | phrase | error | |
| oxford-comma | phrase | warning | |
| no-robotic-transitions | density | warning | threshold 18 |
| no-em-dash-overuse | density | warning | threshold 8 |
| no-rule-of-three | density | warning | threshold 12 |
| no-passive-overuse | density | warning | threshold 25 |
| no-hedging | density | warning | threshold 10 |
| no-empty-emphasis | density | warning | threshold 12 |
| no-parenthetical-asides | density | warning | threshold 15 |
| sentence-length | statistical | warning | metric sentence-length, max_words 45 |
| no-repetitive-openers | statistical | warning | metric repetitive-openers, run_length 3 |
| no-sycophantic-openers | leading | error | |
| no-throat-clearing | leading | error | |
| empty-hedge | inferential | warning | granularity sentence — NEW |
| padded-elaboration | inferential | warning | granularity paragraph — NEW |

`empty-hedge`: flag hedges carrying no information about actual uncertainty
("It could perhaps be argued that…" bad; "Damages are uncertain because treatment is ongoing" fine).
`padded-elaboration`: flag sentences/clauses that restate the previous point with
no new information (AI padding). Both need ≥3 flag + ≥3 pass examples, rubrics
written for a small local judge model (short, concrete, no meta-instructions).

## 10. `crates/lawlint-judge` (phase 2, native-only)

Implements `lawlint_core::Judge`. **ax (`axllm`) is the AI interface for ALL backends** —
the judge is one `AxJudge` whose prompt/typed-output layer (ax signatures, validation,
retry) is backend-independent; backends are `AxAIClient` implementations:

1. **`AxJudge`**: implements `lawlint_core::Judge` over `Box<dyn axllm::AxAIClient>`.
   Uses an ax signature to produce the `JudgeFinding[]` JSON contract (§7). One judge,
   any backend. Trait is sync; wrap ax's async internals in a private tokio runtime.
2. **`CandleClient` (day one, default)**: custom `AxAIClient` impl — required method is
   just `chat(&mut self, request: Value) -> AxResult<Value>` (verified, axllm v23,
   dyn-compatible). Runs **candle** inference in-process (`candle-core`,
   `candle-transformers`, `hf-hub`, `tokenizers`): quantized small instruct model
   (default Qwen2.5-1.5B-Instruct GGUF), CPU/Metal, lazy model download with progress,
   greedy/temp-0 sampling. Parses the incoming chat-completions-shaped request
   (messages → chat template) and returns a chat-completions-shaped response
   (`choices[0].message.content`).
3. **Cloud backends (feature `cloud`)**: stock ax clients — `OpenAICompatibleClient`
   (custom base URL; also covers any local OpenAI-compatible server such as a
   candle-based mistral.rs/candle-vllm sidecar, the documented fallback if
   `CandleClient` hits friction), Anthropic, Gemini, etc. Zero judge-logic changes.
4. Backend selection: `create_judge(&JudgeOptions) -> Result<Box<dyn Judge>>` keyed on
   `model` (`local:<hf-repo>` default; `anthropic:<model>`, `openai:<base-url>#<model>`, …).
Disk cache (`~/.cache/lawlint/judge/`) implementing `JudgeCache` lives here or in CLI.

## 11. Consumers (phase 2)

- **CLI**: new API; config `lawlint.config.json` → `LintOptions` (+ `ruleDirs`); flags `--rules/--disable/--markdown/--format/--max-warnings/--quiet` as today, plus `--judge`, `--fix`, `rules` (list, `--json`), **`rules test <file-or-dir>`** — runs each YAML rule's own examples (`patterns` vs `examples.bad/good`; inferential: flag/pass via judge or `--offline` skip) and reports pass/fail per example. Exit codes: 1 findings-over-limit, 2 I/O or config error.
- **WASM**: `lint(text, options)`, `builtInRulesMeta()` (now `RuleSet::metas()`), `loadRules(yamlFiles)` for playground-authored rules. Tier-3 **inference** is a host concern in the browser: wasm exports the host-driven pair `planJudge(text, options, extraRules?) -> JudgeRequest[]` and `applyJudgeFindings(text, options, requests, findingsPerRequest, extraRules?) -> LintResult` (grounding, hallucination counters, confidence floor, Warning cap all enforced inside wasm — the core invariant holds in-browser). The JS host runs inference however it likes (transformers.js/WebLLM on WebGPU, or cloud). In-process candle-wasm is a possible later addition, not the browser default.
- **Desktop**: keep compiling against new `lint`.

## 12. Testing requirements

- Every module has colocated `#[cfg(test)]` unit tests.
- Integration agent ports the ENTIRE old test suite (`git show HEAD:crates/lawlint-core/src/lib.rs`, tests module) — updating ids to `core/…`, Severity `Info`→`Suggestion`, rule count 20→22 — and keeps golden score/weight parity cases exact. Sentence-count expectations may legitimately shift with legal segmentation; document any change in the test.
- JSON field-name contract test: `ruleId`, `endLine`, `endColumn`, `wordCount`, lowercase severities.
- Judge pipeline fully tested with `MockJudge`: grounding (exact, fuzzy at boundary 0.9, discard), fail-closed retry, cache hit determinism, confidence floor, severity cap.
- Segmentation: legal abbreviation corpus test with ≥15 tricky cases.
```
