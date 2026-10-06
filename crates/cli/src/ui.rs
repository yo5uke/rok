//! Messages for people (on stderr) and data for programs (JSON on stdout).
//!
//! Messages follow the tidyverse style: a symbol, then one line stating what happened, then
//! bullets with details. Colours are used only on a terminal and never when `NO_COLOR` is set.

use std::io::{BufRead, IsTerminal, Write};

use rok_core::ops::Change;

pub struct Ui {
    color: bool,
    /// Answer yes to every question (`--yes`).
    pub yes: bool,
    /// Also print structured results as JSON on stdout (`--json`).
    pub json: bool,
}

const GREEN: &str = "32";
const RED: &str = "31";
const YELLOW: &str = "33";
const BLUE: &str = "34";
const CYAN: &str = "36";
const BOLD_YELLOW: &str = "1;33";

impl Ui {
    pub fn new(yes: bool, json: bool) -> Ui {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Ui {
            color: std::io::stderr().is_terminal() && !no_color,
            yes,
            json,
        }
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    fn line(&self, s: &str) {
        let _ = writeln!(std::io::stderr(), "{s}");
    }

    /// A line without a symbol.
    pub fn line_plain(&self, msg: &str) {
        self.line(msg);
    }

    pub fn success(&self, msg: &str) {
        self.line(&format!("{} {msg}", self.paint(GREEN, "✔")));
    }

    pub fn info(&self, msg: &str) {
        self.line(&format!("{} {msg}", self.paint(CYAN, "ℹ")));
    }

    pub fn warn(&self, msg: &str) {
        self.line(&format!("{} {msg}", self.paint(YELLOW, "!")));
    }

    pub fn error(&self, msg: &str) {
        let mut lines = msg.lines();
        if let Some(first) = lines.next() {
            self.line(&format!("{} {first}", self.paint(RED, "✖")));
        }
        for l in lines {
            self.line(&format!("  {l}"));
        }
    }

    pub fn bullet(&self, msg: &str) {
        self.line(&format!("  • {msg}"));
    }

    /// A progress message, such as "Downloading 12 packages".
    pub fn step(&self, msg: &str) {
        self.line(&format!("  {msg}"));
    }

    /// Lists changed packages: `+` added, `-` removed, `↑` upgraded, `↓` downgraded. A new
    /// major version is marked, since it may break code.
    pub fn changes(&self, changes: &[Change]) {
        for c in changes {
            let s = match (&c.from, &c.to) {
                (None, Some(to)) => format!("{} {} {to}", self.paint(GREEN, "+"), c.name),
                (Some(from), None) => format!("{} {} {from}", self.paint(RED, "-"), c.name),
                (Some(from), Some(to)) if to > from => {
                    let major = to.parts()[0] > from.parts()[0];
                    let mark = if major {
                        format!(" {}", self.paint(BOLD_YELLOW, "(new major version)"))
                    } else {
                        String::new()
                    };
                    format!("{} {} {from} → {to}{mark}", self.paint(BLUE, "↑"), c.name)
                }
                (Some(from), Some(to)) => {
                    format!("{} {} {from} → {to}", self.paint(YELLOW, "↓"), c.name)
                }
                (None, None) => continue,
            };
            self.line(&format!("  {s}"));
        }
    }

    /// A package kept at its version, with the reason.
    pub fn held(&self, name: &str, version: &str, reason: &str) {
        self.line(&format!(
            "  {} {name} {version} ({reason})",
            self.paint(CYAN, "=")
        ));
    }

    /// The symbol for a status level.
    pub fn level(&self, level: rok_core::status::Level, msg: &str) {
        match level {
            rok_core::status::Level::Error => self.error(msg),
            rok_core::status::Level::Warning => self.warn(msg),
            rok_core::status::Level::Info => self.info(msg),
        }
    }

    /// Asks a yes/no question. With `--yes` the answer is yes; without a terminal there is no
    /// one to ask, so it is an error that tells how to proceed.
    pub fn confirm(&self, question: &str, default: bool) -> anyhow::Result<bool> {
        if self.yes {
            return Ok(true);
        }
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(
                "{question}\nThis needs confirmation; run again with `--yes` to proceed."
            );
        }
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        let _ = write!(
            std::io::stderr(),
            "{} {question} {hint} ",
            self.paint(CYAN, "?")
        );
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        Ok(match answer.trim().to_ascii_lowercase().as_str() {
            "" => default,
            "y" | "yes" => true,
            _ => false,
        })
    }

    /// Prints a JSON result on stdout when `--json` was given.
    pub fn result(&self, value: serde_json::Value) {
        if self.json {
            println!("{value}");
        }
    }
}

/// The JSON form of package changes.
pub fn changes_json(changes: &[Change]) -> serde_json::Value {
    changes
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "from": c.from.as_ref().map(|v| v.to_string()),
                "to": c.to.as_ref().map(|v| v.to_string()),
            })
        })
        .collect()
}
