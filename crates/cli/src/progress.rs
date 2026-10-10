//! Progress of downloads on stderr (requirements, chapter 9, "Messages").
//!
//! On a terminal that understands escape sequences: a line for the whole group and one for
//! each of the largest downloads in flight, redrawn in place. On a terminal that does not:
//! one line, rewritten with `\r`. For the R package (`--events`): the same line as an event,
//! which R draws itself. Elsewhere (a log, CI): only the "Downloading ..." step, as before.
//! When the group completes, the progress gives way to one line with what was downloaded.

use std::io::IsTerminal;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rok_core::transfer::Transfers;

use crate::ui::Ui;

/// How progress is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only the "Downloading ..." step.
    Off,
    /// One line, rewritten with `\r`.
    Line,
    /// Several lines, redrawn with escape sequences.
    Bars,
    /// The line as an event for the R package (`\x01progress <line>`; an empty line clears it).
    /// R runs rok in the background and draws it: RStudio and Positron on Windows show what a
    /// program writes only line by line.
    Events,
}

impl Mode {
    /// The mode for stderr.
    pub fn detect() -> Mode {
        if !std::io::stderr().is_terminal() {
            Mode::Off
        } else if std::env::var("TERM").is_ok_and(|t| t == "dumb") || !enable_escapes() {
            Mode::Line
        } else {
            Mode::Bars
        }
    }
}

/// Downloads shown one by one under the group's line.
const SHOWN: usize = 5;
/// Nothing is drawn for a group that finishes sooner (no flicker for small downloads).
const QUIET_START: Duration = Duration::from_millis(150);
/// The shortest time between two drawings.
const REDRAW: Duration = Duration::from_millis(100);

const BAR: usize = 24;
const ITEM_BAR: usize = 16;
const NAME: usize = 20;

pub struct Progress<'a> {
    ui: &'a Ui,
    mode: Mode,
    state: Mutex<State>,
}

impl<'a> Progress<'a> {
    pub fn new(ui: &'a Ui, mode: Mode) -> Progress<'a> {
        Progress {
            ui,
            mode,
            state: Mutex::new(State::new("", 0)),
        }
    }

    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut state);
        if self.mode == Mode::Off {
            return;
        }
        let now = Instant::now();
        if now - state.begun < QUIET_START || state.drawn.is_some_and(|t| now - t < REDRAW) {
            return;
        }
        state.drawn = Some(now);
        let out = match self.mode {
            Mode::Bars => {
                let lines = state.lines(self.ui.color());
                let mut out = state.clear();
                // No wrapping (a wrapped line would throw the count of lines off).
                out.push_str("\x1b[?7l");
                for l in &lines {
                    out.push_str(l);
                    out.push('\n');
                }
                out.push_str("\x1b[?7h");
                state.shown = lines.len();
                out
            }
            Mode::Events => {
                state.shown = 1;
                format!("\x01progress {}\n", state.overall(false))
            }
            _ => {
                let line = state.overall(false);
                let width = line.chars().count();
                let pad = state.shown.saturating_sub(width);
                state.shown = width;
                format!("\r{line}{}", " ".repeat(pad))
            }
        };
        self.ui.write_raw(&out);
    }
}

impl Transfers for Progress<'_> {
    fn begin(&self, what: &str, count: usize) {
        if self.mode == Mode::Off {
            self.ui.step(&format!("Downloading {what}"));
        }
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = State::new(what, count);
    }

    fn start(&self, name: &str, size: Option<u64>) {
        self.update(|s| s.start(name, size));
    }

    fn advance(&self, name: &str, bytes: u64) {
        self.update(|s| s.advance(name, bytes));
    }

    fn finish(&self, name: &str) {
        self.update(|s| s.finish(name));
    }

    fn end(&self, completed: bool) {
        if self.mode == Mode::Off {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let clear = match self.mode {
            Mode::Bars => state.clear(),
            Mode::Events if state.shown > 0 => "\x01progress \n".to_string(),
            _ if state.shown > 0 => format!("\r{}\r", " ".repeat(state.shown)),
            _ => String::new(),
        };
        state.shown = 0;
        self.ui.write_raw(&clear);
        if completed {
            self.ui.step(&state.summary());
        }
    }
}

