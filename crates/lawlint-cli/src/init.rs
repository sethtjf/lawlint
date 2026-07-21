//! `lawlint init` — project setup. Creates `.lawlint/config.json` (and
//! optionally a `.lawlint/rules/` package) in the current directory via an
//! interactive walkthrough.
//!
//! The walkthrough opens with the AI-model catalog, hosted providers first
//! (#50): Anthropic preselected, then OpenAI and Azure Foundry — hosted is
//! the recommended path and requires an API key. Every entry says where
//! text goes ("text is sent to the provider" vs. "no text leaves this
//! machine"), so consent is given once, informed, here. Local models sit
//! behind an explicit advanced entry that states their constraints
//! (multi-GB download, slower inference, measurably lower quality —
//! docs/eval-corpus.md) and requires acknowledging them; the
//! acknowledgment persists as `ai.localAcknowledged` and
//! `--acknowledge-local` is the non-interactive equivalent. The selection
//! lands in the config's `ai` section; hosted API keys go to the
//! user-level credential store (`~/.config/lawlint/credentials`, 0600) —
//! never into the project config, which gets committed.
//!
//! Prompts read stdin line by line; an empty line or EOF accepts the shown
//! default, so a piped or CI invocation degrades to defaults instead of
//! hanging. `--yes` skips the prompts entirely and `--ai MODEL` answers the
//! catalog non-interactively. An existing config — `.lawlint/config.json`
//! under `--force`, else a legacy `lawlint.config.json` — seeds the prompt
//! defaults and its settings are carried into the new file.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use colored::Colorize;
use serde_json::{json, Value};

pub(crate) const DEFAULT_FLOOR: f64 = 0.6;
pub(crate) const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5-20251001";
pub(crate) const DEFAULT_OPENAI_HOSTED_MODEL: &str = "gpt-5.5";
pub(crate) const OPENAI_HOSTED_BASE_URL: &str = "https://api.openai.com/v1";
pub(crate) const DEFAULT_FOUNDRY_DEPLOYMENT: &str = "gpt-5.5";
pub(crate) const DEFAULT_COMPAT_BASE_URL: &str = "http://localhost:11434/v1";
pub(crate) const DEFAULT_COMPAT_MODEL: &str = "llama3.2";
pub(crate) const RULES_DIR: &str = ".lawlint/rules";

// ---- shared prompt copy ------------------------------------------------
// The catalog and local-constraints wording is shared verbatim between the
// non-interactive line walkthrough (`ask_ai`/`ask_local`) and the ratatui
// setup wizard (`init_tui`), so the two front-ends never drift.

/// The AI-catalog question stem (hosted providers first, #50).
pub(crate) const AI_CATALOG_PROMPT: &str =
    "Which AI model should lawlint use? It powers the tier-3 judge and future AI \
     features. Hosted providers are recommended.";

/// The five catalog entries, in display order. Every entry says where text
/// goes; local sits last and is marked advanced.
pub(crate) const AI_CATALOG_OPTIONS: [&str; 5] = [
    "Claude (Anthropic — hosted, recommended; text is sent to the provider, \
     requires API key)",
    "GPT (OpenAI — hosted; text is sent to the provider, requires API key)",
    "Azure Foundry (hosted — text is sent to the provider, requires API key + \
     endpoint)",
    "OpenAI-compatible endpoint (Ollama, vLLM, llama.cpp, … — text goes to that \
     server)",
    "Local model (advanced — private, no text leaves this machine; multi-GB \
     download, measurably lower quality)",
];

/// The local-model constraints, stated with the measured numbers before the
/// acknowledgment (docs/eval-corpus.md).
pub(crate) const LOCAL_CONSTRAINTS: &str =
    "  Local models are private (no text leaves this machine) but constrained:\n\
     \x20   - multi-GB model download on first use, slower inference\n\
     \x20   - measurably lower quality than hosted models: on lawlint's tier-3 eval\n\
     \x20     the local default scored F1 0.111 (empty-hedge) and 0.000\n\
     \x20     (padded-elaboration), with 38 of 330 chunks failing closed\n\
     \x20     (docs/eval-corpus.md)";

/// The two local-model choices, in display order (Qwen default, then Gemma).
pub(crate) const LOCAL_CHOICE_OPTIONS: [&str; 2] = [
    "Qwen2.5-1.5B (~1 GB GGUF download on first use)",
    "Gemma 4 E4B (~16 GB download on first use, runs 4-bit-quantized in memory)",
];

// ---- answers -----------------------------------------------------------

/// One catalog selection: the `ai.model` spec plus any credentials to store
/// (entries keyed by the provider's environment-variable name), plus
/// whether the local-model constraints were acknowledged (#50) — persisted
/// as `ai.localAcknowledged` so the per-use notice stays quiet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AiSelection {
    pub(crate) model: String,
    pub(crate) keys: Vec<(String, String)>,
    pub(crate) acknowledge_local: bool,
}

impl AiSelection {
    pub(crate) fn keyless(model: impl Into<String>) -> Self {
        AiSelection {
            model: model.into(),
            keys: Vec::new(),
            acknowledge_local: false,
        }
    }

    /// A local selection whose constraints were acknowledged.
    pub(crate) fn acknowledged_local(model: impl Into<String>) -> Self {
        AiSelection {
            model: model.into(),
            keys: Vec::new(),
            acknowledge_local: true,
        }
    }
}

