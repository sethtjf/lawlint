//! Interactive TUI launched by a bare `lawlint` command.
//!
//! A transcript-style interface in the Litvue oxblood palette: a wordmark
//! header, a scrolling transcript of inputs and results, a borderless `§`
//! composer at the bottom, and a single dim status line under that.
//!
//! - Type or paste text in the composer and press `Enter` to lint it.
//!   `Alt+Enter` (or `Ctrl+J`) inserts a newline; pasted newlines are kept.
//! - Slash commands: `/open <path>` lints a file (bare `/open` or `Ctrl+O`
//!   opens the built-in file browser), `/fix` applies fixes to the last
//!   linted text, `/prompt` generates an AI revision prompt from the last
//!   lint, `/clear` clears the transcript, `/help` lists commands, `/quit`
//!   exits.
//! - The file browser is an in-TUI overlay: arrow keys move, typing filters,
//!   `Enter` descends into a directory or picks a file, `Left`/`Backspace`
//!   go up, `Esc` cancels.
//! - `PageUp`/`PageDown` scroll the transcript; `Esc` clears the composer;
//!   `Ctrl+C`/`Ctrl+Q` quit.

use crate::ui::{
    build_composer, bullet_line, pad_line, plural, tilde, wrap_line, AMBER, BRAND, BRAND_DARK, DIM,
    GOOD, INPUT_BAR_BG, SELECT_BG, SLATE,
};
use crate::{build_rule_set, find_config, lint_text};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use lawlint_core::{LintOptions, LintResult, RuleSet, Severity};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Terminal;
use ratatui_textarea::TextArea;
use std::io::{self, IsTerminal, Stdout};
use std::path::{Path, PathBuf};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste);
    }
}

pub fn run_tui() -> Result<i32, String> {
    if !io::stdin().is_terminal() {
        return Err("TUI requires an interactive terminal".into());
    }

    enable_raw_mode().map_err(|e| e.to_string())?;
    crossterm::execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)
        .map_err(|e| e.to_string())?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;

    let app = TuiApp::new()?;
    app.run(&mut terminal)
}

/// One transcript line plus an optional style used to pad it to the full
/// terminal width (the highlighted bar behind echoed input).
struct TranscriptLine {
    line: Line<'static>,
    fill: Option<Style>,
}

struct Banner {
    version: String,
    rule_count: usize,
    judge_on: bool,
    config: Option<String>,
    cwd: String,
}

struct TuiApp {
    transcript: Vec<TranscriptLine>,
    composer: TextArea<'static>,
    banner: Banner,
    browser: Option<FileBrowser>,
    /// Scroll distance up from the bottom of the transcript; 0 follows new output.
    scroll_up: usize,
    last_text: Option<String>,
    loaded_path: Option<String>,
    /// Whether `last_text` still matches `loaded_path` on disk; `/fix`
    /// clears it, and `/prompt` falls back to embedding the text then.
    text_matches_disk: bool,
    last_result: Option<LintResult>,
    rules: RuleSet,
    judge: Option<String>,
    options: LintOptions,
}

impl TuiApp {
    fn new() -> Result<Self, String> {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let (config, config_dir) = find_config(cwd.clone())?;
        let rules = build_rule_set(&config, config_dir.as_deref(), &[])?;
        // Same rule as the lint command: AI rules run whenever they can, and
        // stay quiet when they cannot. A config that *enables* the judge
        // without naming a model is still a config error (#50) and aborts the
        // TUI launch with init guidance.
        let judge = crate::ai_decision(&None, false, &config)?.ok();
        let options = LintOptions {
            markdown: Some(false),
            ..Default::default()
        };

        let banner = Banner {
            version: env!("CARGO_PKG_VERSION").to_string(),
            rule_count: rules.metas().len(),
            judge_on: judge.is_some(),
            config: config_dir.as_deref().map(|d| {
                // Mirror find_config's preference: .lawlint/config.json wins
                // over the legacy top-level name.
                let nested = d.join(".lawlint").join("config.json");
                if nested.is_file() {
                    nested.display().to_string()
                } else {
                    d.join("lawlint.config.json").display().to_string()
                }
            }),
            cwd: cwd.display().to_string(),
        };

        Ok(Self {
            transcript: Vec::new(),
            composer: build_composer(),
            banner,
            browser: None,
            scroll_up: 0,
            last_text: None,
            loaded_path: None,
            text_matches_disk: false,
            last_result: None,
            rules,
            judge,
            options,
        })
    }

