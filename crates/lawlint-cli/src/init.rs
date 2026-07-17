//! `lawlint init` — project setup. Creates `.lawlint/config.json` (and
//! optionally a `.lawlint/rules/` package) in the current directory via an
//! interactive walkthrough.
//!
//! Prompts read stdin line by line; an empty line or EOF accepts the shown
//! default, so a piped or CI invocation degrades to defaults instead of
//! hanging. `--yes` skips the prompts entirely. An existing legacy
//! `lawlint.config.json` seeds the prompt defaults and its settings are
//! carried into the new file.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

use colored::Colorize;
use serde_json::{json, Value};

const DEFAULT_FLOOR: f64 = 0.6;
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5-20251001";
const DEFAULT_OPENAI_BASE_URL: &str = "http://localhost:11434/v1";
const DEFAULT_OPENAI_MODEL: &str = "llama3.2";
const RULES_DIR: &str = ".lawlint/rules";

// ---- answers -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum JudgeChoice {
    Disabled,
    /// `None` = the default local repo (omit `model` from the config).
    Local(Option<String>),
    Anthropic(String),
    OpenAi {
        base_url: String,
        model: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct Answers {
    /// `None` = leave whatever the base config already says untouched.
    judge: Option<JudgeChoice>,
    /// Confidence floor; only written when it differs from the engine
    /// default. Meaningless unless the judge is enabled.
    floor: Option<f64>,
    /// `None` = leave the base config's `markdown` untouched.
    markdown: Option<bool>,
    scaffold_rules: bool,
}

impl Answers {
    /// `--yes` defaults: preserve everything a legacy config already says;
    /// on a fresh project, write an explicit `judge.enabled: false` so the
    /// opt-in is discoverable.
    fn accept_defaults(base: &Value) -> Self {
        Answers {
            judge: if base.get("judge").is_some() {
                None
            } else {
                Some(JudgeChoice::Disabled)
            },
            floor: None,
            markdown: None,
            scaffold_rules: false,
        }
    }
}

// ---- config assembly ---------------------------------------------------

fn judge_value(choice: &JudgeChoice, floor: Option<f64>) -> Value {
    let mut judge = serde_json::Map::new();
    let model = match choice {
        JudgeChoice::Disabled => {
            judge.insert("enabled".into(), json!(false));
            return Value::Object(judge);
        }
        JudgeChoice::Local(None) => None,
        JudgeChoice::Local(Some(repo)) => Some(format!("local:{repo}")),
        JudgeChoice::Anthropic(model) => Some(format!("anthropic:{model}")),
        JudgeChoice::OpenAi { base_url, model } => Some(format!("openai:{base_url}#{model}")),
    };
    judge.insert("enabled".into(), json!(true));
    if let Some(model) = model {
        judge.insert("model".into(), json!(model));
    }
    if let Some(floor) = floor {
        judge.insert("floor".into(), json!(floor));
    }
    Value::Object(judge)
}

/// Apply `answers` over `base` (the legacy config, or `{}`). Keys the
/// walkthrough does not cover (enable/disable/severity/thresholds/…) pass
/// through untouched.
fn build_config(base: &Value, answers: &Answers) -> Value {
    let mut config = match base {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    if let Some(choice) = &answers.judge {
        config.insert("judge".into(), judge_value(choice, answers.floor));
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

/// The interactive walkthrough. `base` (legacy config or `{}`) seeds the
/// defaults so re-running init preserves earlier choices by default.
fn ask<R: BufRead, W: Write>(
    base: &Value,
    prompter: &mut Prompter<R, W>,
) -> Result<Answers, String> {
    let base_judge = base.get("judge");
    let base_enabled = base_judge
        .and_then(|judge| judge.get("enabled"))
        .and_then(Value::as_bool)
        == Some(true);
    let base_model = base_judge
        .and_then(|judge| judge.get("model"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let default_choice = if !base_enabled {
        0
    } else if base_model.starts_with("anthropic:") {
        2
    } else if base_model.starts_with("openai:") {
        3
    } else {
        1
    };
    let choice = prompter.choice(
        "Enable the tier-3 AI judge? It powers the inferential (semantic) rules; \
         tiers 1-2 always run.",
        &[
            "No — pattern and density rules only (enable later with --judge)",
            "Yes, local model — downloaded on first run, fully offline after",
            "Yes, Anthropic API — needs ANTHROPIC_API_KEY at lint time",
            "Yes, OpenAI-compatible endpoint (Ollama, vLLM, llama.cpp, …)",
        ],
        default_choice,
    )?;

    let judge = match choice {
        0 => JudgeChoice::Disabled,
        1 => {
            let default_repo = base_model
                .strip_prefix("local:")
                .filter(|repo| !repo.is_empty())
                .unwrap_or(lawlint_judge::DEFAULT_LOCAL_REPO);
            let repo = prompter.line("Hugging Face GGUF repo (repo[#file])", default_repo)?;
            if repo == lawlint_judge::DEFAULT_LOCAL_REPO {
                JudgeChoice::Local(None)
            } else {
                JudgeChoice::Local(Some(repo))
            }
        }
        2 => {
            let default_model = base_model
                .strip_prefix("anthropic:")
                .filter(|model| !model.is_empty())
                .unwrap_or(DEFAULT_ANTHROPIC_MODEL);
            JudgeChoice::Anthropic(prompter.line("Anthropic model", default_model)?)
        }
        _ => {
            let (base_url, model) = base_model
                .strip_prefix("openai:")
                .and_then(|spec| spec.rsplit_once('#'))
                .unwrap_or((DEFAULT_OPENAI_BASE_URL, DEFAULT_OPENAI_MODEL));
            JudgeChoice::OpenAi {
                base_url: prompter.line("Base URL", base_url)?,
                model: prompter.line("Model", model)?,
            }
        }
    };

    let floor = if judge == JudgeChoice::Disabled {
        None
    } else {
        let base_floor = base_judge
            .and_then(|judge| judge.get("floor"))
            .and_then(Value::as_f64)
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
        judge: Some(judge),
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

fn run_init<R: BufRead, W: Write>(
    directory: &Path,
    yes: bool,
    force: bool,
    prompter: &mut Prompter<R, W>,
) -> Result<i32, String> {
    let lawlint_dir = directory.join(".lawlint");
    let config_path = lawlint_dir.join("config.json");
    if config_path.exists() && !force {
        return Err(format!(
            "{} already exists; rerun with --force to overwrite",
            config_path.display()
        ));
    }

    let legacy_path = directory.join("lawlint.config.json");
    let base = if legacy_path.is_file() {
        let text = fs::read_to_string(&legacy_path)
            .map_err(|error| format!("failed to read {}: {error}", legacy_path.display()))?;
        match serde_json::from_str::<Value>(&text) {
            Ok(value @ Value::Object(_)) => {
                prompter.say(&format!(
                    "Found lawlint.config.json — its settings seed the defaults below and \
                     carry over to {}.",
                    ".lawlint/config.json".bold()
                ))?;
                value
            }
            Ok(_) | Err(_) => {
                prompter.say(
                    "Found lawlint.config.json but it is not valid JSON; starting fresh \
                     (the old file is left untouched).",
                )?;
                json!({})
            }
        }
    } else {
        json!({})
    };
    let has_legacy = legacy_path.is_file();

    let answers = if yes {
        Answers::accept_defaults(&base)
    } else {
        prompter.say(&format!(
            "{}\n",
            "lawlint init — sets up .lawlint/config.json for this project.".bold()
        ))?;
        ask(&base, prompter)?
    };

    let config = build_config(&base, &answers);
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

    // The new file shadows the legacy one (same-directory precedence), so
    // offer to remove it; under --yes never delete, just explain.
    if has_legacy {
        let remove = !yes
            && prompter.confirm(
                "Remove the old lawlint.config.json? (its settings were carried over; \
                 .lawlint/config.json now takes precedence)",
                true,
            )?;
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
    for file in &created {
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

pub fn init_command(yes: bool, force: bool) -> Result<i32, String> {
    let directory = std::env::current_dir().map_err(|error| error.to_string())?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut prompter = Prompter {
        input: stdin.lock(),
        output: stdout.lock(),
    };
    run_init(&directory, yes, force, &mut prompter)
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
        let answers = Answers::accept_defaults(&json!({}));
        let config = build_config(&json!({}), &answers);
        assert_eq!(config, json!({"judge": {"enabled": false}}));
    }

    #[test]
    fn accept_defaults_preserves_legacy_judge() {
        let base = json!({"judge": {"enabled": true, "model": "anthropic:m", "floor": 0.8}});
        let answers = Answers::accept_defaults(&base);
        assert_eq!(answers.judge, None);
        let config = build_config(&base, &answers);
        assert_eq!(config["judge"], base["judge"]);
    }

    #[test]
    fn build_config_overwrites_covered_keys_and_passes_through_rest() {
        let base = json!({
            "disable": ["core/no-semicolons"],
            "markdown": true,
            "judge": {"enabled": false},
            "ruleDirs": ["extra"]
        });
        let answers = Answers {
            judge: Some(JudgeChoice::Local(None)),
            floor: Some(0.8),
            markdown: Some(false),
            scaffold_rules: true,
        };
        let config = build_config(&base, &answers);
        assert_eq!(config["disable"], json!(["core/no-semicolons"]));
        assert!(config.get("markdown").is_none());
        assert_eq!(config["judge"], json!({"enabled": true, "floor": 0.8}));
        // Existing dirs preserved, ours appended once.
        assert_eq!(config["ruleDirs"], json!(["extra", RULES_DIR]));
        let again = build_config(&config, &answers);
        assert_eq!(again["ruleDirs"], json!(["extra", RULES_DIR]));
    }

    #[test]
    fn judge_value_model_specs() {
        assert_eq!(
            judge_value(&JudgeChoice::Disabled, None),
            json!({"enabled": false})
        );
        assert_eq!(
            judge_value(&JudgeChoice::Local(Some("foo/bar#q4.gguf".into())), None),
            json!({"enabled": true, "model": "local:foo/bar#q4.gguf"})
        );
        assert_eq!(
            judge_value(&JudgeChoice::Anthropic("m".into()), Some(0.7)),
            json!({"enabled": true, "model": "anthropic:m", "floor": 0.7})
        );
        assert_eq!(
            judge_value(
                &JudgeChoice::OpenAi {
                    base_url: "http://x/v1".into(),
                    model: "m".into()
                },
                None
            ),
            json!({"enabled": true, "model": "openai:http://x/v1#m"})
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
    fn ask_full_local_flow() {
        // choice 2 (local), custom repo, floor 0.8, markdown y, rules y.
        let mut p = prompter("2\nfoo/bar-GGUF\n0.8\ny\ny\n");
        let answers = ask(&json!({}), &mut p).unwrap();
        assert_eq!(
            answers.judge,
            Some(JudgeChoice::Local(Some("foo/bar-GGUF".into())))
        );
        assert_eq!(answers.floor, Some(0.8));
        assert_eq!(answers.markdown, Some(true));
        assert!(answers.scaffold_rules);
    }

    #[test]
    fn ask_default_flow_is_disabled_judge() {
        let mut p = prompter("");
        let answers = ask(&json!({}), &mut p).unwrap();
        assert_eq!(answers.judge, Some(JudgeChoice::Disabled));
        assert_eq!(answers.floor, None);
        assert_eq!(answers.markdown, Some(false));
        assert!(!answers.scaffold_rules);
    }

    #[test]
    fn ask_seeds_defaults_from_legacy_config() {
        let base = json!({"judge": {"enabled": true, "model": "anthropic:claude-x", "floor": 0.7}, "markdown": true});
        // Accept every default: choice → anthropic, model claude-x, floor 0.7,
        // markdown stays on.
        let mut p = prompter("");
        let answers = ask(&base, &mut p).unwrap();
        assert_eq!(
            answers.judge,
            Some(JudgeChoice::Anthropic("claude-x".into()))
        );
        assert_eq!(answers.floor, Some(0.7));
        assert_eq!(answers.markdown, Some(true));
    }

    #[test]
    fn ask_accepting_default_local_repo_omits_model() {
        let mut p = prompter("2\n\n\n\n\n");
        let answers = ask(&json!({}), &mut p).unwrap();
        assert_eq!(answers.judge, Some(JudgeChoice::Local(None)));
    }

    #[test]
    fn run_init_writes_config_and_respects_force() {
        let dir = std::env::temp_dir().join(format!("lawlint-init-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut p = prompter("");
        assert_eq!(run_init(&dir, true, false, &mut p).unwrap(), 0);
        let written = fs::read_to_string(dir.join(".lawlint/config.json")).unwrap();
        let config: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(config, json!({"judge": {"enabled": false}}));

        // Existing config: refuse without --force, overwrite with it.
        let mut p = prompter("");
        assert!(run_init(&dir, true, false, &mut p)
            .unwrap_err()
            .contains("--force"));
        let mut p = prompter("");
        assert_eq!(run_init(&dir, true, true, &mut p).unwrap(), 0);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_init_scaffolds_rules_package_without_clobbering() {
        let dir = std::env::temp_dir().join(format!("lawlint-init-pkg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Defaults except the final rules-package prompt.
        let mut p = prompter("\n\ny\n");
        assert_eq!(run_init(&dir, false, false, &mut p).unwrap(), 0);
        let manifest_path = dir.join(RULES_DIR).join("style.yaml");
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest.starts_with("name: lawlint-init-pkg"));
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["ruleDirs"], json!([RULES_DIR]));

        // Re-run with --force: user edits to the package survive.
        fs::write(&manifest_path, "name: edited\nversion: 9.9.9\n").unwrap();
        let mut p = prompter("\n\ny\n");
        assert_eq!(run_init(&dir, false, true, &mut p).unwrap(), 0);
        assert!(fs::read_to_string(&manifest_path)
            .unwrap()
            .starts_with("name: edited"));

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
        assert_eq!(run_init(&dir, true, false, &mut p).unwrap(), 0);
        let config: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".lawlint/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["disable"], json!(["core/no-semicolons"]));
        assert_eq!(config["judge"], json!({"enabled": true}));
        assert!(dir.join("lawlint.config.json").is_file());

        // Interactive + --force: EOF accepts defaults, including removing
        // the legacy file (confirm defaults to yes).
        let mut p = prompter("");
        assert_eq!(run_init(&dir, false, true, &mut p).unwrap(), 0);
        assert!(!dir.join("lawlint.config.json").exists());

        fs::remove_dir_all(&dir).unwrap();
    }
}