/// Is `model` a local (mistral.rs) spec?
pub(crate) fn is_local_spec(model: &str) -> bool {
    model == "local" || model.starts_with("local:")
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Answers {
    /// `None` = leave the base config's `ai` section untouched.
    pub(crate) ai: Option<AiSelection>,
    /// `None` = leave the base config's `judge` untouched.
    pub(crate) judge_enabled: Option<bool>,
    /// Confidence floor; only written when it differs from the engine
    /// default. Meaningless unless the judge is enabled.
    pub(crate) floor: Option<f64>,
    /// `None` = leave the base config's `markdown` untouched.
    pub(crate) markdown: Option<bool>,
    pub(crate) scaffold_rules: bool,
}

impl Answers {
    /// `--yes` defaults: preserve everything a prior config already says;
    /// on a fresh project, write explicit `judge.enabled: false` and the
    /// recommended hosted default (`anthropic:<default model>`, key read
    /// from the environment or credential store at run time) so both
    /// opt-ins are discoverable. `--ai` still applies under `--yes`.
    fn accept_defaults(base: &Value, ai_override: Option<AiSelection>) -> Self {
        Answers {
            ai: ai_override.or_else(|| {
                if base.get("ai").is_some() {
                    None
                } else {
                    Some(AiSelection::keyless(format!(
                        "anthropic:{DEFAULT_ANTHROPIC_MODEL}"
                    )))
                }
            }),
            judge_enabled: if base.get("judge").is_some() {
                None
            } else {
                Some(false)
            },
            floor: None,
            markdown: None,
            scaffold_rules: false,
        }
    }
}

/// `--ai` shorthand: `qwen`/`gemma` name the catalog's local entries;
/// anything else must be a full model spec.
pub(crate) fn parse_ai_flag(value: &str) -> Result<AiSelection, String> {
    let model = match value {
        "qwen" | "local" => "local".to_string(),
        "gemma" => format!("local:{}", lawlint_judge::DEFAULT_GEMMA_REPO),
        spec if spec.starts_with("local:")
            || spec.starts_with("anthropic:")
            || spec.starts_with("foundry:") =>
        {
            spec.to_string()
        }
        spec if spec.starts_with("openai:") => {
            if !spec.contains('#') {
                return Err(format!(
                    "--ai {spec:?}: openai specs are \"openai:<base-url>#<model>\""
                ));
            }
            spec.to_string()
        }
        other => {
            return Err(format!(
                "--ai {other:?}: use qwen, gemma, or a model spec \
                 (anthropic:<model>, openai:<base-url>#<model>, \
                 foundry:<deployment>, local:<hf-repo>[#<gguf>])"
            ));
        }
    };
    Ok(AiSelection::keyless(model))
}

// ---- config assembly ---------------------------------------------------

/// Apply `answers` over `base` (the existing/legacy config, or `{}`). Keys
/// the walkthrough does not cover (enable/disable/severity/thresholds/…)
/// pass through untouched; so do `ai.features` overrides.
fn build_config(base: &Value, answers: &Answers) -> Value {
    let mut config = match base {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    if let Some(selection) = &answers.ai {
        let mut ai = config
            .get("ai")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        ai.insert("model".into(), json!(selection.model));
        if selection.acknowledge_local {
            ai.insert("localAcknowledged".into(), json!(true));
        }
        config.insert("ai".into(), Value::Object(ai));
    }
    if let Some(enabled) = answers.judge_enabled {
        let mut judge = serde_json::Map::new();
        judge.insert("enabled".into(), json!(enabled));
        if let Some(floor) = answers.floor {
            judge.insert("floor".into(), json!(floor));
        }
        // No `model` key: the spec lives in `ai` now. A legacy `judge.model`
        // seeded the catalog default, so re-running init migrates it.
        config.insert("judge".into(), Value::Object(judge));
    }
    match answers.markdown {
        Some(true) => {
            config.insert("markdown".into(), json!(true));
        }
        Some(false) => {
            config.remove("markdown");
        }
        None => {}
    }
    if answers.scaffold_rules {
        let mut dirs: Vec<String> = config
            .get("ruleDirs")
            .and_then(|v| v.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !dirs.iter().any(|dir| dir == RULES_DIR) {
            dirs.push(RULES_DIR.to_string());
        }
        config.insert("ruleDirs".into(), json!(dirs));
    }
    Value::Object(config)
}

/// Package name for the scaffolded rules package: the project directory
/// name, lowercased with runs of non-alphanumerics collapsed to `-`.
/// "core" is reserved for the built-ins (a duplicate rule id across
/// packages is a load error), and an empty result falls back to "project".
fn package_name(directory: &Path) -> String {
    let raw = directory
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut name = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.trim_matches('-').to_string();
    if name.is_empty() || name == "core" {
        "project".to_string()
    } else {
        name
    }
}

// ---- prompting ---------------------------------------------------------

/// Line-oriented prompt driver over abstract streams so the walkthrough is
/// unit-testable. Empty input or EOF always yields the default.
struct Prompter<R: BufRead, W: Write> {
    input: R,
    output: W,
}

impl<R: BufRead, W: Write> Prompter<R, W> {
    fn say(&mut self, text: &str) -> Result<(), String> {
        writeln!(self.output, "{text}").map_err(|error| error.to_string())
    }

    /// One raw prompt round-trip; `None` = EOF (caller must not re-ask).
    fn read(&mut self, prompt: &str) -> Result<Option<String>, String> {
        write!(self.output, "{prompt} ").map_err(|error| error.to_string())?;
        self.output.flush().map_err(|error| error.to_string())?;
        let mut line = String::new();
        let bytes = self
            .input
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if bytes == 0 {
            writeln!(self.output).map_err(|error| error.to_string())?;
            return Ok(None);
        }
        Ok(Some(line.trim().to_string()))
    }

    fn line(&mut self, prompt: &str, default: &str) -> Result<String, String> {
        let answer = self.read(&format!("{prompt} [{default}]:"))?;
        Ok(match answer {
            None => default.to_string(),
            Some(line) if line.is_empty() => default.to_string(),
            Some(line) => line,
        })
    }

    /// A secret or optional value: empty input and EOF mean "skip".
    fn secret(&mut self, prompt: &str) -> Result<Option<String>, String> {
        let answer = self.read(&format!("{prompt}:"))?;
        Ok(answer.filter(|line| !line.is_empty()))
    }

    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, String> {
        let hint = if default { "Y/n" } else { "y/N" };
        loop {
            let Some(line) = self.read(&format!("{prompt} [{hint}]:"))? else {
                return Ok(default);
            };
            match line.to_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => self.say("Please answer y or n.")?,
            }
        }
    }

    /// Numbered menu; returns the selected index into `options`.
    fn choice(&mut self, prompt: &str, options: &[&str], default: usize) -> Result<usize, String> {
        self.say(prompt)?;
        for (index, option) in options.iter().enumerate() {
            self.say(&format!("  {}) {option}", index + 1))?;
        }
        loop {
            let Some(line) = self.read(&format!("Choice [{}]:", default + 1))? else {
                return Ok(default);
            };
            if line.is_empty() {
                return Ok(default);
            }
            match line.parse::<usize>() {
                Ok(n) if (1..=options.len()).contains(&n) => return Ok(n - 1),
                _ => self.say(&format!("Please enter a number 1-{}.", options.len()))?,
            }
        }
    }

    fn floor(&mut self, default: f64) -> Result<f64, String> {
        loop {
            let line = self.line(
                "Judge confidence floor (findings below it are dropped, 0-1)",
                &format!("{default}"),
            )?;
            match line.parse::<f64>() {
                Ok(value) if (0.0..=1.0).contains(&value) => return Ok(value),
                _ => self.say("Please enter a number between 0 and 1.")?,
            }
        }
    }
}

// ---- AI-preferences step -----------------------------------------------

/// Catalog index whose entry matches `model` (a spec from the seeding
/// config), for the prompt default. Hosted entries lead (#50): a fresh
/// project (empty spec) preselects Anthropic; local specs map to the
/// advanced local entry.
pub(crate) fn default_catalog_index(model: &str) -> usize {
    if is_local_spec(model) {
        4
    } else if let Some(spec) = model.strip_prefix("openai:") {
        if spec.starts_with(OPENAI_HOSTED_BASE_URL) {
            1
        } else {
            3
        }
    } else if model.starts_with("foundry:") {
        2
    } else {
        // anthropic:… and anything unrecognized (including a fresh, empty
        // spec) preselect the recommended hosted entry.
        0
    }
}

/// Gemma GGUFs (any version) have no loader in the bundled mistral.rs
/// runtime — its GGUF loader carries no gemma architecture. Gemma runs
/// from safetensors repos instead (quantized in situ), so warn at
/// selection time, not after a multi-GB download at first run.
pub(crate) fn looks_like_gemma_gguf(repo: &str) -> bool {
    let lower = repo.to_lowercase();
    lower.contains("gemma") && lower.contains("gguf")
}

/// Provider API key prompt. A stored key goes to the user-level credential
/// file; skipping means lawlint reads `$env_name` at run time. Either way
/// the project config only ever names the provider/model.
fn ask_key<R: BufRead, W: Write>(
    prompter: &mut Prompter<R, W>,
    label: &str,
    env_name: &str,
) -> Result<Vec<(String, String)>, String> {
    match prompter.secret(&format!("{label} (Enter to skip and use ${env_name})"))? {
        Some(value) => Ok(vec![(env_name.to_string(), value)]),
        None => {
            prompter.say(&format!(
                "  No key stored; lawlint reads ${env_name} at run time."
            ))?;
            Ok(Vec::new())
        }
    }
}

fn ask_anthropic<R: BufRead, W: Write>(
    base_model: &str,
    prompter: &mut Prompter<R, W>,
) -> Result<AiSelection, String> {
    let default_model = base_model
        .strip_prefix("anthropic:")
        .filter(|model| !model.is_empty())
        .unwrap_or(DEFAULT_ANTHROPIC_MODEL);
    let model = prompter.line("Anthropic model", default_model)?;
    let keys = ask_key(prompter, "Anthropic API key", "ANTHROPIC_API_KEY")?;
    Ok(AiSelection {
        model: format!("anthropic:{model}"),
        keys,
        acknowledge_local: false,
    })
}

/// The advanced local-model flow (#50): states the constraints with the
/// measured numbers, requires acknowledging them, then picks the model.
/// `Ok(None)` = the user declined the acknowledgment. `ack_default` seeds
/// the confirm so re-running init over an already-local config (or with
/// `--acknowledge-local`) Enter-throughs cleanly.
fn ask_local<R: BufRead, W: Write>(
    base_model: &str,
    ack_default: bool,
    prompter: &mut Prompter<R, W>,
) -> Result<Option<AiSelection>, String> {
    prompter.say(LOCAL_CONSTRAINTS)?;
    if !prompter.confirm("Use a local model with these constraints?", ack_default)? {
        return Ok(None);
    }
    let is_gemma = |repo: &str| repo.to_lowercase().contains("gemma");
    let local_choice = prompter.choice(
        "Which local model?",
        &LOCAL_CHOICE_OPTIONS,
        if base_model.strip_prefix("local:").is_some_and(is_gemma) {
            1
        } else {
            0
        },
    )?;
    if local_choice == 0 {
        // A custom non-Gemma repo from the seeding config must survive
        // Enter-through (re-running init preserves earlier choices); the
        // stock repo is spelled "local" so fresh configs stay short.
        let default_repo = base_model
            .strip_prefix("local:")
            .filter(|repo| !repo.is_empty() && !is_gemma(repo))
            .unwrap_or(lawlint_judge::DEFAULT_LOCAL_REPO);
        let repo = prompter.line("Hugging Face GGUF repo (repo[#file])", default_repo)?;
        Ok(Some(AiSelection::acknowledged_local(
            if repo == lawlint_judge::DEFAULT_LOCAL_REPO {
                "local".to_string()
            } else {
                format!("local:{repo}")
            },
        )))
    } else {
        let default_repo = base_model
            .strip_prefix("local:")
            .filter(|repo| is_gemma(repo))
            .unwrap_or(lawlint_judge::DEFAULT_GEMMA_REPO);
        let repo = prompter.line("Hugging Face repo (repo[#file])", default_repo)?;
        if looks_like_gemma_gguf(&repo) {
            prompter.say(&format!(
                "  Note: gemma GGUFs are not runnable by the bundled runtime — use \
                 the safetensors repo (e.g. {}), which runs 4-bit-quantized \
                 in-process. The repo stays editable in .lawlint/config.json.",
                lawlint_judge::DEFAULT_GEMMA_REPO
            ))?;
        }
        Ok(Some(AiSelection::acknowledged_local(format!(
            "local:{repo}"
        ))))
    }
}

/// The model catalog, hosted providers first (#50) with Anthropic
/// preselected. Every entry says where text goes; hosted entries prompt for
/// the provider API key. Local models sit behind the last, advanced entry
/// and require acknowledging their constraints; declining falls back to the
/// recommended hosted entry.
fn ask_ai<R: BufRead, W: Write>(
    base_model: &str,
    ack_default: bool,
    prompter: &mut Prompter<R, W>,
) -> Result<AiSelection, String> {
    let choice = prompter.choice(
        AI_CATALOG_PROMPT,
        &AI_CATALOG_OPTIONS,
        default_catalog_index(base_model),
    )?;

    match choice {
        0 => ask_anthropic(base_model, prompter),
        1 => {
            let default_model = base_model
                .strip_prefix("openai:")
                .and_then(|spec| spec.rsplit_once('#'))
                .map(|(_, model)| model)
                .filter(|model| !model.is_empty())
                .unwrap_or(DEFAULT_OPENAI_HOSTED_MODEL);
            let model = prompter.line("OpenAI model", default_model)?;
            let keys = ask_key(prompter, "OpenAI API key", "OPENAI_API_KEY")?;
            Ok(AiSelection {
                model: format!("openai:{OPENAI_HOSTED_BASE_URL}#{model}"),
                keys,
                acknowledge_local: false,
            })
        }
        2 => {
            let default_deployment = base_model
                .strip_prefix("foundry:")
                .filter(|deployment| !deployment.is_empty())
                .unwrap_or(DEFAULT_FOUNDRY_DEPLOYMENT);
            let deployment = prompter.line("Foundry model deployment", default_deployment)?;
            let mut keys = Vec::new();
            match prompter.secret(
                "Azure Foundry endpoint, e.g. https://<resource>.services.ai.azure.com \
                 (Enter to skip and use $AZURE_FOUNDRY_ENDPOINT)",
            )? {
                Some(endpoint) => keys.push(("AZURE_FOUNDRY_ENDPOINT".to_string(), endpoint)),
                None => prompter.say(
                    "  No endpoint stored; lawlint reads $AZURE_FOUNDRY_ENDPOINT at run time.",
                )?,
            }
            keys.extend(ask_key(
                prompter,
                "Azure Foundry API key",
                "AZURE_FOUNDRY_API_KEY",
            )?);
            Ok(AiSelection {
                model: format!("foundry:{deployment}"),
                keys,
                acknowledge_local: false,
            })
        }
        3 => {
            let (base_url, model) = base_model
                .strip_prefix("openai:")
                .and_then(|spec| spec.rsplit_once('#'))
                .unwrap_or((DEFAULT_COMPAT_BASE_URL, DEFAULT_COMPAT_MODEL));
            let base_url = prompter.line("Base URL", base_url)?;
            let model = prompter.line("Model", model)?;
            Ok(AiSelection::keyless(format!("openai:{base_url}#{model}")))
        }
        _ => match ask_local(base_model, ack_default, prompter)? {
            Some(selection) => Ok(selection),
            None => {
                prompter.say("Using the recommended hosted provider instead.")?;
                ask_anthropic(base_model, prompter)
            }
        },
    }
}

/// The interactive walkthrough. `base` (the existing or legacy config, or
/// `{}`) seeds the defaults so re-running init preserves earlier choices
/// by default. A legacy `judge.model` seeds the catalog too, migrating it
/// into the `ai` section.
fn ask<R: BufRead, W: Write>(
    base: &Value,
    ai_override: Option<AiSelection>,
    acknowledge_local: bool,
    prompter: &mut Prompter<R, W>,
) -> Result<Answers, String> {
    let base_judge = base.get("judge");
    let base_ai_model = base
        .get("ai")
        .and_then(|ai| ai.get("model"))
        .and_then(Value::as_str)
        .or_else(|| {
            base_judge
                .and_then(|judge| judge.get("model"))
                .and_then(Value::as_str)
        })
        .unwrap_or("");

    // The local acknowledgment prompt defaults yes when the user has, in
    // effect, already opted in: `--acknowledge-local`, a config already on
    // a local model (Enter-through must preserve earlier choices), or a
    // prior acknowledgment.
    let ack_default = acknowledge_local
        || is_local_spec(base_ai_model)
        || base
            .get("ai")
            .and_then(|ai| ai.get("localAcknowledged"))
            .and_then(Value::as_bool)
            == Some(true);

    let ai = match ai_override {
        Some(selection) => {
            prompter.say(&format!("AI model: {} (from --ai).", selection.model))?;
            selection
        }
        None => ask_ai(base_ai_model, ack_default, prompter)?,
    };

    let base_enabled = base_judge
        .and_then(|judge| judge.get("enabled"))
        .and_then(Value::as_bool)
        == Some(true);
    let judge_enabled = prompter.confirm(
        "Enable the tier-3 AI judge? It powers the inferential (semantic) rules; \
         tiers 1-2 always run.",
        base_enabled,
    )?;

    let floor = if !judge_enabled {
        None
    } else {
        // An out-of-range floor in the seeding config must not become the
        // prompt default: on EOF the default is re-fed to the validation
        // loop, which would then never terminate.
        let base_floor = base_judge
            .and_then(|judge| judge.get("floor"))
            .and_then(Value::as_f64)
            .filter(|floor| (0.0..=1.0).contains(floor))
            .unwrap_or(DEFAULT_FLOOR);
        let floor = prompter.floor(base_floor)?;
        (floor != DEFAULT_FLOOR).then_some(floor)
    };

    let base_markdown = base.get("markdown").and_then(Value::as_bool) == Some(true);
    let markdown = prompter.confirm(
        "Treat input as Markdown by default? (.md files are auto-detected either way)",
        base_markdown,
    )?;

    let scaffold_rules = prompter.confirm(
        &format!("Create a starter custom-rules package in {RULES_DIR}/?"),
        false,
    )?;

    Ok(Answers {
        ai: Some(ai),
        judge_enabled: Some(judge_enabled),
        floor,
        markdown: Some(markdown),
        scaffold_rules,
    })
}

// ---- scaffolding -------------------------------------------------------

const SAMPLE_RULE: &str = r#"# Example project rule. Check it with: lawlint rules test .lawlint/rules
id: no-avoidance-of-doubt
engine: phrase
severity: warning
description: "Flags the filler phrase \"for the avoidance of doubt\"."
message: "Drop the filler phrase; state the point directly."
examples:
  - bad: "For the avoidance of doubt, the fee is due monthly."
    good: "The fee is due monthly."
patterns:
  - pattern: '(?i)\bfor the avoidance of doubt\b'
    message: "Drop the filler phrase; state the point directly."
    suggestion: "State the point directly."
"#;

/// Write the starter package. Never overwrites: an existing style.yaml or
/// sample rule is left alone (re-running init must not clobber user rules).
fn scaffold_rules_package(directory: &Path, package: &str) -> Result<Vec<String>, String> {
    let root = directory.join(RULES_DIR);
    let rules = root.join("rules");
    fs::create_dir_all(&rules)
        .map_err(|error| format!("failed to create {}: {error}", rules.display()))?;
    let mut created = Vec::new();
    let manifest = root.join("style.yaml");
    if !manifest.exists() {
        fs::write(
            &manifest,
            format!(
                "name: {package}\nversion: 0.1.0\ndescription: Project-specific lawlint rules.\n"
            ),
        )
        .map_err(|error| format!("failed to write {}: {error}", manifest.display()))?;
        created.push(format!("{RULES_DIR}/style.yaml"));
    }
    let sample = rules.join("no-avoidance-of-doubt.yaml");
    if !sample.exists() {
        fs::write(&sample, SAMPLE_RULE)
            .map_err(|error| format!("failed to write {}: {error}", sample.display()))?;
        created.push(format!("{RULES_DIR}/rules/no-avoidance-of-doubt.yaml"));
    }
    Ok(created)
}

// ---- command -----------------------------------------------------------

/// A config file as a seed source: `None` when absent or not a JSON object
/// (init overwrites/ignores broken files rather than erroring — it is the
/// tool you would reach for to fix one).
fn read_config_object(path: &Path) -> Result<Option<Value>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(match serde_json::from_str::<Value>(&text) {
        Ok(value @ Value::Object(_)) => Some(value),
        Ok(_) | Err(_) => None,
    })
}

/// Where the seed defaults came from, so each front-end can narrate it in its
/// own voice (the line flow prints a notice, the wizard shows a styled line).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SeedSource {
    /// An existing `.lawlint/config.json` (reachable only under `--force`).
    ExistingNested,
    /// A legacy `lawlint.config.json` that parsed; its settings carry over.
    LegacyCarried,
    /// A legacy `lawlint.config.json` present but not valid JSON — left alone.
    LegacyUnparseable,
    /// Nothing to seed from; a fresh project.
    Fresh,
}