/// A group of downloads.
struct State {
    what: String,
    count: usize,
    begun: Instant,
    /// Downloads in the order they started.
    items: Vec<Item>,
    finished: usize,
    bytes: u64,
    /// When it was last drawn.
    drawn: Option<Instant>,
    /// What is on the screen: lines (bars) or characters (one line).
    shown: usize,
}

struct Item {
    name: String,
    size: Option<u64>,
    got: u64,
    done: bool,
}

impl State {
    fn new(what: &str, count: usize) -> State {
        State {
            what: what.to_string(),
            count,
            begun: Instant::now(),
            items: Vec::new(),
            finished: 0,
            bytes: 0,
            drawn: None,
            shown: 0,
        }
    }

    fn item(&mut self, name: &str) -> &mut Item {
        let i = match self.items.iter().position(|i| i.name == name) {
            Some(i) => i,
            None => {
                self.items.push(Item {
                    name: name.to_string(),
                    size: None,
                    got: 0,
                    done: false,
                });
                self.items.len() - 1
            }
        };
        &mut self.items[i]
    }

    fn start(&mut self, name: &str, size: Option<u64>) {
        let item = self.item(name);
        // A retry starts over.
        let dropped = std::mem::take(&mut item.got);
        item.size = size;
        self.bytes -= dropped;
    }

    fn advance(&mut self, name: &str, bytes: u64) {
        self.item(name).got += bytes;
        self.bytes += bytes;
    }

    fn finish(&mut self, name: &str) {
        let item = self.item(name);
        if !item.done {
            item.done = true;
            self.finished += 1;
        }
    }

    /// The line for the whole group.
    fn overall(&self, color: bool) -> String {
        if self.count == 1
            && let Some(item) = self.items.first()
        {
            return match item.size {
                Some(size) => format!(
                    "  Downloading {} {} {}/{} MiB",
                    self.what,
                    bar(fraction(item.got, size), BAR, color),
                    mib(item.got),
                    mib(size)
                ),
                None => format!("  Downloading {} {} MiB", self.what, mib(item.got)),
            };
        }
        format!(
            "  Downloading {} {} {}/{}  {} MiB",
            self.what,
            bar(
                fraction(self.finished as u64, self.count as u64),
                BAR,
                color
            ),
            self.finished,
            self.count,
            mib(self.bytes)
        )
    }

    /// Every line to draw: the group, then the largest downloads in flight.
    fn lines(&self, color: bool) -> Vec<String> {
        let mut lines = vec![self.overall(color)];
        if self.count <= 1 {
            return lines;
        }
        let mut active: Vec<&Item> = self.items.iter().filter(|i| !i.done).collect();
        active.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        for item in active.iter().take(SHOWN) {
            let name = truncate(&item.name, NAME);
            lines.push(match item.size {
                Some(size) => format!(
                    "    {name:<NAME$} {} {:>5}/{} MiB",
                    bar(fraction(item.got, size), ITEM_BAR, color),
                    mib(item.got),
                    mib(size)
                ),
                None => format!("    {name:<NAME$} {:>5} MiB", mib(item.got)),
            });
        }
        if active.len() > SHOWN {
            lines.push(format!("    … and {} more", active.len() - SHOWN));
        }
        lines
    }

    /// Escape sequences that erase the lines drawn last.
    fn clear(&self) -> String {
        if self.shown == 0 {
            String::new()
        } else {
            format!("\x1b[{}A\r\x1b[J", self.shown)
        }
    }

    /// The line left when the group completes.
    fn summary(&self) -> String {
        let secs = self.begun.elapsed().as_secs_f64();
        if self.bytes == 0 {
            format!("Downloaded {} in {secs:.1}s", self.what)
        } else {
            format!(
                "Downloaded {}: {} MiB in {secs:.1}s",
                self.what,
                mib(self.bytes)
            )
        }
    }
}

fn fraction(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64).min(1.0)
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

