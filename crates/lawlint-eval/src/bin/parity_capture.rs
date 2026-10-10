use lawlint_core::{
    apply_fixes, default_quote_ground, lint, lint_with, plan_judge_with, remediation_prompt,
    Applicability, Diagnostic, Edit, Intent, JudgeFinding, JudgePlan, JudgeRequest, LintOptions,
    MockJudge, PromptSource, RubricFragment, RuleId, RuleSet, Severity, TextRange, Tier,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

fn canonical(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        Value::Object(values) => {
            let sorted: BTreeMap<_, _> = values.into_iter().collect();
            let mut map = Map::new();
            for (key, value) in sorted {
                map.insert(key, canonical(value));
            }
            Value::Object(map)
        }
        value => value,
    }
}

static INPUTS: std::sync::LazyLock<Mutex<BTreeMap<String, (String, String)>>> =
    std::sync::LazyLock::new(|| Mutex::new(BTreeMap::new()));
static KEEP_PROMPT_SAMPLES: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

fn sha256_hex(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn add(
    files: &mut BTreeMap<String, Vec<(String, Value)>>,
    surface: &str,
    id: String,
    source: Value,
    options: Value,
    text: &str,
    mut output: Value,
) {
    let text_sha256 = sha256_hex(text);
    let input_id = format!("text-{text_sha256}");
    INPUTS
        .lock()
        .unwrap()
        .insert(input_id.clone(), (text.to_owned(), text_sha256.clone()));

    if surface == "lint"
        && id.starts_with("corpus-")
        && source.get("corpus").is_some()
        && options.as_object().is_some_and(serde_json::Map::is_empty)
    {
        if let Some(diagnostics) = output["diagnostics"].as_array_mut() {
            for diagnostic in diagnostics {
                if let Some(fields) = diagnostic.as_object_mut() {
                    fields.remove("excerpt");
                }
            }
        }
    }
    if surface == "fix" && source.get("corpus").is_some() {
        let fixed = output["text"].as_str().unwrap_or_default();
        output = serde_json::json!({
            "outputSha256": sha256_hex(fixed),
            "changed": fixed != text
        });
    }
    if surface == "prompt" {
        if let Some(sample_id) = source.get("corpus").and_then(Value::as_str) {
            if !KEEP_PROMPT_SAMPLES.lock().unwrap().contains(sample_id) {
                let prompt_value = &output["prompt"];
                let prompt = prompt_value.as_str().unwrap_or_default();
                output = serde_json::json!({
                    "promptSha256": prompt_hash(Some(prompt_value)),
                    "promptLength": prompt.len()
                });
            }
        }
    }
    let case = canonical(serde_json::json!({
        "id": id,
        "surface": surface,
        "input": {"source": source, "options": options},
        "inputId": input_id,
        "textSha256": text_sha256,
        "output": output
    }));
    files
        .entry(format!("{surface}.jsonl"))
        .or_default()
        .push((case["id"].as_str().unwrap().to_owned(), case));
}

fn parse_output(text: &str, markdown: bool) -> Value {
    let doc = lawlint_core::parse(text, markdown);
    let blocks: Vec<_> = doc
        .blocks
        .iter()
        .map(|block| {
            serde_json::json!({
                "kind": format!("{:?}", block.kind).to_lowercase(),
                "range": block.range,
                "sentences": block.sentences.iter().map(|sentence| serde_json::json!({
                    "range": sentence.range,
                    "isCitation": sentence.is_citation
                })).collect::<Vec<_>>()
            })
        })
        .collect();
    serde_json::json!({"blocks": blocks})
}

fn parse_options(value: Value) -> LintOptions {
    serde_json::from_value(value).expect("parity options must deserialize")
}

fn source_obj(kind: &str, id: &str) -> Value {
    serde_json::json!({kind: id})
}

fn prompt_hash(prompt: Option<&Value>) -> String {
    prompt
        .and_then(Value::as_str)
        .map(sha256_hex)
        .unwrap_or_else(|| sha256_hex(""))
}

fn add_four(
    files: &mut BTreeMap<String, Vec<(String, Value)>>,
    id: &str,
    source: Value,
    text: &str,
    options_value: Value,
    rules: Option<&RuleSet>,
    markdown: bool,
) {
    let options = parse_options(options_value.clone());
    let lint = rules.map_or_else(
        || lint(text, &options),
        |set| lint_with(text, &options, set),
    );
    let parsed = parse_output(text, markdown);
    add(
        files,
        "parse",
        id.into(),
        source.clone(),
        options_value.clone(),
        text,
        parsed,
    );
    add(
        files,
        "lint",
        id.into(),
        source.clone(),
        options_value.clone(),
        text,
        serde_json::to_value(&lint).unwrap(),
    );
    add(
        files,
        "fix",
        id.into(),
        source.clone(),
        options_value.clone(),
        text,
        serde_json::json!({"text": apply_fixes(text, &lint.diagnostics)}),
    );
    let prompt = remediation_prompt(PromptSource::Text(text), &lint, rules.unwrap_or(&BUILTIN));
    add(
        files,
        "prompt",
        id.into(),
        source,
        options_value,
        text,
        serde_json::json!({"prompt": prompt}),
    );
}

static BUILTIN: std::sync::LazyLock<RuleSet> = std::sync::LazyLock::new(RuleSet::built_in);

fn built_in_rule_set() -> &'static RuleSet {
    &BUILTIN
}