/// The resolved seed for an init run: the base config the answers apply over,
/// plus the legacy file (if any) so the front-end can offer to remove it.
pub(crate) struct Seed {
    pub(crate) base: Value,
    /// The parsed legacy `lawlint.config.json`, if it existed and parsed.
    pub(crate) legacy: Option<Value>,
    pub(crate) legacy_carried: bool,
    pub(crate) legacy_path: PathBuf,
    pub(crate) source: SeedSource,
}

/// Resolve the seed config, mirroring discovery precedence: an existing
/// `.lawlint/config.json` (under `--force`), then a legacy
/// `lawlint.config.json`, then empty — so re-running init preserves prior
/// settings instead of regenerating from scratch. Errors if the nested config
/// already exists without `--force` (init refuses to clobber it).
pub(crate) fn resolve_seed(directory: &Path, force: bool) -> Result<Seed, String> {
    let config_path = directory.join(".lawlint").join("config.json");
    if config_path.exists() && !force {
        return Err(format!(
            "{} already exists; rerun with --force to overwrite",
            config_path.display()
        ));
    }
    let legacy_path = directory.join("lawlint.config.json");
    let existing = read_config_object(&config_path)?;
    let legacy = read_config_object(&legacy_path)?;
    let legacy_exists = legacy_path.is_file();
    let (base, legacy_carried, source) = match (existing, &legacy) {
        (Some(existing), _) => (existing, false, SeedSource::ExistingNested),
        (None, Some(legacy)) => (legacy.clone(), true, SeedSource::LegacyCarried),
        (None, None) => (
            json!({}),
            false,
            if legacy_exists {
                SeedSource::LegacyUnparseable
            } else {
                SeedSource::Fresh
            },
        ),
    };
    Ok(Seed {
        base,
        legacy,
        legacy_carried,
        legacy_path,
        source,
    })
}

