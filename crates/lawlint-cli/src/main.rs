//! lawlint CLI — consumer of the v2 engine (docs/engine-design.md §11).
//!
//! Exit codes: 0 clean, 1 findings over limit (any error-severity finding or
//! warnings > --max-warnings), 2 I/O or config errors.

use clap::{ArgAction, Parser, Subcommand};
use colored::Colorize;
use lawlint_core::{
    apply_fixes, lint_full, lint_with, loader, Applicability, Diagnostic, Judge, JudgeCache,
    JudgeOptions, LintOptions, LintResult, RuleMeta, RuleSet, Severity,
};
use lawlint_judge::DiskCache;
use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

mod tui;
mod update;

#[derive(Debug, Parser)]
#[command(
    name = "lawlint",
    about = "Lint AI-generated legal and general text.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// File to lint ("-" for stdin).
    #[arg(value_name = "FILE", default_value = "-")]
    file: String,
    #[arg(long, value_parser = ["pretty", "json"], default_value = "pretty")]
    format: String,
    /// Enable only these rules (full ids or bare aliases).
    #[arg(long, value_name = "ID,ID", value_delimiter = ',')]
    rules: Option<Vec<String>>,
    #[arg(long, value_name = "ID,ID", value_delimiter = ',')]
    disable: Option<Vec<String>>,
    #[arg(long)]
    markdown: bool,
    #[arg(long, value_name = "N|inf", default_value = "inf")]
    max_warnings: String,
    #[arg(long)]
    quiet: bool,
    /// Additional rule package directory (repeatable; merged over built-ins).
    #[arg(long = "rule-dir", value_name = "DIR", action = ArgAction::Append, global = true)]
    rule_dir: Vec<PathBuf>,
    /// Run the tier-3 judge. Bare `--judge` uses the default local model;
    /// `--judge=MODEL` selects a backend (local:<repo>[#<gguf>],
    /// anthropic:<model>, openai:<base-url>#<model>).
    #[arg(long, value_name = "MODEL", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    judge: Option<String>,
    /// Apply MachineApplicable fixes to FILE in place.
    #[arg(long)]
    fix: bool,
    /// Skip the once-a-day check for a newer lawlint release.
    #[arg(long, global = true)]
    no_update_check: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List rules, or test rule packages.
    Rules {
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        action: Option<RulesAction>,
    },
    /// Download and install the latest lawlint release (docs §11).
    SelfUpdate {
        /// Report current/latest and whether an update is available; no download.
        #[arg(long)]
        check: bool,
        /// Reinstall even when already on the latest version.
        #[arg(long)]
        force: bool,
        /// Install this specific version instead of the latest.
        #[arg(long, value_name = "X")]
        version: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum RulesAction {
    /// Run each YAML rule's own examples and report pass/fail per example.
    Test {
        #[arg(value_name = "FILE_OR_DIR")]
        path: PathBuf,
        /// Judge inferential rules' flag/pass examples (otherwise skipped).
        #[arg(long, value_name = "MODEL", num_args = 0..=1, require_equals = true, default_missing_value = "")]
        judge: Option<String>,
        /// Skip inferential flag/pass examples (never loads a judge model).
        #[arg(long, conflicts_with = "judge")]
        offline: bool,
    },
}

// ---- config ------------------------------------------------------------

/// Walk up from `directory` looking for `lawlint.config.json`. A file that
/// exists but does not parse is a config error (exit 2), not a silent skip.
pub(crate) fn find_config(
    mut directory: PathBuf,
) -> Result<(LintOptions, Option<PathBuf>), String> {
    loop {
        let path = directory.join("lawlint.config.json");
        if path.is_file() {
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let options: LintOptions = serde_json::from_str(&text)
                .map_err(|error| format!("{}: invalid config: {error}", path.display()))?;
            return Ok((options, Some(directory)));
        }
        if !directory.pop() {
            return Ok((LintOptions::default(), None));
        }
    }
}

fn merge_options(mut config: LintOptions, cli: LintOptions) -> LintOptions {
    if cli.enable.is_some() {
        config.enable = cli.enable;
    }
    if cli.disable.is_some() {
        config.disable = cli.disable;
    }
    if cli.markdown.is_some() {
        config.markdown = cli.markdown;
    }
    config.severity = merge_map(config.severity, cli.severity);
    config.thresholds = merge_map(config.thresholds, cli.thresholds);
    config
}

fn merge_map<T>(
    base: Option<HashMap<String, T>>,
    override_map: Option<HashMap<String, T>>,
) -> Option<HashMap<String, T>> {
    match (base, override_map) {
        (None, None) => None,
        (base, Some(overrides)) => {
            let mut merged = base.unwrap_or_default();
            merged.extend(overrides);
            Some(merged)
        }
        (base, None) => base,
    }
}

// ---- rule set ----------------------------------------------------------

/// Built-ins + config `ruleDirs` (relative to the config file's directory)
/// plus `--rule-dir` flags (relative to cwd). LoadError messages are
/// product-quality: propagate them verbatim.
pub(crate) fn build_rule_set(
    config: &LintOptions,
    config_dir: Option<&Path>,
    cli_dirs: &[PathBuf],
) -> Result<RuleSet, String> {
    let mut set = RuleSet::built_in();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(config_dirs) = &config.rule_dirs {
        for dir in config_dirs {
            let path = PathBuf::from(dir);
            dirs.push(match config_dir {
                Some(base) if path.is_relative() => base.join(path),
                _ => path,
            });
        }
    }
    dirs.extend(cli_dirs.iter().cloned());
    for dir in dirs {
        let package = RuleSet::load_dir(&dir).map_err(|error| error.to_string())?;
        set.merge(package).map_err(|error| error.to_string())?;
    }
    Ok(set)
}

// ---- judge -------------------------------------------------------------

/// The judge model to use, if the judge is active at all: the CLI flag wins
/// (bare `--judge` = `Some(None)` = default model), else config
/// `judge.enabled: true`.
pub(crate) fn judge_spec(
    cli_judge: &Option<String>,
    config: &LintOptions,
) -> Option<Option<String>> {
    let config_judge = config.judge.as_ref();
    let config_model = config_judge.and_then(|judge| judge.model.clone());
    match cli_judge {
        Some(model) if !model.is_empty() => Some(Some(model.clone())),
        Some(_) => Some(config_model),
        None if config_judge.and_then(|judge| judge.enabled) == Some(true) => Some(config_model),
        None => None,
    }
}

/// Build the judge + disk cache. A cache failure is not fatal (judge runs
/// uncached); a judge build failure is reported to the caller, who falls
/// back to tiers 1-2.
fn build_judge(
    model: Option<String>,
    floor: Option<f32>,
) -> Result<(Box<dyn Judge>, Option<DiskCache>), String> {
    let options = JudgeOptions {
        enabled: Some(true),
        model,
        floor,
    };
    let judge = lawlint_judge::create_judge(&options).map_err(|error| error.to_string())?;
    let cache = match DiskCache::new() {
        Ok(cache) => Some(cache),
        Err(error) => {
            eprintln!("lawlint: warning: judge cache unavailable ({error}); running uncached");
            None
        }
    };
    Ok((judge, cache))
}

pub(crate) fn lint_text(
    text: &str,
    options: &LintOptions,
    rules: &RuleSet,
    judge: Option<Option<String>>,
) -> LintResult {
    let Some(model) = judge else {
        return lint_with(text, options, rules);
    };
    let floor = options.judge.as_ref().and_then(|judge| judge.floor);
    match build_judge(model, floor) {
        Ok((judge, cache)) => {
            let result = lint_full(
                text,
                options,
                rules,
                judge.as_ref(),
                cache.as_ref().map(|cache| cache as &dyn JudgeCache),
            );
            if let Some(stats) = &result.judge {
                if stats.chunks_failed > 0 {
                    eprintln!(
                        "lawlint: warning: judge failed on {} of {} chunks; those chunks used tiers 1-2 only",
                        stats.chunks_failed, stats.chunks
                    );
                }
            }
            result
        }
        Err(error) => {
            eprintln!("lawlint: warning: judge unavailable ({error}); running tiers 1-2 only");
            lint_with(text, options, rules)
        }
    }
}

// ---- lint command ------------------------------------------------------

fn cli_options(cli: &Cli, markdown: Option<bool>) -> LintOptions {
    LintOptions {
        enable: cli.rules.clone(),
        disable: cli.disable.clone(),
        markdown,
        ..Default::default()
    }
}

fn read_input(file: &str) -> Result<(String, bool), String> {
    if file == "-" {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| format!("failed to read stdin: {error}"))?;
        return Ok((text, false));
    }
    let path = Path::new(file);
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok((
        text,
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md")),
    ))
}