fn inferential_fragments(rules: &RuleSet) -> Vec<RubricFragment> {
    rules
        .instantiate(&LintOptions::default())
        .iter()
        .filter_map(|rule| rule.rubric().cloned())
        .collect()
}

fn finding(rule: &str, quote: &str, explanation: &str, confidence: f32) -> JudgeFinding {
    JudgeFinding {
        rule: rule.into(),
        quote: quote.into(),
        explanation: explanation.into(),
        confidence,
        suggested_rewrite: None,
    }
}

fn grounding_surface(files: &mut BTreeMap<String, Vec<(String, Value)>>, root: &Path) {
    let text = fs::read_to_string(root.join("parity/fixtures/text/judge-input.txt")).unwrap();
    let range = TextRange {
        start: 0,
        end: text.len(),
    };
    let long_quote = "x".repeat(401);
    let findings = vec![
        finding(
            "core/empty-hedge",
            "It could perhaps be argued",
            "Stacked hedge without support.",
            0.93,
        ),
        finding(
            "core/empty-hedge",
            "It could perhaps be argued that",
            "Fuzzy match around the hedge.",
            0.93,
        ),
        finding(
            "core/empty-hedge",
            "A fabricated passage nowhere present",
            "This quote is absent.",
            0.91,
        ),
        finding(
            "unknown/rule",
            "The record does not say why.",
            "Unknown rule must be discarded.",
            0.99,
        ),
        finding(
            "core/empty-hedge",
            &long_quote,
            "Long quote has no fuzzy fallback.",
            0.95,
        ),
        finding(
            "core/empty-hedge",
            "The record does not say why.",
            "Below confidence floor.",
            0.3,
        ),
        finding(
            "core/empty-hedge",
            "Damages are uncertain",
            "The sentence does not violate the empty hedge rule.",
            0.99,
        ),
    ];
    let request = JudgeRequest {
        chunk_range: range,
        chunk_text: text.clone(),
        rules: vec![RuleId("core/empty-hedge".into())],
        prompt: "Mock judge input; no network or AI is used.".into(),
        cache_key_base: "parity-grounding-request".into(),
    };
    let mock = MockJudge::new().respond("", findings);
    let (grounded, stats) = lawlint_core::run_judge(&mock, None, &[request], &text);
    let direct = [
        "It could perhaps be argued",
        "It could perhaps be argue",
        "a fabricated passage nowhere present",
        &long_quote,
        "",
    ]
    .iter()
    .map(|quote| {
        serde_json::json!({
            "quote": quote,
            "span": default_quote_ground(quote, &text, range)
        })
    })
    .collect::<Vec<_>>();
    add(
        files,
        "judge_ground",
        "judge-grounding-mock".into(),
        source_obj("fixture", "text/judge-input.txt"),
        serde_json::json!({"judge":"MockJudge","confidenceFloor":0.6}),
        &text,
        serde_json::json!({"grounded": grounded.iter().map(|(req, finding, span)| serde_json::json!({"request": req, "finding": finding, "span": span})).collect::<Vec<_>>(), "stats": stats, "direct": direct}),
    );
    let low_confidence = finding(
        "core/empty-hedge",
        "It could perhaps be argued",
        "Below confidence floor.",
        0.3,
    );
    let low_confidence_judge = MockJudge::new().respond("", vec![low_confidence]);
    let options = LintOptions {
        judge: Some(lawlint_core::JudgeOptions {
            floor: Some(0.6),
            ..Default::default()
        }),
        ..Default::default()
    };
    let full_result = lawlint_core::lint_full(
        &text,
        &options,
        built_in_rule_set(),
        &low_confidence_judge,
        None,
    );
    add(
        files,
        "judge_ground",
        "judge-grounding-confidence-floor".into(),
        source_obj("fixture", "text/judge-input.txt"),
        serde_json::json!({"judge":"MockJudge","confidenceFloor":0.6}),
        &text,
        serde_json::json!({"result":full_result,"mockCalls":low_confidence_judge.calls()}),
    );

    for (id, raw) in [
        ("fenced", "```json\n[{\"rule\":\"core/empty-hedge\",\"quote\":\"It could perhaps be argued\",\"explanation\":\"It does not violate the empty hedge rule.\",\"confidence\":0.9}]\n```"),
        ("lenient-shape", "Model response: [{\"rule\":\"core/empty-hedge\",\"quote\":\"It could perhaps be argued\",\"explanation\":\"no empty hedge is present\",\"confidence\":0.9,\"rewrite\":null}] end."),
        ("malformed", "not json at all"),
    ] {
        let parsed = lawlint_judge::parse_findings_with_stats(raw);
        let result = match parsed {
            Ok(parsed) => serde_json::json!({"kept": parsed.kept, "droppedNegative": parsed.dropped_negative, "error": null}),
            Err(error) => serde_json::json!({"kept": [], "droppedNegative": [], "error": error.to_string()}),
        };
        add(files, "judge_ground", format!("judge-parse-{id}"), source_obj("rule", "core/empty-hedge"), serde_json::json!({"parser":"parse_findings_with_stats"}), raw, result);
    }
}

