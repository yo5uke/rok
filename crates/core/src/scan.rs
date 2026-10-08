//! The code scanner (requirements chapter 7): which packages the project's R code uses, and
//! which packages the scanner rules suggest. It only suggests; nothing is added by itself.
//!
//! R scripts are read with a small lexer that knows comments, strings (including raw strings)
//! and backquoted names, so `# library(x)` and `"pkg::fun"` are not taken for code. In
//! R Markdown and Quarto files only the R chunks and inline R code are read. Results per file
//! are cached in `.rok/scan.json`, so only changed files are read again.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dcf::is_package_name;
use crate::manifest::ScanRule;

/// Files larger than this are not read (generated or data files with an R extension).
const MAX_FILE: u64 = 5 * 1024 * 1024;
/// Directories never entered (besides hidden ones and nested projects).
const SKIP_DIRS: [&str; 4] = ["renv", "packrat", "node_modules", "__pycache__"];
const CACHE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("scan.rule[{index}]: {message}")]
    Rule { index: usize, message: String },
}

// ---- lexing ----

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Str(String),
    Op(String),
    Num,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    tok: Tok,
    line: u32,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '.' || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '.' || c == '_'
}

/// Splits R code into tokens. `line` is the line number of the first line.
fn lex(src: &str, mut line: u32) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let push = |out: &mut Vec<Token>, tok: Tok, line: u32| out.push(Token { tok, line });
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            // Raw strings: r"(...)", R"[...]", r"--{...}--".
            'r' | 'R'
                if matches!(chars.get(i + 1), Some('"') | Some('\''))
                    && raw_string_end(&chars, i).is_some() =>
            {
                let (end, content, newlines) = raw_string_end(&chars, i).expect("checked");
                push(&mut out, Tok::Str(content), line);
                line += newlines;
                i = end;
            }
            '"' | '\'' => {
                let start_line = line;
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != c {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        if chars[i + 1] == '\n' {
                            line += 1;
                        }
                        s.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                i += 1;
                push(&mut out, Tok::Str(s), start_line);
            }
            '`' => {
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '`' {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                i += 1;
                push(&mut out, Tok::Ident(s), line);
            }
            '%' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '%' && chars[j] != '\n' {
                    j += 1;
                }
                push(&mut out, Tok::Op("%%".into()), line);
                i = j + 1;
            }
            ':' => {
                let n = chars[i..].iter().take(3).take_while(|c| **c == ':').count();
                push(&mut out, Tok::Op(":".repeat(n)), line);
                i += n;
            }
            c if c.is_ascii_digit()
                || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) =>
            {
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                push(&mut out, Tok::Num, line);
            }
            c if is_ident_start(c) => {
                let start = i;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
                push(&mut out, Tok::Ident(chars[start..i].iter().collect()), line);
            }
            c if c.is_whitespace() => i += 1,
            c => {
                push(&mut out, Tok::Op(c.to_string()), line);
                i += 1;
            }
        }
    }
    out
}

/// For a raw string starting at `i` (`r"`): the index after it, its content and the number of
/// newlines in it. `None` if it is not a well-formed raw string.
fn raw_string_end(chars: &[char], i: usize) -> Option<(usize, String, u32)> {
    let quote = chars[i + 1];
    let mut j = i + 2;
    let dashes = chars[j..].iter().take_while(|c| **c == '-').count();
    j += dashes;
    let close = match chars.get(j)? {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    j += 1;
    let start = j;
    while j < chars.len() {
        if chars[j] == close
            && chars[j + 1..].iter().take(dashes).all(|c| *c == '-')
            && chars.get(j + 1 + dashes) == Some(&quote)
        {
            let content: String = chars[start..j].iter().collect();
            let newlines = content.matches('\n').count() as u32;
            return Some((j + dashes + 2, content, newlines));
        }
        j += 1;
    }
    None
}

// ---- what the code does ----

/// A function call: its name, the package if written `pkg::fun`, and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    name: String,
    pkg: Option<String>,
    line: u32,
    args: Vec<Arg>,
}

