//! The interactive setup wizard: a ratatui front-end for `lawlint init` that
//! shares the TUI's visual language (the Litvue oxblood palette, a scrolling
//! `◆`-bulleted transcript, arrow-key selection lists, a `§` input line).
//!
//! It is shown in two places (see `main.rs`):
//! - `lawlint init` in an interactive terminal — then the app launches the TUI.
//! - a bare `lawlint` when no project config is found — with a "skip" escape.
//!
//! The flow mirrors `init::ask` step-for-step (same wording, same defaults),
//! but event-driven instead of line-oriented. It collects an `init::Answers`
//! and funnels it through the same `init::apply_answers`, so the wizard and the
//! non-interactive line walkthrough always write identical config. The
//! line-based `Prompter` in `init.rs` is kept for `--yes`, `--ai`, piped
//! stdin, and CI.

use crate::init::{
    self, AiSelection, Answers, Seed, AI_CATALOG_OPTIONS, AI_CATALOG_PROMPT,
    DEFAULT_ANTHROPIC_MODEL, DEFAULT_COMPAT_BASE_URL, DEFAULT_COMPAT_MODEL, DEFAULT_FLOOR,
    DEFAULT_FOUNDRY_DEPLOYMENT, DEFAULT_OPENAI_HOSTED_MODEL, LOCAL_CHOICE_OPTIONS,
    LOCAL_CONSTRAINTS, OPENAI_HOSTED_BASE_URL,
};
use crate::ui::{
    bullet_line, pad_line, tilde, wrap_line, SelectList, BRAND, BRAND_DARK, DIM, GOOD, INPUT_BAR_BG,
};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Terminal;
use serde_json::Value;
use std::io::{self, IsTerminal, Stdout};
use std::path::PathBuf;

/// Why the wizard was launched — controls whether a "skip" escape is offered
/// and what non-interactive inputs (`--ai`, `--force`, `--acknowledge-local`)
/// carry over from the `init` command.
pub enum SetupContext {
    /// From `lawlint init` in an interactive terminal.
    Explicit {
        force: bool,
        ai: Option<String>,
        acknowledge_local: bool,
    },
    /// From bare `lawlint` with no project config found; offers a skip.
    FirstRun,
}

/// What the wizard did, so the caller knows whether to open the TUI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SetupOutcome {
    /// Config written — the caller launches the TUI (now configured).
    Completed,
    /// The user declined setup (FirstRun only) — the caller opens the TUI
    /// unconfigured.
    Skipped,
    /// The user aborted (`Ctrl+C`/`Esc`) — the caller should exit.
    Aborted,
}

/// Run the wizard on an alternate screen. Seed resolution happens before the
/// terminal is taken over, so an "already configured, no --force" error prints
/// normally instead of being swallowed by the alt-screen.
pub fn run_setup(context: SetupContext) -> Result<SetupOutcome, String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("setup requires an interactive terminal".into());
    }
    let directory = std::env::current_dir().map_err(|error| error.to_string())?;
    let (force, ai_flag, acknowledge_local, first_run) = match &context {
        SetupContext::Explicit {
            force,
            ai,
            acknowledge_local,
        } => (*force, ai.clone(), *acknowledge_local, false),
        SetupContext::FirstRun => (false, None, false, true),
    };
    let ai_override = ai_flag.as_deref().map(init::parse_ai_flag).transpose()?;
    let seed = init::resolve_seed(&directory, force)?;

    enable_raw_mode().map_err(|error| error.to_string())?;
    crossterm::execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)
        .map_err(|error| error.to_string())?;
    let _guard = TerminalGuard;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|error| error.to_string())?;

    // The real wizard launch is a user-initiated setup, so it is allowed to
    // migrate pre-0.8 user-level files.
    let wizard = Wizard::new(
        directory,
        seed,
        ai_override,
        acknowledge_local,
        first_run,
        init::real_user_dirs(),
    );
    wizard.run(&mut terminal)
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste);
    }
}

// ---- wizard state ------------------------------------------------------

/// The active step. Each is either a menu (a `SelectList`) or a single-line
/// input; `enter_stage` sets the matching `Mode`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Welcome, // first-run only: set up now / skip
    Catalog,
    AnthropicModel,
    AnthropicKey,
    OpenAiModel,
    OpenAiKey,
    FoundryDeployment,
    FoundryEndpoint,
    FoundryKey,
    CompatBaseUrl,
    CompatModel,
    LocalConfirm,
    LocalChoice,
    LocalRepo,
    JudgeEnabled,
    Floor,
    Markdown,
    Scaffold,
    LegacyRemove,
    Summary, // work done; any key dismisses
}

enum Mode {
    Menu(SelectList),
    /// A single-line field. Empty submit accepts `default`; `secret` masks the
    /// echo and treats empty as "skip".
    Input {
        default: String,
        secret: bool,
    },
}

struct Wizard {
    directory: PathBuf,
    seed: Seed,
    first_run: bool,
    acknowledge_local: bool,
    /// Pre-0.8 and current user-level directories, for the one-time migration.
    /// `None` — the default, and what every test gets — means `finish()` makes
    /// no change outside `directory`. Resolving `$HOME` inside `finish()`
    /// instead would let a wizard unit test move the developer's own
    /// credentials, which is exactly what happened once.
    user_dirs: Option<(PathBuf, PathBuf)>,

    // Seed-derived defaults (computed once, mirroring init::ask).
    base_model: String,
    ack_default: bool,
    base_judge_enabled: bool,
    base_floor: f64,
    base_markdown: bool,