fn docx_surface(
    files: &mut BTreeMap<String, Vec<(String, Value)>>,
    root: &Path,
    outputs: &mut BTreeMap<PathBuf, Vec<u8>>,
) {
    let mut inputs = vec![(
        "sample".to_string(),
        root.join("crates/lawlint-docx/tests/fixtures/sample.docx"),
    )];
    let fixture_dir = root.join("parity/fixtures/docx");
    for entry in fs::read_dir(&fixture_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|x| x.to_str()) == Some("docx") {
            inputs.push((
                path.file_stem().unwrap().to_string_lossy().into_owned(),
                path,
            ));
        }
    }
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    for (id, path) in inputs {
        let bytes = fs::read(&path).unwrap();
        let text = lawlint_docx::extract(&bytes).unwrap();
        let mut diagnostics = Vec::new();
        let needle = if id == "cross-run-fix" {
            "cease and desist"
        } else {
            "delve"
        };
        if let Some(start) = text.find(needle) {
            let end = start + needle.len();
            diagnostics.push(Diagnostic {
                rule_id: RuleId("parity/manual-fix".into()),
                severity: Severity::Warning,
                tier: Tier::Static,
                intent: Intent::Style,
                span: TextRange { start, end },
                message: "Parity tracked-change fixture.".into(),
                line: 1,
                column: 1,
                end_line: None,
                end_column: None,
                excerpt: needle.into(),
                suggestion: Some("examined".into()),
                weight: None,
                confidence: None,
                fix: Some(lawlint_core::Fix {
                    edits: vec![Edit {
                        range: TextRange { start, end },
                        replacement: "examined".into(),
                    }],
                    applicability: Applicability::MachineApplicable,
                }),
            });
        }
        let options = lawlint_docx::ReviseOptions {
            author: "lawlint parity".into(),
            date: Some("2020-01-02T03:04:05Z".into()),
            include_ai_rewrites: true,
            annotate_findings: true,
        };
        let revised = lawlint_docx::apply_tracked_changes(&bytes, &diagnostics, &options).unwrap();
        let hash = format!("{:x}", Sha256::digest(&revised.bytes));
        outputs.insert(
            PathBuf::from(format!("parity/golden/docx/{id}.docx")),
            revised.bytes,
        );
        let fixture_path = if id == "sample" {
            "../../crates/lawlint-docx/tests/fixtures/sample.docx".to_string()
        } else {
            format!("docx/{id}.docx")
        };
        add(
            files,
            "docx",
            format!("docx-{id}"),
            source_obj("fixture", &fixture_path),
            serde_json::json!({"author":"lawlint parity","date":"2020-01-02T03:04:05Z","includeAiRewrites":true,"annotateFindings":true}),
            &text,
            serde_json::json!({"extract": text, "applied": revised.applied, "annotated": revised.annotated, "skipped": revised.skipped, "sha256": hash, "outputFile": format!("golden/docx/{id}.docx")}),
        );

        // End-to-end variant: real diagnostics from the default lint run.
        let linted = lawlint_core::lint(&text, &LintOptions::default());
        let revised =
            lawlint_docx::apply_tracked_changes(&bytes, &linted.diagnostics, &options).unwrap();
        outputs.insert(
            PathBuf::from(format!("parity/golden/docx/{id}-linted.docx")),
            revised.bytes.clone(),
        );
        add(
            files,
            "docx",
            format!("docx-{id}-linted"),
            source_obj("fixture", &fixture_path),
            serde_json::json!({"author":"lawlint parity","date":"2020-01-02T03:04:05Z","includeAiRewrites":true,"annotateFindings":true,"diagnostics":"lint-default"}),
            &text,
            serde_json::json!({"diagnosticCount": linted.diagnostics.len(), "applied": revised.applied, "annotated": revised.annotated, "skipped": revised.skipped, "sha256": format!("{:x}", Sha256::digest(&revised.bytes)), "outputFile": format!("golden/docx/{id}-linted.docx")}),
        );
    }
}