/// An argument: its name if given, and its value if it is a single string or name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Arg {
    name: Option<String>,
    value: Option<Tok>,
}

impl Call {
    fn named(&self, name: &str) -> Option<&Arg> {
        self.args.iter().find(|a| a.name.as_deref() == Some(name))
    }

    /// The `n`th (from 1) argument without a name.
    fn positional(&self, n: usize) -> Option<&Arg> {
        self.args
            .iter()
            .filter(|a| a.name.is_none())
            .nth(n.checked_sub(1)?)
    }
}

/// The calls in a token stream.
fn calls(tokens: &[Token]) -> Vec<Call> {
    // The matching `)` of every `(`.
    let mut close = vec![usize::MAX; tokens.len()];
    let mut stack = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        match &t.tok {
            Tok::Op(o) if o == "(" => stack.push(i),
            Tok::Op(o) if o == ")" => {
                if let Some(open) = stack.pop() {
                    close[open] = i;
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for i in 1..tokens.len() {
        let (Tok::Op(paren), Tok::Ident(name)) = (&tokens[i].tok, &tokens[i - 1].tok) else {
            continue;
        };
        if paren != "(" || close[i] == usize::MAX {
            continue;
        }
        let before = i.checked_sub(2).map(|j| &tokens[j].tok);
        let pkg = match before {
            Some(Tok::Op(o)) if o == "::" || o == ":::" => {
                match i.checked_sub(3).map(|j| &tokens[j].tok) {
                    Some(Tok::Ident(p)) => Some(p.clone()),
                    _ => continue,
                }
            }
            // `x$fun(` and `x@fun(` call something else.
            Some(Tok::Op(o)) if o == "$" || o == "@" => continue,
            _ => None,
        };
        out.push(Call {
            name: name.clone(),
            pkg,
            line: tokens[i - 1].line,
            args: split_args(&tokens[i + 1..close[i]]),
        });
    }
    out
}

/// The top-level arguments between a call's parentheses.
fn split_args(tokens: &[Token]) -> Vec<Arg> {
    let mut args = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, t) in tokens.iter().enumerate() {
        if let Tok::Op(o) = &t.tok {
            match o.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                "," if depth == 0 => {
                    args.push(arg(&tokens[start..i]));
                    start = i + 1;
                }
                _ => {}
            }
        }
    }
    if start < tokens.len() {
        args.push(arg(&tokens[start..]));
    }
    args
}

fn arg(tokens: &[Token]) -> Arg {
    let (name, rest) = match tokens {
        [
            Token {
                tok: Tok::Ident(n) | Tok::Str(n),
                ..
            },
            Token {
                tok: Tok::Op(eq), ..
            },
            rest @ ..,
        ] if eq == "=" => (Some(n.clone()), rest),
        _ => (None, tokens),
    };
    let value = match rest {
        [t] if matches!(t.tok, Tok::Str(_) | Tok::Ident(_)) => Some(t.tok.clone()),
        _ => None,
    };
    Arg { name, value }
}

/// Packages that the code loads or uses with `pkg::`, with the lines.
fn packages_used(tokens: &[Token], calls: &[Call]) -> BTreeMap<String, Vec<u32>> {
    let mut out: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut add = |name: &str, line: u32| {
        if is_package_name(name) {
            out.entry(name.to_string()).or_default().push(line);
        }
    };
    for w in tokens.windows(3) {
        if let (Tok::Ident(pkg), Tok::Op(op), Tok::Ident(_)) = (&w[0].tok, &w[1].tok, &w[2].tok)
            && (op == "::" || op == ":::")
        {
            add(pkg, w[0].line);
        }
    }
    for c in calls {
        if c.pkg
            .as_deref()
            .is_some_and(|p| p != "base" && p != "pacman")
        {
            continue;
        }
        let first = c.named("package").or_else(|| c.positional(1));
        match c.name.as_str() {
            "library" | "require" => {
                let by_name = c
                    .named("character.only")
                    .and_then(|a| a.value.as_ref())
                    .is_some_and(|v| matches!(v, Tok::Ident(t) if t == "TRUE" || t == "T"));
                match first.and_then(|a| a.value.as_ref()) {
                    Some(Tok::Str(p)) => add(p, c.line),
                    Some(Tok::Ident(p)) if !by_name => add(p, c.line),
                    _ => {}
                }
            }
            "requireNamespace" | "loadNamespace" => {
                if let Some(Tok::Str(p)) = first.and_then(|a| a.value.as_ref()) {
                    add(p, c.line);
                }
            }
            "p_load" => {
                for a in c.args.iter().filter(|a| a.name.is_none()) {
                    if let Some(Tok::Str(p) | Tok::Ident(p)) = &a.value {
                        add(p, c.line);
                    }
                }
            }
            _ => {}
        }
    }
    for lines in out.values_mut() {
        lines.sort_unstable();
        lines.dedup();
    }
    out
}

// ---- rules ----

/// A scanner rule: when the condition holds, suggest the packages.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub when: Condition,
    pub suggest: Vec<String>,
    /// Suggest the matched argument value itself (for example `modelsummary(output = "gt")`).
    pub suggest_value: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// A file with one of these extensions exists.
    File(Vec<String>),
    /// An R Markdown or Quarto chunk in this language exists.
    Chunk(String),
    /// One of these functions is called, optionally with an argument value.
    Call {
        names: Vec<String>,
        arg: Option<ArgCondition>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArgCondition {
    pub name: String,
    pub position: Option<usize>,
    pub values: Vec<String>,
    pub suffix: Option<String>,
}

fn strings(v: &toml::Value) -> Option<Vec<String>> {
    match v {
        toml::Value::String(s) => Some(vec![s.clone()]),
        toml::Value::Array(a) => a.iter().map(|x| x.as_str().map(str::to_string)).collect(),
        _ => None,
    }
}

impl Rule {
    /// A rule from its `when` table and suggestions (as in rok.toml's `[[scan.rule]]`).
    pub fn new(
        when: &toml::Table,
        suggest: Vec<String>,
        suggest_value: bool,
    ) -> Result<Rule, String> {
        let allowed = [
            "file", "chunk", "call", "arg", "position", "value", "suffix",
        ];
        if let Some(k) = when.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(format!(
                "unknown condition `{k}` (use {})",
                allowed.join(", ")
            ));
        }
        let text = |k: &str| -> Result<Option<Vec<String>>, String> {
            when.get(k)
                .map(|v| {
                    strings(v).ok_or_else(|| format!("`{k}` must be a string or a list of strings"))
                })
                .transpose()
        };
        let condition = match (text("file")?, text("chunk")?, text("call")?) {
            (Some(exts), None, None) => Condition::File(exts),
            (None, Some(c), None) if c.len() == 1 => Condition::Chunk(c[0].to_ascii_lowercase()),
            (None, None, Some(names)) => {
                let arg = match text("arg")? {
                    None => None,
                    Some(a) if a.len() == 1 => Some(ArgCondition {
                        name: a[0].clone(),
                        position: match when.get("position") {
                            None => None,
                            Some(p) => Some(
                                p.as_integer()
                                    .filter(|n| *n >= 1)
                                    .ok_or("`position` must be a number from 1")?
                                    as usize,
                            ),
                        },
                        values: text("value")?.unwrap_or_default(),
                        suffix: text("suffix")?.map(|s| s.concat()),
                    }),
                    Some(_) => return Err("`arg` must be one name".into()),
                };
                Condition::Call { names, arg }
            }
            _ => {
                return Err("give exactly one of `file`, `chunk` (one language) and `call`".into());
            }
        };
        let has_arg = matches!(&condition, Condition::Call { arg: Some(_), .. });
        if !has_arg
            && ["position", "value", "suffix"]
                .iter()
                .any(|k| when.contains_key(*k))
        {
            return Err("`position`, `value` and `suffix` need `call` and `arg`".into());
        }
        if suggest_value && !has_arg {
            return Err("suggesting the argument's value needs `call` and `arg`".into());
        }
        Ok(Rule {
            when: condition,
            suggest,
            suggest_value,
        })
    }

    /// Describes the condition for messages: `geom_sf()`, `.qmd files`, `python chunks`.
    fn describe(&self) -> String {
        match &self.when {
            Condition::File(exts) => format!(".{} files", exts[0]),
            Condition::Chunk(lang) => format!("{lang} chunks"),
            Condition::Call { names, arg: None } => format!("{}()", names.join("()/")),
            Condition::Call {
                names,
                arg: Some(a),
            } => format!("{}({} = ...)", names[0], a.name),
        }
    }
}

/// The rules that come with rok.
pub fn builtin_rules() -> Vec<Rule> {
    #[derive(Deserialize)]
    struct File {
        rule: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        when: toml::Table,
        #[serde(default)]
        suggest: Vec<String>,
        #[serde(default)]
        suggest_value: bool,
    }
    let file: File = toml::from_str(include_str!("scan_rules.toml")).expect("valid built-in rules");
    file.rule
        .into_iter()
        .map(|e| Rule::new(&e.when, e.suggest, e.suggest_value).expect("valid built-in rule"))
        .collect()
}

/// The built-in rules followed by the project's own.
pub fn rules(project: &[ScanRule]) -> Result<Vec<Rule>, ScanError> {
    let mut out = builtin_rules();
    for (index, r) in project.iter().enumerate() {
        out.push(
            Rule::new(&r.when, r.suggest.clone(), false)
                .map_err(|message| ScanError::Rule { index, message })?,
        );
    }
    Ok(out)
}

/// A rule that applies in a file: the packages it suggests, where, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Hit {
    packages: Vec<String>,
    line: u32,
    why: String,
}

