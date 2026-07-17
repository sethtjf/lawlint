//! Interactive TUI launched by a bare `lawlint` command.
//!
//! Top pane: multi-line text editor. Bottom pane: lint results.
//! Ctrl+L lints the current text; Esc or Ctrl+Q quits.

use crate::{build_rule_set, find_config, format_pretty, judge_spec, lint_text};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use lawlint_core::{LintOptions, LintResult, RuleSet};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;
use ratatui_textarea::TextArea;
use std::io::{self, IsTerminal, Stdout};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen);
    }
}

pub fn run_tui() -> Result<i32, String> {
    if !io::stdin().is_terminal() {
        return Err("TUI requires an interactive terminal".into());
    }

    enable_raw_mode().map_err(|e| e.to_string())?;
    crossterm::execute!(io::stdout(), EnterAlternateScreen).map_err(|e| e.to_string())?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;

    let app = TuiApp::new()?;
    app.run(&mut terminal)
}

struct TuiApp {
    textarea: TextArea<'static>,
    output: String,
    last_result: Option<LintResult>,
    rules: RuleSet,
    judge: Option<Option<String>>,
    options: LintOptions,
}

impl TuiApp {
    fn new() -> Result<Self, String> {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let (config, config_dir) = find_config(cwd)?;
        let rules = build_rule_set(&config, config_dir.as_deref(), &[])?;
        let judge = judge_spec(&None, &config);
        let options = LintOptions {
            markdown: Some(false),
            ..Default::default()
        };

        let mut textarea = TextArea::default();
        textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title("Input — Ctrl+L lint | Esc quit"),
        );
        textarea.set_style(Style::default().fg(Color::White));

        Ok(Self {
            textarea,
            output: String::from("Type or paste text and press Ctrl+L to lint."),
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
                    let [header_area, editor_area, output_area] = area.layout(&Layout::vertical([
                        Constraint::Length(1),
                        Constraint::Fill(1),
                        Constraint::Fill(1),
                    ]));

                    let header = Paragraph::new("lawlint interactive").centered();
                    f.render_widget(header, header_area);

                    f.render_widget(&self.textarea, editor_area);

                    let output = Paragraph::new(Text::from(self.output.as_str()))
                        .block(Block::default().borders(Borders::ALL).title("Results"))
                        .wrap(Wrap { trim: true });
                    f.render_widget(output, output_area);
                })
                .map_err(|e| e.to_string())?;

            if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                if should_quit(&key) {
                    return Ok(self.exit_code());
                }
                if is_lint_shortcut(&key) {
                    self.lint()?;
                    continue;
                }
                if is_fix_shortcut(&key) {
                    self.apply_fixes();
                    continue;
                }
                self.textarea.input(key);
            }
        }
    }

    fn lint(&mut self) -> Result<(), String> {
        let text = self.textarea.lines().join("\n");
        let result = lint_text(&text, &self.options, &self.rules, self.judge.clone());
        self.last_result = Some(result.clone());
        self.output = format_pretty(&result, false, false);
        Ok(())
    }

    fn apply_fixes(&mut self) {
        let text = self.textarea.lines().join("\n");
        let Some(result) = &self.last_result else {
            return;
        };
        let fixed = lawlint_core::apply_fixes(&text, &result.diagnostics);
        self.textarea = TextArea::from(fixed.lines());
        self.textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title("Input — Ctrl+L lint | Esc quit"),
        );
        self.textarea.set_style(Style::default().fg(Color::White));
    }

    fn exit_code(&self) -> i32 {
        if let Some(result) = &self.last_result {
            crate::exit_code(result, "inf")
        } else {
            0
        }
    }
}

fn should_quit(key: &KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q'))
}

fn is_lint_shortcut(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('l')
}

fn is_fix_shortcut(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('f')
}