/// The result of writing an init run's answers to disk.
pub(crate) struct Applied {
    /// Project-relative paths created (config, and any scaffolded rule files).
    pub(crate) created: Vec<String>,
    /// Where credentials were stored, if any — front-ends echo this so the
    /// user knows the key landed outside the committed project config.
    pub(crate) credential_message: Option<String>,
}

/// Apply `answers` over `base`: write `.lawlint/config.json`, scaffold the
/// starter rules package if requested, and store any API credentials in the
/// user-level file. Front-end-agnostic — legacy-file removal and the on-screen
/// summary stay with each caller. Both the line walkthrough and the ratatui
/// wizard funnel through here so they write identical config.
pub(crate) fn apply_answers(
    directory: &Path,
    base: &Value,
    answers: &Answers,
    credentials_path: Option<&Path>,
) -> Result<Applied, String> {
    let lawlint_dir = directory.join(".lawlint");
    let config_path = lawlint_dir.join("config.json");
    let config = build_config(base, answers);
    fs::create_dir_all(&lawlint_dir)
        .map_err(|error| format!("failed to create {}: {error}", lawlint_dir.display()))?;
    let mut text = serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?;
    text.push('\n');
    fs::write(&config_path, text)
        .map_err(|error| format!("failed to write {}: {error}", config_path.display()))?;

    let mut created = vec![".lawlint/config.json".to_string()];
    if answers.scaffold_rules {
        created.extend(scaffold_rules_package(directory, &package_name(directory))?);
    }

    // Credentials land outside the project (the config gets committed);
    // report exactly where the key went.
    let credential_message = match &answers.ai {
        Some(selection) if !selection.keys.is_empty() => {
            let path = match credentials_path {
                Some(path) => {
                    lawlint_judge::credentials::store_at(path, &selection.keys)?;
                    path.to_path_buf()
                }
                None => lawlint_judge::credentials::store(&selection.keys)?,
            };
            Some(format!(
                "Stored {} in {} (outside your project, owner-only permissions).",
                selection
                    .keys
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                path.display()
            ))
        }
        _ => None,
    };

    Ok(Applied {
        created,
        credential_message,
    })
}