fn call_matches(rule_name: &str, call: &Call) -> bool {
    match rule_name.split_once("::") {
        Some((pkg, name)) => call.name == name && call.pkg.as_deref().is_none_or(|p| p == pkg),
        None => call.name == rule_name,
    }
}

fn hits(rules: &[Rule], ext: &str, chunks: &[(String, u32)], calls: &[Call]) -> Vec<Hit> {
    let mut out = Vec::new();
    for rule in rules {
        let why = rule.describe();
        match &rule.when {
            Condition::File(exts) => {
                if exts.iter().any(|e| e == ext) {
                    out.push(Hit {
                        packages: rule.suggest.clone(),
                        line: 0,
                        why,
                    });
                }
            }
            Condition::Chunk(lang) => {
                if let Some((_, line)) = chunks.iter().find(|(l, _)| l == lang) {
                    out.push(Hit {
                        packages: rule.suggest.clone(),
                        line: *line,
                        why,
                    });
                }
            }
            Condition::Call { names, arg } => {
                for call in calls
                    .iter()
                    .filter(|c| names.iter().any(|n| call_matches(n, c)))
                {
                    let value = match arg {
                        None => None,
                        Some(a) => {
                            let found = call
                                .named(&a.name)
                                .or_else(|| a.position.and_then(|p| call.positional(p)));
                            let Some(Tok::Str(v)) = found.and_then(|x| x.value.as_ref()) else {
                                continue;
                            };
                            let ok = (a.values.is_empty() && a.suffix.is_none())
                                || a.values.iter().any(|x| x == v)
                                || a.suffix.as_ref().is_some_and(|s| {
                                    v.to_ascii_lowercase().ends_with(&s.to_ascii_lowercase())
                                });
                            if !ok {
                                continue;
                            }
                            Some(v.clone())
                        }
                    };
                    let mut packages = rule.suggest.clone();
                    if rule.suggest_value
                        && let Some(v) = &value
                        && is_package_name(v)
                    {
                        packages.push(v.clone());
                    }
                    out.push(Hit {
                        packages,
                        line: call.line,
                        why: why.clone(),
                    });
                }
            }
        }
    }
    out
}

