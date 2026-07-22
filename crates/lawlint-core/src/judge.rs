//! Tier-3 (inferential) pipeline. Types verbatim from docs/engine-design.md §7
//! [skeleton — complete]; plan/run/ground + MockJudge/MemoryCache bodies are
//! agent F's.
//!
//! Invariant: a judge finding that cannot be **grounded** to a source span
//! does not exist. Core stays inference-agnostic and wasm-safe; real judge
//! backends live in `crates/lawlint-judge` (native only).

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::document::{BlockKind, Document};
use crate::error::JudgeError;
use crate::types::{RuleId, Severity, TextRange};

/// Bump when the prompt template changes; part of every cache key.
/// 3: text moved above the rubrics so per-rule requests share a cacheable
/// prefix.
pub const PROMPT_VERSION: &str = "3";

/// Default chars of document text per request when no profile says otherwise.
/// Sized for the smallest supported backend; [`JudgePlan::for_context`] raises
/// it for models that can hold more.
const TARGET_CHUNK_CHARS: usize = 1200;

/// Minimum `normalized_levenshtein` similarity for fuzzy grounding.
const GROUND_FLOOR: f64 = 0.9;

/// Quotes longer than this (in chars) never take the fuzzy fallback: the
/// window scan costs O(chunk_chars × quote_chars²), quotes are untrusted
/// model output, and a single long hallucinated quote must not hang the lint
/// run (and re-hang it on every cache hit, since raw findings are cached
/// before grounding). Long quotes still ground via exact substring match —
/// which is what a genuinely verbatim quote does anyway.
const MAX_FUZZY_QUOTE_CHARS: usize = 400;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubricFragment {
    pub rule: RuleId,
    pub severity: Severity,
    pub granularity: Granularity,
    pub rubric: String,
    pub flag_examples: Vec<String>,
    pub pass_examples: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Sentence,
    Paragraph,
    Document,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeRequest {
    pub chunk_range: TextRange,
    pub chunk_text: String,
    pub rules: Vec<RuleId>,
    pub prompt: String,
    /// sha256(chunk_text + rubric_set_hash + PROMPT_VERSION).
    /// Full cache key = sha256(cache_key_base + model_id).
    pub cache_key_base: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeFinding {
    pub rule: String,
    pub quote: String,
    pub explanation: String,
    pub confidence: f32,
    pub suggested_rewrite: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeStats {
    pub chunks: usize,
    pub cache_hits: usize,
    pub chunks_failed: usize,
    pub grounded: usize,
    /// Discarded (ungroundable / unknown-rule) findings, per rule.
    pub hallucinated: std::collections::HashMap<String, usize>,
    /// Why the first failed chunk failed. Chunks fail closed and the run
    /// continues, so without this the user sees a count with no cause — and
    /// the causes (a truncated reasoning model, a bad key, a 404 deployment)
    /// need very different fixes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_failure: Option<String>,
}

pub trait Judge: Send + Sync {
    fn evaluate(&self, req: &JudgeRequest) -> Result<Vec<JudgeFinding>, JudgeError>;
    fn model_id(&self) -> &str;

    /// How many requests this judge can serve at once. The default of 1 keeps
    /// every existing backend sequential; a backend opts in only if it can
    /// genuinely serve concurrent calls (a hosted API with a client per
    /// worker can, a single in-process local model cannot).
    fn max_concurrency(&self) -> usize {
        1
    }
}

pub trait JudgeCache: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<JudgeFinding>>;
    fn put(&self, key: &str, findings: &[JudgeFinding]);
}

// ---- Hashing helpers ---------------------------------------------------

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Deterministic hash over an ordered rubric set (all fields, via JSON).
fn rubric_set_hash(rubrics: &[&RubricFragment]) -> String {
    let json = serde_json::to_string(rubrics).expect("RubricFragment serializes to JSON");
    sha256_hex(&json)
}

/// Full cache key for a request under a specific model.
fn full_cache_key(cache_key_base: &str, model_id: &str) -> String {
    sha256_hex(&format!("{cache_key_base}{model_id}"))
}

// ---- Prompt ------------------------------------------------------------

fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Suggestion => "suggestion",
    }
}

fn granularity_name(g: Granularity) -> &'static str {
    match g {
        Granularity::Sentence => "sentence",
        Granularity::Paragraph => "paragraph",
        Granularity::Document => "document",
    }
}

/// Small deterministic prompt template: the text under review, the rubrics
/// with flag/pass examples, and a strict instruction to emit a JSON array
/// matching the `JudgeFinding` schema with verbatim quotes.
///
/// **Field order is load-bearing.** The constant preamble and the unit text
/// come first, the rule-specific rubric last, so every per-rule request over
/// the same unit shares a byte-identical prefix. Providers bill a cached
/// prefix at a discount, which is what keeps one-request-per-rule affordable:
/// the marginal cost of rule N+1 is its rubric, not another copy of the
/// document. Moving the rubric above the text silently forfeits that.
fn build_prompt(chunk_text: &str, rubrics: &[&RubricFragment]) -> String {
    let mut p = String::new();
    p.push_str(
        "You are a strict legal-writing reviewer. Evaluate the text below against \
         each rule that follows it. Only report real violations.\n\n",
    );
    p.push_str("Text to evaluate:\n<<<\n");
    p.push_str(chunk_text);
    p.push_str("\n>>>\n\n");
    for r in rubrics {
        let _ = writeln!(
            p,
            "Rule `{}` (severity: {}, granularity: {}):",
            r.rule.0,
            severity_name(r.severity),
            granularity_name(r.granularity)
        );
        let _ = writeln!(p, "Rubric: {}", r.rubric);
        p.push_str("Flag examples (violations):\n");
        for ex in &r.flag_examples {
            let _ = writeln!(p, "- {ex}");
        }
        p.push_str("Pass examples (acceptable, do NOT flag):\n");
        for ex in &r.pass_examples {
            let _ = writeln!(p, "- {ex}");
        }
        p.push('\n');
    }
    // The explicit clean-chunk example and the "never describe a pass" line
    // target a small-model failure mode: emitting the pass verdict as a
    // finding object ("The text does not flag any empty hedge") instead of
    // returning [] — see the parse-time polarity guard in lawlint-judge.
    p.push_str(
        "Respond with ONLY a JSON array (no prose, no code fences). Each element \
         must be an object of the form {\"rule\": \"<one of the rule ids above>\", \
         \"quote\": \"<excerpt copied VERBATIM from the text>\", \"explanation\": \
         \"<one short sentence stating the violation>\", \"confidence\": <number \
         between 0.0 and 1.0>, \"suggested_rewrite\": \"<replacement text>\" or \
         null}. The quote must appear verbatim in the text. Report ONLY \
         violations: never emit an object stating that a rule is satisfied, not \
         violated, or not present. If nothing violates any rule, respond with \
         exactly []. Example — for the clean text \"The parties shall meet on \
         the first business day of each month.\" the entire correct response \
         is:\n[]",
    );
    p
}

fn build_request(range: TextRange, text: String, rubrics: &[&RubricFragment]) -> JudgeRequest {
    let set_hash = rubric_set_hash(rubrics);
    let cache_key_base = sha256_hex(&format!("{text}{set_hash}{PROMPT_VERSION}"));
    let prompt = build_prompt(&text, rubrics);
    JudgeRequest {
        chunk_range: range,
        chunk_text: text,
        rules: rubrics.iter().map(|r| r.rule.clone()).collect(),
        prompt,
        cache_key_base,
    }
}

// ---- Planning ----------------------------------------------------------