    fn run(mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<i32, String> {
        loop {
            terminal
                .draw(|f| {
                    let area = f.area();
                    let composer_height = (self.composer.lines().len() as u16).clamp(1, 8);
                    let [transcript_area, _, composer_area, footer_area] =
                        area.layout(&Layout::vertical([
                            Constraint::Fill(1),
                            Constraint::Length(1),
                            Constraint::Length(composer_height),
                            Constraint::Length(1),
                        ]));

                    let width = transcript_area.width as usize;
                    let height = transcript_area.height as usize;
                    if width >= 8 && height > 0 {
                        let mut lines: Vec<Line> = Vec::new();
                        for line in banner_lines(&self.banner) {
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
                        let visible = lines[start..end].to_vec();
                        f.render_widget(Paragraph::new(Text::from(visible)), transcript_area);
                    }

                    let [prompt_area, input_area] = composer_area.layout(&Layout::horizontal([
                        Constraint::Length(2),
                        Constraint::Fill(1),
                    ]));
                    let prompt = Paragraph::new(Span::styled(
                        "§",
                        Style::default().fg(BRAND).add_modifier(Modifier::BOLD),
                    ));
                    f.render_widget(prompt, prompt_area);
                    f.render_widget(&self.composer, input_area);

                    f.render_widget(Paragraph::new(self.footer_line()), footer_area);

                    if let Some(browser) = &self.browser {
                        render_browser(f, browser, transcript_area);
                    }
                })
                .map_err(|e| e.to_string())?;

            match event::read().map_err(|e| e.to_string())? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if should_quit(&key) {
                        return Ok(self.exit_code());
                    }
                    if self.browser.is_some() {
                        self.browser_key(key);
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.open_browser();
                        }
                        KeyCode::PageUp => self.scroll_up += 10,
                        KeyCode::PageDown => self.scroll_up = self.scroll_up.saturating_sub(10),
                        KeyCode::Esc => self.composer = build_composer(),
                        KeyCode::Enter
                            if key.modifiers.contains(KeyModifiers::ALT)
                                || key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            self.composer.insert_newline();
                        }
                        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.composer.insert_newline();
                        }
                        KeyCode::Enter => {
                            if let Some(code) = self.submit() {
                                return Ok(code);
                            }
                        }
                        _ => {
                            self.composer.input(key);
                        }
                    }
                }
                Event::Paste(text) => {
                    self.composer.insert_str(text);
                }
                _ => {}
            }
        }
    }

    /// Handle a submitted composer line. Returns an exit code when the user
    /// asked to quit.
    fn submit(&mut self) -> Option<i32> {
        let text = self.composer.lines().join("\n");
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return None;
        }
        self.composer = build_composer();
        self.push_input_bar(&text);
        self.scroll_up = 0;

        if let Some(command) = trimmed.strip_prefix('/') {
            let mut parts = command.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or_default();
            let arg = parts.next().unwrap_or("").trim().to_string();
            match name {
                "open" => {
                    if arg.is_empty() {
                        self.open_browser();
                    } else if let Err(e) = self.open_path(&arg) {
                        self.push_error(e);
                    }
                }
                "fix" => {
                    if let Err(e) = self.cmd_fix() {
                        self.push_error(e);
                    }
                }
                "prompt" => {
                    if let Err(e) = self.cmd_prompt() {
                        self.push_error(e);
                    }
                }
                "clear" => self.transcript.clear(),
                "help" => self.push_help(),
                "quit" | "exit" | "q" => return Some(self.exit_code()),
                other => self.push_error(format!("Unknown command: /{other} — try /help")),
            }
            return None;
        }

        self.last_text = Some(trimmed.clone());
        self.loaded_path = None;
        self.text_matches_disk = false;
        self.lint_and_push(&trimmed);
        None
    }

    fn open_browser(&mut self) {
        let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.browser = Some(FileBrowser::new(dir));
    }

    fn browser_key(&mut self, key: KeyEvent) {
        let Some(browser) = self.browser.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.browser = None,
            KeyCode::Up => browser.move_selection(-1),
            KeyCode::Down => browser.move_selection(1),
            KeyCode::PageUp => browser.move_selection(-10),
            KeyCode::PageDown => browser.move_selection(10),
            KeyCode::Left => browser.ascend(),
            KeyCode::Backspace => {
                if browser.filter.is_empty() {
                    browser.ascend();
                } else {
                    browser.filter.pop();
                    browser.selected = 0;
                }
            }
            KeyCode::Right => {
                if browser.selected_is_dir() {
                    browser.descend();
                }
            }
            KeyCode::Enter => {
                if browser.selected_is_dir() {
                    browser.descend();
                } else if let Some(path) = browser.selected_path() {
                    self.browser = None;
                    let raw = path.display().to_string();
                    self.push_input_bar(&format!("/open {raw}"));
                    self.scroll_up = 0;
                    if let Err(e) = self.open_path(&raw) {
                        self.push_error(e);
                    }
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                browser.filter.push(c);
                browser.selected = 0;
            }
            _ => {}
        }
    }

    fn open_path(&mut self, raw: &str) -> Result<(), String> {
        let path = Path::new(raw);
        if !path.exists() {
            return Err(format!("Path does not exist: {raw}"));
        }
        if !path.is_file() {
            return Err(format!("Not a file: {raw}"));
        }
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        self.push_note(&format!(
            "Loaded {raw} ({})",
            plural(text.lines().count(), "line")
        ));
        self.loaded_path = Some(raw.to_string());
        self.last_text = Some(text.clone());
        self.text_matches_disk = true;
        self.lint_and_push(&text);
        Ok(())
    }

    fn cmd_fix(&mut self) -> Result<(), String> {
        let Some(text) = self.last_text.clone() else {
            return Err("Nothing to fix yet — lint some text first.".into());
        };
        // Re-lint so fix offsets match the text they are applied to.
        let result = lint_text(&text, &self.options, &self.rules, self.judge.clone());
        let fixed = lawlint_core::apply_fixes(&text, &result.diagnostics);
        if fixed == text {
            self.push_note("No applicable fixes.");
            return Ok(());
        }

        self.push_blank();
        self.push(bullet_line(
            Span::styled("Changes", Style::default().add_modifier(Modifier::BOLD)),
            match &self.loaded_path {
                Some(path) => format!(" · {path} is unchanged on disk"),
                None => String::new(),
            },
        ));
        let diff = crate::diff::diff_lines(&text, &fixed);
        for entry in crate::diff::with_context(&diff, 1) {
            let (prefix, content, color) = match &entry {
                Some(crate::diff::DiffLine::Removed(s)) => ("- ", s.as_str(), BRAND),
                Some(crate::diff::DiffLine::Added(s)) => ("+ ", s.as_str(), GOOD),
                Some(crate::diff::DiffLine::Same(s)) => ("  ", s.as_str(), DIM),
                None => ("···", "", DIM),
            };
            self.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{prefix}{content}"), Style::default().fg(color)),
            ]));
        }

        self.last_text = Some(fixed.clone());
        self.text_matches_disk = false;
        self.lint_and_push(&fixed);
        Ok(())
    }

    fn cmd_prompt(&mut self) -> Result<(), String> {
        let Some(text) = self.last_text.clone() else {
            return Err("Nothing to prompt yet — lint some text first.".into());
        };
        // Re-lint so the brief reflects the current text and its offsets.
        let result = lint_text(&text, &self.options, &self.rules, self.judge.clone());
        // Reference the file by path while the buffer still matches disk;
        // otherwise (typed text, or after /fix) embed the text itself.
        let source = match &self.loaded_path {
            Some(path) if self.text_matches_disk => lawlint_core::PromptSource::File(path),
            _ => lawlint_core::PromptSource::Text(&text),
        };
        let Some(prompt) = lawlint_core::remediation_prompt(source, &result, &self.rules) else {
            self.push_note("No issues found — nothing to fix.");
            return Ok(());
        };

        self.push_blank();
        self.push(bullet_line(
            Span::styled(
                "Copy this into your AI model",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            String::new(),
        ));
        for line in prompt.lines() {
            let color = if line.starts_with("## ") { BRAND } else { DIM };
            self.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line.to_string(), Style::default().fg(color)),
            ]));
        }
        self.push_blank();
        Ok(())
    }

    fn lint_and_push(&mut self, text: &str) {
        let result = lint_text(text, &self.options, &self.rules, self.judge.clone());
        self.push_result(&result);
        self.last_result = Some(result);
    }

    fn push_result(&mut self, result: &LintResult) {
        self.push_blank();

        let issue_count = result.diagnostics.len();
        let headline = if issue_count == 0 {
            Span::styled(
                "No issues found",
                Style::default().fg(GOOD).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                plural(issue_count, "issue"),
                Style::default().add_modifier(Modifier::BOLD),
            )
        };
        self.push(bullet_line(
            headline,
            format!(
                " · score {}/100 · {}, {}",
                result.stats.score,
                plural(result.stats.word_count, "word"),
                plural(result.stats.sentence_count, "sentence")
            ),
        ));

        for d in &result.diagnostics {
            let (color, label) = match d.severity {
                Severity::Error => (BRAND, "error"),
                Severity::Warning => (AMBER, "warn"),
                Severity::Suggestion => (SLATE, "info"),
            };
            self.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{label:<5} "), Style::default().fg(color)),
                Span::styled(
                    format!("{}:{} ", d.line, d.column),
                    Style::default().fg(DIM),
                ),
                Span::styled(d.rule_id.0.clone(), Style::default().fg(BRAND)),
                Span::raw(" "),
                Span::raw(d.message.clone()),
            ]));
            if !d.excerpt.is_empty() {
                self.push(Line::from(vec![
                    Span::raw("        "),
                    Span::styled(d.excerpt.clone(), Style::default().fg(DIM)),
                ]));
            }
            if let Some(s) = &d.suggestion {
                self.push(Line::from(vec![
                    Span::raw("        "),
                    Span::styled(format!("→ {s}"), Style::default().fg(GOOD)),
                ]));
            }
        }
        self.push_blank();
    }

    fn push_help(&mut self) {
        self.push_blank();
        self.push(bullet_line(
            Span::styled("Commands", Style::default().add_modifier(Modifier::BOLD)),
            String::new(),
        ));
        let rows: &[(&str, &str)] = &[
            ("/open <path>", "lint a file (bare /open browses)"),
            ("/fix", "apply fixes to the last linted text"),
            (
                "/prompt",
                "generate an AI revision prompt from the last lint",
            ),
            ("/clear", "clear the transcript"),
            ("/help", "show this help"),
            ("/quit", "exit (also Ctrl+C)"),
        ];
        for (cmd, desc) in rows {
            self.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{cmd:<14}"), Style::default().fg(BRAND)),
                Span::styled((*desc).to_string(), Style::default().fg(DIM)),
            ]));
        }
        self.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Enter lint · Alt+Enter newline · Ctrl+O browse · PgUp/PgDn scroll · Esc clear input",
                Style::default().fg(DIM),
            ),
        ]));
        self.push_blank();
    }

    fn push_input_bar(&mut self, text: &str) {
        self.push_blank();
        let bar = Style::default().bg(INPUT_BAR_BG);
        for (i, line) in text.lines().enumerate() {
            let prefix = if i == 0 { "§ " } else { "  " };
            let mut spans = Vec::new();
            if i == 0 {
                spans.push(Span::styled(
                    "§ ",
                    Style::default().fg(BRAND).bg(INPUT_BAR_BG),
                ));
            } else {
                spans.push(Span::styled(prefix.to_string(), bar));
            }
            spans.push(Span::styled(line.to_string(), bar));
            self.transcript.push(TranscriptLine {
                line: Line::from(spans),
                fill: Some(bar),
            });
        }
    }

    fn push_note(&mut self, message: &str) {
        self.push_blank();
        self.push(bullet_line(Span::raw(message.to_string()), String::new()));
    }

    fn push_error(&mut self, message: String) {
        self.push_blank();
        self.push(Line::from(vec![
            Span::styled("◆ ", Style::default().fg(BRAND)),
            Span::styled(message, Style::default().fg(BRAND)),
        ]));
    }

    fn push(&mut self, line: Line<'static>) {
        self.transcript.push(TranscriptLine { line, fill: None });
    }

    fn push_blank(&mut self) {
        self.push(Line::default());
    }

    fn footer_line(&self) -> Line<'static> {
        if self.browser.is_some() {
            return Line::from(vec![
                Span::styled(
                    "open file",
                    Style::default().fg(BRAND).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " · ↑↓ select · Enter open · ← parent · type to filter · Esc cancel",
                    Style::default().fg(DIM),
                ),
            ]);
        }
        let mut spans = vec![
            Span::styled(
                "lawlint",
                Style::default().fg(BRAND).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " · Enter to lint · /help for commands · Ctrl+C to quit",
                Style::default().fg(DIM),
            ),
        ];
        if let Some(path) = &self.loaded_path {
            spans.push(Span::styled(" · ", Style::default().fg(DIM)));
            spans.push(Span::styled(path.clone(), Style::default().fg(BRAND)));
        }
        if let Some(result) = &self.last_result {
            let score = result.stats.score;
            let color = if score >= 80 {
                GOOD
            } else if score >= 50 {
                AMBER
            } else {
                BRAND
            };
            spans.push(Span::styled(" · ", Style::default().fg(DIM)));
            spans.push(Span::styled(
                format!("score {score}/100"),
                Style::default().fg(color),
            ));
        }
        Line::from(spans)
    }

    fn exit_code(&self) -> i32 {
        if let Some(result) = &self.last_result {
            crate::exit_code(result, "inf")
        } else {
            0
        }
    }
}