// ---- files ----

/// What one file says about packages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Facts {
    packages: BTreeMap<String, Vec<u32>>,
    hits: Vec<Hit>,
}

fn extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    matches!(ext, "R" | "r" | "Rmd" | "rmd" | "qmd").then(|| ext.to_string())
}

/// Reads a file: R scripts whole; R Markdown and Quarto by their chunks and inline code.
fn read_facts(text: &str, ext: &str, rules: &[Rule]) -> Facts {
    let mut tokens = Vec::new();
    let mut chunks = Vec::new();
    if ext == "R" || ext == "r" {
        tokens = lex(text, 1);
    } else {
        let mut in_chunk: Option<(String, String, u32)> = None; // (fence, language, first line)
        let mut code = String::new();
        for (i, line) in text.lines().enumerate() {
            let n = i as u32 + 1;
            let trimmed = line.trim_start();
            if let Some((fence, lang, start)) = &in_chunk {
                if trimmed.starts_with(fence.as_str())
                    && trimmed
                        .trim_end()
                        .trim_end_matches(fence.chars().next().unwrap_or('`'))
                        .is_empty()
                {
                    if lang == "r" {
                        tokens.extend(lex(&code, *start));
                    }
                    code.clear();
                    in_chunk = None;
                } else {
                    code.push_str(line);
                    code.push('\n');
                }
                continue;
            }
            let fence: String = trimmed
                .chars()
                .take_while(|c| *c == '`' || *c == '~')
                .collect();
            if fence.len() >= 3 && trimmed[fence.len()..].trim_start().starts_with('{') {
                let header = trimmed[fence.len()..].trim_start()[1..].trim_start();
                let lang: String = header
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<String>()
                    .to_ascii_lowercase();
                chunks.push((lang.clone(), n));
                in_chunk = Some((fence, lang, n + 1));
                continue;
            }
            // Inline R code: `r expr`.
            let mut rest = line;
            while let Some(at) = rest.find("`r ") {
                let after = &rest[at + 3..];
                let end = after.find('`').unwrap_or(after.len());
                tokens.extend(lex(&after[..end], n));
                rest = &after[end.min(after.len())..];
                if rest.starts_with('`') {
                    rest = &rest[1..];
                }
            }
        }
    }
    let calls = calls(&tokens);
    Facts {
        packages: packages_used(&tokens, &calls),
        hits: hits(rules, ext, &chunks, &calls),
    }
}