fn colors_enabled() -> bool {
    io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn severity_colored(severity: Severity, value: &str, color: bool) -> String {
    if !color {
        return value.to_string();
    }
    match severity {
        Severity::Error => value.red().to_string(),
        Severity::Warning => value.yellow().to_string(),
        Severity::Suggestion => value.cyan().to_string(),
    }
}

pub(crate) fn format_pretty(result: &LintResult, quiet: bool, color: bool) -> String {
    if quiet {
        return String::new();
    }
    let mut lines = Vec::new();
    if result.diagnostics.is_empty() {
        let check = "✓ No issues found.";
        lines.push(if color {
            check.green().to_string()
        } else {
            check.into()
        });
    }
    for diagnostic in &result.diagnostics {
        let location = format!("{}:{}", diagnostic.line, diagnostic.column);
        let location = severity_colored(diagnostic.severity, &location, color);
        let rule = if color {
            diagnostic.rule_id.0.bold().to_string()
        } else {
            diagnostic.rule_id.0.clone()
        };
        lines.push(format!("{location} {rule} {}", diagnostic.message));
        let excerpt = if color {
            diagnostic.excerpt.dimmed().to_string()
        } else {
            diagnostic.excerpt.clone()
        };
        lines.push(format!("  {excerpt}"));
        if let Some(suggestion) = &diagnostic.suggestion {
            let value = format!("Suggestion: {suggestion}");
            lines.push(format!(
                "  {}",
                if color {
                    value.dimmed().to_string()
                } else {
                    value
                }
            ));
        }
    }
    lines.push(format!(
        "\nHuman-likeness score: {}/100 ({} words, {} sentences)",
        result.stats.score, result.stats.word_count, result.stats.sentence_count
    ));
    lines.join("\n")
}

fn pretty(result: &LintResult, quiet: bool) {
    println!("{}", format_pretty(result, quiet, colors_enabled()));
}

fn machine_fix_count(diagnostics: &[Diagnostic]) -> usize {
    diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .fix
                .as_ref()
                .is_some_and(|fix| fix.applicability == Applicability::MachineApplicable)
        })
        .count()
}