fn bar(fraction: f64, width: usize, color: bool) -> String {
    let full = (fraction * width as f64).round() as usize;
    let rest = width - full;
    if color {
        format!(
            "\x1b[36m{}\x1b[0m\x1b[90m{}\x1b[0m",
            "━".repeat(full),
            "━".repeat(rest)
        )
    } else {
        format!("{}{}", "━".repeat(full), "─".repeat(rest))
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}

/// Lets the console interpret escape sequences (Windows only needs asking). False if it cannot.
#[cfg(windows)]
fn enable_escapes() -> bool {
    use std::os::windows::io::AsRawHandle;

    type Handle = *mut std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleMode(handle: Handle, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: Handle, mode: u32) -> i32;
    }
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    let handle = std::io::stderr().as_raw_handle();
    let mut mode = 0;
    // SAFETY: the handle is this process's standard error, and `mode` points to a live u32.
    unsafe {
        GetConsoleMode(handle, &mut mode) != 0
            && (mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0
                || SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0)
    }
}

#[cfg(not(windows))]
fn enable_escapes() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn many_downloads_show_the_count_bytes_and_the_largest_in_flight() {
        let mut s = State::new("8 packages", 8);
        for (i, size) in [9, 1, 15, 3, 2, 4, 5, 6].iter().enumerate() {
            s.start(&format!("pkg{i}"), Some(size * MB));
        }
        s.advance("pkg2", 6 * MB);
        s.advance("pkg1", MB);
        s.finish("pkg1");
        let item = |name: &str, full: usize, got: &str, size: &str| {
            format!(
                "    {name:<20} {}{} {got:>5}/{size} MiB",
                "━".repeat(full),
                "─".repeat(16 - full)
            )
        };
        assert_eq!(
            s.lines(false),
            [
                format!(
                    "  Downloading 8 packages ━━━{} 1/8  7.0 MiB",
                    "─".repeat(21)
                ),
                item("pkg2", 6, "6.0", "15.0"),
                item("pkg0", 0, "0.0", "9.0"),
                item("pkg7", 0, "0.0", "6.0"),
                item("pkg6", 0, "0.0", "5.0"),
                item("pkg5", 0, "0.0", "4.0"),
                "    … and 2 more".to_string(),
            ]
        );
    }

    #[test]
    fn one_download_shows_its_bytes() {
        let mut s = State::new("R 4.6.1 (portable)", 1);
        s.start("R 4.6.1 (portable)", Some(100 * MB));
        s.advance("R 4.6.1 (portable)", 25 * MB);
        assert_eq!(
            s.lines(false),
            [format!(
                "  Downloading R 4.6.1 (portable) ━━━━━━{} 25.0/100.0 MiB",
                "─".repeat(18)
            )]
        );
        let mut unknown = State::new("1 package", 1);
        unknown.start("sf", None);
        unknown.advance("sf", MB / 2);
        assert_eq!(unknown.lines(false), ["  Downloading 1 package 0.5 MiB"]);
    }

    #[test]
    fn a_retry_starts_the_bytes_over() {
        let mut s = State::new("1 package", 1);
        s.start("sf", Some(4 * MB));
        s.advance("sf", 3 * MB);
        s.start("sf", Some(4 * MB));
        s.advance("sf", MB);
        assert_eq!(s.bytes, MB);
        s.finish("sf");
        s.finish("sf");
        assert_eq!(s.finished, 1);
    }

    #[test]
    fn the_summary_names_the_group_and_its_size() {
        let mut s = State::new("2 packages", 2);
        s.advance("a", 3 * MB);
        assert!(
            s.summary()
                .starts_with("Downloaded 2 packages: 3.0 MiB in ")
        );
        let none = State::new("1 source package", 1);
        assert!(
            none.summary()
                .starts_with("Downloaded 1 source package in ")
        );
    }

    #[test]
    fn bars_are_cleared_by_moving_up_over_them() {
        let mut s = State::new("2 packages", 2);
        assert_eq!(s.clear(), "");
        s.shown = 3;
        assert_eq!(s.clear(), "\x1b[3A\r\x1b[J");
    }

    #[test]
    fn long_names_are_cut() {
        assert_eq!(truncate("a".repeat(25).as_str(), 20).chars().count(), 20);
        assert_eq!(truncate("sf", 20), "sf");
    }
}