// ---- the project ----

/// Where something was found: a file relative to the project root, and a line (0 for the
/// file as a whole).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Place {
    pub file: String,
    pub line: u32,
}

impl std::fmt::Display for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            f.write_str(&self.file)
        } else {
            write!(f, "{}:{}", self.file, self.line)
        }
    }
}

/// What the scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Packages the code uses directly, with where.
    pub used: BTreeMap<String, Vec<Place>>,
    /// Packages the rules suggest, with why and where.
    pub suggested: BTreeMap<String, Vec<(String, Place)>>,
}

impl Report {
    /// Whether the code needs `name`, directly or through a rule.
    pub fn needs(&self, name: &str) -> bool {
        self.used.contains_key(name) || self.suggested.contains_key(name)
    }

    /// The first place that needs `name`, with why for a rule, for messages.
    pub fn first_reason(&self, name: &str) -> Option<String> {
        if let Some(p) = self.used.get(name).and_then(|v| v.first()) {
            return Some(p.to_string());
        }
        let (why, p) = self.suggested.get(name)?.first()?;
        Some(format!("for {why} in {p}"))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    rules: String,
    files: BTreeMap<String, Entry>,
    /// Packages already suggested at startup, so only new ones are shown.
    #[serde(default)]
    shown: BTreeSet<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    mtime: u128,
    size: u64,
    facts: Facts,
}

/// How much of the project to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every directory (sync, status, remove, why).
    Full,
    /// Only directories where code was found before (R startup); falls back to a full scan
    /// the first time.
    Quick,
}

