//! `lawlint init` — project setup. Creates `.lawlint/config.json` (and
//! optionally a `.lawlint/rules/` package) in the current directory via an
//! interactive walkthrough.
//!
//! The walkthrough opens with the AI-model catalog: Anthropic preselected,
//! then OpenAI and Azure Foundry, then a self-hosted OpenAI-compatible
//! endpoint. Every entry says where text goes ("text is sent to the
//! provider" vs. "text goes to that server"), so consent is given once,
//! informed, here. The self-hosted entry is how lawlint runs without text
//! leaving the machine — it points at Ollama, vLLM or llama.cpp and needs no
//! API key. The selection lands in the config's `ai` section; hosted API keys
//! go to the user-level credential store (`~/.lawlint/credentials`, 0600) —
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

/// The four catalog entries, in display order. Every entry says where text
/// goes. The last is the private path: a server the user runs.
pub(crate) const AI_CATALOG_OPTIONS: [&str; 4] = [
    "Claude (Anthropic — hosted, recommended; text is sent to the provider, \
     requires API key)",
    "GPT (OpenAI — hosted; text is sent to the provider, requires API key)",
    "Azure Foundry (hosted — text is sent to the provider, requires API key + \
     endpoint)",
    "Self-hosted OpenAI-compatible endpoint (Ollama, vLLM, llama.cpp, … — text \
     goes only to that server, no API key needed)",
];

// ---- answers -----------------------------------------------------------

/// One catalog selection: the `ai.model` spec plus any credentials to store,
/// keyed by the provider's environment-variable name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AiSelection {
    pub(crate) model: String,
    pub(crate) keys: Vec<(String, String)>,
}

impl AiSelection {
    pub(crate) fn keyless(model: impl Into<String>) -> Self {
        AiSelection {
            model: model.into(),
            keys: Vec::new(),
        }
    }
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

/// `--ai` takes a full model spec. A `local:` spec names the removed
/// in-process backend, so it is rejected here with the same migration
/// guidance the judge gives rather than being accepted and failing later.
pub(crate) fn parse_ai_flag(value: &str) -> Result<AiSelection, String> {
    let model = match value {
        "qwen" | "gemma" | "local" => {
            return Err(format!("--ai {value:?}: {LOCAL_REMOVED_HINT}"));
        }
        spec if spec.starts_with("local:") => {
            return Err(format!("--ai {spec:?}: {LOCAL_REMOVED_HINT}"));
        }
        spec if spec.starts_with("anthropic:") || spec.starts_with("foundry:") => spec.to_string(),
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
                "--ai {other:?}: use a model spec (anthropic:<model>, \
                 openai:<base-url>#<model>, foundry:<deployment>)"
            ));
        }
    };
    Ok(AiSelection::keyless(model))
}

/// Shared wording for a `--ai` value naming the removed local backend. Leads
/// with the replacement that keeps text on the machine, since that is what
/// the user was after.
pub(crate) const LOCAL_REMOVED_HINT: &str =
    "in-process local models were removed in 0.9 (they could not judge these \
     rules — docs/eval-corpus.md). Run an OpenAI-compatible server such as \
     Ollama and use \"openai:http://localhost:11434/v1#<model>\" to keep text \
     on your machine";

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
        // A config carried over from before 0.9 may still hold it; the field
        // no longer means anything, so stop propagating it.
        ai.remove("localAcknowledged");
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
/// config), for the prompt default. A fresh project (empty spec) preselects
/// Anthropic. A stale `local:` spec has no entry to map to, so it also lands
/// on the recommended one — the migration is a re-pick, not a rename.
pub(crate) fn default_catalog_index(model: &str) -> usize {
    if let Some(spec) = model.strip_prefix("openai:") {
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
    })
}