// ---- file browser ------------------------------------------------------

struct DirRow {
    name: String,
    is_dir: bool,
}

struct FileBrowser {
    dir: PathBuf,
    entries: Vec<DirRow>,
    error: Option<String>,
    filter: String,
    selected: usize,
}

impl FileBrowser {
    fn new(dir: PathBuf) -> Self {
        let mut browser = Self {
            dir,
            entries: Vec::new(),
            error: None,
            filter: String::new(),
            selected: 0,
        };
        browser.reload();
        browser
    }

    fn reload(&mut self) {
        self.entries.clear();
        self.error = None;
        self.filter.clear();
        self.selected = 0;
        if self.dir.parent().is_some() {
            self.entries.push(DirRow {
                name: "..".into(),
                is_dir: true,
            });
        }
        let read = match std::fs::read_dir(&self.dir) {
            Ok(read) => read,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let mut rows: Vec<DirRow> = read
            .filter_map(|entry| entry.ok())
            .map(|entry| DirRow {
                is_dir: entry.file_type().map(|t| t.is_dir()).unwrap_or(false),
                name: entry.file_name().to_string_lossy().into_owned(),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.entries.extend(rows);
    }

    /// Indices into `entries` that match the current filter. While a filter
    /// is active `..` is hidden too, so the first match is always selected;
    /// `Left`/`Backspace` still navigate to the parent.
    fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, row)| needle.is_empty() || row.name.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect()
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self.visible().len();
        if count == 0 {
            return;
        }
        let current = self.selected.min(count - 1) as isize;
        self.selected = (current + delta).clamp(0, count as isize - 1) as usize;
    }

    fn selected_row(&self) -> Option<&DirRow> {
        let visible = self.visible();
        visible
            .get(self.selected.min(visible.len().saturating_sub(1)))
            .map(|&i| &self.entries[i])
    }

    fn selected_is_dir(&self) -> bool {
        self.selected_row().map(|row| row.is_dir).unwrap_or(false)
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.selected_row().map(|row| self.dir.join(&row.name))
    }

    fn descend(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.name == ".." {
            self.ascend();
            return;
        }
        self.dir = self.dir.join(&row.name);
        self.reload();
    }

    fn ascend(&mut self) {
        if let Some(parent) = self.dir.parent() {
            self.dir = parent.to_path_buf();
            self.reload();
        }
    }
}

fn render_browser(f: &mut ratatui::Frame, browser: &FileBrowser, host: Rect) {
    if host.height < 5 || host.width < 20 {
        return;
    }
    let height = host.height.min(16);
    let area = Rect {
        x: host.x,
        y: host.y + host.height - height,
        width: host.width,
        height,
    };
    f.render_widget(Clear, area);

    let filter_hint = if browser.filter.is_empty() {
        " type to filter ".to_string()
    } else {
        format!(" filter: {} ", browser.filter)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BRAND_DARK))
        .title(Line::from(Span::styled(
            format!(
                " open file · {} ",
                tilde(&browser.dir.display().to_string())
            ),
            Style::default().fg(BRAND).add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(Span::styled(
            filter_hint,
            Style::default().fg(DIM),
        )));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    if let Some(error) = &browser.error {
        lines.push(Line::from(Span::styled(
            format!("cannot read directory: {error}"),
            Style::default().fg(BRAND),
        )));
    } else {
        let visible = browser.visible();
        if visible.is_empty() {
            lines.push(Line::from(Span::styled(
                "no matches",
                Style::default().fg(DIM),
            )));
        }
        let rows = inner.height as usize;
        let selected = browser.selected.min(visible.len().saturating_sub(1));
        let start = (selected + 1).saturating_sub(rows);
        for (pos, &index) in visible.iter().enumerate().skip(start).take(rows) {
            let row = &browser.entries[index];
            let is_selected = pos == selected;
            let name = if row.is_dir {
                format!("{}/", row.name)
            } else {
                row.name.clone()
            };
            let base = if is_selected {
                Style::default().bg(SELECT_BG).add_modifier(Modifier::BOLD)
            } else if row.name.starts_with('.') && row.name != ".." {
                Style::default().fg(DIM)
            } else if row.is_dir {
                Style::default().fg(BRAND)
            } else {
                Style::default()
            };
            let marker = if is_selected { "▸ " } else { "  " };
            let mut line = Line::from(vec![
                Span::styled(marker.to_string(), base),
                Span::styled(name, base),
            ]);
            if is_selected {
                line = pad_line(line, inner.width as usize, Style::default().bg(SELECT_BG));
            }
            lines.push(line);
        }
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

// ---- shared rendering --------------------------------------------------

/// The Litvue wordmark header: an oxblood left bar with the tool name and
/// session facts, followed by dim getting-started hints.
fn banner_lines(banner: &Banner) -> Vec<Line<'static>> {
    let bar = Span::styled("▌ ", Style::default().fg(BRAND_DARK));
    let with_bar = |spans: Vec<Span<'static>>| {
        let mut all = vec![bar.clone()];
        all.extend(spans);
        Line::from(all)
    };

    vec![
        with_bar(vec![
            Span::styled(
                "LAWLINT",
                Style::default().fg(BRAND).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  v{}", banner.version), Style::default().fg(DIM)),
        ]),
        with_bar(vec![Span::styled(
            "human-writing linter for legal text",
            Style::default().fg(DIM),
        )]),
        with_bar(Vec::new()),
        with_bar(vec![Span::styled(
            format!(
                "{} · judge {} · {}",
                plural(banner.rule_count, "rule"),
                if banner.judge_on { "on" } else { "off" },
                banner
                    .config
                    .clone()
                    .map(|p| tilde(&p))
                    .unwrap_or_else(|| "no config file".to_string())
            ),
            Style::default().fg(DIM),
        )]),
        with_bar(vec![Span::styled(
            tilde(&banner.cwd),
            Style::default().fg(DIM),
        )]),
        Line::default(),
        Line::from(vec![
            Span::styled(
                "  Type or paste text, then press ",
                Style::default().fg(DIM),
            ),
            Span::styled("Enter", Style::default().fg(BRAND)),
            Span::styled(" to lint it", Style::default().fg(DIM)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("/open", Style::default().fg(BRAND)),
            Span::styled(" browse files · ", Style::default().fg(DIM)),
            Span::styled("/fix", Style::default().fg(BRAND)),
            Span::styled(" apply fixes · ", Style::default().fg(DIM)),
            Span::styled("/help", Style::default().fg(BRAND)),
            Span::styled(" all commands", Style::default().fg(DIM)),
        ]),
        Line::default(),
    ]
}

fn should_quit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q'))
}