fn cache_path(root: &Path) -> PathBuf {
    root.join(".rok").join("scan.json")
}

fn rules_key(rules: &[Rule]) -> String {
    let digest = Sha256::digest(format!("{CACHE_VERSION}{rules:?}").as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The code files of the project: every directory except hidden ones, `renv` and the like,
/// and nested projects. Symbolic links to directories are not followed.
fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let nested = path.join(crate::manifest::FILE_NAME).is_file();
            // An installed package (in a library inside the project) is not the project's code.
            let installed = path.join("Meta").is_dir() && path.join("DESCRIPTION").is_file();
            if !name.starts_with('.')
                && !SKIP_DIRS.contains(&name.as_str())
                && !nested
                && !installed
            {
                walk(root, &path, out);
            }
        } else if extension(&path).is_some() && !(dir == root && name.starts_with('.')) {
            out.push(path);
        }
    }
}

/// Scans the project at `root` with `rules`, reading only files that changed since the last
/// scan, and updates the cache.
pub fn scan(root: &Path, rules: &[Rule], mode: Mode) -> Result<Report, ScanError> {
    let key = rules_key(rules);
    let mut cache: Cache = std::fs::read(cache_path(root))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|c: &Cache| c.version == CACHE_VERSION && c.rules == key)
        .unwrap_or_default();
    let files = match mode {
        Mode::Quick if !cache.files.is_empty() => {
            // The directories that had code, and the files known before.
            let mut dirs: BTreeSet<PathBuf> = cache
                .files
                .keys()
                .filter_map(|f| root.join(f).parent().map(Path::to_path_buf))
                .collect();
            dirs.insert(root.to_path_buf());
            let mut out = Vec::new();
            for d in dirs {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let p = e.path();
                    if e.file_type().is_ok_and(|t| t.is_file())
                        && extension(&p).is_some()
                        && !(d == root && e.file_name().to_string_lossy().starts_with('.'))
                    {
                        out.push(p);
                    }
                }
            }
            out
        }
        _ => {
            let mut out = Vec::new();
            walk(root, root, &mut out);
            out
        }
    };
    let mut seen = HashSet::new();
    let mut changed = cache.version != CACHE_VERSION;
    for path in files {
        let rel = relative(root, &path);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() > MAX_FILE {
            continue;
        }
        seen.insert(rel.clone());
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        if cache
            .files
            .get(&rel)
            .is_some_and(|e| e.mtime == mtime && e.size == meta.len())
        {
            continue;
        }
        let text = std::fs::read(&path).map_err(|source| ScanError::Io {
            path: path.clone(),
            source,
        })?;
        let ext = extension(&path).expect("code file");
        let facts = read_facts(&String::from_utf8_lossy(&text), &ext, rules);
        cache.files.insert(
            rel,
            Entry {
                mtime,
                size: meta.len(),
                facts,
            },
        );
        changed = true;
    }
    let before = cache.files.len();
    cache
        .files
        .retain(|f, _| seen.contains(f) || (mode == Mode::Quick && root.join(f).is_file()));
    changed |= cache.files.len() != before;
    if changed || cache.rules != key {
        cache.version = CACHE_VERSION;
        cache.rules = key;
        save_cache(root, &cache)?;
    }

    let mut report = Report::default();
    for (file, entry) in &cache.files {
        for (pkg, lines) in &entry.facts.packages {
            let places = report.used.entry(pkg.clone()).or_default();
            places.extend(lines.iter().map(|l| Place {
                file: file.clone(),
                line: *l,
            }));
        }
        for hit in &entry.facts.hits {
            for pkg in &hit.packages {
                report.suggested.entry(pkg.clone()).or_default().push((
                    hit.why.clone(),
                    Place {
                        file: file.clone(),
                        line: hit.line,
                    },
                ));
            }
        }
    }
    Ok(report)
}