fn generate(root: &Path) -> (BTreeMap<String, Vec<u8>>, BTreeMap<String, usize>) {
    let started = Instant::now();
    let mut files: BTreeMap<String, Vec<(String, Value)>> = BTreeMap::new();
    let mut binary_files = BTreeMap::new();
    INPUTS.lock().unwrap().clear();
    KEEP_PROMPT_SAMPLES.lock().unwrap().clear();
    let corpus_path = root.join("crates/lawlint-eval/corpus/corpus.jsonl");
    let samples = lawlint_eval::load_jsonl(&corpus_path).expect("load corpus");
    let mut sorted_sample_ids: Vec<_> = samples.iter().map(|sample| sample.id.clone()).collect();
    sorted_sample_ids.sort();
    KEEP_PROMPT_SAMPLES
        .lock()
        .unwrap()
        .extend(sorted_sample_ids.into_iter().take(25));
    for sample in &samples {
        add_four(
            &mut files,
            &format!("corpus-{}", sample.id),
            source_obj("corpus", &sample.id),
            &sample.text,
            serde_json::json!({}),
            None,
            false,
        );
    }
    let fixture_dir = root.join("parity/fixtures/text");
    let mut fixtures = Vec::new();
    for entry in fs::read_dir(&fixture_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            fixtures.push((name, fs::read_to_string(path).unwrap()));
        }
    }
    fixtures.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, text) in &fixtures {
        let id = format!("fixture-{name}");
        let md = name == "markdown.md";
        add_four(
            &mut files,
            &id,
            source_obj("fixture", &format!("text/{name}")),
            text,
            serde_json::json!({"markdown": md}),
            None,
            md,
        );
        for (suffix, options) in [
            (
                "enable",
                serde_json::json!({"enable":["core/no-ai-cliches"],"markdown":md}),
            ),
            (
                "disable",
                serde_json::json!({"disable":["no-ai-cliches"],"markdown":md}),
            ),
            (
                "severity",
                serde_json::json!({"severity":{"no-ai-cliches":"error"},"markdown":md}),
            ),
            (
                "thresholds",
                serde_json::json!({"thresholds":{"sentence-length":15.0},"markdown":md}),
            ),
            (
                "alias",
                serde_json::json!({"enable":["no-ai-cliches"],"markdown":md}),
            ),
        ] {
            let opts = parse_options(options.clone());
            let result = lint(text, &opts);
            let case_id = format!("fixture-{name}-{suffix}");
            add(
                &mut files,
                "lint",
                case_id.clone(),
                source_obj("fixture", &format!("text/{name}")),
                options.clone(),
                text,
                serde_json::to_value(&result).unwrap(),
            );
            if suffix == "enable" {
                add(
                    &mut files,
                    "parse",
                    case_id.clone(),
                    source_obj("fixture", &format!("text/{name}")),
                    options.clone(),
                    text,
                    parse_output(text, md),
                );
                add(
                    &mut files,
                    "fix",
                    case_id.clone(),
                    source_obj("fixture", &format!("text/{name}")),
                    options.clone(),
                    text,
                    serde_json::json!({"text":apply_fixes(text, &result.diagnostics)}),
                );
                add(
                    &mut files,
                    "prompt",
                    case_id,
                    source_obj("fixture", &format!("text/{name}")),
                    options,
                    text,
                    serde_json::json!({"prompt":remediation_prompt(PromptSource::Text(text), &result, built_in_rule_set())}),
                );
            }
        }
        if md {
            add_four(
                &mut files,
                "fixture-markdown.md-markdown-false",
                source_obj("fixture", &format!("text/{name}")),
                text,
                serde_json::json!({"markdown":false}),
                None,
                false,
            );
        }
    }
    for (n, sample) in samples.iter().take(10).enumerate() {
        let id = format!("options-corpus-{:02}-{}", n + 1, sample.id);
        for (suffix, options) in [
            (
                "enable",
                serde_json::json!({"enable":["core/no-ai-cliches"]}),
            ),
            ("disable", serde_json::json!({"disable":["no-ai-cliches"]})),
            (
                "severity",
                serde_json::json!({"severity":{"no-ai-cliches":"error"}}),
            ),
            (
                "thresholds",
                serde_json::json!({"thresholds":{"sentence-length":15.0}}),
            ),
            (
                "alias",
                serde_json::json!({"enable":["no-ai-cliches"],"disable":["no-hedging"]}),
            ),
        ] {
            let rules = built_in_rule_set();
            let opts = parse_options(options.clone());
            let result = lint_with(&sample.text, &opts, rules);
            let cid = format!("{id}-{suffix}");
            add(
                &mut files,
                "lint",
                cid.clone(),
                source_obj("corpus", &sample.id),
                options.clone(),
                &sample.text,
                serde_json::to_value(&result).unwrap(),
            );
            if suffix == "enable" {
                // Include parse/fix/prompt for an option case as well.
                add(
                    &mut files,
                    "parse",
                    cid.clone(),
                    source_obj("corpus", &sample.id),
                    options.clone(),
                    &sample.text,
                    parse_output(&sample.text, false),
                );
                add(
                    &mut files,
                    "fix",
                    cid.clone(),
                    source_obj("corpus", &sample.id),
                    options.clone(),
                    &sample.text,
                    serde_json::json!({"text":apply_fixes(&sample.text, &result.diagnostics)}),
                );
                add(
                    &mut files,
                    "prompt",
                    cid,
                    source_obj("corpus", &sample.id),
                    options,
                    &sample.text,
                    serde_json::json!({"prompt":remediation_prompt(PromptSource::Text(&sample.text), &result, rules)}),
                );
            }
        }
    }
    // Every built-in and simple-English rule example is linted against its package rule set.
    let simple_dir = root.join("packages/simple-english");
    let simple = RuleSet::load_dir(&simple_dir).expect("load simple-english");
    let all_rules = vec![
        ("builtin", built_in_rule_set().clone()),
        ("simple-english", simple.clone()),
    ];
    for (package, rules) in &all_rules {
        for meta in rules.metas() {
            let rule_instances = rules.instantiate(&LintOptions {
                enable: Some(vec![meta.id.0.clone()]),
                ..Default::default()
            });
            let fragment = rule_instances
                .iter()
                .find_map(|rule| rule.rubric().cloned());
            // (label, group, kind, index, text): `example` labels come from
            // metadata bad/good pairs, `rubric` labels from the inferential
            // flag/pass lists — the groups must not share id space.
            let mut examples: Vec<(String, &'static str, &'static str, usize, String)> = meta
                .examples
                .iter()
                .enumerate()
                .flat_map(|(i, ex)| {
                    vec![
                        (
                            format!("example-flag-{i}"),
                            "example",
                            "flag",
                            i,
                            ex.bad.clone(),
                        ),
                        (
                            format!("example-pass-{i}"),
                            "example",
                            "pass",
                            i,
                            ex.good.clone(),
                        ),
                    ]
                })
                .collect();
            if let Some(f) = fragment {
                examples.extend(
                    f.flag_examples
                        .iter()
                        .enumerate()
                        .map(|(i, s)| (format!("rubric-flag-{i}"), "rubric", "flag", i, s.clone())),
                );
                examples.extend(
                    f.pass_examples
                        .iter()
                        .enumerate()
                        .map(|(i, s)| (format!("rubric-pass-{i}"), "rubric", "pass", i, s.clone())),
                );
            }
            for (label, group, kind, index, text) in examples {
                let options_value = serde_json::json!({"enable":[meta.id.0]});
                let id = format!(
                    "rule-{package}-{}-{label}",
                    meta.id.0.split('/').next_back().unwrap()
                );
                let source = serde_json::json!({"rule":meta.id.0,"example":kind,"index":index,"group":group});
                add_four(
                    &mut files,
                    &id,
                    source,
                    &text,
                    options_value,
                    Some(rules),
                    false,
                );
            }
        }
    }
    // Separate canonical rules metadata contract and simple-english metadata.
    for (package, rules) in &all_rules {
        let entries: Vec<_> = rules.metas().iter().map(|meta| {
            let flat_id = meta.id.0.split_once('/').map_or(meta.id.0.as_str(), |(_, rest)| rest);
            let mut wrapped = serde_json::json!({"tier":meta.tier,"scope":meta.scope,"severity":meta.severity,"intent":meta.intent,"description":meta.description,"docsUrl":meta.docs_url,"examples":meta.examples});
            if let Some(rationale) = &meta.rationale { wrapped["rationale"] = serde_json::json!(rationale); }
            if let Some(explanation) = &meta.explanation { wrapped["explanation"] = serde_json::json!(explanation); }
            serde_json::json!({"id":flat_id,"meta":wrapped})
        }).collect();
        let text = format!("{package} built-in metadata");
        add(
            &mut files,
            "rules_meta",
            format!("rules-meta-{package}"),
            serde_json::json!({"package":package}),
            serde_json::json!({}),
            &text,
            serde_json::json!({"rules":entries}),
        );
    }
    // Invalid rule-file diagnostics from the public loader API.
    let invalid_dir = root.join("parity/fixtures/invalid-rules");
    for entry in fs::read_dir(&invalid_dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        // Parse with the repo-relative name so error messages stay
        // checkout-independent — parse_rule_file would embed the absolute path.
        let relative = format!("parity/fixtures/invalid-rules/{name}");
        let text = fs::read_to_string(&path).unwrap();
        let result = lawlint_core::loader::parse_rule(&relative, &text)
            .map(|_| Value::Null)
            .unwrap_or_else(|err| serde_json::json!({"error":err.to_string()}));
        add(
            &mut files,
            "loader_errors",
            format!("loader-error-{}", name.trim_end_matches(".md")),
            source_obj("fixture", &format!("invalid-rules/{name}")),
            serde_json::json!({"loader":"parse_rule_file"}),
            &text,
            result,
        );
    }
    // Judge planning for built-in inferential rules with both requested plans.
    let judge_text = fs::read_to_string(root.join("parity/fixtures/text/judge-input.txt")).unwrap();
    let doc = lawlint_core::parse(&judge_text, false);
    let fragments = inferential_fragments(built_in_rule_set());
    let refs: Vec<_> = fragments.iter().collect();
    for (id, plan) in [
        ("default", JudgePlan::default()),
        (
            "perRule-false-context-48",
            JudgePlan {
                context_chars: 48,
                per_rule: false,
            },
        ),
    ] {
        let requests = plan_judge_with(&doc, &judge_text, &refs, &plan);
        add(
            &mut files,
            "judge_plan",
            format!("judge-plan-{id}"),
            source_obj("fixture", "text/judge-input.txt"),
            serde_json::json!({"contextChars":plan.context_chars,"perRule":plan.per_rule}),
            &judge_text,
            serde_json::json!({"requests":requests}),
        );
    }
    grounding_surface(&mut files, root);
    docx_surface(&mut files, root, &mut binary_files);

    let mut output = BTreeMap::new();
    let mut counts = BTreeMap::new();
    for (file, mut cases) in files {
        cases.sort_by(|a, b| a.0.cmp(&b.0));
        let mut seen = HashSet::new();
        for (id, _) in &cases {
            assert!(seen.insert(id.clone()), "duplicate case id {id} in {file}");
        }
        counts.insert(file.trim_end_matches(".jsonl").to_string(), cases.len());
        let contents = cases
            .into_iter()
            .map(|(_, case)| format!("{}\n", serde_json::to_string(&case).unwrap()))
            .collect::<String>();
        output.insert(format!("parity/golden/{file}"), contents.into_bytes());
    }
    let inputs = INPUTS.lock().unwrap();
    let input_lines = inputs
        .iter()
        .map(|(id, (text, hash))| {
            format!(
                "{}\n",
                serde_json::to_string(&serde_json::json!({"id":id,"text":text,"sha256":hash}))
                    .unwrap()
            )
        })
        .collect::<String>();
    output.insert(
        "parity/golden/inputs.jsonl".into(),
        input_lines.into_bytes(),
    );
    drop(inputs);
    let json_size: usize = output.values().map(Vec::len).sum();
    let docx_size: usize = binary_files.values().map(Vec::len).sum();
    println!(
        "Captured {} corpus samples, {} surfaces, {:.1}s; golden bytes: JSONL={} docx={} total={}",
        samples.len(),
        counts.len(),
        started.elapsed().as_secs_f64(),
        json_size,
        docx_size,
        json_size + docx_size
    );
    for (surface, count) in &counts {
        println!("{surface}: {count}");
    }
    for (path, bytes) in binary_files {
        output.insert(path.to_string_lossy().to_string(), bytes);
    }
    (output, counts)
}