pub(crate) fn exit_code(result: &LintResult, max_warnings: &str) -> i32 {
    let warnings = result
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Warning)
        .count();
    let limit = if max_warnings == "inf" {
        None
    } else {
        max_warnings.parse::<f64>().ok()
    };
    let over = result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
        || limit.is_some_and(|limit| warnings as f64 > limit);
    if over {
        1
    } else {
        0
    }
}

fn lint_command(cli: &Cli) -> Result<i32, String> {
    if cli.fix && cli.file == "-" {
        return Err("--fix requires a FILE argument (cannot fix stdin)".into());
    }
    let (text, file_markdown) = read_input(&cli.file)?;
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let (config, config_dir) = find_config(cwd)?;
    let markdown = if cli.file == "-" {
        cli.markdown.then_some(true)
    } else {
        Some(cli.markdown || file_markdown)
    };
    let rules = build_rule_set(&config, config_dir.as_deref(), &cli.rule_dir)?;
    let judge = judge_spec(&cli.judge, &config);
    let options = merge_options(config, cli_options(cli, markdown));
    let result = lint_text(&text, &options, &rules, judge);

    if cli.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
        );
    } else {
        pretty(&result, cli.quiet);
    }

    if cli.fix {
        let fixed = apply_fixes(&text, &result.diagnostics);
        let count = machine_fix_count(&result.diagnostics);
        if fixed != text {
            fs::write(&cli.file, &fixed)
                .map_err(|error| format!("failed to write {}: {error}", cli.file))?;
        }
        // Status line, not lint output: stderr, so `--format json` stdout
        // stays machine-parseable.
        eprintln!(
            "Applied {count} fix{} to {}",
            if count == 1 { "" } else { "es" },
            cli.file
        );
    }

    Ok(exit_code(&result, &cli.max_warnings))
}

// ---- rules command -----------------------------------------------------

/// One entry of the `rules --json` contract: a flat id (package namespace
/// stripped, matching `docsUrl` slugs and the website's `/rules/<id>` routes)
/// wrapping a camelCase `meta` object — the pre-rewrite CLI shape that
/// `rules:generate` feeds to apps/website/src/pages/rules/*. Core's
/// `RuleMeta` serializes namespaced ids and snake_case `docs_url`; do not
/// print it directly.
fn rule_meta_json(meta: &RuleMeta) -> serde_json::Value {
    let flat_id = meta
        .id
        .0
        .split_once('/')
        .map_or(meta.id.0.as_str(), |(_, rest)| rest);
    let mut wrapped = serde_json::json!({
        "tier": meta.tier,
        "scope": meta.scope,
        "severity": meta.severity,
        "description": meta.description,
        "docsUrl": meta.docs_url,
        "examples": meta.examples,
    });
    if let Some(rationale) = &meta.rationale {
        wrapped["rationale"] = serde_json::json!(rationale);
    }
    serde_json::json!({ "id": flat_id, "meta": wrapped })
}

