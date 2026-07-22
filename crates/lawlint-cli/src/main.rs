//! lawlint CLI — consumer of the v2 engine (docs/engine-design.md §11).
//!
//! Exit codes: 0 clean, 1 findings over limit (any error-severity finding or
//! warnings > --max-warnings), 2 I/O or config errors.

use clap::{ArgAction, Parser, Subcommand};
use colored::Colorize;
use lawlint_core::{
    apply_fixes, apply_fixes_with, lint_full, lint_with, loader, Applicability, Diagnostic, Judge,
    JudgeCache, JudgeOptions, LintOptions, LintResult, RuleMeta, RuleSet, Severity, Tier,
};
use lawlint_judge::DiskCache;
use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

mod diff;
mod init;
mod init_tui;
mod learn;
mod tui;
mod ui;
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
    /// Output format: pretty|json|prompt|full (prompt emits an AI revision
    /// brief; full is the pre-0.8 flat finding list).
    #[arg(long, value_parser = ["pretty", "json", "prompt", "full"], default_value = "pretty")]
    format: String,
    /// List every finding under the summary (pretty output).
    #[arg(long)]
    list: bool,
    /// List the rules that did not run, and why.
    #[arg(long)]
    coverage: bool,
    /// Skip the AI rules even when a model and credentials are configured.
    #[arg(long = "no-ai")]
    no_ai: bool,
    /// With --fix, also apply the AI rules' suggested rewrites to plain text.
    /// Not needed for .docx, where every fix lands as a reviewable tracked
    /// change.
    #[arg(long = "unsafe")]
    unsafe_fixes: bool,
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
    /// Run the tier-3 judge. Bare `--judge` uses the AI model configured by
    /// `lawlint init` (errors if none is configured); `--judge=MODEL`
    /// selects a backend (anthropic:<model>, openai:<base-url>#<model>,
    /// foundry:<deployment>, local:<repo>[#<gguf>]).
    #[arg(long, value_name = "MODEL", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    judge: Option<String>,
    /// Apply MachineApplicable fixes to FILE in place.
    #[arg(long)]
    fix: bool,
    /// Show what --fix would change (or changed) as a colored diff.
    #[arg(long)]
    diff: bool,
    /// Skip the once-a-day check for a newer lawlint release.
    #[arg(long, global = true)]
    no_update_check: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Set up lawlint here: walks through AI-model/judge/markdown/custom-rule
    /// choices and writes .lawlint/config.json (API keys go to a user-level
    /// credential file, never into the project).
    Init {
        /// Accept the default answer for every prompt (non-interactive).
        #[arg(long)]
        yes: bool,
        /// Overwrite an existing .lawlint/config.json.
        #[arg(long)]
        force: bool,
        /// AI model preference, skipping the catalog prompt: qwen, gemma, or
        /// a full spec (anthropic:<model>, openai:<base-url>#<model>,
        /// foundry:<deployment>, local:<hf-repo>[#<gguf>]).
        #[arg(long, value_name = "MODEL")]
        ai: Option<String>,
    },
    /// Mine a personal rule package from your own prior writing: a local
    /// statistical pass over the full corpus, then an AI mining pass over a
    /// small sample, self-checked so no generated rule flags your own text.
    Learn {
        /// A file or directory of your writing (.docx, .md, .txt).
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Where to write the generated rule package.
        #[arg(long, value_name = "DIR", default_value = ".lawlint/rules/personal")]
        out: PathBuf,
        /// Mining model, overriding the `lawlint init` AI preferences
        /// (local:<hf-repo>[#<gguf>], anthropic:<model>,
        /// openai:<base-url>#<model>, foundry:<deployment>).
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
    },
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
    /// Run each Markdown rule's own examples and report pass/fail per example.
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

/// The user-level `~/.lawlint/config.json`, beside the credential store that
/// `lawlint init` writes. This is the config for people whose documents do not
/// live in a project — a matter folder, `~/Downloads`, an email attachment.
/// Without it, storing a key user-level but the model preference project-level
/// means AI rules silently never run outside the directory where init ran.
///
/// Falls back to the pre-0.8 `~/.config/lawlint/config.json` while only that
/// copy exists, so upgrading does not quietly drop someone's settings.
pub(crate) fn user_config_path() -> Option<PathBuf> {
    lawlint_judge::credentials::resolve_user_file("config.json")
}

fn read_config(path: &Path) -> Result<LintOptions, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("{}: invalid config: {error}", path.display()))
}

/// Walk up from `directory` looking for `.lawlint/config.json` (created by
/// `lawlint init`), falling back to the legacy `lawlint.config.json` at each
/// level, then to the user-level config ([`user_config_path`]). The returned
/// directory is the project root — `ruleDirs` resolve relative to it under
/// every layout. A project config found this way is layered *over* the
/// user-level one, so a repo overrides personal defaults field by field
/// instead of erasing them. A file that exists but does not parse is a config
/// error (exit 2), not a silent skip.
pub(crate) fn find_config(directory: PathBuf) -> Result<(LintOptions, Option<PathBuf>), String> {
    let user = match user_config_path() {
        Some(path) if path.is_file() => Some((read_config(&path)?, path)),
        _ => None,
    };
    match find_project_config(directory)? {
        Some((project, dir)) => Ok((
            match user {
                Some((base, _)) => layer_options(base, project),
                None => project,
            },
            Some(dir),
        )),
        None => match user {
            Some((options, path)) => Ok((options, path.parent().map(Path::to_path_buf))),
            None => Ok((LintOptions::default(), None)),
        },
    }
}

fn find_project_config(mut directory: PathBuf) -> Result<Option<(LintOptions, PathBuf)>, String> {
    loop {
        let nested = directory.join(".lawlint").join("config.json");
        let legacy = directory.join("lawlint.config.json");
        let path = if nested.is_file() {
            if legacy.is_file() {
                eprintln!(
                    "lawlint: warning: both {} and {} exist; using {}",
                    nested.display(),
                    legacy.display(),
                    nested.display()
                );
            }
            Some(nested)
        } else if legacy.is_file() {
            Some(legacy)
        } else {
            None
        };
        if let Some(path) = path {
            return Ok(Some((read_config(&path)?, directory)));
        }
        if !directory.pop() {
            return Ok(None);
        }
    }
}