/// How requests are cut for a given backend.
///
/// This is the knob that lets one pipeline serve a 1.5B local model and a
/// frontier hosted model well. Everything model-shaped lives here rather than
/// in constants, so a better model is a config change, not a rewrite; core
/// itself stays inference-agnostic and never maps model names to profiles
/// (that belongs to `lawlint-judge`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JudgePlan {
    /// Max chars of document text per request.
    pub context_chars: usize,
    /// One request per rule instead of one per unit carrying every rubric.
    ///
    /// Per-rule requests give each rule the model's undivided attention and
    /// its own cache entry, so adding a rule no longer invalidates the others.
    /// They cost one request per rule per unit, which prompt-prefix caching
    /// makes cheap — see [`build_prompt`] on why the text comes first.
    pub per_rule: bool,
}

impl Default for JudgePlan {
    /// The conservative profile: small units, rubrics bundled. Correct for the
    /// weakest supported backend, which is what an unconfigured caller gets.
    fn default() -> Self {
        JudgePlan {
            context_chars: TARGET_CHUNK_CHARS,
            per_rule: false,
        }
    }
}

impl JudgePlan {
    /// A profile for a backend that can hold `context_chars` of document text
    /// per request. Anything above the conservative default is taken to be a
    /// model worth addressing one rule at a time.
    pub fn for_context(context_chars: usize) -> Self {
        JudgePlan {
            context_chars: context_chars.max(1),
            per_rule: context_chars > TARGET_CHUNK_CHARS,
        }
    }
}

/// Merge runs of consecutive prose blocks (paragraphs + list items) into
/// chunks of up to ~`context_chars` chars. Non-prose blocks (headings,
/// block quotes, code) break a run and are never included. So does any
/// non-whitespace source between two blocks: markdown constructs that emit no
/// `Block` at all (HTML blocks, thematic breaks, tables) must neither be
/// merged into a chunk's text nor bridge two runs. A single oversized block
/// becomes its own chunk (blocks are never split).
fn chunk_prose_blocks(doc: &Document, source: &str, context_chars: usize) -> Vec<TextRange> {
    let mut chunks = Vec::new();
    let mut current: Option<TextRange> = None;
    let mut current_chars = 0usize;
    for block in &doc.blocks {
        let prose = matches!(block.kind, BlockKind::Paragraph | BlockKind::ListItem);
        if !prose {
            if let Some(r) = current.take() {
                chunks.push(r);
            }
            current_chars = 0;
            continue;
        }
        let block_chars = block.range.slice(source).chars().count();
        // The gap between the current chunk and this block must be pure
        // whitespace, or merging would embed invisible non-block source
        // (raw HTML, `---`, …) in the text sent to the judge.
        let gap_is_blank = |r: &TextRange| {
            source
                .get(r.end..block.range.start)
                .is_some_and(|gap| gap.chars().all(char::is_whitespace))
        };
        match current {
            Some(ref mut r) if current_chars + block_chars <= context_chars && gap_is_blank(r) => {
                r.end = block.range.end;
                current_chars += block_chars;
            }
            Some(r) => {
                chunks.push(r);
                current = Some(block.range);
                current_chars = block_chars;
            }
            None => {
                current = Some(block.range);
                current_chars = block_chars;
            }
        }
    }
    if let Some(r) = current {
        chunks.push(r);
    }
    chunks
}

/// [`plan_judge_with`] under the conservative [`JudgePlan::default`].
pub fn plan_judge(doc: &Document, source: &str, rules: &[&RubricFragment]) -> Vec<JudgeRequest> {
    plan_judge_with(doc, source, rules, &JudgePlan::default())
}

/// Plan tier-3 requests under `plan`: merge consecutive prose blocks into
/// units of up to `plan.context_chars`, then emit either one request per unit
/// carrying every sentence/paragraph rubric, or — when `plan.per_rule` —
/// one request per rule per unit. Document-granularity rubrics always see the
/// whole source, since that is the point of the granularity.
///
/// Requests come back in a deterministic order (units in document order,
/// rules in rule order within each unit) so a plan is reproducible and its
/// findings land in a stable sequence.
pub fn plan_judge_with<'a>(
    doc: &Document,
    source: &str,
    rules: &[&'a RubricFragment],
    plan: &JudgePlan,
) -> Vec<JudgeRequest> {
    let chunk_rubrics: Vec<&RubricFragment> = rules
        .iter()
        .copied()
        .filter(|r| {
            matches!(
                r.granularity,
                Granularity::Sentence | Granularity::Paragraph
            )
        })
        .collect();
    let doc_rubrics: Vec<&RubricFragment> = rules
        .iter()
        .copied()
        .filter(|r| r.granularity == Granularity::Document)
        .collect();

    // One request per rubric, or one carrying them all.
    let split = |rubrics: &[&'a RubricFragment]| -> Vec<Vec<&'a RubricFragment>> {
        if plan.per_rule {
            rubrics.iter().map(|r| vec![*r]).collect()
        } else {
            vec![rubrics.to_vec()]
        }
    };

    let mut reqs = Vec::new();
    if !chunk_rubrics.is_empty() {
        for range in chunk_prose_blocks(doc, source, plan.context_chars) {
            let text = range.slice(source);
            if text.trim().is_empty() {
                continue;
            }
            for group in split(&chunk_rubrics) {
                reqs.push(build_request(range, text.to_string(), &group));
            }
        }
    }
    if !doc_rubrics.is_empty() && !source.trim().is_empty() {
        let range = TextRange {
            start: 0,
            end: source.len(),
        };
        for group in split(&doc_rubrics) {
            reqs.push(build_request(range, source.to_string(), &group));
        }
    }
    reqs
}

// ---- Execution ---------------------------------------------------------

/// Run planned requests through `judge` (with optional cache), ground each
/// finding to a source span, and collect stats. On backend error or malformed
/// output, retry once; then fail the chunk closed (zero findings) and count
/// `chunks_failed`.
pub fn run_judge(
    judge: &dyn Judge,
    cache: Option<&dyn JudgeCache>,
    reqs: &[JudgeRequest],
    source: &str,
) -> (Vec<(JudgeRequest, JudgeFinding, TextRange)>, JudgeStats) {
    run_judge_with_progress(judge, cache, reqs, source, &mut |_, _| {})
}

/// What one request produced: cached or fetched findings, or the error that
/// failed the chunk closed. Fetching is the only concurrent stage; grounding
/// and stats stay on one thread, so results must be carried out of that stage
/// rather than folded into `JudgeStats` as they arrive.
enum Fetched {
    Cached(Vec<JudgeFinding>),
    Fresh(Vec<JudgeFinding>),
    Failed(String),
}

/// Cache lookup, else evaluate with one retry, else fail closed. Pure with
/// respect to `JudgeStats` — the caller folds the outcome in request order so
/// a parallel fetch cannot reorder stats or findings.
fn fetch_one(judge: &dyn Judge, cache: Option<&dyn JudgeCache>, req: &JudgeRequest) -> Fetched {
    let key = full_cache_key(&req.cache_key_base, judge.model_id());
    if let Some(hit) = cache.and_then(|c| c.get(&key)) {
        return Fetched::Cached(hit);
    }
    match judge.evaluate(req).or_else(|_| judge.evaluate(req)) {
        Ok(findings) => {
            if let Some(c) = cache {
                c.put(&key, &findings);
            }
            Fetched::Fresh(findings)
        }
        Err(error) => Fetched::Failed(error.to_string()),
    }
}

/// Fetch every request sequentially, reporting progress as each completes.
fn fetch_serial(
    judge: &dyn Judge,
    cache: Option<&dyn JudgeCache>,
    reqs: &[JudgeRequest],
    on_progress: &mut dyn FnMut(usize, usize),
) -> Vec<Fetched> {
    let total = reqs.len();
    let mut out = Vec::with_capacity(total);
    for (index, req) in reqs.iter().enumerate() {
        out.push(fetch_one(judge, cache, req));
        on_progress(index + 1, total);
    }
    out
}

/// Fetch up to `workers` requests at a time, preserving request order in the
/// returned slots.
///
/// Progress is reported from the calling thread over a channel rather than
/// from the workers, so `on_progress` needs no `Send` bound and front-ends
/// keep drawing from the thread they already own. Scoped threads let the
/// workers borrow `judge`/`cache`/`reqs` directly (all `Sync`), so nothing
/// here clones a request or a client.
#[cfg(not(target_arch = "wasm32"))]
fn fetch_parallel(
    judge: &dyn Judge,
    cache: Option<&dyn JudgeCache>,
    reqs: &[JudgeRequest],
    workers: usize,
    on_progress: &mut dyn FnMut(usize, usize),
) -> Vec<Fetched> {
    let total = reqs.len();
    let slots: Vec<Mutex<Option<Fetched>>> = (0..total).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<()>();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let (next, slots) = (&next, &slots);
            scope.spawn(move || loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(req) = reqs.get(index) else { break };
                let fetched = fetch_one(judge, cache, req);
                *slots[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(fetched);
                // A closed receiver just means nobody is drawing progress.
                let _ = tx.send(());
            });
        }
        // Drop this thread's sender so the drain below ends when the last
        // worker exits, then report each completion as it lands.
        drop(tx);
        let mut done = 0;
        while rx.recv().is_ok() {
            done += 1;
            on_progress(done, total);
        }
    });

    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(|e| e.into_inner())
                // A worker that panicked leaves its slot empty; fail that
                // chunk closed rather than losing every other result.
                .unwrap_or_else(|| Fetched::Failed("judge worker panicked".to_string()))
        })
        .collect()
}