fn rules_list(json: bool, cli_dirs: &[PathBuf]) -> Result<i32, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let (config, config_dir) = find_config(cwd)?;
    let rules = build_rule_set(&config, config_dir.as_deref(), cli_dirs)?;
    if json {
        let entries: Vec<serde_json::Value> =
            rules.metas().into_iter().map(rule_meta_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&entries).map_err(|error| error.to_string())?
        );
        return Ok(0);
    }
    let color = colors_enabled();
    for meta in rules.metas() {
        let severity = match meta.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Suggestion => "suggestion",
        };
        let id = if color {
            meta.id.0.bold().to_string()
        } else {
            meta.id.0.clone()
        };
        println!(
            "{id:<36} {:<12} {}",
            severity_colored(meta.severity, severity, color),
            meta.description
        );
    }
    Ok(0)
}

// ---- rules test --------------------------------------------------------

/// Collect the rule YAML files under `path`: a single file, a package dir
/// (`style.yaml` + `rules/`), or a loose directory of rule files.
fn collect_rule_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        return Err(format!(
            "failed to read {}: no such file or directory",
            path.display()
        ));
    }
    let rules_dir = if path.join("style.yaml").is_file() && path.join("rules").is_dir() {
        path.join("rules")
    } else {
        path.to_path_buf()
    };
    let mut files: Vec<PathBuf> = fs::read_dir(&rules_dir)
        .map_err(|error| format!("failed to read {}: {error}", rules_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|ext| ext.to_str()),
                Some("yaml") | Some("yml")
            ) && p.file_name().and_then(|name| name.to_str()) != Some("style.yaml")
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "no rule YAML files found in {}",
            rules_dir.display()
        ));
    }
    Ok(files)
}

/// The package name for a rule file: the nearest `style.yaml` manifest
/// (sibling, or sibling of the containing `rules/` dir), else "test".
fn package_name_for(file: &Path) -> String {
    let mut candidates = Vec::new();
    if let Some(parent) = file.parent() {
        candidates.push(parent.join("style.yaml"));
        if let Some(grandparent) = parent.parent() {
            candidates.push(grandparent.join("style.yaml"));
        }
    }
    for candidate in candidates {
        if let Ok(text) = fs::read_to_string(&candidate) {
            if let Ok(manifest) = loader::parse_manifest(&candidate.display().to_string(), &text) {
                return manifest.name;
            }
        }
    }
    "test".to_string()
}