    // Accumulating answers.
    ai: Option<AiSelection>,
    judge_enabled: Option<bool>,
    floor: Option<f64>,
    markdown: Option<bool>,
    scaffold_rules: bool,
    remove_legacy: Option<bool>,

    // In-flight AI selection built across sub-steps.
    pending_model: String,
    pending_keys: Vec<(String, String)>,
    pending_local_gemma: bool,

    transcript: Vec<TranscriptLine>,
    scroll_up: usize,
    stage: Stage,
    mode: Mode,
    input: String,
    error: Option<String>,
    outcome: SetupOutcome,
    awaiting_dismiss: bool,
}

/// A transcript line plus an optional full-width fill (for the echoed-answer
/// bar), matching the TUI's input-bar treatment.
struct TranscriptLine {
    line: Line<'static>,
    fill: Option<Style>,
}

impl Wizard {
    fn new(
        directory: PathBuf,
        seed: Seed,
        ai_override: Option<AiSelection>,
        acknowledge_local: bool,
        first_run: bool,
        user_dirs: Option<(PathBuf, PathBuf)>,
    ) -> Self {
        let base = &seed.base;
        let base_judge = base.get("judge");
        let base_model = base
            .get("ai")
            .and_then(|ai| ai.get("model"))
            .and_then(Value::as_str)
            .or_else(|| {
                base_judge
                    .and_then(|judge| judge.get("model"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("")
            .to_string();
        let ack_default = acknowledge_local
            || init::is_local_spec(&base_model)
            || base
                .get("ai")
                .and_then(|ai| ai.get("localAcknowledged"))
                .and_then(Value::as_bool)
                == Some(true);
        let base_judge_enabled = base_judge
            .and_then(|judge| judge.get("enabled"))
            .and_then(Value::as_bool)
            == Some(true);
        let base_floor = base_judge
            .and_then(|judge| judge.get("floor"))
            .and_then(Value::as_f64)
            .filter(|floor| (0.0..=1.0).contains(floor))
            .unwrap_or(DEFAULT_FLOOR);
        let base_markdown = base.get("markdown").and_then(Value::as_bool) == Some(true);

        let mut wizard = Wizard {
            directory,
            seed,
            first_run,
            acknowledge_local,
            user_dirs,
            base_model,
            ack_default,
            base_judge_enabled,
            base_floor,
            base_markdown,
            ai: None,
            judge_enabled: None,
            floor: None,
            markdown: None,
            scaffold_rules: false,
            remove_legacy: None,
            pending_model: String::new(),
            pending_keys: Vec::new(),
            pending_local_gemma: false,
            transcript: Vec::new(),
            scroll_up: 0,
            stage: Stage::Welcome,
            mode: Mode::Input {
                default: String::new(),
                secret: false,
            },
            input: String::new(),
            error: None,
            outcome: SetupOutcome::Completed,
            awaiting_dismiss: false,
        };

        // Choose the first step: an `--ai` override skips the whole catalog;
        // a first-run offers the skip; otherwise start at the catalog.
        if let Some(selection) = ai_override {
            wizard.push_note(&format!("AI model: {} (from --ai).", selection.model));
            wizard.ai = Some(selection);
            wizard.enter_stage(Stage::JudgeEnabled);
        } else if first_run {
            wizard.enter_stage(Stage::Welcome);
        } else {
            wizard.enter_stage(Stage::Catalog);
        }
        wizard
    }

    fn run(
        mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<SetupOutcome, String> {
        loop {
            terminal
                .draw(|f| self.draw(f))
                .map_err(|error| error.to_string())?;

            match event::read().map_err(|error| error.to_string())? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if should_abort(&key) {
                        return Ok(SetupOutcome::Aborted);
                    }
                    if let Some(outcome) = self.handle_key(key)? {
                        return Ok(outcome);
                    }
                }
                Event::Paste(text)
                    if matches!(self.mode, Mode::Input { .. }) && !self.awaiting_dismiss =>
                {
                    self.input.push_str(&text.replace(['\n', '\r'], " "));
                }
                _ => {}
            }
        }
    }

    /// Dispatch a keypress. `Ok(Some(outcome))` ends the wizard.
    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<SetupOutcome>, String> {
        if self.awaiting_dismiss {
            return Ok(Some(self.outcome));
        }
        match &mut self.mode {
            Mode::Menu(list) => match key.code {
                KeyCode::Up => {
                    list.move_selection(-1);
                    Ok(None)
                }
                KeyCode::Down => {
                    list.move_selection(1);
                    Ok(None)
                }
                KeyCode::Enter => {
                    let selected = list.selected;
                    self.submit_menu(selected)
                }
                _ => Ok(None),
            },
            Mode::Input { secret, .. } => {
                let secret = *secret;
                match key.code {
                    KeyCode::Enter => self.submit_input(),
                    KeyCode::Backspace => {
                        self.input.pop();
                        Ok(None)
                    }
                    KeyCode::Esc => {
                        self.input.clear();
                        Ok(None)
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let _ = secret; // masking happens at render time
                        self.input.push(c);
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            }
        }
    }

    // ---- transitions ---------------------------------------------------

    /// The resolved value of the active input: the typed text, or the stage
    /// default when left empty.
    fn input_value(&self) -> String {
        let typed = self.input.trim();
        if typed.is_empty() {
            if let Mode::Input { default, .. } = &self.mode {
                return default.clone();
            }
        }
        typed.to_string()
    }

    fn submit_menu(&mut self, selected: usize) -> Result<Option<SetupOutcome>, String> {
        match self.stage {
            Stage::Welcome => {
                if selected == 0 {
                    self.push_answer("Set up lawlint now");
                    self.enter_stage(Stage::Catalog);
                } else {
                    self.push_answer("Skip for now");
                    self.outcome = SetupOutcome::Skipped;
                    return Ok(Some(SetupOutcome::Skipped));
                }
            }
            Stage::Catalog => {
                self.push_answer(catalog_short(selected));
                match selected {
                    0 => {
                        self.pending_keys.clear();
                        self.enter_stage(Stage::AnthropicModel);
                    }
                    1 => {
                        self.pending_keys.clear();
                        self.enter_stage(Stage::OpenAiModel);
                    }
                    2 => {
                        self.pending_keys.clear();
                        self.enter_stage(Stage::FoundryDeployment);
                    }
                    3 => self.enter_stage(Stage::CompatBaseUrl),
                    _ => self.enter_stage(Stage::LocalConfirm),
                }
            }
            Stage::LocalConfirm => {
                let yes = selected == 0;
                self.push_answer(if yes { "Yes" } else { "No" });
                if yes {
                    self.enter_stage(Stage::LocalChoice);
                } else {
                    self.push_note("Using the recommended hosted provider instead.");
                    self.pending_keys.clear();
                    self.enter_stage(Stage::AnthropicModel);
                }
            }
            Stage::LocalChoice => {
                self.pending_local_gemma = selected == 1;
                self.push_answer(if self.pending_local_gemma {
                    "Gemma 4 E4B"
                } else {
                    "Qwen2.5-1.5B"
                });
                self.enter_stage(Stage::LocalRepo);
            }
            Stage::JudgeEnabled => {
                let yes = selected == 0;
                self.push_answer(if yes { "Yes" } else { "No" });
                self.judge_enabled = Some(yes);
                if yes {
                    self.enter_stage(Stage::Floor);
                } else {
                    self.floor = None;
                    self.enter_stage(Stage::Markdown);
                }
            }
            Stage::Markdown => {
                let yes = selected == 0;
                self.push_answer(if yes { "Yes" } else { "No" });
                self.markdown = Some(yes);
                self.enter_stage(Stage::Scaffold);
            }
            Stage::Scaffold => {
                let yes = selected == 0;
                self.push_answer(if yes { "Yes" } else { "No" });
                self.scaffold_rules = yes;
                if self.seed.legacy.is_some() {
                    self.enter_stage(Stage::LegacyRemove);
                } else {
                    return self.finish();
                }
            }
            Stage::LegacyRemove => {
                let yes = selected == 0;
                self.push_answer(if yes { "Yes" } else { "No" });
                self.remove_legacy = Some(yes);
                return self.finish();
            }
            _ => {}
        }
        Ok(None)
    }

    fn submit_input(&mut self) -> Result<Option<SetupOutcome>, String> {
        let value = self.input_value();
        match self.stage {
            Stage::AnthropicModel => {
                self.pending_model = value.clone();
                self.push_answer(&value);
                self.enter_stage(Stage::AnthropicKey);
            }
            Stage::AnthropicKey => {
                self.take_key("ANTHROPIC_API_KEY");
                let selection = AiSelection {
                    model: format!("anthropic:{}", self.pending_model),
                    keys: std::mem::take(&mut self.pending_keys),
                    acknowledge_local: false,
                };
                self.finish_ai(selection);
            }
            Stage::OpenAiModel => {
                self.pending_model = value.clone();
                self.push_answer(&value);
                self.enter_stage(Stage::OpenAiKey);
            }
            Stage::OpenAiKey => {
                self.take_key("OPENAI_API_KEY");
                let selection = AiSelection {
                    model: format!("openai:{OPENAI_HOSTED_BASE_URL}#{}", self.pending_model),
                    keys: std::mem::take(&mut self.pending_keys),
                    acknowledge_local: false,
                };
                self.finish_ai(selection);
            }
            Stage::FoundryDeployment => {
                self.pending_model = value.clone();
                self.push_answer(&value);
                self.enter_stage(Stage::FoundryEndpoint);
            }
            Stage::FoundryEndpoint => {
                self.take_secret("AZURE_FOUNDRY_ENDPOINT", "endpoint");
                self.enter_stage(Stage::FoundryKey);
            }
            Stage::FoundryKey => {
                self.take_key("AZURE_FOUNDRY_API_KEY");
                let selection = AiSelection {
                    model: format!("foundry:{}", self.pending_model),
                    keys: std::mem::take(&mut self.pending_keys),
                    acknowledge_local: false,
                };
                self.finish_ai(selection);
            }
            Stage::CompatBaseUrl => {
                self.pending_model = value.clone();
                self.push_answer(&value);
                self.enter_stage(Stage::CompatModel);
            }
            Stage::CompatModel => {
                self.push_answer(&value);
                let selection =
                    AiSelection::keyless(format!("openai:{}#{}", self.pending_model, value));
                self.finish_ai(selection);
            }
            Stage::LocalRepo => {
                self.push_answer(&value);
                let selection = if self.pending_local_gemma {
                    if init::looks_like_gemma_gguf(&value) {
                        self.push_note(&format!(
                            "Note: gemma GGUFs are not runnable by the bundled runtime — use \
                             the safetensors repo (e.g. {}), which runs 4-bit-quantized \
                             in-process. The repo stays editable in .lawlint/config.json.",
                            lawlint_judge::DEFAULT_GEMMA_REPO
                        ));
                    }
                    AiSelection::acknowledged_local(format!("local:{value}"))
                } else if value == lawlint_judge::DEFAULT_LOCAL_REPO {
                    AiSelection::acknowledged_local("local")
                } else {
                    AiSelection::acknowledged_local(format!("local:{value}"))
                };
                self.finish_ai(selection);
            }
            Stage::Floor => match value.parse::<f64>() {
                Ok(floor) if (0.0..=1.0).contains(&floor) => {
                    self.error = None;
                    self.push_answer(&value);
                    self.floor = (floor != DEFAULT_FLOOR).then_some(floor);
                    self.enter_stage(Stage::Markdown);
                }
                _ => {
                    self.error = Some("Enter a number between 0 and 1.".into());
                    self.input.clear();
                }
            },
            _ => {}
        }
        Ok(None)
    }

    /// A hosted-provider key prompt just resolved: store the typed key, or note
    /// the fallback to the environment variable when skipped.
    fn take_key(&mut self, env_name: &str) {
        let value = self.input.trim().to_string();
        if value.is_empty() {
            self.push_note(&format!(
                "No key stored; lawlint reads ${env_name} at run time."
            ));
        } else {
            self.push_answer(&"•".repeat(value.chars().count().min(24)));
            self.pending_keys.push((env_name.to_string(), value));
        }
    }

    /// Like `take_key` but for a non-secret-but-optional value (the Foundry
    /// endpoint), echoed in the clear.
    fn take_secret(&mut self, env_name: &str, label: &str) {
        let value = self.input.trim().to_string();
        if value.is_empty() {
            self.push_note(&format!(
                "No {label} stored; lawlint reads ${env_name} at run time."
            ));
        } else {
            self.push_answer(&value);
            self.pending_keys.push((env_name.to_string(), value));
        }
    }

    fn finish_ai(&mut self, selection: AiSelection) {
        self.ai = Some(selection);
        self.enter_stage(Stage::JudgeEnabled);
    }

    /// All answers collected — apply the `--acknowledge-local` fixup, write the
    /// config via the shared path, remove the legacy file if asked, and render
    /// the summary. Returns `Some(Completed)` only after the user dismisses it.
    fn finish(&mut self) -> Result<Option<SetupOutcome>, String> {
        // `--acknowledge-local` attaches to a local selection (parity with the
        // line flow); interactive local choices already carry it.
        if self.acknowledge_local {
            if let Some(selection) = &mut self.ai {
                if init::is_local_spec(&selection.model) {
                    selection.acknowledge_local = true;
                }
            }
        }

        let answers = Answers {
            ai: self.ai.clone(),
            judge_enabled: self.judge_enabled,
            floor: self.floor,
            markdown: self.markdown,
            scaffold_rules: self.scaffold_rules,
        };
        let dirs = self.user_dirs.clone();
        let applied = init::apply_answers(
            &self.directory,
            &self.seed.base,
            &answers,
            &init::UserScope {
                credentials: None,
                dirs: dirs.as_ref().map(|(a, b)| (a.as_path(), b.as_path())),
            },
        )?;

        self.push_blank();
        if let Some(message) = &applied.credential_message {
            self.push_note(message);
        }

        // Legacy removal: the new nested config shadows the old file, so honor
        // the earlier choice.
        if self.seed.legacy.is_some() {
            if self.remove_legacy == Some(true) {
                std::fs::remove_file(&self.seed.legacy_path).map_err(|error| {
                    format!(
                        "failed to remove {}: {error}",
                        self.seed.legacy_path.display()
                    )
                })?;
                self.push_note("Removed lawlint.config.json.");
            } else {
                self.push_note(
                    "Keeping lawlint.config.json; note that .lawlint/config.json takes precedence.",
                );
            }
        }

        self.push_blank();
        for file in &applied.created {
            self.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("created ", Style::default().fg(GOOD)),
                Span::raw(file.clone()),
            ]));
        }
        self.push_blank();
        self.push(bullet_line(
            Span::styled(
                "Setup complete",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            " · opening lawlint…".to_string(),
        ));
        self.push(Line::from(Span::styled(
            "  Edit .lawlint/config.json any time to adjust settings.",
            Style::default().fg(DIM),
        )));
        self.push(Line::from(Span::styled(
            "  Press any key to continue.",
            Style::default().fg(DIM),
        )));

        self.stage = Stage::Summary;
        self.awaiting_dismiss = true;
        self.outcome = SetupOutcome::Completed;
        Ok(None)
    }

    /// Set up the widget and question for `stage`, pushing the question into the
    /// transcript. Mirrors the corresponding `Prompter` calls in `init::ask`.
    fn enter_stage(&mut self, stage: Stage) {
        self.stage = stage;
        self.input.clear();
        self.error = None;
        match stage {
            Stage::Welcome => {
                self.push_question("Set up lawlint for this project?", "");
                self.mode = Mode::Menu(SelectList::new(
                    vec![
                        "Set up lawlint now (recommended)".to_string(),
                        "Skip for now — just open lawlint".to_string(),
                    ],
                    0,
                ));
            }
            Stage::Catalog => {
                self.push_question(AI_CATALOG_PROMPT, "");
                self.mode = Mode::Menu(SelectList::new(
                    AI_CATALOG_OPTIONS.iter().map(|s| s.to_string()).collect(),
                    init::default_catalog_index(&self.base_model),
                ));
            }
            Stage::AnthropicModel => {
                let default = self
                    .base_model
                    .strip_prefix("anthropic:")
                    .filter(|model| !model.is_empty())
                    .unwrap_or(DEFAULT_ANTHROPIC_MODEL)
                    .to_string();
                self.text_stage("Anthropic model", default);
            }
            Stage::AnthropicKey => {
                self.secret_stage("Anthropic API key (Enter to skip and use $ANTHROPIC_API_KEY)");
            }
            Stage::OpenAiModel => {
                let default = self
                    .base_model
                    .strip_prefix("openai:")
                    .and_then(|spec| spec.rsplit_once('#'))
                    .map(|(_, model)| model)
                    .filter(|model| !model.is_empty())
                    .unwrap_or(DEFAULT_OPENAI_HOSTED_MODEL)
                    .to_string();
                self.text_stage("OpenAI model", default);
            }
            Stage::OpenAiKey => {
                self.secret_stage("OpenAI API key (Enter to skip and use $OPENAI_API_KEY)");
            }
            Stage::FoundryDeployment => {
                let default = self
                    .base_model
                    .strip_prefix("foundry:")
                    .filter(|deployment| !deployment.is_empty())
                    .unwrap_or(DEFAULT_FOUNDRY_DEPLOYMENT)
                    .to_string();
                self.text_stage("Foundry model deployment", default);
            }
            Stage::FoundryEndpoint => {
                self.secret_stage(
                    "Azure Foundry endpoint, e.g. https://<resource>.services.ai.azure.com \
                     (Enter to skip and use $AZURE_FOUNDRY_ENDPOINT)",
                );
            }
            Stage::FoundryKey => {
                self.secret_stage(
                    "Azure Foundry API key (Enter to skip and use $AZURE_FOUNDRY_API_KEY)",
                );
            }
            Stage::CompatBaseUrl => {
                let (base_url, model) = self
                    .base_model
                    .strip_prefix("openai:")
                    .and_then(|spec| spec.rsplit_once('#'))
                    .unwrap_or((DEFAULT_COMPAT_BASE_URL, DEFAULT_COMPAT_MODEL));
                self.pending_local_gemma = false;
                self.pending_model = base_url.to_string();
                let _ = model;
                self.text_stage("Base URL", base_url.to_string());
            }
            Stage::CompatModel => {
                let default = self
                    .base_model
                    .strip_prefix("openai:")
                    .and_then(|spec| spec.rsplit_once('#'))
                    .map(|(_, model)| model)
                    .filter(|model| !model.is_empty())
                    .unwrap_or(DEFAULT_COMPAT_MODEL)
                    .to_string();
                self.text_stage("Model", default);
            }
            Stage::LocalConfirm => {
                for line in LOCAL_CONSTRAINTS.lines() {
                    self.push(Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(DIM),
                    )));
                }
                self.push_question("Use a local model with these constraints?", "");
                self.mode = Mode::Menu(yes_no(self.ack_default));
            }
            Stage::LocalChoice => {
                let is_gemma = |repo: &str| repo.to_lowercase().contains("gemma");
                let default = if self.base_model.strip_prefix("local:").is_some_and(is_gemma) {
                    1
                } else {
                    0
                };
                self.push_question("Which local model?", "");
                self.mode = Mode::Menu(SelectList::new(
                    LOCAL_CHOICE_OPTIONS.iter().map(|s| s.to_string()).collect(),
                    default,
                ));
            }
            Stage::LocalRepo => {
                let is_gemma = |repo: &str| repo.to_lowercase().contains("gemma");
                let (label, default) = if self.pending_local_gemma {
                    let default = self
                        .base_model
                        .strip_prefix("local:")
                        .filter(|repo| is_gemma(repo))
                        .unwrap_or(lawlint_judge::DEFAULT_GEMMA_REPO)
                        .to_string();
                    ("Hugging Face repo (repo[#file])", default)
                } else {
                    let default = self
                        .base_model
                        .strip_prefix("local:")
                        .filter(|repo| !repo.is_empty() && !is_gemma(repo))
                        .unwrap_or(lawlint_judge::DEFAULT_LOCAL_REPO)
                        .to_string();
                    ("Hugging Face GGUF repo (repo[#file])", default)
                };
                self.text_stage(label, default);
            }
            Stage::JudgeEnabled => {
                self.push_question(
                    "Enable the tier-3 AI judge? It powers the inferential (semantic) rules; \
                     tiers 1-2 always run.",
                    "",
                );
                self.mode = Mode::Menu(yes_no(self.base_judge_enabled));
            }
            Stage::Floor => {
                self.text_stage(
                    "Judge confidence floor (findings below it are dropped, 0-1)",
                    format!("{}", self.base_floor),
                );
            }
            Stage::Markdown => {
                self.push_question(
                    "Treat input as Markdown by default? (.md files are auto-detected either way)",
                    "",
                );
                self.mode = Mode::Menu(yes_no(self.base_markdown));
            }
            Stage::Scaffold => {
                self.push_question(
                    &format!(
                        "Create a starter custom-rules package in {}/?",
                        init::RULES_DIR
                    ),
                    "",
                );
                self.mode = Mode::Menu(yes_no(false));
            }
            Stage::LegacyRemove => {
                let question = if self.seed.legacy_carried {
                    "Remove the old lawlint.config.json? (its settings were carried over; \
                     .lawlint/config.json now takes precedence)"
                } else {
                    "Remove the old lawlint.config.json? (.lawlint/config.json takes \
                     precedence, so it is unused)"
                };
                self.push_question(question, "");
                self.mode = Mode::Menu(yes_no(true));
            }
            Stage::Summary => {}
        }
        self.scroll_up = 0;
    }

    fn text_stage(&mut self, label: &str, default: String) {
        self.push_question(label, &format!(" [{default}]"));
        self.mode = Mode::Input {
            default,
            secret: false,
        };
    }

    fn secret_stage(&mut self, label: &str) {
        self.push_question(label, "");
        self.mode = Mode::Input {
            default: String::new(),
            secret: true,
        };
    }

    // ---- transcript helpers --------------------------------------------

    fn push(&mut self, line: Line<'static>) {
        self.transcript.push(TranscriptLine { line, fill: None });
    }

    fn push_blank(&mut self) {
        self.push(Line::default());
    }

    fn push_question(&mut self, head: &str, dim_tail: &str) {
        self.push_blank();
        self.push(bullet_line(
            Span::styled(
                head.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            dim_tail.to_string(),
        ));
    }

    fn push_note(&mut self, message: &str) {
        self.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(message.to_string(), Style::default().fg(DIM)),
        ]));
    }

    /// Echo the user's answer as a dim, right-of-question line, so the
    /// transcript reads as a Q/A log.
    fn push_answer(&mut self, answer: &str) {
        let bar = Style::default().bg(INPUT_BAR_BG);
        self.transcript.push(TranscriptLine {
            line: Line::from(vec![
                Span::styled("  › ", Style::default().fg(BRAND).bg(INPUT_BAR_BG)),
                Span::styled(
                    answer.to_string(),
                    Style::default().fg(DIM).bg(INPUT_BAR_BG),
                ),
            ]),
            fill: Some(bar),
        });
    }

    // ---- rendering -----------------------------------------------------

    fn draw(&mut self, f: &mut ratatui::Frame) {
        let area = f.area();
        let active_height = match &self.mode {
            _ if self.awaiting_dismiss => 0,
            Mode::Menu(list) => list.options.len() as u16,
            Mode::Input { .. } => 1,
        };
        let [transcript_area, _spacer, active_area, footer_area] =
            area.layout(&Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Length(active_height),
                Constraint::Length(1),
            ]));

        let width = transcript_area.width as usize;
        let height = transcript_area.height as usize;
        if width >= 8 && height > 0 {
            let mut lines: Vec<Line> = Vec::new();
            for line in self.banner_lines() {
                lines.extend(wrap_line(&line, width));
            }
            for entry in &self.transcript {
                for wrapped in wrap_line(&entry.line, width) {
                    lines.push(match entry.fill {
                        Some(fill) => pad_line(wrapped, width, fill),
                        None => wrapped,
                    });
                }
            }
            let max_scroll = lines.len().saturating_sub(height);
            self.scroll_up = self.scroll_up.min(max_scroll);
            let end = lines.len() - self.scroll_up;
            let start = end.saturating_sub(height);
            f.render_widget(
                Paragraph::new(Text::from(lines[start..end].to_vec())),
                transcript_area,
            );
        }

        if active_height > 0 {
            match &self.mode {
                Mode::Menu(list) => {
                    f.render_widget(
                        Paragraph::new(Text::from(list.lines(active_area.width as usize))),
                        active_area,
                    );
                }
                Mode::Input { secret, .. } => {
                    let shown = if *secret {
                        "•".repeat(self.input.chars().count())
                    } else {
                        self.input.clone()
                    };
                    let mut spans = vec![Span::styled(
                        "§ ",
                        Style::default().fg(BRAND).add_modifier(Modifier::BOLD),
                    )];
                    if shown.is_empty() {
                        spans.push(Span::styled(
                            " ",
                            Style::default().add_modifier(Modifier::REVERSED),
                        ));
                    } else {
                        spans.push(Span::raw(shown));
                        spans.push(Span::styled(
                            " ",
                            Style::default().add_modifier(Modifier::REVERSED),
                        ));
                    }
                    f.render_widget(Paragraph::new(Line::from(spans)), active_area);
                }
            }
        }

        f.render_widget(Paragraph::new(self.footer_line()), footer_area);
    }

    fn footer_line(&self) -> Line<'static> {
        if let Some(error) = &self.error {
            return Line::from(vec![
                Span::styled("◆ ", Style::default().fg(BRAND)),
                Span::styled(error.clone(), Style::default().fg(BRAND)),
            ]);
        }
        let label = if self.first_run {
            "first-time setup"
        } else {
            "lawlint setup"
        };
        let hint = match self.mode {
            _ if self.awaiting_dismiss => " · press any key to open lawlint",
            Mode::Menu(_) => " · ↑↓ select · Enter confirm · Ctrl+C cancel",
            Mode::Input { .. } => " · Enter accept · Ctrl+C cancel",
        };
        Line::from(vec![
            Span::styled(
                label,
                Style::default().fg(BRAND).add_modifier(Modifier::BOLD),
            ),
            Span::styled(hint, Style::default().fg(DIM)),
        ])
    }

    /// The wordmark header, matching the TUI's `▌ LAWLINT` banner.
    fn banner_lines(&self) -> Vec<Line<'static>> {
        let bar = Span::styled("▌ ", Style::default().fg(BRAND_DARK));
        let with_bar = |spans: Vec<Span<'static>>| {
            let mut all = vec![bar.clone()];
            all.extend(spans);
            Line::from(all)
        };
        let subtitle = if self.first_run {
            "first-time setup · answering a few questions writes .lawlint/config.json".to_string()
        } else {
            "project setup · writes .lawlint/config.json".to_string()
        };
        vec![
            with_bar(vec![
                Span::styled(
                    "LAWLINT",
                    Style::default().fg(BRAND).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  v{}", env!("CARGO_PKG_VERSION")),
                    Style::default().fg(DIM),
                ),
            ]),
            with_bar(vec![Span::styled(subtitle, Style::default().fg(DIM))]),
            with_bar(vec![Span::styled(
                tilde(&self.directory.display().to_string()),
                Style::default().fg(DIM),
            )]),
            Line::default(),
        ]
    }
}