/// [`run_judge`], reporting `(completed, total)` after each chunk so a caller
/// can drive a progress indicator. Each request is a network round trip, so
/// without this a multi-section document looks like a hung process.
///
/// Requests run up to [`Judge::max_concurrency`] at a time. Findings and stats
/// are folded in request order regardless, so the diagnostics a document
/// produces never depend on how the fetches interleaved — a linter that
/// reordered its own output under concurrency would be unusable in CI.
pub fn run_judge_with_progress(
    judge: &dyn Judge,
    cache: Option<&dyn JudgeCache>,
    reqs: &[JudgeRequest],
    source: &str,
    on_progress: &mut dyn FnMut(usize, usize),
) -> (Vec<(JudgeRequest, JudgeFinding, TextRange)>, JudgeStats) {
    let mut out = Vec::new();
    let mut stats = JudgeStats {
        chunks: reqs.len(),
        ..JudgeStats::default()
    };
    let total = reqs.len();
    on_progress(0, total);

    // The worker count is scoped to this branch: on wasm it would be a dead
    // binding, and core builds for wasm with `-D warnings`.
    #[cfg(not(target_arch = "wasm32"))]
    let fetched = {
        let workers = judge.max_concurrency().clamp(1, total.max(1));
        if workers > 1 {
            fetch_parallel(judge, cache, reqs, workers, on_progress)
        } else {
            fetch_serial(judge, cache, reqs, on_progress)
        }
    };
    // wasm has no threads; core must stay wasm-safe (design invariant 7).
    #[cfg(target_arch = "wasm32")]
    let fetched = fetch_serial(judge, cache, reqs, on_progress);

    for (req, fetched) in reqs.iter().zip(fetched) {
        debug_assert!(
            req.chunk_range.start <= req.chunk_range.end && req.chunk_range.end <= source.len(),
            "chunk_range must index the original source"
        );
        let findings = match fetched {
            Fetched::Cached(findings) => {
                stats.cache_hits += 1;
                findings
            }
            Fetched::Fresh(findings) => findings,
            Fetched::Failed(error) => {
                stats.chunks_failed += 1;
                stats.first_failure.get_or_insert(error);
                continue;
            }
        };
        for mut finding in findings {
            // Findings naming rules not in this request do not exist.
            if !req.rules.iter().any(|r| r.0 == finding.rule) {
                *stats.hallucinated.entry(finding.rule.clone()).or_insert(0) += 1;
                continue;
            }
            finding.confidence = if finding.confidence.is_nan() {
                0.0
            } else {
                finding.confidence.clamp(0.0, 1.0)
            };
            match default_quote_ground(&finding.quote, &req.chunk_text, req.chunk_range) {
                Some(span) => {
                    stats.grounded += 1;
                    out.push((req.clone(), finding, span));
                }
                None => {
                    *stats.hallucinated.entry(finding.rule.clone()).or_insert(0) += 1;
                }
            }
        }
    }
    (out, stats)
}

// ---- Grounding ---------------------------------------------------------

/// Ground a finding's `quote` inside its chunk: (1) exact substring match;
/// (2) fuzzy (quotes up to `MAX_FUZZY_QUOTE_CHARS` chars only): best
/// same-char-length window by `strsim::normalized_levenshtein`, floor 0.9;
/// (3) `None` → discard and count `hallucinated[rule]`. Returned range is
/// absolute (original source byte offsets).
pub fn default_quote_ground(
    quote: &str,
    chunk_text: &str,
    chunk_range: TextRange,
) -> Option<TextRange> {
    if quote.is_empty() {
        return None;
    }
    // (1) Exact substring.
    if let Some(idx) = chunk_text.find(quote) {
        return Some(TextRange {
            start: chunk_range.start + idx,
            end: chunk_range.start + idx + quote.len(),
        });
    }
    // (2) Fuzzy: best window of the same char length, one window per char
    // position (O(chunk chars) windows), respecting char boundaries.
    let quote_chars = quote.chars().count();
    if quote_chars > MAX_FUZZY_QUOTE_CHARS {
        return None;
    }
    let boundaries: Vec<usize> = chunk_text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(chunk_text.len()))
        .collect();
    let chunk_chars = boundaries.len() - 1;
    if quote_chars > chunk_chars {
        return None;
    }
    // Rolling char-multiset ("bag") prefilter, maintained in O(1) per slide:
    // `surplus` = number of chars the window has that the quote's multiset
    // lacks, a lower bound on the true Levenshtein distance. Windows whose
    // bound already puts the score below `GROUND_FLOOR` skip the O(quote²)
    // Levenshtein entirely, so scanning for a hallucinated quote stays
    // ~O(chunk chars) instead of O(chunk × quote²).
    let chars: Vec<char> = chunk_text.chars().collect();
    let mut diff: HashMap<char, i32> = HashMap::new();
    for c in quote.chars() {
        *diff.entry(c).or_insert(0) -= 1;
    }
    let mut surplus: i64 = 0;
    for &c in &chars[..quote_chars] {
        let e = diff.entry(c).or_insert(0);
        *e += 1;
        if *e > 0 {
            surplus += 1;
        }
    }
    let mut best: Option<(f64, usize)> = None;
    for start in 0..=(chunk_chars - quote_chars) {
        if start > 0 {
            let out = chars[start - 1];
            let e = diff.entry(out).or_insert(0);
            if *e > 0 {
                surplus -= 1;
            }
            *e -= 1;
            let inc = chars[start + quote_chars - 1];
            let e = diff.entry(inc).or_insert(0);
            *e += 1;
            if *e > 0 {
                surplus += 1;
            }
        }
        // Same formula strsim uses (both strings have `quote_chars` chars),
        // so the at-the-floor comparison is bit-exact: distance ≥ surplus
        // implies score ≤ bound.
        let bound = 1.0 - surplus as f64 / quote_chars as f64;
        if bound < GROUND_FLOOR {
            continue;
        }
        let window = &chunk_text[boundaries[start]..boundaries[start + quote_chars]];
        let score = strsim::normalized_levenshtein(quote, window);
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, start));
        }
    }
    let (score, start) = best?;
    if score >= GROUND_FLOOR {
        Some(TextRange {
            start: chunk_range.start + boundaries[start],
            end: chunk_range.start + boundaries[start + quote_chars],
        })
    } else {
        None
    }
}

