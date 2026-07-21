//! Shared terminal-UI primitives for the interactive front-ends (the bare
//! `lawlint` TUI in `tui.rs` and the setup wizard in `init_tui.rs`).
//!
//! Both screens draw from one palette and one set of line helpers so they read
//! as the same application — the Litvue oxblood identity, not a generic
//! terminal form. Everything here is pure/rendering-only; no I/O.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui_textarea::TextArea;

// Litvue palette, from apps/desktop/src/styles.css. Foreground accents use a
// lighter tint of the oxblood hue so they stay readable on dark terminals.
/// Oxblood tinted up for foreground accents (base: --oxblood #7e2528).
pub const BRAND: Color = Color::Rgb(199, 62, 71);
/// True oxblood, for borders and quiet accents.
pub const BRAND_DARK: Color = Color::Rgb(126, 37, 40);
/// Warm muted gray (--muted #82796f).
pub const DIM: Color = Color::Rgb(130, 121, 111);
/// Red-tinted bar behind echoed input.
pub const INPUT_BAR_BG: Color = Color::Rgb(56, 40, 40);
/// Selected row in a menu / the file browser.
pub const SELECT_BG: Color = Color::Rgb(94, 32, 34);
/// Green from the desktop app's status dot (#557b59, tinted up).
pub const GOOD: Color = Color::Rgb(118, 160, 123);
/// Warm amber for warnings.
pub const AMBER: Color = Color::Rgb(202, 152, 74);
/// Slate for suggestions.
pub const SLATE: Color = Color::Rgb(126, 148, 158);

/// A borderless single-line composer/input styled to disappear into the
/// transcript. The setup wizard reuses it for every free-text step; the TUI
/// uses it for the `§` composer.
pub fn build_composer() -> TextArea<'static> {
    let mut t = TextArea::default();
    t.set_style(Style::default().fg(Color::Reset));
    t.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    t.set_cursor_line_style(Style::default());
    t.set_placeholder_text("Type or paste text to lint, /help for commands");
    t.set_placeholder_style(Style::default().fg(DIM));
    t
}

/// A `◆`-bulleted headline with an optional dim tail — the shared shape for
/// section headers in both the transcript and the wizard.
pub fn bullet_line(head: Span<'static>, dim_tail: String) -> Line<'static> {
    let mut spans = vec![Span::styled("◆ ", Style::default().fg(BRAND)), head];
    if !dim_tail.is_empty() {
        spans.push(Span::styled(dim_tail, Style::default().fg(DIM)));
    }
    Line::from(spans)
}

pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Shorten a path for display by replacing the home directory with `~`.
pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}

// ---- line wrapping -----------------------------------------------------

/// Word-wrap a styled line to `width` characters, preserving span styles.
pub fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let total: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    if total <= width || width == 0 {
        return vec![line.clone()];
    }

    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();

    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        // Skip leading spaces on continuation rows.
        if !rows.is_empty() {
            while start < chars.len() && chars[start].0 == ' ' {
                start += 1;
            }
        }
        if start >= chars.len() {
            break;
        }
        let hard_end = (start + width).min(chars.len());
        let end = if hard_end == chars.len() {
            hard_end
        } else {
            // Break at the last space in the window, or hard-break mid-word.
            (start..hard_end)
                .rev()
                .find(|&i| chars[i].0 == ' ')
                .map(|i| i + 1)
                .unwrap_or(hard_end)
        };
        rows.push(spans_from_chars(&chars[start..end]));
        start = end;
    }
    rows
}

/// Rebuild spans from styled characters, merging consecutive equal styles.
pub fn spans_from_chars(chars: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current = String::new();
    let mut style = Style::default();
    for (c, s) in chars {
        if current.is_empty() || *s == style {
            style = *s;
            current.push(*c);
        } else {
            spans.push(Span::styled(current.clone(), style));
            current.clear();
            current.push(*c);
            style = *s;
        }
    }
    if !current.is_empty() {
        spans.push(Span::styled(current, style));
    }
    Line::from(spans)
}

/// Extend a line with `fill`-styled spaces out to the full terminal width.
pub fn pad_line(mut line: Line<'static>, width: usize, fill: Style) -> Line<'static> {
    let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    if used < width {
        line.spans
            .push(Span::styled(" ".repeat(width - used), fill));
    }
    line
}

// ---- selection menu ----------------------------------------------------

/// A vertical arrow-key menu rendered in the same style as the file browser's
/// selection (a `▸` marker and an oxblood highlight bar on the active row).
/// The setup wizard uses one per catalog / yes-no step.
pub struct SelectList {
    pub options: Vec<String>,
    pub selected: usize,
}

impl SelectList {
    pub fn new(options: Vec<String>, selected: usize) -> Self {
        let selected = selected.min(options.len().saturating_sub(1));
        Self { options, selected }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.options.is_empty() {
            return;
        }
        let current = self.selected as isize;
        self.selected = (current + delta).clamp(0, self.options.len() as isize - 1) as usize;
    }

    /// The menu as styled lines, indented two columns to align with the
    /// transcript's `◆` bullets. The selected row is bold on `SELECT_BG` and
    /// padded to the full `width`.
    pub fn lines(&self, width: usize) -> Vec<Line<'static>> {
        self.options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let is_selected = index == self.selected;
                let (marker, style) = if is_selected {
                    (
                        "▸ ",
                        Style::default().bg(SELECT_BG).add_modifier(Modifier::BOLD),
                    )
                } else {
                    ("  ", Style::default().fg(DIM))
                };
                let line = Line::from(vec![
                    Span::raw("  "),
                    Span::styled(marker.to_string(), style),
                    Span::styled(option.clone(), style),
                ]);
                if is_selected {
                    pad_line(line, width, Style::default().bg(SELECT_BG))
                } else {
                    line
                }
            })
            .collect()
    }
}
