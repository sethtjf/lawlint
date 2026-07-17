//! Interactive TUI launched by a bare `lawlint` command.
//!
//! Top pane: an editor/file-path input. Bottom pane: lint results.
//! - Type or paste text in the editor and press `Ctrl+L` to lint.
//! - Press `Ctrl+O` to open a file path field; type a path and press `Enter`, or
//!   press `Ctrl+O` again to browse with a native file dialog.
//! - Press `Esc`/`Ctrl+Q` to quit.

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
use std::path::Path;

const CONTENT_TITLE: &str = "Input — Ctrl+L lint | Ctrl+O open file | Esc quit";
const PATH_TITLE: &str = "File path — Enter to load | Ctrl+O browse | Esc cancel";

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

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Content,
    Path,
}

struct TuiApp {
    mode: Mode,
    textarea: TextArea<'static>,
    content_text: String,
    path_text: String,
    loaded_path: Option<String>,
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

        let content_text = String::new();
        let path_text = String::new();
        let output =
            String::from("Type or paste text and press Ctrl+L to lint, or Ctrl+O to open a file.");

        Ok(Self {
            mode: Mode::Content,
            textarea: build_textarea(&content_text, CONTENT_TITLE),
            content_text,
            path_text,
            loaded_path: None,
            output,
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
                    let [header_area, path_area, editor_area, output_area] =
                        area.layout(&Layout::vertical([
                            Constraint::Length(1),
                            if self.mode == Mode::Path {
                                Constraint::Length(3)
                            } else {
                                Constraint::Length(0)
                            },
                            if self.mode == Mode::Content {
                                Constraint::Fill(1)
                            } else {
                                Constraint::Length(0)
                            },
                            Constraint::Fill(1),
                        ]));

                    let header = Paragraph::new("lawlint interactive").centered();
                    f.render_widget(header, header_area);

                    if self.mode == Mode::Path {
                        f.render_widget(&self.textarea, path_area);
                    } else {
                        f.render_widget(&self.textarea, editor_area);
                    }

                    let output = Paragraph::new(Text::from(self.output.as_str()))
                        .block(Block::default().borders(Borders::ALL).title("Results"))
                        .wrap(Wrap { trim: true });
                    f.render_widget(output, output_area);
                })
                .map_err(|e| e.to_string())?;

            if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                if self.mode == Mode::Path && key.code == KeyCode::Esc {
                    self.cancel_path_mode();
                    continue;
                }
                if should_quit(&key) {
                    return Ok(self.exit_code());
                }
                if is_lint_shortcut(&key) && self.mode == Mode::Content {
                    self.lint()?;
                    continue;
                }
                if is_fix_shortcut(&key) && self.mode == Mode::Content {
                    self.apply_fixes()?;
                    continue;
                }
                if is_open_shortcut(&key) {
                    self.open_file(terminal)?;
                    continue;
                }
                if self.mode == Mode::Path && key.code == KeyCode::Enter {
                    self.load_file()?;
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

    fn apply_fixes(&mut self) -> Result<(), String> {
        self.lint()?;
        let text = self.textarea.lines().join("\n");
        let Some(result) = &self.last_result else {
            return Ok(());
        };
        let fixed = lawlint_core::apply_fixes(&text, &result.diagnostics);
        self.set_content(&fixed);
        self.lint()
    }

    fn open_file(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<(), String> {
        if self.mode == Mode::Path {
            self.browse_file(terminal)?;
        } else {
            self.enter_path_mode();
        }
        Ok(())
    }

    fn enter_path_mode(&mut self) {
        self.content_text = self.textarea.lines().join("\n");
        self.mode = Mode::Path;
        self.textarea = build_textarea(&self.path_text, PATH_TITLE);
        self.output = "Enter a file path and press Enter, or press Ctrl+O to browse.".into();
    }

    fn cancel_path_mode(&mut self) {
        self.path_text = self.textarea.lines().join("\n");
        self.mode = Mode::Content;
        self.textarea = build_textarea(&self.content_text, CONTENT_TITLE);
        self.output = if let Some(path) = &self.loaded_path {
            format!("Loaded: {}", path)
        } else {
            "Type or paste text and press Ctrl+L to lint, or Ctrl+O to open a file.".into()
        };
    }

    fn load_file(&mut self) -> Result<(), String> {
        let raw = self.textarea.lines().join("").trim().to_string();
        if raw.is_empty() {
            return Err("Enter a file path".into());
        }
        let path = Path::new(&raw);
        if !path.exists() {
            return Err(format!("Path does not exist: {}", raw));
        }
        if !path.is_file() {
            return Err(format!("Not a file: {}", raw));
        }
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        self.loaded_path = Some(raw);
        self.path_text = self.loaded_path.clone().unwrap_or_default();
        self.set_content(&text);
        self.lint()
    }

    fn browse_file(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<(), String> {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen);
        let result = rfd::FileDialog::new()
            .set_directory(std::env::current_dir().map_err(|e| e.to_string())?)
            .pick_file();
        let _ = enable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), EnterAlternateScreen);
        let _ = terminal.clear();

        if let Some(path) = result {
            let path_str = path.display().to_string();
            self.path_text = path_str.clone();
            self.textarea = build_textarea(&path_str, PATH_TITLE);
            self.load_file()?;
        }
        Ok(())
    }

    fn set_content(&mut self, text: &str) {
        self.content_text = text.to_string();
        self.mode = Mode::Content;
        self.textarea = build_textarea(text, CONTENT_TITLE);
    }

    fn exit_code(&self) -> i32 {
        if let Some(result) = &self.last_result {
            crate::exit_code(result, "inf")
        } else {
            0
        }
    }
}

fn build_textarea(text: &str, title: &'static str) -> TextArea<'static> {
    let mut t = TextArea::from(text.lines());
    t.set_block(Block::default().borders(Borders::ALL).title(title));
    t.set_style(Style::default().fg(Color::White));
    t
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

fn is_open_shortcut(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o')
}