static TEST_PKG_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Build a single-rule `RuleSet` for one rule file by staging a throwaway
/// package directory (core's only public loading path is `load_dir`).
fn single_rule_set(file: &Path, package: &str) -> Result<RuleSet, String> {
    let staging = std::env::temp_dir().join(format!(
        "lawlint-rules-test-{}-{}",
        std::process::id(),
        TEST_PKG_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let build = || -> Result<RuleSet, String> {
        let rules_dir = staging.join("rules");
        fs::create_dir_all(&rules_dir).map_err(|error| error.to_string())?;
        fs::write(
            staging.join("style.yaml"),
            format!("name: {package}\nversion: 0.0.0\n"),
        )
        .map_err(|error| error.to_string())?;
        let name = file
            .file_name()
            .ok_or_else(|| format!("{}: not a file", file.display()))?;
        fs::copy(file, rules_dir.join(name)).map_err(|error| error.to_string())?;
        RuleSet::load_dir(&staging).map_err(|error| error.to_string())
    };
    let result = build();
    let _ = fs::remove_dir_all(&staging);
    result
}

struct TestTally {
    passed: usize,
    failed: usize,
    skipped: usize,
}

impl TestTally {
    fn record(&mut self, label: &str, outcome: Result<(), String>, color: bool) {
        match outcome {
            Ok(()) => {
                self.passed += 1;
                let tag = if color {
                    "pass".green().to_string()
                } else {
                    "pass".into()
                };
                println!("  {label:<10} {tag}");
            }
            Err(reason) => {
                self.failed += 1;
                let tag = if color {
                    "FAIL".red().to_string()
                } else {
                    "FAIL".into()
                };
                println!("  {label:<10} {tag} — {reason}");
            }
        }
    }

    fn skip(&mut self, label: &str, reason: &str, color: bool) {
        self.skipped += 1;
        let tag = if color {
            "skip".yellow().to_string()
        } else {
            "skip".into()
        };
        println!("  {label:<10} {tag} ({reason})");
    }

    fn record_outcome(&mut self, label: &str, outcome: ExampleOutcome, color: bool) {
        match outcome {
            ExampleOutcome::Pass => self.record(label, Ok(()), color),
            ExampleOutcome::Fail(reason) => self.record(label, Err(reason), color),
            ExampleOutcome::Skip(reason) => self.skip(label, &reason, color),
        }
    }
}

fn has_rule(result: &LintResult, full_id: &str) -> bool {
    result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.rule_id.0 == full_id)
}

#[derive(Debug)]
enum ExampleOutcome {
    Pass,
    Fail(String),
    Skip(String),
}

/// Verdict for one judged flag/pass example. A flagged finding is always
/// conclusive, but "no finding" proves nothing when the judge backend failed
/// on chunks — report the failure instead of a vacuous pass/FAIL that blames
/// the rubric for a broken judge.
fn judge_example_outcome(result: &LintResult, full_id: &str, expect_flag: bool) -> ExampleOutcome {
    if has_rule(result, full_id) {
        return if expect_flag {
            ExampleOutcome::Pass
        } else {
            ExampleOutcome::Fail(format!(
                "expected no {full_id} finding, but the judge flagged it"
            ))
        };
    }
    if let Some(stats) = &result.judge {
        if stats.chunks_failed > 0 {
            return ExampleOutcome::Skip(format!(
                "judge failed on {} of {} chunks",
                stats.chunks_failed, stats.chunks
            ));
        }
    }
    if expect_flag {
        ExampleOutcome::Fail(format!(
            "expected the judge to flag {full_id}, got no finding"
        ))
    } else {
        ExampleOutcome::Pass
    }
}

fn rules_test(path: &Path, judge_flag: &Option<String>, offline: bool) -> Result<i32, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let (config, _) = find_config(cwd)?;
    let files = collect_rule_files(path)?;
    let color = colors_enabled();
    let mut tally = TestTally {
        passed: 0,
        failed: 0,
        skipped: 0,
    };

    // Judge built lazily on the first inferential example; a build failure
    // downgrades to "no judge" (examples skipped), never a crash.
    // Outer Option: not yet attempted; inner: build outcome.
    type JudgeState = Option<(Box<dyn Judge>, Option<DiskCache>)>;
    // Unlike `lint`, the judge runs only when `--judge` is passed explicitly
    // ("otherwise skipped", per the flag's help and design doc §11): a config
    // that enables the judge for lint runs must not trigger model downloads
    // or cloud calls from `rules test`. Bare `--judge` still inherits the
    // config model. `--offline` forces the skip.
    let judge_model = if offline || judge_flag.is_none() {
        None
    } else {
        judge_spec(judge_flag, &config)
    };
    let mut judge_state: Option<JudgeState> = None;
    // The config's judge options ride along in `options` so lint_full's
    // tier-3 confidence gate honors a configured `judge.floor` (a bare
    // LintOptions::default() would silently pin the floor to 0.6).
    let options = LintOptions {
        judge: config.judge.clone(),
        ..LintOptions::default()
    };
    let floor = options.judge.as_ref().and_then(|judge| judge.floor);

    for file in &files {
        let display = file.display().to_string();
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) => {
                println!("{display}");
                tally.record("load", Err(format!("failed to read: {error}")), color);
                continue;
            }
        };
        let def = match loader::parse_rule(&display, &text) {
            Ok(def) => def,
            Err(error) => {
                println!("{display}");
                tally.record("load", Err(error.to_string()), color);
                continue;
            }
        };
        let package = package_name_for(file);
        let full_id = format!("{package}/{}", def.id);
        println!(
            "{}",
            if color {
                full_id.bold().to_string()
            } else {
                full_id.clone()
            }
        );
        let set = match single_rule_set(file, &package) {
            Ok(set) => set,
            Err(error) => {
                tally.record("load", Err(error), color);
                continue;
            }
        };

        if def.engine == "inferential" {
            if judge_model.is_none() {
                for index in 0..def.flag_examples.len() {
                    tally.skip(
                        &format!("flag[{index}]"),
                        "inferential; run with --judge",
                        color,
                    );
                }
                for index in 0..def.pass_examples.len() {
                    tally.skip(
                        &format!("pass[{index}]"),
                        "inferential; run with --judge",
                        color,
                    );
                }
                continue;
            }
            if judge_state.is_none() {
                judge_state = Some(match build_judge(judge_model.clone().unwrap(), floor) {
                    Ok(built) => Some(built),
                    Err(error) => {
                        eprintln!(
                            "lawlint: warning: judge unavailable ({error}); skipping inferential examples"
                        );
                        None
                    }
                });
            }
            let Some(Some((judge, cache))) = judge_state.as_ref() else {
                for index in 0..def.flag_examples.len() {
                    tally.skip(&format!("flag[{index}]"), "judge unavailable", color);
                }
                for index in 0..def.pass_examples.len() {
                    tally.skip(&format!("pass[{index}]"), "judge unavailable", color);
                }
                continue;
            };
            let cache_ref = cache.as_ref().map(|cache| cache as &dyn JudgeCache);
            for (index, example) in def.flag_examples.iter().enumerate() {
                let result = lint_full(example, &options, &set, judge.as_ref(), cache_ref);
                let outcome = judge_example_outcome(&result, &full_id, true);
                tally.record_outcome(&format!("flag[{index}]"), outcome, color);
            }
            for (index, example) in def.pass_examples.iter().enumerate() {
                let result = lint_full(example, &options, &set, judge.as_ref(), cache_ref);
                let outcome = judge_example_outcome(&result, &full_id, false);
                tally.record_outcome(&format!("pass[{index}]"), outcome, color);
            }
            continue;
        }

        if def.examples.is_empty() {
            tally.skip("examples", "rule declares no examples", color);
            continue;
        }
        for (index, example) in def.examples.iter().enumerate() {
            let bad = lint_with(&example.bad, &options, &set);
            let outcome = if has_rule(&bad, &full_id) {
                Ok(())
            } else {
                Err(format!("expected at least one {full_id} finding, got none"))
            };
            tally.record(&format!("bad[{index}]"), outcome, color);

            let good = lint_with(&example.good, &options, &set);
            let outcome = if has_rule(&good, &full_id) {
                Err(format!(
                    "expected no {full_id} finding, got {}",
                    good.diagnostics.len()
                ))
            } else {
                Ok(())
            };
            tally.record(&format!("good[{index}]"), outcome, color);
        }
    }

    println!(
        "\nrules test: {} passed, {} failed, {} skipped",
        tally.passed, tally.failed, tally.skipped
    );
    Ok(if tally.failed > 0 { 1 } else { 0 })
}