fn run_init<R: BufRead, W: Write>(
    directory: &Path,
    yes: bool,
    force: bool,
    ai_flag: Option<&str>,
    acknowledge_local: bool,
    credentials_path: Option<&Path>,
    prompter: &mut Prompter<R, W>,
) -> Result<i32, String> {
    let ai_override = ai_flag.map(parse_ai_flag).transpose()?;

    let Seed {
        base,
        legacy,
        legacy_carried,
        legacy_path,
        source,
    } = resolve_seed(directory, force)?;
    match source {
        SeedSource::ExistingNested => prompter.say(&format!(
            "Existing {} found — its settings seed the defaults below and carry over.",
            ".lawlint/config.json".bold()
        ))?,
        SeedSource::LegacyCarried => prompter.say(&format!(
            "Found lawlint.config.json — its settings seed the defaults below and \
             carry over to {}.",
            ".lawlint/config.json".bold()
        ))?,
        SeedSource::LegacyUnparseable => prompter.say(
            "Found lawlint.config.json but it is not valid JSON; starting fresh \
             (the old file is left untouched).",
        )?,
        SeedSource::Fresh => {}
    }

    let mut answers = if yes {
        Answers::accept_defaults(&base, ai_override)
    } else {
        prompter.say(&format!(
            "{}\n",
            "lawlint init — sets up .lawlint/config.json for this project.".bold()
        ))?;
        ask(&base, ai_override, acknowledge_local, prompter)?
    };

    // `--acknowledge-local`: the non-interactive acknowledgment (#50). It
    // attaches to a local selection from `--ai`, or — when `--yes` left an
    // existing `ai` section untouched — to the base config's local model,
    // so CI can acknowledge an existing setup without re-answering prompts.
    if acknowledge_local {
        match &mut answers.ai {
            Some(selection) if is_local_spec(&selection.model) => {
                selection.acknowledge_local = true;
            }
            Some(_) => {}
            None => {
                let base_local = base
                    .get("ai")
                    .and_then(|ai| ai.get("model"))
                    .and_then(Value::as_str)
                    .filter(|model| is_local_spec(model));
                if let Some(model) = base_local {
                    answers.ai = Some(AiSelection::acknowledged_local(model));
                }
            }
        }
    }

    let applied = apply_answers(directory, &base, &answers, credentials_path)?;
    if let Some(message) = &applied.credential_message {
        prompter.say(message)?;
    }

    // The new file shadows the legacy one (same-directory precedence), so
    // offer to remove it; under --yes never delete, just explain. The offer
    // only fires when the legacy file parsed cleanly — an unparseable one
    // stays untouched, as promised above.
    if legacy.is_some() {
        let prompt = if legacy_carried {
            "Remove the old lawlint.config.json? (its settings were carried over; \
             .lawlint/config.json now takes precedence)"
        } else {
            "Remove the old lawlint.config.json? (.lawlint/config.json takes \
             precedence, so it is unused)"
        };
        let remove = !yes && prompter.confirm(prompt, true)?;
        if remove {
            fs::remove_file(&legacy_path)
                .map_err(|error| format!("failed to remove {}: {error}", legacy_path.display()))?;
            prompter.say("Removed lawlint.config.json.")?;
        } else {
            prompter.say(
                "Keeping lawlint.config.json; note that .lawlint/config.json takes precedence.",
            )?;
        }
    }

    prompter.say("")?;
    for file in &applied.created {
        prompter.say(&format!("  {} {file}", "created".green()))?;
    }
    prompter.say(&format!(
        "\nNext steps:\n  lawlint <file>          lint a document\n  lawlint rules           list active rules{}\n  Edit .lawlint/config.json any time to adjust settings.",
        if answers.scaffold_rules {
            "\n  lawlint rules test .lawlint/rules   check your custom rules"
        } else {
            ""
        }
    ))?;
    Ok(0)
}