fn check_diff(path: &Path, expected: &[u8], actual: &[u8]) -> Vec<String> {
    let left: Vec<&str> = std::str::from_utf8(expected)
        .unwrap_or("")
        .lines()
        .collect();
    let right: Vec<&str> = std::str::from_utf8(actual).unwrap_or("").lines().collect();
    let mut ids = Vec::new();
    for index in 0..left.len().max(right.len()) {
        if left.get(index) != right.get(index) {
            let id = right
                .get(index)
                .or_else(|| left.get(index))
                .and_then(|line| serde_json::from_str::<Value>(line).ok())
                .and_then(|v| v["id"].as_str().map(str::to_owned))
                .unwrap_or_else(|| format!("line-{}", index + 1));
            if !ids.contains(&id) {
                ids.push(id);
            }
            if ids.len() == 5 {
                break;
            }
        }
    }
    if ids.is_empty() {
        vec![format!("{}: binary content differs", path.display())]
    } else {
        ids.into_iter()
            .map(|id| format!("{}: {id}", path.display()))
            .collect()
    }
}

fn main() {
    let check = std::env::args().skip(1).any(|arg| arg == "--check");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = root.canonicalize().expect("repository root");
    let (expected, _) = generate(&root);
    let mut differences = Vec::new();
    for (relative, bytes) in &expected {
        let path = root.join(relative);
        if check {
            match fs::read(&path) {
                Ok(existing) if existing == *bytes => {}
                Ok(existing) => differences.extend(check_diff(&path, &existing, bytes)),
                Err(_) => differences.push(format!("{}: missing", path.display())),
            }
        } else if relative.ends_with(".docx") {
            fs::write(&path, bytes).expect("write docx golden");
        } else {
            fs::create_dir_all(path.parent().unwrap()).expect("create golden dir");
            fs::write(&path, bytes).expect("write golden JSONL");
        }
    }
    // Any golden file on disk the generator no longer produces is stale:
    // report it in check mode, delete it when writing.
    let golden_dir = root.join("parity/golden");
    let mut on_disk: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&golden_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                for nested in fs::read_dir(&path).into_iter().flatten().flatten() {
                    on_disk.push(nested.path());
                }
            } else {
                on_disk.push(path);
            }
        }
    }
    for path in on_disk {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if !expected.contains_key(relative.as_str()) {
            if check {
                differences.push(format!("{}: stale golden not generated", path.display()));
            } else {
                fs::remove_file(&path).expect("remove stale golden");
            }
        }
    }
    if check && !differences.is_empty() {
        eprintln!(
            "Parity mismatch (first differing case ids):\n{}",
            differences.join("\n")
        );
        std::process::exit(1);
    }
    if check {
        println!("Parity goldens match.");
    }
}