// ---- entry -------------------------------------------------------------

fn run(cli: Cli) -> Result<i32, String> {
    // A bare `lawlint` in an interactive terminal launches the TUI instead of
    // blocking on stdin.
    if std::env::args().len() == 1 && io::stdin().is_terminal() {
        return tui::run_tui();
    }

    match &cli.command {
        Some(Command::Rules { json, action }) => match action {
            Some(RulesAction::Test {
                path,
                judge,
                offline,
            }) => rules_test(path, judge, *offline),
            None => rules_list(*json, &cli.rule_dir),
        },
        Some(Command::SelfUpdate {
            check,
            force,
            version,
        }) => update::self_update(env!("CARGO_PKG_VERSION"), *check, *force, version.clone()),
        None => {
            let code = lint_command(&cli)?;
            // At the very END of a normal lint run, after output is written:
            // subtle, at-most-daily update notice. Never changes the exit code.
            update::maybe_notify(
                env!("CARGO_PKG_VERSION"),
                &update::NotifyOptions {
                    no_update_check_flag: cli.no_update_check,
                    json_format: cli.format == "json",
                },
            );
            Ok(code)
        }
    }
}

fn main() {
    match run(Cli::parse()) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("lawlint: {error}");
            std::process::exit(2);
        }
    }
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use lawlint_core::{Edit, Fix, RuleId, TextRange, Tier};

    fn options_with(f: impl FnOnce(&mut LintOptions)) -> LintOptions {
        let mut options = LintOptions::default();
        f(&mut options);
        options
    }

    #[test]
    fn merge_options_cli_wins_and_maps_deep_merge() {
        let config = options_with(|o| {
            o.enable = Some(vec!["a".into()]);
            o.markdown = Some(true);
            o.severity = Some(
                [
                    ("x".to_string(), Severity::Error),
                    ("y".to_string(), Severity::Warning),
                ]
                .into_iter()
                .collect(),
            );
            o.thresholds = Some([("t".to_string(), 8.0)].into_iter().collect());
        });
        let cli = options_with(|o| {
            o.enable = Some(vec!["b".into()]);
            o.severity = Some(
                [("x".to_string(), Severity::Suggestion)]
                    .into_iter()
                    .collect(),
            );
        });
        let merged = merge_options(config, cli);
        assert_eq!(merged.enable, Some(vec!["b".to_string()]));
        assert_eq!(merged.markdown, Some(true)); // CLI None → config kept
        let severity = merged.severity.unwrap();
        assert_eq!(severity.get("x"), Some(&Severity::Suggestion)); // overridden
        assert_eq!(severity.get("y"), Some(&Severity::Warning)); // preserved
        assert_eq!(merged.thresholds.unwrap().get("t"), Some(&8.0));
    }

    #[test]
    fn judge_spec_resolution() {
        let off = LintOptions::default();
        assert_eq!(judge_spec(&None, &off), None);
        // Bare --judge → default model.
        assert_eq!(judge_spec(&Some(String::new()), &off), Some(None));
        // Explicit model on the flag.
        assert_eq!(
            judge_spec(&Some("local:foo".into()), &off),
            Some(Some("local:foo".into()))
        );
        // Config-enabled with a model.
        let config = options_with(|o| {
            o.judge = Some(JudgeOptions {
                enabled: Some(true),
                model: Some("anthropic:m".into()),
                floor: None,
            });
        });
        assert_eq!(judge_spec(&None, &config), Some(Some("anthropic:m".into())));
        // Bare --judge inherits the config model.
        assert_eq!(
            judge_spec(&Some(String::new()), &config),
            Some(Some("anthropic:m".into()))
        );
        // Config present but not enabled → off.
        let disabled = options_with(|o| {
            o.judge = Some(JudgeOptions {
                enabled: None,
                model: Some("anthropic:m".into()),
                floor: None,
            });
        });
        assert_eq!(judge_spec(&None, &disabled), None);
    }

    fn diagnostic(severity: Severity, fix: Option<Fix>) -> Diagnostic {
        Diagnostic {
            rule_id: RuleId("core/x".into()),
            severity,
            tier: Tier::Static,
            span: TextRange { start: 0, end: 1 },
            message: "m".into(),
            line: 1,
            column: 1,
            end_line: None,
            end_column: None,
            excerpt: String::new(),
            suggestion: None,
            weight: None,
            confidence: None,
            fix,
        }
    }

    fn result_with(diagnostics: Vec<Diagnostic>) -> LintResult {
        LintResult {
            diagnostics,
            stats: lawlint_core::Stats {
                word_count: 0,
                sentence_count: 0,
                score: 100,
            },
            judge: None,
        }
    }

    #[test]
    fn exit_code_error_and_max_warnings() {
        let clean = result_with(vec![]);
        assert_eq!(exit_code(&clean, "inf"), 0);

        let error = result_with(vec![diagnostic(Severity::Error, None)]);
        assert_eq!(exit_code(&error, "inf"), 1);

        let warnings = result_with(vec![
            diagnostic(Severity::Warning, None),
            diagnostic(Severity::Warning, None),
        ]);
        assert_eq!(exit_code(&warnings, "inf"), 0);
        assert_eq!(exit_code(&warnings, "2"), 0);
        assert_eq!(exit_code(&warnings, "1"), 1);
        // Suggestions never trip the limit.
        let suggestion = result_with(vec![diagnostic(Severity::Suggestion, None)]);
        assert_eq!(exit_code(&suggestion, "0"), 0);
        // Unparseable limit behaves like inf (legacy UX).
        assert_eq!(exit_code(&warnings, "banana"), 0);
    }

    #[test]
    fn machine_fix_count_only_counts_machine_applicable() {
        let fix = |applicability| Fix {
            edits: vec![Edit {
                range: TextRange { start: 0, end: 1 },
                replacement: "y".into(),
            }],
            applicability,
        };
        let diagnostics = vec![
            diagnostic(Severity::Error, Some(fix(Applicability::MachineApplicable))),
            diagnostic(Severity::Error, Some(fix(Applicability::MaybeIncorrect))),
            diagnostic(Severity::Error, None),
        ];
        assert_eq!(machine_fix_count(&diagnostics), 1);
    }

    #[test]
    fn rule_meta_json_matches_website_contract() {
        // Regression: `rules --json` must keep the pre-rewrite shape — flat
        // ids and a camelCase `meta` wrapper — or the website rules pages
        // (rule.meta.description / rule.meta.docsUrl) break.
        use lawlint_core::{Scope, Tier};
        let meta = lawlint_core::RuleMeta {
            id: RuleId("core/no-em-dash".into()),
            tier: Tier::Static,
            scope: Scope::Prose,
            severity: Severity::Warning,
            description: "Flags em dashes.".into(),
            docs_url: "https://lawlint.com/rules/no-em-dash".into(),
            rationale: None,
            examples: vec![],
        };
        let value = rule_meta_json(&meta);
        assert_eq!(value["id"], "no-em-dash"); // namespace stripped
        let wrapped = &value["meta"];
        assert_eq!(wrapped["description"], "Flags em dashes.");
        assert_eq!(wrapped["severity"], "warning");
        assert_eq!(wrapped["docsUrl"], "https://lawlint.com/rules/no-em-dash");
        assert!(wrapped.get("docs_url").is_none(), "must be camelCase");
        assert!(wrapped.get("rationale").is_none(), "None rationale skipped");

        let mut with_rationale = meta.clone();
        with_rationale.rationale = Some("why".into());
        assert_eq!(rule_meta_json(&with_rationale)["meta"]["rationale"], "why");
    }

    #[test]
    fn judge_example_outcome_surfaces_chunk_failures() {
        // Regression: a judge that errored on chunks must not produce a
        // vacuous pass (pass_examples) or rubric-blaming FAIL (flag_examples).
        let stats = |chunks, chunks_failed| lawlint_core::JudgeStats {
            chunks,
            chunks_failed,
            ..Default::default()
        };
        let mut all_failed = result_with(vec![]);
        all_failed.judge = Some(stats(2, 2));
        assert!(matches!(
            judge_example_outcome(&all_failed, "t/x", true),
            ExampleOutcome::Skip(reason) if reason.contains("2 of 2 chunks")
        ));
        assert!(matches!(
            judge_example_outcome(&all_failed, "t/x", false),
            ExampleOutcome::Skip(_)
        ));

        // A real finding is conclusive even when other chunks failed.
        let mut flagged = result_with(vec![diagnostic(Severity::Warning, None)]);
        flagged.judge = Some(stats(2, 1));
        assert!(matches!(
            judge_example_outcome(&flagged, "core/x", true),
            ExampleOutcome::Pass
        ));
        assert!(matches!(
            judge_example_outcome(&flagged, "core/x", false),
            ExampleOutcome::Fail(_)
        ));

        // A clean judge run keeps the normal pass/FAIL semantics.
        let mut clean = result_with(vec![]);
        clean.judge = Some(stats(1, 0));
        assert!(matches!(
            judge_example_outcome(&clean, "t/x", true),
            ExampleOutcome::Fail(reason) if reason.contains("got no finding")
        ));
        assert!(matches!(
            judge_example_outcome(&clean, "t/x", false),
            ExampleOutcome::Pass
        ));
    }

    #[test]
    fn collect_rule_files_variants() {
        let base = std::env::temp_dir().join(format!("lawlint-cli-collect-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let rules = base.join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(base.join("style.yaml"), "name: firm\nversion: 1.0.0\n").unwrap();
        fs::write(rules.join("b.yaml"), "x").unwrap();
        fs::write(rules.join("a.yml"), "x").unwrap();
        fs::write(rules.join("notes.txt"), "x").unwrap();

        // Package dir → rules/*.yaml|yml, sorted.
        let files = collect_rule_files(&base).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|p| p.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["a.yml", "b.yaml"]
        );
        // Loose dir (no manifest) → its yaml files, style.yaml excluded.
        let files = collect_rule_files(&rules).unwrap();
        assert_eq!(files.len(), 2);
        // Single file.
        let single = collect_rule_files(&rules.join("b.yaml")).unwrap();
        assert_eq!(single.len(), 1);
        // Missing path → error.
        assert!(collect_rule_files(&base.join("nope")).is_err());

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn package_name_from_manifest_or_default() {
        let base = std::env::temp_dir().join(format!("lawlint-cli-pkgname-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let rules = base.join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(base.join("style.yaml"), "name: firm\nversion: 1.0.0\n").unwrap();
        let rule = rules.join("no-x.yaml");
        fs::write(&rule, "id: no-x\nengine: phrase\npatterns: [\"z\"]\n").unwrap();
        assert_eq!(package_name_for(&rule), "firm");

        let loose = base.join("loose.yaml");
        fs::write(&loose, "id: loose\nengine: phrase\npatterns: [\"z\"]\n").unwrap();
        // style.yaml IS a sibling here, so the manifest still wins.
        assert_eq!(package_name_for(&loose), "firm");

        let orphan_dir = base.join("orphan");
        fs::create_dir_all(&orphan_dir).unwrap();
        let orphan = orphan_dir.join("o.yaml");
        fs::write(&orphan, "x").unwrap();
        // Parent of parent is `base` which has style.yaml… so use a deeper dir.
        let deep_dir = base.join("deep").join("deeper");
        fs::create_dir_all(&deep_dir).unwrap();
        let deep = deep_dir.join("d.yaml");
        fs::write(&deep, "x").unwrap();
        assert_eq!(package_name_for(&deep), "test");

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn single_rule_set_builds_and_reports_rule_errors() {
        let base = std::env::temp_dir().join(format!("lawlint-cli-single-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let good = base.join("no-x.yaml");
        fs::write(
            &good,
            "id: no-x\nengine: phrase\nseverity: error\npatterns: [\"zebra\"]\n",
        )
        .unwrap();
        let set = single_rule_set(&good, "firm").unwrap();
        assert_eq!(set.metas().len(), 1);
        assert_eq!(set.metas()[0].id.0, "firm/no-x");
        let result = lint_with("A zebra appears.", &LintOptions::default(), &set);
        assert!(has_rule(&result, "firm/no-x"));

        let bad = base.join("broken.yaml");
        fs::write(&bad, "id: broken\nengine: phrase\npatterns: [\"(\"]\n").unwrap();
        assert!(single_rule_set(&bad, "firm").is_err());

        fs::remove_dir_all(&base).unwrap();
    }
}
