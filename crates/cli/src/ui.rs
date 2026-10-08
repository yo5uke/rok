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
    /// Questions already answered yes, by id (`--confirmed`, used by the R package).
    confirmed: Vec<String>,
}

/// A question that someone must answer, raised when rok runs for a program: with `--json` and
/// no terminal, as the R package calls it. Nothing has been changed yet. The program asks the
/// question, then runs rok again with `--confirmed <id>` (yes/no) or `<flag> <value>` (a choice).
#[derive(Debug)]
pub struct NeedsAnswer {
    pub id: String,
    pub question: String,
    pub default: bool,
    /// For a choice: (value, label) of each option, and the flag that passes the value.
    pub options: Vec<(String, String)>,
    pub flag: Option<String>,
}

impl std::fmt::Display for NeedsAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (needs an answer)", self.question)
    }
}

impl std::error::Error for NeedsAnswer {}

impl NeedsAnswer {
    pub fn to_json(&self) -> serde_json::Value {
        let mut needs = serde_json::json!({
            "id": self.id,
            "question": self.question,
            "default": self.default,
        });
        if let Some(flag) = &self.flag {
            needs["flag"] = flag.as_str().into();
            needs["options"] = self
                .options
                .iter()
                .map(|(value, label)| serde_json::json!({ "value": value, "label": label }))
                .collect();
        }
        serde_json::json!({ "needs": needs })
    }
}

const GREEN: &str = "32";
const RED: &str = "31";
const YELLOW: &str = "33";
const BLUE: &str = "34";
const CYAN: &str = "36";
const BOLD_YELLOW: &str = "1;33";

impl Ui {
    pub fn new(yes: bool, json: bool, confirmed: Vec<String>) -> Ui {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Ui {
            color: std::io::stderr().is_terminal() && !no_color,
            yes,
            json,
            confirmed,
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
            let note = c
                .note
                .as_ref()
                .map(|n| format!(" ({n})"))
                .unwrap_or_default();
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
                (Some(from), Some(to)) if to == from => {
                    format!("{} {} {to}", self.paint(BLUE, "~"), c.name)
                }
                (Some(from), Some(to)) => {
                    format!("{} {} {from} → {to}", self.paint(YELLOW, "↓"), c.name)
                }
                (None, None) => continue,
            };
            self.line(&format!("  {s}{note}"));
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

    /// Asks a yes/no question, identified by `id`. With `--yes` (or `--confirmed <id>`) the
    /// answer is yes. Without a terminal there is no one to ask: for a program (`--json`) the
    /// question is returned as [`NeedsAnswer`], otherwise it is an error that tells how to
    /// proceed.
    pub fn confirm(&self, id: &str, question: &str, default: bool) -> anyhow::Result<bool> {
        if self.yes || self.confirmed.iter().any(|c| c == id) {
            return Ok(true);
        }
        if !std::io::stdin().is_terminal() {
            if self.json {
                return Err(NeedsAnswer {
                    id: id.to_string(),
                    question: question.to_string(),
                    default,
                    options: Vec::new(),
                    flag: None,
                }
                .into());
            }
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

    /// Asks to pick one of `options` ((value, label), numbered from 1; the first is the
    /// default). With `--yes` the first is picked. Without a terminal, a program (`--json`)
    /// gets a [`NeedsAnswer`]; otherwise it is an error that says to pass `flag <value>`.
    pub fn choose(
        &self,
        id: &str,
        question: &str,
        options: &[(String, String)],
        flag: &str,
    ) -> anyhow::Result<usize> {
        if self.yes {
            return Ok(0);
        }
        if !std::io::stdin().is_terminal() {
            if self.json {
                return Err(NeedsAnswer {
                    id: id.to_string(),
                    question: question.to_string(),
                    default: true,
                    options: options.to_vec(),
                    flag: Some(flag.to_string()),
                }
                .into());
            }
            let mut msg = question.to_string();
            for (i, (value, label)) in options.iter().enumerate() {
                msg.push_str(&format!("\n  {}. {label} ({flag} {value})", i + 1));
            }
            anyhow::bail!("{msg}\nThis needs a choice; run again with one of the flags shown.");
        }
        self.line(&format!("{} {question}", self.paint(CYAN, "?")));
        for (i, (_, label)) in options.iter().enumerate() {
            self.line(&format!("  {}. {label}", i + 1));
        }
        loop {
            let _ = write!(std::io::stderr(), "  Choose 1-{} [1]: ", options.len());
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            if std::io::stdin().lock().read_line(&mut answer)? == 0 {
                anyhow::bail!("Cancelled. Nothing was changed.");
            }
            match answer.trim() {
                "" => return Ok(0),
                a => match a.parse::<usize>() {
                    Ok(n) if (1..=options.len()).contains(&n) => return Ok(n - 1),
                    _ => continue,
                },
            }
        }
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
                "note": c.note,
            })
        })
        .collect()
}