pub fn init_command(
    yes: bool,
    force: bool,
    ai: Option<&str>,
    acknowledge_local: bool,
) -> Result<i32, String> {
    let directory = std::env::current_dir().map_err(|error| error.to_string())?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut prompter = Prompter {
        input: stdin.lock(),
        output: stdout.lock(),
    };
    run_init(
        &directory,
        yes,
        force,
        ai,
        acknowledge_local,
        None,
        &mut prompter,
    )
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn prompter(input: &str) -> Prompter<Cursor<Vec<u8>>, Vec<u8>> {
        Prompter {
            input: Cursor::new(input.as_bytes().to_vec()),
            output: Vec::new(),
        }
    }

    #[test]
    fn build_config_fresh_defaults() {
        // #50: the fresh default is the recommended hosted provider, never
        // a local model.
        let answers = Answers::accept_defaults(&json!({}), None);
        let config = build_config(&json!({}), &answers);
        assert_eq!(
            config,
            json!({
                "judge": {"enabled": false},
                "ai": {"model": format!("anthropic:{DEFAULT_ANTHROPIC_MODEL}")}
            })
        );
    }

    #[test]
    fn build_config_writes_local_acknowledgment() {
        let answers = Answers {
            ai: Some(AiSelection::acknowledged_local("local")),
            judge_enabled: Some(false),
            floor: None,
            markdown: None,
            scaffold_rules: false,
        };
        let config = build_config(&json!({}), &answers);
        assert_eq!(
            config["ai"],
            json!({"model": "local", "localAcknowledged": true})
        );
        // Unacknowledged selections never write the key.
        let answers = Answers {
            ai: Some(AiSelection::keyless("local")),
            judge_enabled: Some(false),
            floor: None,
            markdown: None,
            scaffold_rules: false,
        };
        let config = build_config(&json!({}), &answers);
        assert_eq!(config["ai"], json!({"model": "local"}));
    }

    #[test]
    fn accept_defaults_preserves_existing_judge_and_ai() {
        let base = json!({
            "judge": {"enabled": true, "model": "anthropic:m", "floor": 0.8},
            "ai": {"model": "foundry:d", "features": {"learn": "local"}}
        });
        let answers = Answers::accept_defaults(&base, None);
        assert_eq!(answers.ai, None);
        assert_eq!(answers.judge_enabled, None);
        let config = build_config(&base, &answers);
        assert_eq!(config["judge"], base["judge"]);
        assert_eq!(config["ai"], base["ai"]);
    }

    #[test]
    fn accept_defaults_applies_ai_override() {
        let base = json!({"ai": {"model": "local"}});
        let selection = AiSelection::keyless("anthropic:m");
        let answers = Answers::accept_defaults(&base, Some(selection));
        let config = build_config(&base, &answers);
        assert_eq!(config["ai"], json!({"model": "anthropic:m"}));
    }

    #[test]
    fn build_config_overwrites_covered_keys_and_passes_through_rest() {
        let base = json!({
            "disable": ["core/no-semicolons"],
            "markdown": true,
            "judge": {"enabled": false, "model": "anthropic:old"},
            "ai": {"model": "local", "features": {"learn": "anthropic:m"}},
            "ruleDirs": ["extra"]
        });
        let answers = Answers {
            ai: Some(AiSelection::keyless("foundry:d")),
            judge_enabled: Some(true),
            floor: Some(0.8),
            markdown: Some(false),
            scaffold_rules: true,
        };
        let config = build_config(&base, &answers);
        assert_eq!(config["disable"], json!(["core/no-semicolons"]));
        assert!(config.get("markdown").is_none());
        // The legacy judge.model is dropped (migrated into `ai`); the
        // per-feature overrides survive the model rewrite.
        assert_eq!(config["judge"], json!({"enabled": true, "floor": 0.8}));
        assert_eq!(
            config["ai"],
            json!({"model": "foundry:d", "features": {"learn": "anthropic:m"}})
        );
        // Existing dirs preserved, ours appended once.
        assert_eq!(config["ruleDirs"], json!(["extra", RULES_DIR]));
        let again = build_config(&config, &answers);
        assert_eq!(again["ruleDirs"], json!(["extra", RULES_DIR]));
    }

    #[test]
    fn parse_ai_flag_shorthands_and_specs() {
        assert_eq!(parse_ai_flag("qwen").unwrap().model, "local");
        assert_eq!(
            parse_ai_flag("gemma").unwrap().model,
            format!("local:{}", lawlint_judge::DEFAULT_GEMMA_REPO)
        );
        assert_eq!(
            parse_ai_flag("anthropic:claude-x").unwrap().model,
            "anthropic:claude-x"
        );
        assert_eq!(
            parse_ai_flag("openai:http://x/v1#m").unwrap().model,
            "openai:http://x/v1#m"
        );
        assert_eq!(parse_ai_flag("foundry:d").unwrap().model, "foundry:d");
        // openai without '#', and unknown words, are rejected with guidance.
        assert!(parse_ai_flag("openai:http://x/v1")
            .unwrap_err()
            .contains("#<model>"));
        assert!(parse_ai_flag("gpt4").unwrap_err().contains("qwen, gemma"));
        // No key entries ever come from the flag.
        assert!(parse_ai_flag("anthropic:m").unwrap().keys.is_empty());
    }

    #[test]
    fn default_catalog_index_maps_specs() {
        // Hosted-first (#50): fresh (empty) and anthropic specs preselect
        // the recommended hosted entry; local specs map to the advanced
        // local entry at the end.
        assert_eq!(default_catalog_index(""), 0);
        assert_eq!(default_catalog_index("anthropic:m"), 0);
        assert_eq!(
            default_catalog_index(&format!("openai:{OPENAI_HOSTED_BASE_URL}#gpt-5.5")),
            1
        );
        assert_eq!(default_catalog_index("foundry:d"), 2);
        assert_eq!(
            default_catalog_index("openai:http://localhost:11434/v1#m"),
            3
        );
        assert_eq!(default_catalog_index("local"), 4);
        assert_eq!(default_catalog_index("local:foo/bar-GGUF"), 4);
        assert_eq!(
            default_catalog_index("local:google/gemma-4-E4B-it-qat-q4_0-gguf"),
            4
        );
    }

    #[test]
    fn package_name_sanitizes() {
        assert_eq!(package_name(Path::new("/x/My Firm LLP")), "my-firm-llp");
        assert_eq!(package_name(Path::new("/x/lawlint")), "lawlint");
        assert_eq!(package_name(Path::new("/x/core")), "project");
        assert_eq!(package_name(Path::new("/x/---")), "project");
    }

    #[test]
    fn prompter_defaults_on_empty_and_eof() {
        let mut p = prompter("\n");
        assert_eq!(p.line("Model", "default").unwrap(), "default");
        // EOF (input exhausted) also yields defaults — no infinite loop.
        assert!(p.confirm("Sure?", true).unwrap());
        assert_eq!(p.choice("Pick", &["a", "b"], 1).unwrap(), 1);
        assert_eq!(p.floor(0.6).unwrap(), 0.6);
        assert_eq!(p.secret("Key").unwrap(), None);
    }

    #[test]
    fn prompter_rejects_then_accepts_valid_input() {
        let mut p = prompter("maybe\nyes\n9\n2\n1.5\n0.75\n");
        assert!(p.confirm("Sure?", false).unwrap());
        assert_eq!(p.choice("Pick", &["a", "b"], 0).unwrap(), 1);
        assert_eq!(p.floor(0.6).unwrap(), 0.75);
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("Please answer y or n."));
        assert!(transcript.contains("Please enter a number 1-2."));
        assert!(transcript.contains("between 0 and 1"));
    }

    #[test]
    fn ask_ai_local_choices() {
        // Choice 5 (local, advanced) + acknowledgment + Qwen: the stock
        // repo is spelled "local", and the acknowledgment is recorded.
        let mut p = prompter("5\ny\n1\n\n");
        assert_eq!(
            ask_ai("", false, &mut p).unwrap(),
            AiSelection::acknowledged_local("local")
        );
        // The constraints are stated with the measured numbers before the
        // acknowledgment prompt.
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("0.111"), "{transcript}");
        assert!(transcript.contains("38 of 330"), "{transcript}");
        assert!(transcript.contains("docs/eval-corpus.md"), "{transcript}");

        // Custom Qwen repo: full local:<repo> spec.
        let mut p = prompter("5\ny\n1\nme/custom-qwen-GGUF\n");
        assert_eq!(
            ask_ai("", false, &mut p).unwrap().model,
            "local:me/custom-qwen-GGUF"
        );

        // Gemma 4: config-editable repo; the default (safetensors, runs via
        // in-situ quantization) draws no warning.
        let mut p = prompter("5\ny\n2\n\n");
        let selection = ask_ai("", false, &mut p).unwrap();
        assert_eq!(
            selection.model,
            format!("local:{}", lawlint_judge::DEFAULT_GEMMA_REPO)
        );
        assert!(selection.acknowledge_local);
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(!transcript.contains("not runnable"));

        // A gemma GGUF repo (no gemma architecture in the bundled runtime's
        // GGUF loader) warns at selection time, before any download.
        let mut p = prompter("5\ny\n2\nunsloth/gemma-4-E4B-it-GGUF\n");
        let selection = ask_ai("", false, &mut p).unwrap();
        assert_eq!(selection.model, "local:unsloth/gemma-4-E4B-it-GGUF");
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("not runnable"));
        assert!(transcript.contains(lawlint_judge::DEFAULT_GEMMA_REPO));
    }

    #[test]
    fn ask_ai_local_decline_falls_back_to_hosted() {
        // Declining the constraints acknowledgment (the fresh default)
        // lands on the recommended hosted provider, not on a local model.
        let mut p = prompter("5\nn\n");
        let selection = ask_ai("", false, &mut p).unwrap();
        assert_eq!(
            selection.model,
            format!("anthropic:{DEFAULT_ANTHROPIC_MODEL}")
        );
        assert!(!selection.acknowledge_local);
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("recommended hosted"), "{transcript}");

        // EOF at the acknowledgment behaves like declining: no local model
        // without an explicit yes.
        let mut p = prompter("5\n");
        let selection = ask_ai("", false, &mut p).unwrap();
        assert_eq!(
            selection.model,
            format!("anthropic:{DEFAULT_ANTHROPIC_MODEL}")
        );
    }

    #[test]
    fn ask_ai_preserves_custom_local_repo_on_defaults() {
        // Regression: a custom non-Gemma local repo seeds the local entry's
        // repo prompt, so Enter-through keeps it instead of resetting to
        // the stock model. The already-local seed makes the acknowledgment
        // default yes, so EOF passes through it.
        let mut p = prompter("");
        assert_eq!(
            ask_ai("local:me/custom-qwen-GGUF#m.gguf", true, &mut p).unwrap(),
            AiSelection::acknowledged_local("local:me/custom-qwen-GGUF#m.gguf")
        );
    }

    #[test]
    fn ask_ai_hosted_choices_collect_keys_and_caveats() {
        // Anthropic (choice 1, preselected): default model, key stored
        // under its env-var name.
        let mut p = prompter("1\n\nsk-ant-secret\n");
        let selection = ask_ai("", false, &mut p).unwrap();
        assert_eq!(
            selection.model,
            format!("anthropic:{DEFAULT_ANTHROPIC_MODEL}")
        );
        assert_eq!(
            selection.keys,
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-ant-secret".to_string())]
        );
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("text is sent to the provider"));
        // Hosted first, recommended, local last and marked advanced.
        assert!(transcript.contains("Hosted providers are recommended"));
        assert!(transcript.contains("1) Claude (Anthropic"));
        assert!(transcript.contains("5) Local model (advanced"));

        // OpenAI hosted: skipping the key stores nothing and points at the
        // env var instead.
        let mut p = prompter("2\n\n\n");
        let selection = ask_ai("", false, &mut p).unwrap();
        assert_eq!(
            selection.model,
            format!("openai:{OPENAI_HOSTED_BASE_URL}#{DEFAULT_OPENAI_HOSTED_MODEL}")
        );
        assert!(selection.keys.is_empty());
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("$OPENAI_API_KEY"));

        // Foundry: endpoint + key both land in the credential entries.
        let mut p = prompter("3\ngpt-5.5\nhttps://res.services.ai.azure.com\nfoundry-key\n");
        let selection = ask_ai("", false, &mut p).unwrap();
        assert_eq!(selection.model, "foundry:gpt-5.5");
        assert_eq!(
            selection.keys,
            vec![
                (
                    "AZURE_FOUNDRY_ENDPOINT".to_string(),
                    "https://res.services.ai.azure.com".to_string()
                ),
                (
                    "AZURE_FOUNDRY_API_KEY".to_string(),
                    "foundry-key".to_string()
                ),
            ]
        );
    }

    #[test]
    fn ask_ai_compat_endpoint_choice() {
        let mut p = prompter("4\n\n\n");
        assert_eq!(
            ask_ai("", false, &mut p).unwrap().model,
            format!("openai:{DEFAULT_COMPAT_BASE_URL}#{DEFAULT_COMPAT_MODEL}")
        );
        // An existing compat spec seeds both prompts.
        let mut p = prompter("\n\n\n");
        assert_eq!(
            ask_ai("openai:http://gpu-box:8000/v1#qwen3", false, &mut p)
                .unwrap()
                .model,
            "openai:http://gpu-box:8000/v1#qwen3"
        );
    }

    #[test]
    fn ask_default_flow_is_hosted_anthropic_and_disabled_judge() {
        // #50: accepting every default on a fresh project selects the
        // recommended hosted provider — never a local model, and no
        // download-sized surprises.
        let mut p = prompter("");
        let answers = ask(&json!({}), None, false, &mut p).unwrap();
        assert_eq!(
            answers.ai,
            Some(AiSelection::keyless(format!(
                "anthropic:{DEFAULT_ANTHROPIC_MODEL}"
            )))
        );
        assert_eq!(answers.judge_enabled, Some(false));
        assert_eq!(answers.floor, None);
        assert_eq!(answers.markdown, Some(false));
        assert!(!answers.scaffold_rules);
    }

    #[test]
    fn ask_seeds_defaults_from_legacy_judge_model() {
        // A pre-`ai` config: judge.model seeds the catalog, so accepting
        // every default migrates it into the ai section.
        let base = json!({"judge": {"enabled": true, "model": "anthropic:claude-x", "floor": 0.7}, "markdown": true});
        let mut p = prompter("");
        let answers = ask(&base, None, false, &mut p).unwrap();
        assert_eq!(answers.ai, Some(AiSelection::keyless("anthropic:claude-x")));
        assert_eq!(answers.judge_enabled, Some(true));
        assert_eq!(answers.floor, Some(0.7));
        assert_eq!(answers.markdown, Some(true));
    }

    #[test]
    fn ask_preserves_custom_local_judge_model_on_defaults() {
        // Regression: a legacy judge.model naming a custom local repo must
        // survive accepting every default — the repo prompt is seeded with
        // it, so migration into `ai` keeps the spec verbatim (and records
        // the acknowledgment the Enter-through passed).
        let base = json!({"judge": {"enabled": true, "model": "local:me/custom-qwen-GGUF"}});
        let mut p = prompter("");
        let answers = ask(&base, None, false, &mut p).unwrap();
        assert_eq!(
            answers.ai,
            Some(AiSelection::acknowledged_local("local:me/custom-qwen-GGUF"))
        );
        assert_eq!(answers.judge_enabled, Some(true));
    }

    #[test]
    fn ask_ai_override_skips_the_catalog() {
        // Only the judge/markdown/rules prompts consume input.
        let mut p = prompter("y\n0.8\n\n\n");
        let answers = ask(
            &json!({}),
            Some(AiSelection::keyless("foundry:d")),
            false,
            &mut p,
        )
        .unwrap();
        assert_eq!(answers.ai, Some(AiSelection::keyless("foundry:d")));
        assert_eq!(answers.judge_enabled, Some(true));
        assert_eq!(answers.floor, Some(0.8));
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("from --ai"));
    }

    #[test]
    fn ask_clamps_out_of_range_seed_floor() {
        // Regression: an invalid floor in the seeding config must not become
        // the prompt default — on EOF it would re-feed the validation loop
        // forever. It falls back to the engine default instead.
        let base = json!({"judge": {"enabled": true, "floor": 5.0}});
        let mut p = prompter("");
        let answers = ask(&base, None, false, &mut p).unwrap();
        assert_eq!(answers.floor, None); // 0.6 == engine default, omitted
    }

    #[test]
    fn run_init_writes_config_and_respects_force() {
        let dir = std::env::temp_dir().join(format!("lawlint-init-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, false, None, false, None, &mut p).unwrap(),
            0
        );
        let written = fs::read_to_string(dir.join(".lawlint/config.json")).unwrap();
        let config: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            config,
            json!({
                "judge": {"enabled": false},
                "ai": {"model": format!("anthropic:{DEFAULT_ANTHROPIC_MODEL}")}
            })
        );

        // Existing config: refuse without --force, overwrite with it.
        let mut p = prompter("");
        assert!(run_init(&dir, true, false, None, false, None, &mut p)
            .unwrap_err()
            .contains("--force"));
        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, true, None, false, None, &mut p).unwrap(),
            0
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_init_ai_flag_sets_preference_non_interactively() {
        let dir = std::env::temp_dir().join(format!("lawlint-init-ai-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, false, Some("gemma"), false, None, &mut p).unwrap(),
            0
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(
            config["ai"]["model"],
            json!(format!("local:{}", lawlint_judge::DEFAULT_GEMMA_REPO))
        );
        // Without --acknowledge-local the acknowledgment is not written —
        // the per-use notice keeps printing.
        assert!(config["ai"].get("localAcknowledged").is_none());

        // An invalid value errors before any file is touched.
        let mut p = prompter("");
        assert!(run_init(&dir, true, true, Some("nope"), false, None, &mut p).is_err());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_init_acknowledge_local_flag_non_interactive() {
        let dir = std::env::temp_dir().join(format!("lawlint-init-ack-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // CI path: --yes --ai qwen --acknowledge-local writes the local
        // model with the acknowledgment persisted.
        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, false, Some("qwen"), true, None, &mut p).unwrap(),
            0
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(
            config["ai"],
            json!({"model": "local", "localAcknowledged": true})
        );

        // Acknowledging an existing local config without re-answering:
        // --yes --force --acknowledge-local (no --ai) attaches the
        // acknowledgment to the base config's local model.
        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, true, None, true, None, &mut p).unwrap(),
            0
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(
            config["ai"],
            json!({"model": "local", "localAcknowledged": true})
        );

        // The flag never invents an acknowledgment for hosted selections.
        let mut p = prompter("");
        assert_eq!(
            run_init(
                &dir,
                true,
                true,
                Some("anthropic:claude-x"),
                true,
                None,
                &mut p
            )
            .unwrap(),
            0
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["ai"]["model"], json!("anthropic:claude-x"));
        // The earlier acknowledgment survives as pass-through state, but no
        // new write happens for hosted models: drop it from the base first
        // to observe that.
        fs::write(
            dir.join(".lawlint/config.json"),
            r#"{"ai": {"model": "foundry:d"}}"#,
        )
        .unwrap();
        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, true, None, true, None, &mut p).unwrap(),
            0
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["ai"], json!({"model": "foundry:d"}));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_init_stores_keys_outside_the_project() {
        let dir = std::env::temp_dir().join(format!("lawlint-init-keys-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let credentials = dir.join("user-level").join("credentials");

        // Catalog choice 1 (Anthropic, preselected), default model, a key,
        // then judge no / markdown no / rules no.
        let mut p = prompter("1\n\nsk-ant-secret\n\n\n\n");
        assert_eq!(
            run_init(&dir, false, false, None, false, Some(&credentials), &mut p).unwrap(),
            0
        );

        // The key is in the credential file, with owner-only permissions…
        assert_eq!(
            lawlint_judge::credentials::lookup_at(&credentials, "ANTHROPIC_API_KEY").unwrap(),
            "sk-ant-secret"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&credentials).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // …and nowhere in the project config, which only names the model.
        let written = fs::read_to_string(dir.join(".lawlint/config.json")).unwrap();
        assert!(!written.contains("sk-ant-secret"));
        let config: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            config["ai"]["model"],
            json!(format!("anthropic:{DEFAULT_ANTHROPIC_MODEL}"))
        );
        // The transcript says where the key went.
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("outside your project"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_init_scaffolds_rules_package_without_clobbering() {
        let dir = std::env::temp_dir().join(format!("lawlint-init-pkg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Defaults (Anthropic catalog entry, model, key skip, judge,
        // markdown) except the final rules-package prompt.
        let mut p = prompter("\n\n\n\n\ny\n");
        assert_eq!(
            run_init(&dir, false, false, None, false, None, &mut p).unwrap(),
            0
        );
        let manifest_path = dir.join(RULES_DIR).join("style.yaml");
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest.starts_with("name: lawlint-init-pkg"));
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["ruleDirs"], json!([RULES_DIR]));

        // Re-run with --force: user edits to the package survive.
        fs::write(&manifest_path, "name: edited\nversion: 9.9.9\n").unwrap();
        let mut p = prompter("\n\n\n\n\ny\n");
        assert_eq!(
            run_init(&dir, false, true, None, false, None, &mut p).unwrap(),
            0
        );
        assert!(fs::read_to_string(&manifest_path)
            .unwrap()
            .starts_with("name: edited"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_init_force_preserves_existing_nested_config() {
        // Regression: a forced re-init must seed from the existing
        // .lawlint/config.json, not regenerate from scratch.
        let dir = std::env::temp_dir().join(format!("lawlint-init-renit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".lawlint")).unwrap();
        fs::write(
            dir.join(".lawlint/config.json"),
            r#"{"disable": ["core/no-semicolons"], "severity": {"no-hedging": "error"}, "judge": {"enabled": true, "floor": 0.8}}"#,
        )
        .unwrap();

        // --yes: everything preserved verbatim.
        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, true, None, false, None, &mut p).unwrap(),
            0
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["disable"], json!(["core/no-semicolons"]));
        assert_eq!(config["severity"], json!({"no-hedging": "error"}));
        assert_eq!(config["judge"], json!({"enabled": true, "floor": 0.8}));

        // Interactive, accepting every default: uncovered keys survive and
        // the prompts re-seed from the existing values (judge stays enabled
        // with its 0.8 floor).
        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, false, true, None, false, None, &mut p).unwrap(),
            0
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["disable"], json!(["core/no-semicolons"]));
        assert_eq!(config["severity"], json!({"no-hedging": "error"}));
        assert_eq!(config["judge"], json!({"enabled": true, "floor": 0.8}));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_init_leaves_unparseable_legacy_untouched() {
        // Regression: an invalid legacy file is promised "left untouched" —
        // the removal offer (whose EOF default is yes) must not fire.
        let dir = std::env::temp_dir().join(format!("lawlint-init-broken-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("lawlint.config.json"), "{not json").unwrap();

        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, false, false, None, false, None, &mut p).unwrap(),
            0
        );
        assert_eq!(
            fs::read_to_string(dir.join("lawlint.config.json")).unwrap(),
            "{not json"
        );
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("left untouched"));
        assert!(!transcript.contains("Remove the old"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_init_migrates_legacy_config() {
        let dir = std::env::temp_dir().join(format!("lawlint-init-legacy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("lawlint.config.json"),
            r#"{"disable": ["core/no-semicolons"], "judge": {"enabled": true}}"#,
        )
        .unwrap();

        // --yes: settings carried over verbatim, legacy file kept.
        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, false, None, false, None, &mut p).unwrap(),
            0
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["disable"], json!(["core/no-semicolons"]));
        assert_eq!(config["judge"], json!({"enabled": true}));
        assert!(dir.join("lawlint.config.json").is_file());

        // Interactive + --force: EOF accepts defaults, including removing
        // the legacy file (confirm defaults to yes).
        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, false, true, None, false, None, &mut p).unwrap(),
            0
        );
        assert!(!dir.join("lawlint.config.json").exists());

        fs::remove_dir_all(&dir).unwrap();
    }
}