// ---- MockJudge ---------------------------------------------------------

#[derive(Debug)]
enum ScriptedResponse {
    Findings(Vec<JudgeFinding>),
    BackendError(String),
    Malformed(String),
}

/// Scripted judge for tests — the engine must be fully testable with zero
/// inference.
///
/// Script entries pair a chunk-text substring matcher with a FIFO queue of
/// responses; `evaluate` pops the next response from the first matching
/// non-empty queue (an empty matcher matches every request). Requests with no
/// scripted response return `Ok(vec![])`. Queued responses per matcher allow
/// scripting retry sequences (error first, findings second).
#[derive(Debug)]
pub struct MockJudge {
    model: String,
    script: Mutex<Vec<(String, VecDeque<ScriptedResponse>)>>,
    calls: AtomicUsize,
}

impl MockJudge {
    pub fn new() -> Self {
        Self::with_model("mock")
    }

    /// A mock with a specific model id (cache keys include the model id).
    pub fn with_model(model: impl Into<String>) -> Self {
        MockJudge {
            model: model.into(),
            script: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        }
    }

    /// Queue a successful response for requests whose chunk text contains
    /// `matcher` ("" matches all requests).
    pub fn respond(self, matcher: &str, findings: Vec<JudgeFinding>) -> Self {
        self.push(matcher, ScriptedResponse::Findings(findings))
    }

    /// Queue a backend error for matching requests.
    pub fn respond_err(self, matcher: &str, message: &str) -> Self {
        self.push(matcher, ScriptedResponse::BackendError(message.to_string()))
    }

    /// Queue a malformed-response error for matching requests.
    pub fn respond_malformed(self, matcher: &str, raw: &str) -> Self {
        self.push(matcher, ScriptedResponse::Malformed(raw.to_string()))
    }

    /// Total number of `evaluate` calls received (cache-hit assertions).
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn push(self, matcher: &str, response: ScriptedResponse) -> Self {
        {
            let mut script = self.script.lock().unwrap();
            if let Some((_, queue)) = script.iter_mut().find(|(m, _)| m == matcher) {
                queue.push_back(response);
            } else {
                let mut queue = VecDeque::new();
                queue.push_back(response);
                script.push((matcher.to_string(), queue));
            }
        }
        self
    }
}

impl Default for MockJudge {
    fn default() -> Self {
        MockJudge::new()
    }
}

impl Judge for MockJudge {
    fn evaluate(&self, req: &JudgeRequest) -> Result<Vec<JudgeFinding>, JudgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut script = self.script.lock().unwrap();
        for (matcher, queue) in script.iter_mut() {
            if matcher.is_empty() || req.chunk_text.contains(matcher.as_str()) {
                if let Some(response) = queue.pop_front() {
                    return match response {
                        ScriptedResponse::Findings(f) => Ok(f),
                        ScriptedResponse::BackendError(m) => Err(JudgeError::Backend(m)),
                        ScriptedResponse::Malformed(m) => Err(JudgeError::MalformedResponse(m)),
                    };
                }
            }
        }
        Ok(Vec::new())
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

// ---- MemoryCache -------------------------------------------------------

/// In-memory `JudgeCache` provided by core; disk cache is the CLI's concern.
#[derive(Debug, Default)]
pub struct MemoryCache {
    inner: Mutex<HashMap<String, Vec<JudgeFinding>>>,
}

impl MemoryCache {
    pub fn new() -> Self {
        MemoryCache::default()
    }
}

impl JudgeCache for MemoryCache {
    fn get(&self, key: &str) -> Option<Vec<JudgeFinding>> {
        self.inner.lock().unwrap().get(key).cloned()
    }
    fn put(&self, key: &str, findings: &[JudgeFinding]) {
        self.inner
            .lock()
            .unwrap()
            .insert(key.to_string(), findings.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Block, BlockKind};

    // ---- Test helpers --------------------------------------------------

    fn frag(rule: &str, granularity: Granularity) -> RubricFragment {
        RubricFragment {
            rule: RuleId(rule.to_string()),
            severity: Severity::Warning,
            granularity,
            rubric: format!("Rubric text for {rule}."),
            flag_examples: vec!["flag one".into(), "flag two".into(), "flag three".into()],
            pass_examples: vec!["pass one".into(), "pass two".into(), "pass three".into()],
        }
    }

    fn finding(rule: &str, quote: &str, confidence: f32) -> JudgeFinding {
        JudgeFinding {
            rule: rule.to_string(),
            quote: quote.to_string(),
            explanation: "because".into(),
            confidence,
            suggested_rewrite: None,
        }
    }

    fn req(text: &str, rules: &[&str]) -> JudgeRequest {
        JudgeRequest {
            chunk_range: TextRange {
                start: 0,
                end: text.len(),
            },
            chunk_text: text.to_string(),
            rules: rules.iter().map(|r| RuleId(r.to_string())).collect(),
            prompt: format!("PROMPT for: {text}"),
            cache_key_base: format!("base:{text}:{}", rules.join(",")),
        }
    }

    /// Build a Document of Paragraph blocks from paragraphs split on "\n\n".
    /// Ranges index `source` exactly; sentences are irrelevant to planning.
    fn para_doc(source: &str) -> Document {
        let mut blocks = Vec::new();
        let mut offset = 0usize;
        for part in source.split("\n\n") {
            if !part.trim().is_empty() {
                blocks.push(Block {
                    kind: BlockKind::Paragraph,
                    range: TextRange {
                        start: offset,
                        end: offset + part.len(),
                    },
                    sentences: Vec::new(),
                });
            }
            offset += part.len() + 2;
        }
        Document { blocks }
    }

    fn block(kind: BlockKind, start: usize, end: usize) -> Block {
        Block {
            kind,
            range: TextRange { start, end },
            sentences: Vec::new(),
        }
    }

    // ---- Skeleton tests (kept) -----------------------------------------

    #[test]
    fn granularity_serde_lowercase() {
        assert_eq!(
            serde_json::to_string(&Granularity::Sentence).unwrap(),
            "\"sentence\""
        );
        assert_eq!(
            serde_json::to_string(&Granularity::Paragraph).unwrap(),
            "\"paragraph\""
        );
        assert_eq!(
            serde_json::to_string(&Granularity::Document).unwrap(),
            "\"document\""
        );
        let g: Granularity = serde_json::from_str("\"paragraph\"").unwrap();
        assert_eq!(g, Granularity::Paragraph);
    }

    #[test]
    fn judge_stats_default_and_camel_case() {
        let s = JudgeStats::default();
        assert_eq!(s.chunks, 0);
        assert!(s.hallucinated.is_empty());
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("cacheHits").is_some());
        assert!(v.get("chunksFailed").is_some());
        assert!(v.get("grounded").is_some());
        assert!(v.get("hallucinated").is_some());
    }