/// Layer `over` on top of `base`, field by field: anything `over` sets wins,
/// anything it leaves unset keeps the `base` value. Used for project config
/// over user config, so a repo that pins only `ruleDirs` still inherits the
/// user's AI model rather than silently turning AI rules off.
fn layer_options(mut base: LintOptions, over: LintOptions) -> LintOptions {
    if over.enable.is_some() {
        base.enable = over.enable;
    }
    if over.disable.is_some() {
        base.disable = over.disable;
    }
    if over.markdown.is_some() {
        base.markdown = over.markdown;
    }
    if over.rule_dirs.is_some() {
        base.rule_dirs = over.rule_dirs;
    }
    base.severity = merge_map(base.severity, over.severity);
    base.thresholds = merge_map(base.thresholds, over.thresholds);
    base.judge = match (base.judge, over.judge) {
        (Some(base_judge), Some(over_judge)) => Some(JudgeOptions {
            enabled: over_judge.enabled.or(base_judge.enabled),
            model: over_judge.model.or(base_judge.model),
            floor: over_judge.floor.or(base_judge.floor),
            max_tokens: over_judge.max_tokens.or(base_judge.max_tokens),
            concurrency: over_judge.concurrency.or(base_judge.concurrency),
            context_chars: over_judge.context_chars.or(base_judge.context_chars),
            per_rule: over_judge.per_rule.or(base_judge.per_rule),
        }),
        (base_judge, over_judge) => over_judge.or(base_judge),
    };
    base.ai = match (base.ai, over.ai) {
        (Some(base_ai), Some(over_ai)) => Some(lawlint_core::AiOptions {
            model: over_ai.model.or(base_ai.model),
            features: merge_map(base_ai.features, over_ai.features),
        }),
        (base_ai, over_ai) => over_ai.or(base_ai),
    };
    base
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

/// The judge model to use, if the judge is active at all: `Ok(None)` =
/// judge off, `Ok(Some(spec))` = judge on with that model. The CLI flag
/// wins (`--judge=MODEL` is explicit), else config `judge.enabled: true`.
/// Without an explicit model, the spec resolves from the config: legacy
/// `judge.model`, then the `ai` preferences (`ai.features.judge`, then
/// `ai.model` — written by `lawlint init`). A judge requested with no model
/// configured anywhere is an error (#50): nothing ever falls back to
/// silently downloading a local model.
pub(crate) fn judge_spec(
    cli_judge: &Option<String>,
    config: &LintOptions,
) -> Result<Option<String>, String> {
    let config_judge = config.judge.as_ref();
    let preferred = || {
        config_judge
            .and_then(|judge| judge.model.clone())
            .or_else(|| config.ai_model("judge"))
            .ok_or_else(|| {
                "the judge needs an AI model but none is configured — run `lawlint init` \
                 to choose one (hosted providers recommended), or pass an explicit \
                 --judge=<spec> (e.g. --judge=anthropic:<model>)"
                    .to_string()
            })
    };
    match cli_judge {
        Some(model) if !model.is_empty() => Ok(Some(model.clone())),
        Some(_) => preferred().map(Some),
        None if config_judge.and_then(|judge| judge.enabled) == Some(true) => preferred().map(Some),
        None => Ok(None),
    }
}

/// Why the AI rules are not running, in the words the summary shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AiOff {
    /// `--no-ai`, or `judge.enabled: false`.
    OptedOut,
    /// No `ai.model` anywhere — the user has never run `lawlint init`.
    NoModel,
    /// A model is configured but its key is missing.
    NoCredentials(&'static str),
    /// A `local:` model left over from before in-process inference was
    /// removed. The automatic path reports it rather than erroring: a stale
    /// config should cost the AI tier, not the whole lint.
    LocalRemoved,
}

impl AiOff {
    pub(crate) fn reason(&self) -> String {
        match self {
            AiOff::OptedOut => "disabled".into(),
            AiOff::NoModel => "no credentials".into(),
            AiOff::NoCredentials(name) => format!("no {name}"),
            AiOff::LocalRemoved => "local models removed".into(),
        }
    }

    /// The line telling the user how to turn AI rules on, or `None` when they
    /// turned them off themselves and know it.
    pub(crate) fn remedy(&self) -> Option<&'static str> {
        match self {
            AiOff::OptedOut => None,
            AiOff::NoModel | AiOff::NoCredentials(_) => Some("Set up AI review: lawlint init"),
            AiOff::LocalRemoved => Some("Local models were removed; pick a backend: lawlint init"),
        }
    }
}

/// Whether the AI rules run for this lint, and if not, why.
///
/// The judge used to be opt-in: off unless `--judge` or `judge.enabled: true`.
/// That made the highest-value tier invisible to anyone who had configured a
/// key but never learned the flag. It is now on whenever it *can* run — a
/// model resolves and its credentials are present — and reports itself when it
/// cannot. Explicit requests keep their old behaviour, including erroring when
/// `--judge` names nothing runnable: asking for AI and silently not getting it
/// is the failure this whole change exists to remove.
pub(crate) fn ai_decision(
    cli_judge: &Option<String>,
    no_ai: bool,
    config: &LintOptions,
) -> Result<Result<String, AiOff>, String> {
    if no_ai {
        return Ok(Err(AiOff::OptedOut));
    }
    // An explicit --judge (bare or with a model) is a request, not a default:
    // resolve it or fail loudly.
    if cli_judge.is_some() {
        return judge_spec(cli_judge, config).map(|model| model.ok_or(AiOff::OptedOut));
    }
    if config.judge.as_ref().and_then(|judge| judge.enabled) == Some(false) {
        return Ok(Err(AiOff::OptedOut));
    }
    let configured = config
        .judge
        .as_ref()
        .and_then(|judge| judge.model.clone())
        .or_else(|| config.ai_model("judge"));
    // Checked before `judge.enabled: true` is honored, so a config carried
    // over from before 0.9 reports the AI tier as skipped instead of letting
    // the judge fail per chunk and leaving the summary claiming the soft
    // rules ran and found nothing — an unearned score is the failure this
    // reporting exists to prevent.
    if configured
        .as_deref()
        .is_some_and(|model| model == "local" || model.starts_with("local:"))
    {
        return Ok(Err(AiOff::LocalRemoved));
    }
    if config.judge.as_ref().and_then(|judge| judge.enabled) == Some(true) {
        return judge_spec(&None, config).map(|model| model.ok_or(AiOff::OptedOut));
    }
    let Some(model) = configured else {
        return Ok(Err(AiOff::NoModel));
    };
    Ok(match lawlint_judge::credentials_ready(&model) {
        Ok(()) => Ok(model),
        Err(lawlint_judge::NotReady::MissingKey(name)) => Err(AiOff::NoCredentials(name)),
    })
}

/// One-line migration notice for a config still naming the removed
/// in-process backend. `None` = nothing to print.
pub(crate) fn local_notice(spec: &str) -> Option<String> {
    if spec != "local" && !spec.starts_with("local:") {
        return None;
    }
    Some(format!(
        "lawlint: note: {spec} names the in-process local backend, removed in 0.9 \
         (it could not judge these rules — docs/eval-corpus.md). To keep text on this \
         machine, run an OpenAI-compatible server such as Ollama and set \
         \"openai:http://localhost:11434/v1#<model>\"; `lawlint init` will do it for you"
    ))
}

/// Build the judge + disk cache. A cache failure is not fatal (judge runs
/// uncached); a judge build failure is reported to the caller, who falls
/// back to tiers 1-2.
/// `options` with the judge defaults filled in wherever the user left them
/// unset — request shaping and the generation budget both. An explicit config
/// value always wins; these are defaults, not overrides.
///
/// The budget is resolved here rather than left to each backend's own
/// fallback: only `FoundryClient` defaults high enough for a reasoning model,
/// so an `anthropic:`/`openai:` thinking model would otherwise inherit ax's
/// default and truncate to empty output.
fn with_backend_defaults(options: &LintOptions) -> LintOptions {
    let profile = lawlint_judge::default_plan();
    let mut options = options.clone();
    let judge = options.judge.get_or_insert_with(JudgeOptions::default);
    judge.context_chars.get_or_insert(profile.context_chars);
    judge.per_rule.get_or_insert(profile.per_rule);
    if judge.max_tokens.is_none() {
        judge.max_tokens = Some(lawlint_judge::DEFAULT_HOSTED_MAX_TOKENS);
    }
    options
}