fn save_cache(root: &Path, cache: &Cache) -> Result<(), ScanError> {
    let path = cache_path(root);
    // A project without .rok/ (not initialised) is scanned without a cache.
    if !root.join(".rok").is_dir() {
        return Ok(());
    }
    let bytes = serde_json::to_vec(cache).expect("serializable");
    crate::fsutil::write_atomic(&path, &bytes).map_err(|source| ScanError::Io { path, source })
}

/// Of `names`, those not suggested at startup before; they are remembered as shown.
pub fn new_since_last_time(
    root: &Path,
    names: &BTreeSet<String>,
) -> Result<Vec<String>, ScanError> {
    let Some(mut cache) = std::fs::read(cache_path(root))
        .ok()
        .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
    else {
        return Ok(names.iter().cloned().collect());
    };
    let new: Vec<String> = names.difference(&cache.shown).cloned().collect();
    if !new.is_empty() || cache.shown != *names {
        cache.shown = names.clone();
        save_cache(root, &cache)?;
    }
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(code: &str, ext: &str) -> Facts {
        read_facts(code, ext, &builtin_rules())
    }

    fn used(code: &str) -> Vec<String> {
        facts(code, "R").packages.into_keys().collect()
    }

    #[test]
    fn finds_loaded_and_qualified_packages() {
        let code = r#"
library(fixest)
suppressPackageStartupMessages(library("dplyr"))
require(data.table)
requireNamespace("arrow", quietly = TRUE)
x <- sf::st_read("a.gpkg")
pacman::p_load(readxl, "fs")
library(pkg, character.only = TRUE)   # pkg is a variable
requireNamespace(other)               # so is other
"#;
        assert_eq!(
            used(code),
            [
                "arrow",
                "data.table",
                "dplyr",
                "fixest",
                "fs",
                "pacman",
                "readxl",
                "sf"
            ]
        );
    }

    #[test]
    fn ignores_comments_and_strings() {
        let code = r#"
# library(commented)
x <- "library(instring)"
y <- 'pkg::in_single'
z <- r"(library(inraw) ")"
w <- r"--[ ggplot2::fake ]--"
library(real) # library(after)
"#;
        assert_eq!(used(code), ["real"]);
        assert_eq!(facts(code, "R").packages["real"], [7]);
    }

    #[test]
    fn reads_only_r_code_in_documents() {
        let doc = "---\ntitle: x\n---\n\nText mentioning library(prose).\n\n```{r setup, include=FALSE}\nlibrary(knitr)\n```\n\n```{python}\nimport pandas\n```\n\nThe mean is `r dplyr::n()`.\n\n````{r}\nlibrary(insidefence)\n````\n";
        let f = facts(doc, "qmd");
        assert_eq!(
            f.packages.keys().collect::<Vec<_>>(),
            ["dplyr", "insidefence", "knitr"]
        );
        assert_eq!(f.packages["knitr"], [8]);
        assert_eq!(f.packages["dplyr"], [15]);
        let suggested: Vec<&String> = f.hits.iter().flat_map(|h| &h.packages).collect();
        assert_eq!(suggested, ["knitr", "rmarkdown", "reticulate"]);
    }

    #[test]
    fn applies_rules_with_arguments() {
        let code = r#"
p + geom_sf() + ggplot2::coord_sf()
ggsave("plot.SVG", p)
ggsave(filename = "plot.png")
modelsummary(models, output = "gt")
modelsummary(models, output = "latex")
x$geom_hex()
"#;
        let f = facts(code, "R");
        let got: Vec<(String, u32)> = f
            .hits
            .iter()
            .flat_map(|h| h.packages.iter().map(move |p| (p.clone(), h.line)))
            .collect();
        assert_eq!(
            got,
            [
                ("sf".to_string(), 2),
                ("sf".to_string(), 2),
                ("svglite".to_string(), 3),
                ("gt".to_string(), 5)
            ]
        );
    }

    #[test]
    fn validates_project_rules() {
        let table = |s: &str| -> toml::Table { toml::from_str(s).unwrap() };
        let r = Rule::new(
            &table(r#"call = "mytools::render_report""#),
            vec!["officer".into()],
            false,
        )
        .unwrap();
        let call = |pkg: Option<&str>| Call {
            name: "render_report".into(),
            pkg: pkg.map(str::to_string),
            line: 1,
            args: Vec::new(),
        };
        let Condition::Call { names, .. } = &r.when else {
            panic!()
        };
        assert!(call_matches(&names[0], &call(Some("mytools"))));
        assert!(call_matches(&names[0], &call(None)));
        assert!(!call_matches(&names[0], &call(Some("other"))));
        assert!(Rule::new(&table(r#"calls = "x""#), vec![], false).is_err());
        assert!(
            Rule::new(
                &table(
                    r#"file = "qmd"
call = "x""#
                ),
                vec![],
                false
            )
            .is_err()
        );
        assert!(
            Rule::new(
                &table(
                    r#"file = "qmd"
suffix = ".svg""#
                ),
                vec![],
                false
            )
            .is_err()
        );
    }

    #[test]
    fn scans_a_project_with_a_cache() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join(".rok")).unwrap();
        std::fs::create_dir_all(root.join("code")).unwrap();
        std::fs::create_dir_all(root.join("renv/library")).unwrap();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("code/a.R"), "library(fixest)\n").unwrap();
        std::fs::write(
            root.join("report.qmd"),
            "```{r}\nggplot2::geom_hex()\n```\n",
        )
        .unwrap();
        std::fs::write(root.join("renv/library/x.R"), "library(ignored)\n").unwrap();
        std::fs::write(root.join("nested/rok.toml"), "").unwrap();
        std::fs::write(root.join("nested/n.R"), "library(ignored2)\n").unwrap();
        std::fs::write(root.join(".Rprofile"), "library(hidden)\n").unwrap();
        // A library with an installed package inside the project.
        std::fs::create_dir_all(root.join("lib/pkg/Meta")).unwrap();
        std::fs::create_dir_all(root.join("lib/pkg/doc")).unwrap();
        std::fs::write(root.join("lib/pkg/DESCRIPTION"), "Package: pkg\n").unwrap();
        std::fs::write(root.join("lib/pkg/doc/vignette.R"), "library(ignored3)\n").unwrap();
        let rules = builtin_rules();

        let r = scan(root, &rules, Mode::Full).unwrap();
        assert_eq!(r.used.keys().collect::<Vec<_>>(), ["fixest", "ggplot2"]);
        assert_eq!(r.first_reason("fixest").unwrap(), "code/a.R:1");
        assert_eq!(
            r.first_reason("hexbin").unwrap(),
            "for geom_hex() in report.qmd:2"
        );
        assert!(r.needs("knitr") && r.needs("rmarkdown"));
        assert!(root.join(".rok/scan.json").is_file());

        // A changed file is read again; a quick scan finds new files in known directories.
        std::fs::write(root.join("code/a.R"), "library(fixest)\nlibrary(broom)\n").unwrap();
        std::fs::write(root.join("code/b.R"), "library(sf)\n").unwrap();
        let r = scan(root, &rules, Mode::Quick).unwrap();
        assert!(r.used.contains_key("broom") && r.used.contains_key("sf"));
        std::fs::remove_file(root.join("code/b.R")).unwrap();
        let r = scan(root, &rules, Mode::Quick).unwrap();
        assert!(!r.used.contains_key("sf"));

        // Startup suggestions are shown once.
        let names: BTreeSet<String> = ["sf".to_string()].into();
        assert_eq!(new_since_last_time(root, &names).unwrap(), ["sf"]);
        assert!(new_since_last_time(root, &names).unwrap().is_empty());
    }
}
