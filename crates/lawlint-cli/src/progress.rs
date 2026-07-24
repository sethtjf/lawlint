//! A multi-line, live-updating terminal progress display for commands that
//! run several concurrent *named* workers at once — currently `lawlint
//! learn`'s pass-2 mining fan-out, one line per lens.
//!
//! Sibling to `main.rs`'s single-line `Spinner`, which stays untouched: that
//! one animates one line for one moving quantity ("section N of M"), not N
//! independently-named workers finishing in any order. Same rendering
//! discipline as `Spinner` — only animates on a real terminal, background
//! redraw thread, `Drop` always leaves the terminal clean — just extended to
//! a block of lines instead of one.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_INTERVAL: Duration = Duration::from_millis(120);

/// What one worker line shows right now.
enum LineState {
    Pending,
    Running(String),
    Done(String),
    Failed(String),
}

struct MultiSpinnerState {
    labels: Vec<String>,
    lines: Vec<Mutex<LineState>>,
    running: AtomicBool,
}

impl MultiSpinnerState {
    fn render_line(frame: usize, label: &str, state: &LineState) -> String {
        match state {
            LineState::Pending => {
                format!("  {} {label}", SPINNER_FRAMES[frame % SPINNER_FRAMES.len()])
            }
            LineState::Running(status) => format!(
                "  {} {label}  {status}",
                SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
            ),
            LineState::Done(summary) => format!("  \u{2713} {label}  {summary}"),
            LineState::Failed(reason) => format!("  \u{2717} {label}  failed: {reason}"),
        }
    }

    fn redraw(&self, frame: usize) {
        // Move the cursor up to the first worker line, then reprint every
        // line clear-to-end so a shorter new line never leaves stale tail
        // characters from a longer old one.
        eprint!("\x1b[{}A", self.labels.len());
        for (label, line) in self.labels.iter().zip(&self.lines) {
            let state = line.lock().unwrap_or_else(|e| e.into_inner());
            eprintln!("\r\x1b[2K{}", Self::render_line(frame, label, &state));
        }
        let _ = io::stderr().flush();
    }
}

/// A live block of N named lines under one header, redrawn in place.
pub(crate) struct MultiSpinner {
    state: Arc<MultiSpinnerState>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl MultiSpinner {
    /// Prints `header` once, then reserves and animates one line per label.
    /// `quiet` mirrors `Spinner::new`'s gating: no animation when the caller
    /// asked for quiet output or stderr is not a terminal (a pipe or CI log
    /// must never receive cursor-movement escapes).
    pub(crate) fn new(header: &str, labels: Vec<String>, quiet: bool) -> Self {
        let state = Arc::new(MultiSpinnerState {
            lines: labels
                .iter()
                .map(|_| Mutex::new(LineState::Pending))
                .collect(),
            labels,
            running: AtomicBool::new(false),
        });
        let mut handle = None;
        if !quiet && io::stderr().is_terminal() {
            state.running.store(true, Ordering::Relaxed);
            eprintln!("{header}");
            for label in &state.labels {
                eprintln!("  {} {label}", SPINNER_FRAMES[0]);
            }
            let redraw_state = Arc::clone(&state);
            handle = Some(std::thread::spawn(move || {
                let mut frame = 0usize;
                while redraw_state.running.load(Ordering::Relaxed) {
                    redraw_state.redraw(frame);
                    frame += 1;
                    std::thread::sleep(SPINNER_INTERVAL);
                }
            }));
        } else if !quiet {
            eprintln!("{header}");
        }
        Self {
            state,
            handle: Mutex::new(handle),
        }
    }

    fn set(&self, index: usize, new_state: LineState) {
        if let Some(line) = self.state.lines.get(index) {
            *line.lock().unwrap_or_else(|e| e.into_inner()) = new_state;
        }
    }

    pub(crate) fn set_running(&self, index: usize, status: impl Into<String>) {
        self.set(index, LineState::Running(status.into()));
    }

    pub(crate) fn set_done(&self, index: usize, summary: impl Into<String>) {
        self.set(index, LineState::Done(summary.into()));
    }

    pub(crate) fn set_failed(&self, index: usize, reason: impl Into<String>) {
        self.set(index, LineState::Failed(reason.into()));
    }

    /// Stop animating and leave the final per-line states on screen — unlike
    /// the single-line `Spinner`, the finished summary is the content worth
    /// keeping, not a transient status to wipe.
    pub(crate) fn finish(&self) {
        if !self.state.running.swap(false, Ordering::Relaxed) {
            return;
        }
        if let Some(handle) = self.handle.lock().ok().and_then(|mut h| h.take()) {
            let _ = handle.join();
        }
        self.state.redraw(0);
    }
}

impl Drop for MultiSpinner {
    fn drop(&mut self) {
        self.finish();
    }
}