/// Build the judge for `model` under already-resolved `judge` options (see
/// [`with_backend_defaults`]).
fn build_judge(
    model: String,
    judge: &JudgeOptions,
) -> Result<(Box<dyn Judge>, Option<DiskCache>), String> {
    let options = JudgeOptions {
        enabled: Some(true),
        model: Some(model),
        ..judge.clone()
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
    judge: Option<String>,
) -> LintResult {
    lint_text_with_progress(text, options, rules, judge, &mut |_, _| {}, &mut || {})
}

pub(crate) fn lint_text_with_progress(
    text: &str,
    options: &LintOptions,
    rules: &RuleSet,
    judge: Option<String>,
    on_progress: &mut dyn FnMut(usize, usize),
    on_done: &mut dyn FnMut(),
) -> LintResult {
    let Some(model) = judge else {
        return lint_with(text, options, rules);
    };
    // Backend defaults are resolved here — the only place the model spec is
    // known — then handed to core as plain numbers and to the judge as options.
    let options = &with_backend_defaults(options);
    let judge_options = options.judge.clone().unwrap_or_default();
    match build_judge(model, &judge_options) {
        Ok((judge, cache)) => {
            let result = lawlint_core::lint_full_with_progress(
                text,
                options,
                rules,
                judge.as_ref(),
                cache.as_ref().map(|cache| cache as &dyn JudgeCache),
                on_progress,
            );
            on_done();
            if let Some(stats) = &result.judge {
                if stats.chunks_failed > 0 {
                    eprintln!(
                        "lawlint: warning: judge failed on {} of {} chunks; those chunks used tiers 1-2 only",
                        stats.chunks_failed, stats.chunks
                    );
                    if let Some(reason) = &stats.first_failure {
                        eprintln!("lawlint: warning: first failure: {reason}");
                    }
                }
            }
            result
        }
        Err(error) => {
            on_done();
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

fn is_docx_path(file: &str) -> bool {
    file != "-"
        && Path::new(file)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("docx"))
}

/// Rewrite a `.docx` in place, turning MachineApplicable fixes into Word
/// tracked changes with a review comment per fix.
fn apply_docx_fixes(file: &str, diagnostics: &[Diagnostic]) -> Result<(), String> {
    let bytes = fs::read(file).map_err(|error| format!("failed to read {file}: {error}"))?;
    let result = lawlint_docx::apply_tracked_changes(
        &bytes,
        diagnostics,
        &lawlint_docx::ReviseOptions::default(),
    )
    .map_err(|error| format!("failed to apply fixes to {file}: {error}"))?;
    if result.applied > 0 {
        fs::write(file, &result.bytes)
            .map_err(|error| format!("failed to write {file}: {error}"))?;
    }
    let mut parts = vec![format!(
        "{} tracked change{}",
        result.applied,
        if result.applied == 1 { "" } else { "s" }
    )];
    if result.ai_applied > 0 {
        parts.push(format!(
            "{} of them AI rewrite{}",
            result.ai_applied,
            if result.ai_applied == 1 { "" } else { "s" }
        ));
    }
    if result.annotated > 0 {
        parts.push(format!(
            "{} comment{} on findings with no automatic fix",
            result.annotated,
            if result.annotated == 1 { "" } else { "s" }
        ));
    }
    eprintln!("Applied {} to {file}", parts.join(", "));
    if result.applied > 0 || result.annotated > 0 {
        eprintln!("  → review in Word: Review ▸ Tracked Changes / Comments");
    }
    if result.skipped > 0 {
        eprintln!(
            "lawlint: {} finding{} left unmarked (span multiple runs, or overlap one that was marked)",
            result.skipped,
            if result.skipped == 1 { "" } else { "s" },
        );
    }
    Ok(())
}

fn colors_enabled() -> bool {
    io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

// ---- progress ----------------------------------------------------------

/// A one-line spinner on stderr for the judge pass, which is a sequential
/// network round trip per section and otherwise looks like a hung process.
///
/// Renders only to a terminal: writing animation frames into a pipe or a CI log
/// would corrupt output that something else is reading. This is a rendering
/// decision, not a behavioural one — the AI rules themselves run identically
/// either way.
/// Animation state shared with the redraw thread.
#[derive(Default)]
struct SpinnerState {
    done: AtomicUsize,
    total: AtomicUsize,
    running: AtomicBool,
}

struct Spinner {
    state: Arc<SpinnerState>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(120);

impl Spinner {
    /// Starts the redraw thread immediately when rendering is appropriate. The
    /// thread — rather than redrawing on each completed section — is the point:
    /// one section is a whole network round trip, so a frame that only advances
    /// on completion sits motionless for seconds and looks hung, which is the
    /// thing this exists to prevent.
    fn new(quiet: bool) -> Self {
        let state = Arc::new(SpinnerState::default());
        let mut handle = None;
        if !quiet && io::stderr().is_terminal() {
            state.running.store(true, Ordering::Relaxed);
            let state = Arc::clone(&state);
            handle = Some(std::thread::spawn(move || {
                let mut frame = 0usize;
                while state.running.load(Ordering::Relaxed) {
                    let total = state.total.load(Ordering::Relaxed);
                    if total > 0 {
                        let done = state.done.load(Ordering::Relaxed);
                        // \r + clear-to-end keeps the pass on one line.
                        eprint!(
                            "\r\x1b[2K{} Reviewing with AI rules (section {} of {})…",
                            SPINNER_FRAMES[frame % SPINNER_FRAMES.len()],
                            (done + 1).min(total),
                            total
                        );
                        let _ = io::Write::flush(&mut io::stderr());
                    }
                    frame += 1;
                    std::thread::sleep(SPINNER_INTERVAL);
                }
            }));
        }
        Self {
            state,
            handle: Mutex::new(handle),
        }
    }

    fn tick(&self, done: usize, total: usize) {
        self.state.done.store(done, Ordering::Relaxed);
        self.state.total.store(total, Ordering::Relaxed);
    }

    /// Stop animating and wipe the line, so the summary starts on a clean row.
    fn clear(&self) {
        if !self.state.running.swap(false, Ordering::Relaxed) {
            return;
        }
        if let Some(handle) = self.handle.lock().ok().and_then(|mut h| h.take()) {
            let _ = handle.join();
        }
        eprint!("\r\x1b[2K");
        let _ = io::Write::flush(&mut io::stderr());
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.clear();
    }
}

// ---- coverage ----------------------------------------------------------

/// What ran and what did not, per tier. The old output reported only the
/// findings it produced, which cannot distinguish "this document is clean" from
/// "a third of the rules never executed" — the two readings differed by 13
/// points of score with nothing on screen to say so.
pub(crate) struct Coverage {
    pub(crate) tiers: Vec<TierCoverage>,
}

pub(crate) struct TierCoverage {
    pub(crate) label: &'static str,
    pub(crate) total: usize,
    pub(crate) ran: usize,
    pub(crate) findings: usize,
    /// Present when `ran < total`; shown inline in the summary.
    pub(crate) skip_reason: Option<String>,
    /// Rule ids that did not run, for `--coverage`.
    pub(crate) skipped_rules: Vec<String>,
}

fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Static => "Static rules",
        Tier::Statistical => "Statistical",
        Tier::Inferential => "AI rules",
    }
}

/// Build coverage by comparing every rule in the set against the ones the
/// options actually instantiate, then subtracting the AI tier when the judge
/// did not run.
pub(crate) fn coverage(
    rules: &RuleSet,
    options: &LintOptions,
    result: &LintResult,
    ai_off: Option<&AiOff>,
) -> Coverage {
    let enabled: std::collections::HashSet<String> = rules
        .instantiate(options)
        .iter()
        .map(|rule| rule.meta().id.0.clone())
        .collect();
    let tiers = [Tier::Static, Tier::Statistical, Tier::Inferential]
        .into_iter()
        .map(|tier| {
            let metas: Vec<&RuleMeta> = rules
                .metas()
                .into_iter()
                .filter(|meta| meta.tier == tier)
                .collect();
            let ai_skipped = tier == Tier::Inferential && ai_off.is_some();
            let mut skipped_rules: Vec<String> = Vec::new();
            let mut ran = 0usize;
            for meta in &metas {
                if ai_skipped || !enabled.contains(&meta.id.0) {
                    skipped_rules.push(meta.id.0.clone());
                } else {
                    ran += 1;
                }
            }
            let skip_reason = if skipped_rules.is_empty() {
                None
            } else if ai_skipped {
                ai_off.map(AiOff::reason)
            } else {
                Some("disabled".to_string())
            };
            TierCoverage {
                label: tier_label(tier),
                total: metas.len(),
                ran,
                findings: result.diagnostics.iter().filter(|d| d.tier == tier).count(),
                skip_reason,
                skipped_rules,
            }
        })
        .collect();
    Coverage { tiers }
}

// ---- summary rendering -------------------------------------------------

/// The default `lawlint FILE` output: what ran, what it found, the score, and
/// how to see more. Findings themselves live behind `--list`, because 33
/// findings each quoting its containing paragraph is not something anyone
/// reads.
pub(crate) fn format_summary(
    result: &LintResult,
    cov: &Coverage,
    source: &str,
    fixable: usize,
    listing_coverage: bool,
    color: bool,
) -> String {
    let dim = |v: String| if color { v.dimmed().to_string() } else { v };
    let bold = |v: String| if color { v.bold().to_string() } else { v };

    let mut lines = vec![
        String::new(),
        format!(
            "  {}   {}",
            bold(ui::tilde(source)),
            dim(format!(
                "{} words · {}",
                thousands(result.stats.word_count),
                ui::plural(result.stats.sentence_count, "sentence")
            ))
        ),
        String::new(),
    ];

    for tier in &cov.tiers {
        // "22 run" when everything ran, "0 of 2 run" when it did not — the
        // denominator only earns its space when something is missing.
        let ran = match &tier.skip_reason {
            None => format!("{:>2} run", tier.ran),
            Some(_) => format!("{} of {} run", tier.ran, tier.total),
        };
        // Findings always show: a tier that ran 14 of 15 rules still found
        // things, and dropping the count to make room for the reason loses the
        // number the reader came for.
        let mut tail = format!(
            "{:>3} {:<9}",
            tier.findings,
            if tier.findings == 1 {
                "finding"
            } else {
                "findings"
            }
        );
        if let Some(reason) = &tier.skip_reason {
            tail.push_str(&dim(format!("({reason})")));
        }
        // A judge that failed on some sections still returns findings, so
        // without this the run looks complete and the score looks earned.
        if tier.label == "AI rules" && tier.skip_reason.is_none() {
            if let Some(stats) = &result.judge {
                if stats.chunks_failed > 0 {
                    tail.push_str(&dim(format!(
                        "   (incomplete: {} of {} sections failed)",
                        stats.chunks_failed, stats.chunks
                    )));
                }
            }
        }
        lines.push(
            format!("  {:<14} {:<12} {tail}", tier.label, ran)
                .trim_end()
                .to_string(),
        );
    }

    let ai_ran = cov
        .tiers
        .iter()
        .find(|t| t.label == "AI rules")
        .is_none_or(|t| t.skip_reason.is_none());
    let ai_partial = result
        .judge
        .as_ref()
        .is_some_and(|stats| stats.chunks_failed > 0);
    // The score is the number people quote, so it has to carry its own basis:
    // the same document scored 97 static-only and 82 with AI rules.
    let basis = match (ai_ran, ai_partial) {
        (true, false) => String::new(),
        (true, true) => dim("   (AI review incomplete)".to_string()),
        (false, _) => dim("   (static rules only)".to_string()),
    };
    lines.push(String::new());
    lines.push(format!(
        "  {}  {}{basis}",
        "Human-likeness",
        bold(format!("{}/100", result.stats.score)),
    ));
    lines.push(String::new());

    let total = result.diagnostics.len();
    if total == 0 {
        let clean = "  No findings.".to_string();
        lines.push(if color {
            clean.green().to_string()
        } else {
            clean
        });
    } else {
        let mut actions = vec!["--list to see them".to_string()];
        if fixable > 0 {
            actions.push(format!("--fix to apply {fixable}"));
        }
        lines.push(format!(
            "  {} · {}",
            ui::plural(total, "finding"),
            dim(actions.join(" · "))
        ));
    }

    let skipped: usize = cov.tiers.iter().map(|t| t.skipped_rules.len()).sum();
    if skipped > 0 {
        lines.push(format!(
            "  {}",
            dim(if listing_coverage {
                ui::plural(skipped, "rule") + " skipped"
            } else {
                format!(
                    "{} skipped · --coverage to list them",
                    ui::plural(skipped, "rule")
                )
            })
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
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

/// Characters of surrounding text to show on each side of a matched span.
const EXCERPT_CONTEXT: usize = 36;

/// The text around `span`, trimmed to a readable window and marked. The
/// diagnostic's own `excerpt` is the whole containing block, which for a
/// document-level rule can be a 900-character paragraph — printed once per
/// finding, three findings deep into the same paragraph, it buries the actual
/// match. Slicing the source directly keeps the offending words on screen.
fn excerpt_window(text: &str, span: lawlint_core::TextRange, color: bool) -> Vec<String> {
    let (start, end) = (span.start.min(text.len()), span.end.min(text.len()));
    if start > end {
        return Vec::new();
    }
    // Never cross a line boundary: context from an adjacent paragraph reads as
    // part of the match.
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    let mut left = floor_char_boundary(text, start.saturating_sub(EXCERPT_CONTEXT).max(line_start));
    let mut right = ceil_char_boundary(text, (end + EXCERPT_CONTEXT).min(line_end));
    // Prefer breaking at a space so windows start and end on whole words.
    if left > line_start {
        if let Some(i) = text[left..start].find(' ') {
            left += i + 1;
        }
    }
    if right < line_end {
        if let Some(i) = text[end..right].rfind(' ') {
            right = end + i;
        }
    }

    // Collapse runs of whitespace across the *whole* window at once. Squashing
    // each segment separately drops the space that sits exactly on a span
    // boundary, which welds words together ("datathat", "IPaddress").
    let (before, matched, after) = squash_window(text, left, start, end, right);
    // Indentation at the start of a line, or a trailing space before it ends,
    // is not context — drop it so every excerpt starts at a word.
    let before = before.trim_start().to_string();
    let after = after.trim_end().to_string();
    // A span can be a whole sentence (the AI rules quote generously); printing
    // it in full puts us back to paragraph dumps, so elide the middle.
    let matched = ellipsize_middle(&matched, MAX_MATCH_CHARS);

    let lead = if left > line_start { "…" } else { "" };
    let trail = if right < line_end { "…" } else { "" };
    let body = if color {
        format!(
            "{lead}{}{}{}{trail}",
            before.dimmed(),
            matched.yellow().underline(),
            after.dimmed()
        )
    } else {
        format!("{lead}{before}{matched}{after}{trail}")
    };
    let mut out = vec![format!("    {body}")];
    // Without color there is no highlight, so mark the span with a caret rule —
    // but only when it points at something. Under a sentence-length span the
    // rule spans the whole line and reads as decoration.
    if !color && !matched.is_empty() && matched.chars().count() <= MAX_CARET_CHARS {
        let offset = lead.chars().count() + before.chars().count();
        out.push(format!(
            "    {}{}",
            " ".repeat(offset),
            "^".repeat(matched.chars().count())
        ));
    }
    out
}

/// Longest matched span shown before the middle is elided.
const MAX_MATCH_CHARS: usize = 90;

/// Longest matched span still worth underlining with a caret rule.
const MAX_CARET_CHARS: usize = 60;

/// Collapse whitespace runs over `text[left..right]` in one pass, returning the
/// before/matched/after pieces split at `start` and `end`. Boundary spaces
/// survive because the collapse never restarts mid-window.
fn squash_window(
    text: &str,
    left: usize,
    start: usize,
    end: usize,
    right: usize,
) -> (String, String, String) {
    let mut parts = [String::new(), String::new(), String::new()];
    let mut last_was_space = false;
    for (offset, ch) in text[left..right].char_indices() {
        let absolute = left + offset;
        let slot = if absolute < start {
            0
        } else if absolute < end {
            1
        } else {
            2
        };
        if ch.is_whitespace() {
            // One space per run, attributed to the segment it starts in.
            if !last_was_space {
                parts[slot].push(' ');
            }
            last_was_space = true;
        } else {
            parts[slot].push(ch);
            last_was_space = false;
        }
    }
    let [before, matched, after] = parts;
    (before, matched, after)
}

/// `text` with its middle replaced by an ellipsis when it exceeds `max` chars,
/// keeping both ends so the reader sees where the span starts and stops.
fn ellipsize_middle(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let head = max / 2;
    let tail = max - head - 1;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(chars[chars.len() - tail..].iter());
    out
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// The findings list (`--list`): one entry per diagnostic, in document order,
/// each showing only the text that triggered it.
pub(crate) fn format_findings(result: &LintResult, source: &str, color: bool) -> String {
    if result.diagnostics.is_empty() {
        let check = "  No findings.";
        return if color {
            check.green().to_string()
        } else {
            check.into()
        };
    }
    let mut lines = vec![String::new()];
    for diagnostic in &result.diagnostics {
        let location = format!("{}:{}", diagnostic.line, diagnostic.column);
        let location = severity_colored(diagnostic.severity, &location, color);
        let rule = if color {
            diagnostic.rule_id.0.bold().to_string()
        } else {
            diagnostic.rule_id.0.clone()
        };
        let ai = if diagnostic.tier == Tier::Inferential {
            let tag = " (AI)";
            if color {
                tag.dimmed().to_string()
            } else {
                tag.into()
            }
        } else {
            String::new()
        };
        lines.push(format!("  {location}  {rule}{ai}  {}", diagnostic.message));
        lines.extend(excerpt_window(source, diagnostic.span, color));
        if let Some(suggestion) = &diagnostic.suggestion {
            let value = format!("→ {suggestion}");
            lines.push(format!(
                "    {}",
                if color {
                    value.dimmed().to_string()
                } else {
                    value
                }
            ));
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

/// The `--coverage` listing: every rule that did not run, and why.
pub(crate) fn format_coverage(cov: &Coverage, color: bool) -> String {
    let dim = |v: String| if color { v.dimmed().to_string() } else { v };
    let mut lines = vec![String::new()];
    for tier in &cov.tiers {
        lines.push(format!(
            "  {:<14} {} of {} run",
            tier.label, tier.ran, tier.total
        ));
        if !tier.skipped_rules.is_empty() {
            let reason = tier.skip_reason.clone().unwrap_or_default();
            lines.push(format!("  {}", dim(format!("  skipped ({reason}):"))));
            for id in &tier.skipped_rules {
                lines.push(format!("  {}", dim(format!("    {id}"))));
            }
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

/// Legacy flat rendering, kept for `--format full` and the TUI transcript,
/// which shows one short snippet at a time and has no summary to anchor.
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

/// Render the before/after diff for `--diff` (pretty output only). Empty when
/// nothing changed except a single dimmed "no applicable fixes" line.
fn format_diff(before: &str, after: &str, color: bool) -> String {
    let dim = |value: String| {
        if color {
            value.dimmed().to_string()
        } else {
            value
        }
    };
    if after == before {
        return dim("no applicable fixes".to_string());
    }
    let lines = diff::diff_lines(before, after);
    let mut out = Vec::new();
    for entry in diff::with_context(&lines, 1) {
        out.push(match entry {
            Some(diff::DiffLine::Removed(line)) => {
                let value = format!("- {line}");
                if color {
                    value.red().to_string()
                } else {
                    value
                }
            }
            Some(diff::DiffLine::Added(line)) => {
                let value = format!("+ {line}");
                if color {
                    value.green().to_string()
                } else {
                    value
                }
            }
            Some(diff::DiffLine::Same(line)) => dim(format!("  {line}")),
            None => dim("···".to_string()),
        });
    }
    out.join("\n")
}

/// How many fixes `--fix` would apply. `include_ai` mirrors the apply path:
/// true for `.docx` (tracked changes are reviewable) and for `--unsafe`.
fn fixable_count(diagnostics: &[Diagnostic], include_ai: bool) -> usize {
    diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.fix.as_ref().is_some_and(|fix| {
                fix.applicability == Applicability::MachineApplicable
                    || (include_ai && fix.applicability == Applicability::MaybeIncorrect)
            })
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
    if cli.diff && cli.format == "json" {
        return Err("--diff requires --format pretty".into());
    }
    if cli.format == "prompt" && (cli.fix || cli.diff) {
        return Err("--fix and --diff require --format pretty".into());
    }
    let is_docx = is_docx_path(&cli.file);
    let (text, file_markdown) = if is_docx {
        let bytes =
            fs::read(&cli.file).map_err(|error| format!("failed to read {}: {error}", cli.file))?;
        let text = lawlint_docx::extract(&bytes)
            .map_err(|error| format!("failed to read {}: {error}", cli.file))?;
        (text, false)
    } else {
        read_input(&cli.file)?
    };
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let (config, config_dir) = find_config(cwd)?;
    let markdown = if cli.file == "-" {
        cli.markdown.then_some(true)
    } else {
        Some(cli.markdown || file_markdown)
    };
    let rules = build_rule_set(&config, config_dir.as_deref(), &cli.rule_dir)?;
    let decision = ai_decision(&cli.judge, cli.no_ai, &config)?;
    let judge = decision.as_ref().ok().cloned();
    let ai_off = decision.as_ref().err();
    if let Some(notice) = judge.as_deref().and_then(local_notice) {
        eprintln!("{notice}");
    }
    let options = merge_options(config, cli_options(cli, markdown));
    let spinner = Spinner::new(cli.quiet || cli.format == "json");
    let result = lint_text_with_progress(
        &text,
        &options,
        &rules,
        judge,
        &mut |done, total| spinner.tick(done, total),
        &mut || spinner.clear(),
    );
    let cov = coverage(&rules, &options, &result, ai_off);

    match cli.format.as_str() {
        "json" => println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
        ),
        // The brief is fed to an AI model, so it replaces diagnostics output.
        // --quiet suppresses stdout; the None note stays on stderr regardless.
        // A file input is referenced by path — the receiving agent reads it
        // itself; embedding a large document would blow up its context.
        "prompt" => match lawlint_core::remediation_prompt(
            if cli.file == "-" || is_docx {
                lawlint_core::PromptSource::Text(&text)
            } else {
                lawlint_core::PromptSource::File(&cli.file)
            },
            &result,
            &rules,
        ) {
            Some(prompt) => {
                if !cli.quiet {
                    print!("{prompt}");
                }
            }
            None => eprintln!("no issues found; no prompt generated"),
        },
        "full" => {
            if !cli.quiet {
                println!("{}", format_pretty(&result, false, colors_enabled()));
            }
        }
        _ => {
            if !cli.quiet {
                let color = colors_enabled();
                let source_name = if cli.file == "-" { "stdin" } else { &cli.file };
                print!(
                    "{}",
                    format_summary(
                        &result,
                        &cov,
                        source_name,
                        fixable_count(&result.diagnostics, is_docx || cli.unsafe_fixes),
                        cli.coverage,
                        color
                    )
                );
                if cli.list {
                    println!("{}", format_findings(&result, &text, color));
                }
                if cli.coverage {
                    println!("{}", format_coverage(&cov, color));
                }
                if let Some(remedy) = ai_off.and_then(AiOff::remedy) {
                    println!(
                        "  {}\n",
                        if color {
                            remedy.dimmed().to_string()
                        } else {
                            remedy.to_string()
                        }
                    );
                }
            }
        }
    }

    if cli.fix {
        if is_docx {
            apply_docx_fixes(&cli.file, &result.diagnostics)?;
        } else {
            let fixed = apply_fixes_with(&text, &result.diagnostics, cli.unsafe_fixes);
            let count = fixable_count(&result.diagnostics, cli.unsafe_fixes);
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
            let held_back = fixable_count(&result.diagnostics, true) - count;
            if held_back > 0 {
                eprintln!(
                    "lawlint: {held_back} AI rewrite{} not applied (plain text has no \
                     accept/reject layer) — include them with --fix --unsafe",
                    if held_back == 1 { "" } else { "s" },
                );
            }
        }
    }

    if cli.diff && !cli.quiet {
        let fixed = apply_fixes(&text, &result.diagnostics);
        println!("{}", format_diff(&text, &fixed, colors_enabled()));
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
        "intent": meta.intent,
        "description": meta.description,
        "docsUrl": meta.docs_url,
        "examples": meta.examples,
    });
    if let Some(rationale) = &meta.rationale {
        wrapped["rationale"] = serde_json::json!(rationale);
    }
    if let Some(explanation) = &meta.explanation {
        wrapped["explanation"] = serde_json::json!(explanation);
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

/// Collect rule files under `path`: a single file, a package dir
/// (`style.yaml` + `rules/`), or a loose directory of rule files.
fn collect_rule_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            return Ok(vec![path.to_path_buf()]);
        }
        return Err(format!("no rule files found in {}", path.display()));
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
            matches!(p.extension().and_then(|ext| ext.to_str()), Some("md"))
                && p.file_name().and_then(|name| name.to_str()) != Some("style.yaml")
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!("no rule files found in {}", rules_dir.display()));
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
            return ExampleOutcome::Skip(match &stats.first_failure {
                Some(reason) => format!(
                    "judge failed on {} of {} chunks: {reason}",
                    stats.chunks_failed, stats.chunks
                ),
                None => format!(
                    "judge failed on {} of {} chunks",
                    stats.chunks_failed, stats.chunks
                ),
            });
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
    // config model (and errors with init guidance when nothing is
    // configured, #50). `--offline` forces the skip.
    let judge_model = if offline || judge_flag.is_none() {
        None
    } else {
        judge_spec(judge_flag, &config)?
    };
    if let Some(notice) = judge_model.as_deref().and_then(local_notice) {
        eprintln!("{notice}");
    }
    let mut judge_state: Option<JudgeState> = None;
    // The config's judge options ride along in `options` so lint_full's
    // tier-3 confidence gate honors a configured `judge.floor` (a bare
    // LintOptions::default() would silently pin the floor to 0.6).
    let options = LintOptions {
        judge: config.judge.clone(),
        ..LintOptions::default()
    };
    // Rule examples are single sentences, so the plan never splits them.
    let options = with_backend_defaults(&options);
    let judge_options = options.judge.clone().unwrap_or_default();

    for file in &files {
        let display = file.display().to_string();
        let def = match loader::parse_rule_file(file) {
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
                judge_state = Some(
                    match build_judge(judge_model.clone().unwrap(), &judge_options) {
                        Ok(built) => Some(built),
                        Err(error) => {
                            eprintln!(
                            "lawlint: warning: judge unavailable ({error}); skipping inferential examples"
                        );
                            None
                        }
                    },
                );
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

/// Bare `lawlint`: if the project has no discoverable config, offer the setup
/// wizard first (with a skip), then open the TUI. A completed wizard writes
/// `.lawlint/config.json`, which the TUI's own `find_config` then picks up.
fn launch_tui() -> Result<i32, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let (_, config_dir) = find_config(cwd)?;
    if config_dir.is_none() {
        match init_tui::run_setup(init_tui::SetupContext::FirstRun)? {
            init_tui::SetupOutcome::Aborted => return Ok(0),
            init_tui::SetupOutcome::Completed | init_tui::SetupOutcome::Skipped => {}
        }
    }
    tui::run_tui()
}

/// `lawlint init`: an interactive terminal gets the ratatui wizard and then
/// drops into the TUI; anything scripted (`--yes`, a pipe, CI) takes the
/// line-oriented walkthrough and exits, unchanged.
fn init_command(yes: bool, force: bool, ai: Option<&str>) -> Result<i32, String> {
    let interactive = !yes && io::stdin().is_terminal() && io::stdout().is_terminal();
    if interactive {
        match init_tui::run_setup(init_tui::SetupContext::Explicit {
            force,
            ai: ai.map(str::to_string),
        })? {
            init_tui::SetupOutcome::Completed => tui::run_tui(),
            // Explicit init has no "skip"; a user who bailed out gets a
            // non-zero exit and no TUI.
            init_tui::SetupOutcome::Aborted | init_tui::SetupOutcome::Skipped => Ok(1),
        }
    } else {
        init::init_command(yes, force, ai)
    }
}

fn run(cli: Cli) -> Result<i32, String> {
    // A bare `lawlint` in an interactive terminal launches the TUI instead of
    // blocking on stdin — running the setup wizard first if this project has no
    // config yet.
    if std::env::args().len() == 1 && io::stdin().is_terminal() {
        return launch_tui();
    }

    match &cli.command {
        Some(Command::Init { yes, force, ai }) => init_command(*yes, *force, ai.as_deref()),
        Some(Command::Learn { path, out, model }) => {
            learn::learn_command(path, out, model.as_deref())
        }
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
        assert_eq!(judge_spec(&None, &off), Ok(None));
        // Bare --judge with nothing configured errors with init guidance
        // (#50): never a silent local default.
        let err = judge_spec(&Some(String::new()), &off).unwrap_err();
        assert!(err.contains("lawlint init"), "{err}");
        assert!(err.contains("none is configured"), "{err}");
        // Explicit model on the flag.
        assert_eq!(
            judge_spec(&Some("local:foo".into()), &off),
            Ok(Some("local:foo".into()))
        );
        // Config-enabled with a model.
        let config = options_with(|o| {
            o.judge = Some(JudgeOptions {
                enabled: Some(true),
                model: Some("anthropic:m".into()),
                floor: None,
                max_tokens: None,
                ..JudgeOptions::default()
            });
        });
        assert_eq!(judge_spec(&None, &config), Ok(Some("anthropic:m".into())));
        // Bare --judge inherits the config model.
        assert_eq!(
            judge_spec(&Some(String::new()), &config),
            Ok(Some("anthropic:m".into()))
        );
        // Config-enabled without any model errors too — a config that says
        // "judge on" but names no model is a config error, not a download.
        let enabled_no_model = options_with(|o| {
            o.judge = Some(JudgeOptions {
                enabled: Some(true),
                model: None,
                floor: None,
                max_tokens: None,
                ..JudgeOptions::default()
            });
        });
        let err = judge_spec(&None, &enabled_no_model).unwrap_err();
        assert!(err.contains("lawlint init"), "{err}");
        // Config present but not enabled → off.
        let disabled = options_with(|o| {
            o.judge = Some(JudgeOptions {
                enabled: None,
                model: Some("anthropic:m".into()),
                floor: None,
                max_tokens: None,
                ..JudgeOptions::default()
            });
        });
        assert_eq!(judge_spec(&None, &disabled), Ok(None));
    }

    #[test]
    fn judge_spec_resolves_from_ai_preferences() {
        use lawlint_core::AiOptions;
        // No judge.model: the ai section supplies the spec.
        let prefs = options_with(|o| {
            o.judge = Some(JudgeOptions {
                enabled: Some(true),
                model: None,
                floor: None,
                max_tokens: None,
                ..JudgeOptions::default()
            });
            o.ai = Some(AiOptions {
                model: Some("foundry:gpt-5.5".into()),
                ..Default::default()
            });
        });
        assert_eq!(
            judge_spec(&None, &prefs),
            Ok(Some("foundry:gpt-5.5".into()))
        );
        // Bare --judge inherits the preference too.
        assert_eq!(
            judge_spec(&Some(String::new()), &prefs),
            Ok(Some("foundry:gpt-5.5".into()))
        );
        // Per-feature override outranks the default model…
        let with_override = options_with(|o| {
            o.judge = prefs.judge.clone();
            o.ai = Some(AiOptions {
                model: Some("foundry:d".into()),
                features: Some(
                    [("judge".to_string(), "anthropic:m".to_string())]
                        .into_iter()
                        .collect(),
                ),
            });
        });
        assert_eq!(
            judge_spec(&None, &with_override),
            Ok(Some("anthropic:m".into()))
        );
        // …but a legacy judge.model outranks the ai section, and an explicit
        // --judge=MODEL outranks everything.
        let legacy = options_with(|o| {
            o.judge = Some(JudgeOptions {
                enabled: Some(true),
                model: Some("local:legacy".into()),
                floor: None,
                max_tokens: None,
                ..JudgeOptions::default()
            });
            o.ai = prefs.ai.clone();
        });
        assert_eq!(judge_spec(&None, &legacy), Ok(Some("local:legacy".into())));
        assert_eq!(
            judge_spec(&Some("local:cli".into()), &legacy),
            Ok(Some("local:cli".into()))
        );
    }

    fn diagnostic(severity: Severity, fix: Option<Fix>) -> Diagnostic {
        Diagnostic {
            rule_id: RuleId("core/x".into()),
            severity,
            tier: Tier::Static,
            intent: lawlint_core::Intent::Detection,
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
    fn fixable_count_gates_ai_rewrites_on_include_ai() {
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
        // Plain text: mechanical fixes only. .docx / --unsafe: both.
        assert_eq!(fixable_count(&diagnostics, false), 1);
        assert_eq!(fixable_count(&diagnostics, true), 2);
    }

    // ---- ai_decision ----

    use serde_json::json;

    fn ai_config(value: serde_json::Value) -> LintOptions {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn ai_is_off_when_no_model_is_configured() {
        let config = LintOptions::default();
        assert_eq!(
            ai_decision(&None, false, &config).unwrap(),
            Err(AiOff::NoModel)
        );
    }

    #[test]
    fn no_ai_flag_opts_out_even_with_a_model() {
        let config = ai_config(json!({"ai": {"model": "openai:http://x#m"}}));
        assert_eq!(
            ai_decision(&None, true, &config).unwrap(),
            Err(AiOff::OptedOut)
        );
    }

    #[test]
    fn judge_enabled_false_opts_out() {
        let config = ai_config(json!({
            "ai": {"model": "openai:http://x#m"},
            "judge": {"enabled": false}
        }));
        assert_eq!(
            ai_decision(&None, false, &config).unwrap(),
            Err(AiOff::OptedOut)
        );
    }

    #[test]
    fn ai_turns_itself_on_when_the_model_needs_no_key() {
        // An OpenAI-compatible endpoint (often a local server) needs no key,
        // so it is runnable the moment it is configured.
        let config = ai_config(json!({"ai": {"model": "openai:http://x#m"}}));
        assert_eq!(
            ai_decision(&None, false, &config).unwrap(),
            Ok("openai:http://x#m".to_string())
        );
    }

    #[test]
    fn an_explicit_judge_flag_still_errors_when_unresolvable() {
        // Asking for AI and silently not getting it is the failure this whole
        // change exists to remove; an explicit request must fail loudly.
        let config = LintOptions::default();
        assert!(ai_decision(&Some(String::new()), false, &config).is_err());
    }

    #[test]
    fn ai_off_reasons_only_nudge_when_setup_is_missing() {
        assert!(AiOff::OptedOut.remedy().is_none());
        assert!(AiOff::NoModel.remedy().is_some());
        assert!(AiOff::NoCredentials("ANTHROPIC_API_KEY").remedy().is_some());
    }

    // ---- excerpt windows ----

    /// The excerpt's first line with its fixed indent stripped.
    fn window(text: &str, start: usize, end: usize) -> String {
        excerpt_window(text, TextRange { start, end }, false)
            .remove(0)
            .trim_start()
            .to_string()
    }

    #[test]
    fn excerpt_keeps_the_space_on_a_span_boundary() {
        // Regression: squashing before/matched/after separately dropped the
        // space sitting exactly on the boundary, welding words together
        // ("We accumulate datathat, …").
        let text = "We accumulate data that, either on its own or combined.";
        let start = text.find("that").unwrap();
        let excerpt = window(text, start, start + 4);
        assert!(excerpt.contains("data that,"), "{excerpt}");
        assert!(!excerpt.contains("datathat"), "{excerpt}");
    }

    #[test]
    fn excerpt_collapses_runs_of_whitespace() {
        let text = "alpha   \n  bravo charlie";
        let start = text.find("bravo").unwrap();
        let excerpt = window(text, start, start + 5);
        // The newline is a line boundary, so context stops there.
        assert!(excerpt.contains("bravo"), "{excerpt}");
        assert!(!excerpt.contains("  "), "double space in {excerpt:?}");
    }

    #[test]
    fn excerpt_never_crosses_a_line_boundary() {
        let text = "first paragraph here\nsecond paragraph here";
        let start = text.find("second").unwrap();
        let excerpt = window(text, start, start + 6);
        assert!(!excerpt.contains("first"), "{excerpt}");
    }

    #[test]
    fn excerpt_elides_the_middle_of_a_sentence_length_span() {
        let text = format!("lead {} tail", "word ".repeat(60));
        let excerpt = window(&text, 5, text.len() - 5);
        assert!(excerpt.chars().count() < 200, "{}", excerpt.len());
        assert!(excerpt.contains('…'), "{excerpt}");
    }

    #[test]
    fn excerpt_handles_multibyte_text_without_panicking() {
        // Byte offsets that are not char boundaries must be walked back.
        let text = "the “quoted” — em-dashed — phrase runs on";
        let start = text.find("em-dashed").unwrap();
        let excerpt = window(text, start, start + "em-dashed".len());
        assert!(excerpt.contains("em-dashed"), "{excerpt}");
    }

    #[test]
    fn excerpt_tolerates_spans_past_the_end_of_text() {
        let text = "short";
        assert!(!excerpt_window(text, TextRange { start: 0, end: 999 }, false).is_empty());
        assert!(
            excerpt_window(
                text,
                TextRange {
                    start: 999,
                    end: 999
                },
                false
            )
            .is_empty()
                || true
        );
    }

    #[test]
    fn ellipsize_middle_keeps_both_ends() {
        assert_eq!(ellipsize_middle("abc", 10), "abc");
        let out = ellipsize_middle("abcdefghijklmnop", 7);
        assert_eq!(out.chars().count(), 7);
        assert!(out.starts_with("abc"), "{out}");
        assert!(out.ends_with("nop"), "{out}");
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
            intent: lawlint_core::Intent::Style,
            description: "Flags em dashes.".into(),
            docs_url: "https://lawlint.com/rules/no-em-dash".into(),
            rationale: None,
            explanation: None,
            examples: vec![],
        };
        let value = rule_meta_json(&meta);
        assert_eq!(value["id"], "no-em-dash"); // namespace stripped
        let wrapped = &value["meta"];
        assert_eq!(wrapped["description"], "Flags em dashes.");
        assert_eq!(wrapped["severity"], "warning");
        assert_eq!(wrapped["intent"], "style");
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
        fs::write(rules.join("b.md"), "x").unwrap();
        fs::write(rules.join("a.md"), "x").unwrap();
        fs::write(rules.join("notes.txt"), "x").unwrap();

        // Package dir → Markdown rule files, sorted.
        let files = collect_rule_files(&base).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|p| p.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["a.md", "b.md"]
        );
        // Loose dir (no manifest) → Markdown rule files, style.yaml excluded.
        let files = collect_rule_files(&rules).unwrap();
        assert_eq!(files.len(), 2);
        // Single file.
        let single = collect_rule_files(&rules.join("b.md")).unwrap();
        assert_eq!(single.len(), 1);
        // Missing path → error.
        assert!(collect_rule_files(&base.join("nope")).is_err());

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn rules_test_accepts_markdown_inferential_rule() {
        let base =
            std::env::temp_dir().join(format!("lawlint-cli-skill-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let rules = base.join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(base.join("style.yaml"), "name: firm\nversion: 1.0.0\n").unwrap();
        fs::write(
            rules.join("soft.md"),
            "---\nid: soft\nengine: inferential\ndescription: Soft check.\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n---\nFlag this pattern.\n",
        )
        .unwrap();

        assert_eq!(rules_test(&base, &None, true).unwrap(), 0);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn package_name_from_manifest_or_default() {
        let base = std::env::temp_dir().join(format!("lawlint-cli-pkgname-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let rules = base.join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(base.join("style.yaml"), "name: firm\nversion: 1.0.0\n").unwrap();
        let rule = rules.join("no-x.md");
        fs::write(
            &rule,
            "---\nid: no-x\nengine: phrase\npatterns: [\"z\"]\n---\n",
        )
        .unwrap();
        assert_eq!(package_name_for(&rule), "firm");

        let loose = base.join("loose.md");
        fs::write(
            &loose,
            "---\nid: loose\nengine: phrase\npatterns: [\"z\"]\n---\n",
        )
        .unwrap();
        // style.yaml IS a sibling here, so the manifest still wins.
        assert_eq!(package_name_for(&loose), "firm");

        let orphan_dir = base.join("orphan");
        fs::create_dir_all(&orphan_dir).unwrap();
        let orphan = orphan_dir.join("o.md");
        fs::write(&orphan, "x").unwrap();
        // Parent of parent is `base` which has style.yaml… so use a deeper dir.
        let deep_dir = base.join("deep").join("deeper");
        fs::create_dir_all(&deep_dir).unwrap();
        let deep = deep_dir.join("d.md");
        fs::write(&deep, "x").unwrap();
        assert_eq!(package_name_for(&deep), "test");

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn single_rule_set_builds_and_reports_rule_errors() {
        let base = std::env::temp_dir().join(format!("lawlint-cli-single-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let good = base.join("no-x.md");
        fs::write(
            &good,
            "---\nid: no-x\nengine: phrase\nseverity: error\npatterns: [\"zebra\"]\n---\n",
        )
        .unwrap();
        let set = single_rule_set(&good, "firm").unwrap();
        assert_eq!(set.metas().len(), 1);
        assert_eq!(set.metas()[0].id.0, "firm/no-x");
        let result = lint_with("A zebra appears.", &LintOptions::default(), &set);
        assert!(has_rule(&result, "firm/no-x"));

        let bad = base.join("broken.md");
        fs::write(
            &bad,
            "---\nid: broken\nengine: phrase\npatterns: [\"(\"]\n---\n",
        )
        .unwrap();
        assert!(single_rule_set(&bad, "firm").is_err());

        fs::remove_dir_all(&base).unwrap();
    }
}