/// The model catalog with Anthropic preselected. Every entry says where text
/// goes; hosted entries prompt for the provider API key. The last entry is a
/// server the user runs, which is how text stays on the machine.
fn ask_ai<R: BufRead, W: Write>(
    base_model: &str,
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
            })
        }
        _ => {
            let (base_url, model) = base_model
                .strip_prefix("openai:")
                .and_then(|spec| spec.rsplit_once('#'))
                .unwrap_or((DEFAULT_COMPAT_BASE_URL, DEFAULT_COMPAT_MODEL));
            prompter.say(
                "  Text goes only to this server. Point it at Ollama, vLLM or \
                 llama.cpp; no API key is needed.",
            )?;
            let base_url = prompter.line("Base URL", base_url)?;
            let model = prompter.line("Model", model)?;
            Ok(AiSelection::keyless(format!("openai:{base_url}#{model}")))
        }
    }
}

/// The interactive walkthrough. `base` (the existing or legacy config, or
/// `{}`) seeds the defaults so re-running init preserves earlier choices
/// by default. A legacy `judge.model` seeds the catalog too, migrating it
/// into the `ai` section.
fn ask<R: BufRead, W: Write>(
    base: &Value,
    ai_override: Option<AiSelection>,
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

    let ai = match ai_override {
        Some(selection) => {
            prompter.say(&format!("AI model: {} (from --ai).", selection.model))?;
            selection
        }
        None => ask_ai(base_ai_model, prompter)?,
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

const SAMPLE_RULE: &str = r#"---
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
---

# Example project rule. Check it with: lawlint rules test .lawlint/rules
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
    let sample = rules.join("no-avoidance-of-doubt.md");
    if !sample.exists() {
        fs::write(&sample, SAMPLE_RULE)
            .map_err(|error| format!("failed to write {}: {error}", sample.display()))?;
        created.push(format!("{RULES_DIR}/rules/no-avoidance-of-doubt.md"));
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
/// Everything `init` may touch *outside* the project directory.
///
/// `Default` is inert — no explicit credential path, no migration — so a
/// caller that says nothing changes nothing outside the project. Only the real
/// entry points fill it in. The inverse (discovering `$HOME` internally) let a
/// unit test move a developer's own credential file once already.
#[derive(Default)]
pub(crate) struct UserScope<'a> {
    /// Write credentials here instead of the resolved default.
    pub credentials: Option<&'a Path>,
    /// `(legacy, current)` directories for the one-time 0.8 migration.
    pub dirs: Option<(&'a Path, &'a Path)>,
}

/// The real user-level directories, `(legacy, current)`, resolved from the
/// environment. Only the interactive entry points call this: everything else
/// takes the directories as arguments so it cannot touch a developer's home.
/// `None` when `$LAWLINT_CREDENTIALS` pins the file somewhere explicit, or no
/// home directory can be determined — in both cases there is nothing to
/// migrate.
pub(crate) fn real_user_dirs() -> Option<(PathBuf, PathBuf)> {
    if std::env::var_os("LAWLINT_CREDENTIALS").is_some() {
        return None;
    }
    let legacy = lawlint_judge::credentials::legacy_config_home()?;
    let current = lawlint_judge::credentials::config_home()?;
    Some((legacy, current))
}

pub(crate) fn apply_answers(
    directory: &Path,
    base: &Value,
    answers: &Answers,
    scope: &UserScope<'_>,
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
            let (path, moved_from) = match scope.credentials {
                Some(path) => {
                    lawlint_judge::credentials::store_at(path, &selection.keys)?;
                    (path.to_path_buf(), None)
                }
                None => lawlint_judge::credentials::store(&selection.keys)?,
            };
            let mut message = format!(
                "Stored {} in {} (outside your project, owner-only permissions).",
                selection
                    .keys
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                path.display()
            );
            // Relocating a file of API keys is not something to do quietly.
            if let Some(from) = moved_from {
                message.push_str(&format!(
                    "\nMoved your existing credentials here from {}.",
                    from.display()
                ));
            }
            Some(message)
        }
        _ => None,
    };

    // `init` is the one user-initiated setup moment, so it is where pre-0.8
    // user-level files get tidied into ~/.lawlint. Everywhere else the legacy
    // path stays readable rather than being moved out from under a running
    // command. Runs whether or not a key was entered this time: someone
    // upgrading and re-running init should end up migrated either way.
    //
    // `user_dirs` is None for every caller but the real CLI entry point, so a
    // test that forgets to isolate cannot move files in the developer's home.
    let mut credential_message = credential_message;
    if let Some((legacy_dir, new_dir)) = scope.dirs {
        let moved: Vec<String> = ["credentials", "config.json"]
            .into_iter()
            .filter_map(|name| {
                lawlint_judge::credentials::migrate_between(legacy_dir, new_dir, name)
                    .map(|(from, to)| format!("Moved {} to {}.", from.display(), to.display()))
            })
            .collect();
        if !moved.is_empty() {
            let note = moved.join("\n");
            credential_message = Some(match credential_message {
                Some(existing) => format!("{existing}\n{note}"),
                None => note,
            });
        }
    }

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
    scope: &UserScope<'_>,
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

    let answers = if yes {
        Answers::accept_defaults(&base, ai_override)
    } else {
        prompter.say(&format!(
            "{}\n",
            "lawlint init — sets up .lawlint/config.json for this project.".bold()
        ))?;
        ask(&base, ai_override, prompter)?
    };

    let applied = apply_answers(directory, &base, &answers, scope)?;
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

pub fn init_command(yes: bool, force: bool, ai: Option<&str>) -> Result<i32, String> {
    let directory = std::env::current_dir().map_err(|error| error.to_string())?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut prompter = Prompter {
        input: stdin.lock(),
        output: stdout.lock(),
    };
    let dirs = real_user_dirs();
    let scope = UserScope {
        credentials: None,
        dirs: dirs.as_ref().map(|(a, b)| (a.as_path(), b.as_path())),
    };
    run_init(&directory, yes, force, ai, &scope, &mut prompter)
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
        // The removed local shorthands must not silently resolve to anything.
        for stale in ["qwen", "gemma", "local", "local:foo/bar"] {
            let err = parse_ai_flag(stale).unwrap_err();
            assert!(err.contains("openai:http://localhost:11434/v1"), "{err}");
        }
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
        assert!(parse_ai_flag("gpt4")
            .unwrap_err()
            .contains("anthropic:<model>"));
        // No key entries ever come from the flag.
        assert!(parse_ai_flag("anthropic:m").unwrap().keys.is_empty());
    }

    #[test]
    fn default_catalog_index_maps_specs() {
        // Fresh (empty) and anthropic specs preselect the recommended entry.
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
        // A stale local spec has no entry of its own; it falls back to the
        // recommended one so re-running init is a clean re-pick.
        assert_eq!(default_catalog_index("local"), 0);
        assert_eq!(default_catalog_index("local:foo/bar-GGUF"), 0);
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
    fn ask_ai_hosted_choices_collect_keys_and_caveats() {
        // Anthropic (choice 1, preselected): default model, key stored
        // under its env-var name.
        let mut p = prompter("1\n\nsk-ant-secret\n");
        let selection = ask_ai("", &mut p).unwrap();
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
        // Hosted first and recommended; the private path is the self-hosted
        // endpoint, and it is the last entry.
        assert!(transcript.contains("Hosted providers are recommended"));
        assert!(transcript.contains("1) Claude (Anthropic"));
        assert!(transcript.contains("4) Self-hosted OpenAI-compatible endpoint"));
        assert!(!transcript.contains("Local model"));

        // OpenAI hosted: skipping the key stores nothing and points at the
        // env var instead.
        let mut p = prompter("2\n\n\n");
        let selection = ask_ai("", &mut p).unwrap();
        assert_eq!(
            selection.model,
            format!("openai:{OPENAI_HOSTED_BASE_URL}#{DEFAULT_OPENAI_HOSTED_MODEL}")
        );
        assert!(selection.keys.is_empty());
        let transcript = String::from_utf8(p.output).unwrap();
        assert!(transcript.contains("$OPENAI_API_KEY"));

        // Foundry: endpoint + key both land in the credential entries.
        let mut p = prompter("3\ngpt-5.5\nhttps://res.services.ai.azure.com\nfoundry-key\n");
        let selection = ask_ai("", &mut p).unwrap();
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
            ask_ai("", &mut p).unwrap().model,
            format!("openai:{DEFAULT_COMPAT_BASE_URL}#{DEFAULT_COMPAT_MODEL}")
        );
        // An existing compat spec seeds both prompts.
        let mut p = prompter("\n\n\n");
        assert_eq!(
            ask_ai("openai:http://gpu-box:8000/v1#qwen3", &mut p)
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
        let answers = ask(&json!({}), None, &mut p).unwrap();
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
        let answers = ask(&base, None, &mut p).unwrap();
        assert_eq!(answers.ai, Some(AiSelection::keyless("anthropic:claude-x")));
        assert_eq!(answers.judge_enabled, Some(true));
        assert_eq!(answers.floor, Some(0.7));
        assert_eq!(answers.markdown, Some(true));
    }

    #[test]
    fn ask_migrates_a_stale_local_model_to_the_recommended_backend() {
        // A config naming the removed in-process backend has nothing to
        // preserve: Enter-through must land on the recommended hosted entry
        // rather than carrying a spec that can no longer run. Everything else
        // about the config still survives.
        let base = json!({"judge": {"enabled": true, "model": "local:me/custom-qwen-GGUF"}});
        let mut p = prompter("");
        let answers = ask(&base, None, &mut p).unwrap();
        assert_eq!(
            answers.ai,
            Some(AiSelection::keyless(format!(
                "anthropic:{DEFAULT_ANTHROPIC_MODEL}"
            )))
        );
        assert_eq!(answers.judge_enabled, Some(true));
    }

    #[test]
    fn ask_ai_override_skips_the_catalog() {
        // Only the judge/markdown/rules prompts consume input.
        let mut p = prompter("y\n0.8\n\n\n");
        let answers = ask(&json!({}), Some(AiSelection::keyless("foundry:d")), &mut p).unwrap();
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
        let answers = ask(&base, None, &mut p).unwrap();
        assert_eq!(answers.floor, None); // 0.6 == engine default, omitted
    }

    #[test]
    fn run_init_writes_config_and_respects_force() {
        let dir = std::env::temp_dir().join(format!("lawlint-init-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, false, None, &UserScope::default(), &mut p).unwrap(),
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
        assert!(
            run_init(&dir, true, false, None, &UserScope::default(), &mut p)
                .unwrap_err()
                .contains("--force")
        );
        let mut p = prompter("");
        assert_eq!(
            run_init(&dir, true, true, None, &UserScope::default(), &mut p).unwrap(),
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
            run_init(
                &dir,
                true,
                false,
                Some("foundry:gpt-5.5"),
                &UserScope::default(),
                &mut p
            )
            .unwrap(),
            0
        );
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["ai"]["model"], json!("foundry:gpt-5.5"));

        // An invalid value errors before any file is touched.
        let mut p = prompter("");
        assert!(run_init(
            &dir,
            true,
            true,
            Some("nope"),
            &UserScope::default(),
            &mut p
        )
        .is_err());

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
            run_init(
                &dir,
                false,
                false,
                None,
                &UserScope {
                    credentials: Some(&credentials),
                    ..Default::default()
                },
                &mut p
            )
            .unwrap(),
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
            run_init(&dir, false, false, None, &UserScope::default(), &mut p).unwrap(),
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
            run_init(&dir, false, true, None, &UserScope::default(), &mut p).unwrap(),
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
            run_init(&dir, true, true, None, &UserScope::default(), &mut p).unwrap(),
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
            run_init(&dir, false, true, None, &UserScope::default(), &mut p).unwrap(),
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
            run_init(&dir, false, false, None, &UserScope::default(), &mut p).unwrap(),
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
            run_init(&dir, true, false, None, &UserScope::default(), &mut p).unwrap(),
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
            run_init(&dir, false, true, None, &UserScope::default(), &mut p).unwrap(),
            0
        );
        assert!(!dir.join("lawlint.config.json").exists());

        fs::remove_dir_all(&dir).unwrap();
    }
}