    #[test]
    fn judge_finding_round_trips() {
        let f = JudgeFinding {
            rule: "core/empty-hedge".into(),
            quote: "It could perhaps be argued".into(),
            explanation: "hedge with no information".into(),
            confidence: 0.8,
            suggested_rewrite: None,
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: JudgeFinding = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rule, f.rule);
        assert_eq!(back.quote, f.quote);
        assert_eq!(back.confidence, f.confidence);
        assert!(back.suggested_rewrite.is_none());
    }

    #[test]
    fn prompt_version_is_stable() {
        // "2": clean-chunk [] example + verdict-discipline instruction (#39).
        // "3": text hoisted above the rubrics so per-rule requests over one
        //      unit share a cacheable prompt prefix.
        assert_eq!(PROMPT_VERSION, "3");
    }

    #[test]
    fn mock_judge_model_id() {
        assert_eq!(MockJudge::new().model_id(), "mock");
    }

    // ---- Grounding -----------------------------------------------------

    #[test]
    fn ground_exact_substring_absolute_offsets() {
        let chunk = "The court held that this applies broadly.";
        let range = TextRange {
            start: 100,
            end: 100 + chunk.len(),
        };
        let got = default_quote_ground("held that", chunk, range).unwrap();
        assert_eq!(
            got,
            TextRange {
                start: 110,
                end: 119
            }
        );
        assert_eq!(&chunk[10..19], "held that");
    }

    #[test]
    fn ground_exact_with_multibyte_prefix() {
        let chunk = "It was—frankly—wrong in every way.";
        let range = TextRange {
            start: 50,
            end: 50 + chunk.len(),
        };
        let idx = chunk.find("wrong").unwrap();
        let got = default_quote_ground("wrong", chunk, range).unwrap();
        assert_eq!(got.start, 50 + idx);
        assert_eq!(got.end, 50 + idx + "wrong".len());
    }

    #[test]
    fn ground_fuzzy_at_floor_accepts() {
        // 10-char quote vs 10-char window with 1 substitution: 1 - 1/10 = 0.9,
        // exactly at the floor — must ground.
        let chunk = "zz abcdefghiX zz";
        let range = TextRange {
            start: 7,
            end: 7 + chunk.len(),
        };
        let got = default_quote_ground("abcdefghij", chunk, range).unwrap();
        assert_eq!(
            got,
            TextRange {
                start: 7 + 3,
                end: 7 + 13
            }
        );
        assert_eq!(&chunk[3..13], "abcdefghiX");
    }

    #[test]
    fn ground_fuzzy_below_floor_discards() {
        // 9-char quote, 1 substitution: 1 - 1/9 ≈ 0.889 < 0.9 — must discard.
        let chunk = "zz abcdefghX zz";
        let range = TextRange {
            start: 0,
            end: chunk.len(),
        };
        assert!(default_quote_ground("abcdefghi", chunk, range).is_none());
    }

    #[test]
    fn ground_fuzzy_respects_char_boundaries() {
        // Multibyte é inside both quote and window; sliding must not panic
        // and the returned range must cover the whole multibyte window.
        let chunk = "so café latte is great indeed";
        let range = TextRange {
            start: 10,
            end: 10 + chunk.len(),
        };
        // 19 chars, 1 substitution (greet/great): 1 - 1/19 ≈ 0.947 >= 0.9.
        let got = default_quote_ground("café latte is greet", chunk, range).unwrap();
        let rel = TextRange {
            start: got.start - 10,
            end: got.end - 10,
        };
        assert_eq!(rel.slice(chunk), "café latte is great");
    }

    #[test]
    fn ground_long_quotes_are_exact_only() {
        // Over MAX_FUZZY_QUOTE_CHARS: exact match still grounds…
        let quote = "q".repeat(MAX_FUZZY_QUOTE_CHARS + 1);
        let chunk = format!("prefix {quote} suffix");
        let range = TextRange {
            start: 0,
            end: chunk.len(),
        };
        let got = default_quote_ground(&quote, &chunk, range).unwrap();
        assert_eq!(got.slice(&chunk), quote);
        // …but a near-miss is discarded instead of running the
        // O(chunk × quote²) fuzzy window scan on untrusted model output.
        let near = format!("X{}", &quote[1..]);
        assert!(default_quote_ground(&near, &chunk, range).is_none());
    }

    #[test]
    fn ground_hallucinated_quote_in_large_chunk_is_fast() {
        // Regression: a ~360-char ungroundable quote against an ~18k-char
        // chunk used to run Levenshtein for every window (minutes in a lint
        // run). The bag-distance prefilter must discard every window, so this
        // test completes instantly; without it, it visibly hangs.
        let chunk = "The court held that the motion fails on procedural grounds. ".repeat(300);
        let quote = "zzz ".repeat(90);
        let range = TextRange {
            start: 0,
            end: chunk.len(),
        };
        assert!(default_quote_ground(quote.trim(), &chunk, range).is_none());
    }

    #[test]
    fn ground_garbage_and_edge_quotes_discard() {
        let chunk = "short text";
        let range = TextRange {
            start: 0,
            end: chunk.len(),
        };
        assert!(default_quote_ground("completely unrelated words here", chunk, range).is_none());
        assert!(default_quote_ground("", chunk, range).is_none());
        // Quote longer (in chars) than the chunk: no window exists.
        assert!(default_quote_ground("short text but much longer", chunk, range).is_none());
    }

    // ---- Planning ------------------------------------------------------

    #[test]
    fn plan_merges_consecutive_small_paragraphs() {
        let source = "First paragraph here.\n\nSecond paragraph here.";
        let doc = para_doc(source);
        let rules = [frag("core/empty-hedge", Granularity::Sentence)];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let reqs = plan_judge(&doc, source, &refs);
        assert_eq!(reqs.len(), 1);
        assert_eq!(
            reqs[0].chunk_range,
            TextRange {
                start: 0,
                end: source.len()
            }
        );
        assert_eq!(reqs[0].chunk_text, source);
        assert_eq!(reqs[0].rules, vec![RuleId("core/empty-hedge".into())]);
    }

    #[test]
    fn plan_splits_when_over_target_chars() {
        // Three ~500-char paragraphs: 500+500 <= 1200 merge, third overflows.
        let para = "x".repeat(500);
        let source = format!("{para}\n\n{para}\n\n{para}");
        let doc = para_doc(&source);
        let rules = [frag("core/empty-hedge", Granularity::Sentence)];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let reqs = plan_judge(&doc, &source, &refs);
        assert_eq!(reqs.len(), 2);
        assert_eq!(
            reqs[0].chunk_range,
            TextRange {
                start: 0,
                end: 1002
            }
        ); // both paras + "\n\n"
        assert_eq!(
            reqs[1].chunk_range,
            TextRange {
                start: 1004,
                end: 1504
            }
        );
        for r in &reqs {
            assert_eq!(r.chunk_text, r.chunk_range.slice(&source));
        }
    }