/// A yes/no menu (Yes first) with the default pre-selected.
fn yes_no(default_yes: bool) -> SelectList {
    SelectList::new(
        vec!["Yes".to_string(), "No".to_string()],
        if default_yes { 0 } else { 1 },
    )
}

/// A short label for a chosen catalog entry, for the answer echo.
fn catalog_short(index: usize) -> &'static str {
    match index {
        0 => "Claude (Anthropic)",
        1 => "GPT (OpenAI)",
        2 => "Azure Foundry",
        3 => "OpenAI-compatible endpoint",
        _ => "Local model",
    }
}

fn should_abort(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q'))
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::{SeedSource, DEFAULT_ANTHROPIC_MODEL};
    use serde_json::json;

    /// A wizard over a synthetic seed, no terminal involved. The transitions
    /// (`submit_menu`/`submit_input`) are the pure part under test; rendering
    /// is not exercised.
    fn wizard(base: Value, first_run: bool) -> Wizard {
        let seed = Seed {
            base,
            legacy: None,
            legacy_carried: false,
            legacy_path: PathBuf::from("lawlint.config.json"),
            source: SeedSource::Fresh,
        };
        Wizard::new(PathBuf::from("."), seed, None, false, first_run, None)
    }

    /// Flatten the transcript to plain text for substring assertions.
    fn transcript_text(w: &Wizard) -> String {
        w.transcript
            .iter()
            .flat_map(|entry| entry.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("")
    }

    /// Type `value` into the active input (empty ⇒ accept the default) and
    /// submit it.
    fn type_and_submit(w: &mut Wizard, value: &str) {
        w.input = value.to_string();
        w.submit_input().unwrap();
    }

    #[test]
    fn catalog_anthropic_defaults_to_hosted_keyless() {
        let mut w = wizard(json!({}), false);
        assert_eq!(w.stage, Stage::Catalog);
        w.submit_menu(0).unwrap(); // Claude (Anthropic)
        assert_eq!(w.stage, Stage::AnthropicModel);
        type_and_submit(&mut w, ""); // accept default model
        assert_eq!(w.stage, Stage::AnthropicKey);
        type_and_submit(&mut w, ""); // skip key
        assert_eq!(w.stage, Stage::JudgeEnabled);
        assert_eq!(
            w.ai,
            Some(AiSelection::keyless(format!(
                "anthropic:{DEFAULT_ANTHROPIC_MODEL}"
            )))
        );
        assert!(transcript_text(&w).contains("$ANTHROPIC_API_KEY"));
    }

    #[test]
    fn anthropic_stores_a_typed_key() {
        let mut w = wizard(json!({}), false);
        w.submit_menu(0).unwrap();
        type_and_submit(&mut w, "claude-x");
        type_and_submit(&mut w, "sk-ant-secret");
        assert_eq!(
            w.ai,
            Some(AiSelection {
                model: "anthropic:claude-x".into(),
                keys: vec![("ANTHROPIC_API_KEY".into(), "sk-ant-secret".into())],
                acknowledge_local: false,
            })
        );
    }

    #[test]
    fn judge_yes_prompts_floor_then_markdown() {
        let mut w = wizard(json!({}), false);
        // Fast-forward to JudgeEnabled via the Anthropic default path.
        w.submit_menu(0).unwrap();
        type_and_submit(&mut w, "");
        type_and_submit(&mut w, "");
        assert_eq!(w.stage, Stage::JudgeEnabled);

        w.submit_menu(0).unwrap(); // enable judge
        assert_eq!(w.stage, Stage::Floor);
        assert_eq!(w.judge_enabled, Some(true));

        // An out-of-range floor is rejected and re-asked.
        type_and_submit(&mut w, "5");
        assert_eq!(w.stage, Stage::Floor);
        assert!(w.error.is_some());

        type_and_submit(&mut w, "0.8");
        assert_eq!(w.stage, Stage::Markdown);
        assert_eq!(w.floor, Some(0.8));

        // The engine-default floor is omitted from the answers.
        let mut w = wizard(json!({}), false);
        w.submit_menu(0).unwrap();
        type_and_submit(&mut w, "");
        type_and_submit(&mut w, "");
        w.submit_menu(0).unwrap();
        type_and_submit(&mut w, &format!("{DEFAULT_FLOOR}"));
        assert_eq!(w.floor, None);
    }

    #[test]
    fn judge_no_skips_floor() {
        let mut w = wizard(json!({}), false);
        w.submit_menu(0).unwrap();
        type_and_submit(&mut w, "");
        type_and_submit(&mut w, "");
        w.submit_menu(1).unwrap(); // no judge
        assert_eq!(w.stage, Stage::Markdown);
        assert_eq!(w.judge_enabled, Some(false));
        assert_eq!(w.floor, None);
    }

    #[test]
    fn local_acknowledged_qwen_default_is_short_spec() {
        let mut w = wizard(json!({}), false);
        w.submit_menu(4).unwrap(); // Local (advanced)
        assert_eq!(w.stage, Stage::LocalConfirm);
        assert!(transcript_text(&w).contains("0.111")); // constraints shown
        w.submit_menu(0).unwrap(); // acknowledge
        assert_eq!(w.stage, Stage::LocalChoice);
        w.submit_menu(0).unwrap(); // Qwen
        assert_eq!(w.stage, Stage::LocalRepo);
        type_and_submit(&mut w, ""); // accept the stock repo
        assert_eq!(w.ai, Some(AiSelection::acknowledged_local("local")));
        assert_eq!(w.stage, Stage::JudgeEnabled);
    }

    #[test]
    fn local_gemma_gguf_repo_warns() {
        let mut w = wizard(json!({}), false);
        w.submit_menu(4).unwrap();
        w.submit_menu(0).unwrap();
        w.submit_menu(1).unwrap(); // Gemma
        type_and_submit(&mut w, "unsloth/gemma-4-E4B-it-GGUF");
        assert_eq!(
            w.ai,
            Some(AiSelection::acknowledged_local(
                "local:unsloth/gemma-4-E4B-it-GGUF"
            ))
        );
        assert!(transcript_text(&w).contains("not runnable"));
    }

    #[test]
    fn local_decline_falls_back_to_hosted() {
        let mut w = wizard(json!({}), false);
        w.submit_menu(4).unwrap();
        w.submit_menu(1).unwrap(); // decline the constraints
        assert_eq!(w.stage, Stage::AnthropicModel);
        assert!(transcript_text(&w).contains("recommended hosted provider"));
    }

    #[test]
    fn compat_endpoint_builds_openai_spec() {
        let mut w = wizard(json!({}), false);
        w.submit_menu(3).unwrap(); // OpenAI-compatible endpoint
        assert_eq!(w.stage, Stage::CompatBaseUrl);
        type_and_submit(&mut w, ""); // default base URL
        assert_eq!(w.stage, Stage::CompatModel);
        type_and_submit(&mut w, ""); // default model
        assert_eq!(
            w.ai,
            Some(AiSelection::keyless(format!(
                "openai:{DEFAULT_COMPAT_BASE_URL}#{DEFAULT_COMPAT_MODEL}"
            )))
        );
        assert!(w.ai.as_ref().unwrap().keys.is_empty());
    }

    #[test]
    fn foundry_collects_endpoint_and_key() {
        let mut w = wizard(json!({}), false);
        w.submit_menu(2).unwrap(); // Azure Foundry
        type_and_submit(&mut w, "gpt-5.5");
        type_and_submit(&mut w, "https://res.services.ai.azure.com");
        type_and_submit(&mut w, "foundry-key");
        let ai = w.ai.clone().unwrap();
        assert_eq!(ai.model, "foundry:gpt-5.5");
        assert_eq!(
            ai.keys,
            vec![
                (
                    "AZURE_FOUNDRY_ENDPOINT".into(),
                    "https://res.services.ai.azure.com".into()
                ),
                ("AZURE_FOUNDRY_API_KEY".into(), "foundry-key".into()),
            ]
        );
    }

    #[test]
    fn first_run_offers_skip() {
        let mut w = wizard(json!({}), true);
        assert_eq!(w.stage, Stage::Welcome);
        assert_eq!(w.submit_menu(1).unwrap(), Some(SetupOutcome::Skipped));
    }

    #[test]
    fn ai_override_skips_the_catalog() {
        let seed = Seed {
            base: json!({}),
            legacy: None,
            legacy_carried: false,
            legacy_path: PathBuf::from("lawlint.config.json"),
            source: SeedSource::Fresh,
        };
        let w = Wizard::new(
            PathBuf::from("."),
            seed,
            Some(AiSelection::keyless("foundry:d")),
            false,
            false,
            None,
        );
        assert_eq!(w.stage, Stage::JudgeEnabled);
        assert_eq!(w.ai, Some(AiSelection::keyless("foundry:d")));
        assert!(transcript_text(&w).contains("from --ai"));
    }

    #[test]
    fn finish_writes_config_matching_the_line_flow() {
        let dir = std::env::temp_dir().join(format!("lawlint-wizard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let seed = Seed {
            base: json!({}),
            legacy: None,
            legacy_carried: false,
            legacy_path: dir.join("lawlint.config.json"),
            source: SeedSource::Fresh,
        };
        let mut w = Wizard::new(dir.clone(), seed, None, false, false, None);
        // Hosted Anthropic default, judge off, markdown off, no scaffold.
        w.submit_menu(0).unwrap();
        type_and_submit(&mut w, "");
        type_and_submit(&mut w, "");
        w.submit_menu(1).unwrap(); // judge no
        w.submit_menu(1).unwrap(); // markdown no
        w.submit_menu(1).unwrap(); // scaffold no → finish()
        assert!(w.awaiting_dismiss);
        assert_eq!(w.outcome, SetupOutcome::Completed);

        let written = std::fs::read_to_string(dir.join(".lawlint/config.json")).unwrap();
        let config: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            config,
            json!({
                "judge": {"enabled": false},
                "ai": {"model": format!("anthropic:{DEFAULT_ANTHROPIC_MODEL}")}
            })
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