    #[test]
    fn plan_non_prose_blocks_break_runs_and_are_excluded() {
        // paragraph, heading, paragraph → two chunks, heading excluded.
        let source = "Para one text.\n\n# Heading\n\nPara two text.";
        let doc = Document {
            blocks: vec![
                block(BlockKind::Paragraph, 0, 14),
                block(BlockKind::Heading, 16, 25),
                block(BlockKind::Paragraph, 27, 41),
            ],
        };
        let rules = [frag("core/empty-hedge", Granularity::Sentence)];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let reqs = plan_judge(&doc, source, &refs);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].chunk_text, "Para one text.");
        assert_eq!(reqs[1].chunk_text, "Para two text.");
        assert!(!reqs.iter().any(|r| r.chunk_text.contains("Heading")));
    }

    #[test]
    fn plan_breaks_runs_on_invisible_non_blocks() {
        // HTML blocks and thematic breaks emit NO Block at all: they must
        // break the chunk run, not get silently embedded in chunk_text sent
        // to the judge (where grounding could land a diagnostic inside them).
        let source = "Para one is fine.\n\n<div>hidden html</div>\n\nPara two is fine.\n\n---\n\nPara three is fine.";
        let doc = crate::document::parse(source, true);
        assert_eq!(doc.blocks.len(), 3); // only the paragraphs
        let rules = [frag("core/empty-hedge", Granularity::Sentence)];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let reqs = plan_judge(&doc, source, &refs);
        assert_eq!(
            reqs.len(),
            3,
            "{:?}",
            reqs.iter().map(|r| &r.chunk_text).collect::<Vec<_>>()
        );
        assert!(reqs.iter().all(|r| !r.chunk_text.contains("div")));
        assert!(reqs.iter().all(|r| !r.chunk_text.contains("---")));
        assert_eq!(reqs[0].chunk_text, "Para one is fine.");
        assert_eq!(reqs[1].chunk_text, "Para two is fine.");
        assert_eq!(reqs[2].chunk_text, "Para three is fine.");
    }

    #[test]
    fn plan_list_items_count_as_prose() {
        let source = "Para text.\n\n- item one";
        let doc = Document {
            blocks: vec![
                block(BlockKind::Paragraph, 0, 10),
                block(BlockKind::ListItem, 12, 22),
            ],
        };
        let rules = [frag("core/empty-hedge", Granularity::Sentence)];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let reqs = plan_judge(&doc, source, &refs);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].chunk_range, TextRange { start: 0, end: 22 });
    }

    #[test]
    fn plan_document_granularity_gets_whole_document_request() {
        let source = "Alpha para.\n\nBeta para.";
        let doc = para_doc(source);
        let rules = [
            frag("core/empty-hedge", Granularity::Sentence),
            frag("core/padded-elaboration", Granularity::Paragraph),
            frag("core/whole-doc", Granularity::Document),
        ];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let reqs = plan_judge(&doc, source, &refs);
        assert_eq!(reqs.len(), 2);
        // Chunk request carries ALL sentence+paragraph rubrics.
        assert_eq!(
            reqs[0].rules,
            vec![
                RuleId("core/empty-hedge".into()),
                RuleId("core/padded-elaboration".into())
            ]
        );
        // Whole-document request carries only document rubrics.
        let doc_req = &reqs[1];
        assert_eq!(
            doc_req.chunk_range,
            TextRange {
                start: 0,
                end: source.len()
            }
        );
        assert_eq!(doc_req.chunk_text, source);
        assert_eq!(doc_req.rules, vec![RuleId("core/whole-doc".into())]);
        // Different rubric sets → different cache key bases.
        assert_ne!(reqs[0].cache_key_base, doc_req.cache_key_base);
    }

    #[test]
    fn plan_no_rules_or_empty_source_yields_no_requests() {
        let source = "Some text.";
        let doc = para_doc(source);
        assert!(plan_judge(&doc, source, &[]).is_empty());
        let rules = [frag("core/whole-doc", Granularity::Document)];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let empty_doc = Document { blocks: vec![] };
        assert!(plan_judge(&empty_doc, "   \n  ", &refs).is_empty());
    }

    #[test]
    fn plan_cache_key_base_is_deterministic_and_content_sensitive() {
        let rules = [frag("core/empty-hedge", Granularity::Sentence)];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let s1 = "One paragraph of text.";
        let a = plan_judge(&para_doc(s1), s1, &refs);
        let b = plan_judge(&para_doc(s1), s1, &refs);
        assert_eq!(a[0].cache_key_base, b[0].cache_key_base);
        assert_eq!(a[0].prompt, b[0].prompt);
        // Different chunk text → different key.
        let s2 = "A different paragraph.";
        let c = plan_judge(&para_doc(s2), s2, &refs);
        assert_ne!(a[0].cache_key_base, c[0].cache_key_base);
        // Different rubric set → different key.
        let rules2 = [frag("core/other-rule", Granularity::Sentence)];
        let refs2: Vec<&RubricFragment> = rules2.iter().collect();
        let d = plan_judge(&para_doc(s1), s1, &refs2);
        assert_ne!(a[0].cache_key_base, d[0].cache_key_base);
        // Keys are sha256 hex.
        assert_eq!(a[0].cache_key_base.len(), 64);
        assert!(a[0].cache_key_base.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn plan_prompt_contains_rubrics_examples_chunk_and_json_contract() {
        let source = "It could perhaps be argued that the claim fails.";
        let doc = para_doc(source);
        let mut r = frag("core/empty-hedge", Granularity::Sentence);
        r.rubric = "Flag hedges that carry no information.".into();
        r.flag_examples[0] = "It could perhaps be argued that…".into();
        r.pass_examples[0] = "Damages are uncertain because treatment is ongoing.".into();
        let rules = [r];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let reqs = plan_judge(&doc, source, &refs);
        let p = &reqs[0].prompt;
        assert!(p.contains("core/empty-hedge"));
        assert!(p.contains("Flag hedges that carry no information."));
        assert!(p.contains("It could perhaps be argued that…"));
        assert!(p.contains("Damages are uncertain because treatment is ongoing."));
        assert!(p.contains(source));
        assert!(p.contains("JSON array"));
        assert!(p.contains("\"rule\""));
        assert!(p.contains("\"quote\""));
        assert!(p.contains("\"confidence\""));
        assert!(p.contains("suggested_rewrite"));
        // Verdict discipline (#39): clean chunks must get [] with a one-shot
        // empty example, and pass verdicts must never be emitted as findings.
        assert!(p.contains("respond with exactly []"));
        assert!(p.contains("never emit an object stating that a rule is satisfied"));
        assert!(p.contains("The parties shall meet on the first business day of each month."));
        assert!(p.contains("the entire correct response is:\n[]"));
    }

    /// Prompt-prefix caching is what makes one-request-per-rule affordable,
    /// and it only works if everything before the rubric is byte-identical
    /// across a unit's rules. Guard the ordering, not just the contents.
    #[test]
    fn plan_per_rule_prompts_share_a_prefix_through_the_document() {
        let source = "It could perhaps be argued that the claim fails.";
        let doc = para_doc(source);
        let rules = [
            frag("core/empty-hedge", Granularity::Sentence),
            frag("core/padded-elaboration", Granularity::Paragraph),
        ];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let plan = JudgePlan {
            context_chars: 24_000,
            per_rule: true,
        };
        let reqs = plan_judge_with(&doc, source, &refs, &plan);

        assert_eq!(reqs.len(), 2, "one request per rule over one unit");
        assert_eq!(reqs[0].rules, vec![RuleId("core/empty-hedge".into())]);
        assert_eq!(
            reqs[1].rules,
            vec![RuleId("core/padded-elaboration".into())]
        );

        // The shared prefix must run through the end of the document block.
        let shared = common_prefix(&reqs[0].prompt, &reqs[1].prompt);
        assert!(
            shared.contains(source) && shared.contains(">>>"),
            "prefix stops before the document ends: {shared:?}"
        );
        // …and must end before either rubric, or they would not differ.
        assert!(!shared.contains("core/empty-hedge"));
    }

    fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
        let len = a
            .char_indices()
            .zip(b.chars())
            .take_while(|((_, x), y)| x == y)
            .map(|((i, x), _)| i + x.len_utf8())
            .last()
            .unwrap_or(0);
        &a[..len]
    }

    #[test]
    fn plan_bundles_rubrics_when_not_per_rule() {
        let source = "It could perhaps be argued that the claim fails.";
        let doc = para_doc(source);
        let rules = [
            frag("core/empty-hedge", Granularity::Sentence),
            frag("core/padded-elaboration", Granularity::Paragraph),
        ];
        let refs: Vec<&RubricFragment> = rules.iter().collect();
        let reqs = plan_judge_with(&doc, source, &refs, &JudgePlan::default());
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].rules.len(), 2);
    }

    /// A bigger context budget must actually collapse units — this is the
    /// whole point of the profile.
    #[test]
    fn plan_context_budget_controls_unit_count() {
        let para = "x".repeat(500);
        let source = format!("{para}\n\n{para}\n\n{para}");
        let doc = para_doc(&source);
        let rules = [frag("core/empty-hedge", Granularity::Sentence)];
        let refs: Vec<&RubricFragment> = rules.iter().collect();

        let small = plan_judge_with(&doc, &source, &refs, &JudgePlan::default());
        assert_eq!(small.len(), 2, "1200-char units split three paragraphs");

        let large = plan_judge_with(
            &doc,
            &source,
            &refs,
            &JudgePlan {
                context_chars: 24_000,
                per_rule: false,
            },
        );
        assert_eq!(large.len(), 1, "one unit holds the whole document");
        assert_eq!(large[0].chunk_text, source);
    }

    #[test]
    fn plan_per_rule_cache_keys_are_independent_per_rule() {
        let source = "It could perhaps be argued that the claim fails.";
        let doc = para_doc(source);
        let hedge = frag("core/empty-hedge", Granularity::Sentence);
        let padded = frag("core/padded-elaboration", Granularity::Paragraph);
        let plan = JudgePlan {
            context_chars: 24_000,
            per_rule: true,
        };

        let both: Vec<&RubricFragment> = vec![&hedge, &padded];
        let one: Vec<&RubricFragment> = vec![&hedge];
        let with_both = plan_judge_with(&doc, source, &both, &plan);
        let with_one = plan_judge_with(&doc, source, &one, &plan);

        // Adding a second rule must not disturb the first rule's cache entry.
        assert_eq!(with_both[0].cache_key_base, with_one[0].cache_key_base);
        assert_ne!(with_both[0].cache_key_base, with_both[1].cache_key_base);
    }

    #[test]
    fn judge_plan_for_context_enables_per_rule_only_above_the_default() {
        assert!(!JudgePlan::for_context(800).per_rule);
        assert!(!JudgePlan::for_context(TARGET_CHUNK_CHARS).per_rule);
        assert!(JudgePlan::for_context(24_000).per_rule);
        assert_eq!(JudgePlan::for_context(24_000).context_chars, 24_000);
        // A zero budget would make chunking loop forever on empty units.
        assert_eq!(JudgePlan::for_context(0).context_chars, 1);
    }

    // ---- Execution -----------------------------------------------------

    /// A judge that serves several requests at once and finishes them out of
    /// order: later requests return fastest, so any code that folds results
    /// as they land (rather than in request order) will visibly reorder.
    struct ConcurrentJudge {
        workers: usize,
        delay: std::time::Duration,
        peak: AtomicUsize,
        in_flight: AtomicUsize,
    }

    impl ConcurrentJudge {
        fn new(workers: usize, delay_ms: u64) -> Self {
            ConcurrentJudge {
                workers,
                delay: std::time::Duration::from_millis(delay_ms),
                peak: AtomicUsize::new(0),
                in_flight: AtomicUsize::new(0),
            }
        }
    }

    impl Judge for ConcurrentJudge {
        fn evaluate(&self, req: &JudgeRequest) -> Result<Vec<JudgeFinding>, JudgeError> {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            // Invert the delay so request 0 is slowest: completion order ends
            // up the reverse of request order.
            let index: u64 = req.chunk_text.trim_start_matches('p').parse().unwrap_or(0);
            std::thread::sleep(self.delay * (4 - index.min(3)) as u32);
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(vec![finding("core/empty-hedge", &req.chunk_text, 0.9)])
        }
        fn model_id(&self) -> &str {
            "concurrent"
        }
        fn max_concurrency(&self) -> usize {
            self.workers
        }
    }

    /// `count` single-chunk requests over one synthetic source, each chunk
    /// labelled `p<i>` so completion order is observable in the output.
    fn indexed_reqs(count: usize) -> (Vec<JudgeRequest>, String) {
        let mut source = String::new();
        let mut reqs = Vec::new();
        for i in 0..count {
            let text = format!("p{i}");
            let start = source.len();
            source.push_str(&text);
            let mut r = req(&text, &["core/empty-hedge"]);
            r.chunk_range = TextRange {
                start,
                end: source.len(),
            };
            reqs.push(r);
        }
        (reqs, source)
    }

    /// The linter contract: concurrency may not change what a document
    /// reports or in what order. Findings come back in request order even
    /// though the requests complete in reverse.
    #[test]
    fn run_parallel_preserves_request_order() {
        let (reqs, source) = indexed_reqs(4);
        let judge = ConcurrentJudge::new(4, 15);
        let (out, stats) = run_judge(&judge, None, &reqs, &source);

        let order: Vec<&str> = out.iter().map(|(_, f, _)| f.quote.as_str()).collect();
        assert_eq!(order, vec!["p0", "p1", "p2", "p3"]);
        assert_eq!(stats.chunks, 4);
        assert_eq!(stats.chunks_failed, 0);
        assert_eq!(stats.grounded, 4);
    }

    /// …and the parallel and serial paths must agree exactly, or the plan a
    /// user gets would depend on their backend's pool size.
    #[test]
    fn run_parallel_matches_serial_output() {
        let (reqs, source) = indexed_reqs(4);
        let (parallel, pstats) = run_judge(&ConcurrentJudge::new(4, 5), None, &reqs, &source);
        let (serial, sstats) = run_judge(&ConcurrentJudge::new(1, 5), None, &reqs, &source);

        let quotes = |v: &[(JudgeRequest, JudgeFinding, TextRange)]| {
            v.iter()
                .map(|(_, f, s)| (f.quote.clone(), *s))
                .collect::<Vec<_>>()
        };
        assert_eq!(quotes(&parallel), quotes(&serial));
        assert_eq!(pstats.grounded, sstats.grounded);
        assert_eq!(pstats.chunks_failed, sstats.chunks_failed);
    }

    #[test]
    fn run_parallel_actually_overlaps_requests() {
        let (reqs, source) = indexed_reqs(4);
        let judge = ConcurrentJudge::new(4, 40);
        let started = std::time::Instant::now();
        run_judge(&judge, None, &reqs, &source);
        let elapsed = started.elapsed();

        // Serial would be (4+3+2+1)*40ms = 400ms; overlapped is bounded by the
        // slowest single request (160ms). Assert well inside that gap.
        assert!(
            elapsed < std::time::Duration::from_millis(320),
            "requests did not overlap: {elapsed:?}"
        );
        assert!(
            judge.peak.load(Ordering::SeqCst) > 1,
            "never had two requests in flight"
        );
    }

    /// A pool bigger than the work must not spawn idle workers or hang the
    /// progress drain.
    #[test]
    fn run_parallel_handles_fewer_requests_than_workers() {
        let (reqs, source) = indexed_reqs(1);
        let mut seen = Vec::new();
        let (out, stats) = run_judge_with_progress(
            &ConcurrentJudge::new(8, 1),
            None,
            &reqs,
            &source,
            &mut |done, total| seen.push((done, total)),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(stats.chunks, 1);
        assert_eq!(seen, vec![(0, 1), (1, 1)]);
    }

    #[test]
    fn run_parallel_reports_progress_for_every_request() {
        let (reqs, source) = indexed_reqs(4);
        let mut seen = Vec::new();
        run_judge_with_progress(
            &ConcurrentJudge::new(4, 5),
            None,
            &reqs,
            &source,
            &mut |done, total| seen.push((done, total)),
        );
        // Monotonic 0..=4, one callback per completion plus the initial 0.
        assert_eq!(seen, vec![(0, 4), (1, 4), (2, 4), (3, 4), (4, 4)]);
    }

    #[test]
    fn run_retries_once_then_succeeds() {
        let text = "The hedge could perhaps be argued.";
        let r = req(text, &["core/empty-hedge"]);
        let judge = MockJudge::new().respond_err(text, "transient").respond(
            text,
            vec![finding("core/empty-hedge", "could perhaps", 0.8)],
        );
        let (out, stats) = run_judge(&judge, None, &[r], text);
        assert_eq!(judge.calls(), 2);
        assert_eq!(out.len(), 1);
        assert_eq!(stats.chunks, 1);
        assert_eq!(stats.chunks_failed, 0);
        assert_eq!(stats.grounded, 1);
        let (_, f, span) = &out[0];
        assert_eq!(f.rule, "core/empty-hedge");
        assert_eq!(span.slice(text), "could perhaps");
    }

    #[test]
    fn run_fails_chunk_closed_after_retry() {
        let text = "Some chunk of prose.";
        let r = req(text, &["core/empty-hedge"]);
        let judge = MockJudge::new()
            .respond_malformed(text, "not json")
            .respond_malformed(text, "still not json");
        let (out, stats) = run_judge(&judge, None, &[r], text);
        assert_eq!(judge.calls(), 2); // exactly one retry
        assert!(out.is_empty());
        assert_eq!(stats.chunks_failed, 1);
        assert_eq!(stats.grounded, 0);
    }

    #[test]
    fn run_failed_chunk_does_not_poison_others() {
        let t1 = "Chunk one fails hard.";
        let t2 = "Chunk two works fine.";
        let judge = MockJudge::new()
            .respond_err(t1, "boom")
            .respond_err(t1, "boom again")
            .respond(t2, vec![finding("core/empty-hedge", "works fine", 0.7)]);
        let reqs = [
            req(t1, &["core/empty-hedge"]),
            req(t2, &["core/empty-hedge"]),
        ];
        let source = t2; // grounding uses chunk_text; ranges are per-request
        let (out, stats) = run_judge(&judge, None, &reqs, source);
        assert_eq!(stats.chunks, 2);
        assert_eq!(stats.chunks_failed, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.quote, "works fine");
    }

    #[test]
    fn run_cache_hit_is_deterministic_and_skips_judge() {
        let text = "It could perhaps be argued that the claim fails.";
        let r = req(text, &["core/empty-hedge"]);
        let cache = MemoryCache::new();
        let judge = MockJudge::new().respond(
            text,
            vec![finding("core/empty-hedge", "could perhaps be argued", 0.9)],
        );
        let (out1, stats1) = run_judge(&judge, Some(&cache), std::slice::from_ref(&r), text);
        assert_eq!(judge.calls(), 1);
        assert_eq!(stats1.cache_hits, 0);
        let (out2, stats2) = run_judge(&judge, Some(&cache), std::slice::from_ref(&r), text);
        assert_eq!(judge.calls(), 1); // no new evaluate call
        assert_eq!(stats2.cache_hits, 1);
        assert_eq!(stats2.grounded, 1);
        assert_eq!(out1.len(), out2.len());
        assert_eq!(out1[0].1.quote, out2[0].1.quote);
        assert_eq!(out1[0].2, out2[0].2);
    }

    #[test]
    fn run_cache_key_includes_model_id() {
        let text = "Model-sensitive caching text.";
        let r = req(text, &["core/empty-hedge"]);
        let cache = MemoryCache::new();
        let j1 = MockJudge::with_model("model-a")
            .respond(text, vec![finding("core/empty-hedge", "caching text", 0.9)]);
        let j2 = MockJudge::with_model("model-b")
            .respond(text, vec![finding("core/empty-hedge", "caching text", 0.9)]);
        run_judge(&j1, Some(&cache), std::slice::from_ref(&r), text);
        let (_, stats2) = run_judge(&j2, Some(&cache), std::slice::from_ref(&r), text);
        assert_eq!(j2.calls(), 1); // different model → cache miss → evaluated
        assert_eq!(stats2.cache_hits, 0);
        // Same model as j1 → hit.
        let j3 = MockJudge::with_model("model-a");
        let (_, stats3) = run_judge(&j3, Some(&cache), std::slice::from_ref(&r), text);
        assert_eq!(j3.calls(), 0);
        assert_eq!(stats3.cache_hits, 1);
    }

    #[test]
    fn run_discards_findings_naming_foreign_rules() {
        let text = "Perfectly quotable prose right here.";
        let r = req(text, &["core/empty-hedge"]);
        let judge = MockJudge::new().respond(
            text,
            vec![
                finding("core/not-in-request", "quotable prose", 0.9),
                finding("core/empty-hedge", "quotable prose", 0.9),
            ],
        );
        let (out, stats) = run_judge(&judge, None, &[r], text);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.rule, "core/empty-hedge");
        assert_eq!(stats.hallucinated.get("core/not-in-request"), Some(&1));
        assert_eq!(stats.grounded, 1);
    }

    #[test]
    fn run_counts_ungroundable_findings_as_hallucinated() {
        let text = "Nothing matches the invented quote.";
        let r = req(text, &["core/empty-hedge"]);
        let judge = MockJudge::new().respond(
            text,
            vec![finding(
                "core/empty-hedge",
                "totally fabricated wording",
                0.9,
            )],
        );
        let (out, stats) = run_judge(&judge, None, &[r], text);
        assert!(out.is_empty());
        assert_eq!(stats.hallucinated.get("core/empty-hedge"), Some(&1));
        assert_eq!(stats.grounded, 0);
        assert_eq!(stats.chunks_failed, 0);
    }

    #[test]
    fn run_clamps_confidence_into_unit_interval() {
        let text = "Overconfident and underconfident findings.";
        let r = req(text, &["core/empty-hedge"]);
        let judge = MockJudge::new().respond(
            text,
            vec![
                finding("core/empty-hedge", "Overconfident", 1.5),
                finding("core/empty-hedge", "underconfident", -0.5),
                finding("core/empty-hedge", "findings", f32::NAN),
            ],
        );
        let (out, _) = run_judge(&judge, None, &[r], text);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].1.confidence, 1.0);
        assert_eq!(out[1].1.confidence, 0.0);
        assert_eq!(out[2].1.confidence, 0.0);
    }

    #[test]
    fn run_unscripted_request_yields_no_findings_but_no_failure() {
        let text = "No script matches this chunk.";
        let r = req(text, &["core/empty-hedge"]);
        let judge = MockJudge::new();
        let (out, stats) = run_judge(&judge, None, &[r], text);
        assert!(out.is_empty());
        assert_eq!(stats.chunks, 1);
        assert_eq!(stats.chunks_failed, 0);
    }

    // ---- MemoryCache ---------------------------------------------------

    #[test]
    fn memory_cache_get_put_roundtrip_and_overwrite() {
        let cache = MemoryCache::new();
        assert!(cache.get("k").is_none());
        cache.put("k", &[finding("core/a", "q", 0.5)]);
        let got = cache.get("k").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].rule, "core/a");
        cache.put("k", &[]);
        assert!(cache.get("k").unwrap().is_empty());
        assert!(cache.get("other").is_none());
    }
}
