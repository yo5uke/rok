//! Editing the top level of a JSON-with-comments file (`.vscode/settings.json`) without
//! touching anything else: comments, formatting and other settings stay as they are
//! (requirements chapter 6, IDE settings).

/// A top-level member: its key, and where its value starts and ends in the text.
#[derive(Debug)]
struct Member {
    key: String,
    value: std::ops::Range<usize>,
}

/// The top-level object: where its braces are and its members.
#[derive(Debug)]
struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

/// Skips whitespace and comments from `i`; returns the next significant byte's index.
fn skip(b: &[u8], mut i: usize) -> Result<usize, String> {
    loop {
        match b.get(i) {
            Some(c) if c.is_ascii_whitespace() => i += 1,
            Some(b'/') if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            Some(b'/') if b.get(i + 1) == Some(&b'*') => {
                let end = b[i + 2..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .ok_or("unterminated comment")?;
                i += 2 + end + 2;
            }
            _ => return Ok(i),
        }
    }
}

/// Skips spaces, tabs and `/* */` comments on the same line from `i`.
fn skip_inline(b: &[u8], mut i: usize) -> usize {
    loop {
        match b.get(i) {
            Some(b' ') | Some(b'\t') => i += 1,
            Some(b'/') if b.get(i + 1) == Some(&b'*') => match skip(b, i) {
                Ok(j) if !b[i..j].contains(&b'\n') => i = j,
                _ => return i,
            },
            _ => return i,
        }
    }
}

/// `i`, or the end of its line if only spaces and a `//` comment follow on that line.
fn line_end_if_only_comment(b: &[u8], i: usize) -> usize {
    let j = skip_inline(b, i);
    match b.get(j) {
        Some(b'\n') | Some(b'\r') => j,
        Some(b'/') if b.get(j + 1) == Some(&b'/') => {
            let mut k = j;
            while k < b.len() && b[k] != b'\n' && b[k] != b'\r' {
                k += 1;
            }
            k
        }
        _ => i,
    }
}

/// The end of the string starting at `i` (just past its closing quote).
fn string_end(b: &[u8], mut i: usize) -> Result<usize, String> {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err("unterminated string".to_string())
}

/// The end of the value starting at `i` (strings, objects, arrays, literals; comments inside
/// objects and arrays are skipped).
fn value_end(b: &[u8], i: usize) -> Result<usize, String> {
    match b.get(i) {
        Some(b'"') => string_end(b, i),
        Some(b'{') | Some(b'[') => {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = string_end(b, j)?;
                        continue;
                    }
                    b'/' if matches!(b.get(j + 1), Some(b'/') | Some(b'*')) => {
                        j = skip(b, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            Err("unterminated object or array".to_string())
        }
        Some(_) => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') && !b[j].is_ascii_whitespace()
            {
                j += 1;
            }
            Ok(j)
        }
        None => Err("missing value".to_string()),
    }
}

fn parse(text: &str) -> Result<Object, String> {
    let b = text.as_bytes();
    let open = skip(b, 0)?;
    if b.get(open) != Some(&b'{') {
        return Err("the file is not a JSON object".to_string());
    }
    let mut members = Vec::new();
    let mut i = skip(b, open + 1)?;
    loop {
        match b.get(i) {
            Some(b'}') => {
                let close = i;
                if b.get(skip(b, close + 1)?).is_some() {
                    return Err("text after the closing brace".to_string());
                }
                return Ok(Object {
                    open,
                    close,
                    members,
                });
            }
            Some(b'"') => {
                let end = string_end(b, i)?;
                let key: String = serde_json::from_str(&text[i..end]).map_err(|e| e.to_string())?;
                let colon = skip(b, end)?;
                if b.get(colon) != Some(&b':') {
                    return Err(format!("expected `:` after {key:?}"));
                }
                let start = skip(b, colon + 1)?;
                let stop = value_end(b, start)?;
                members.push(Member {
                    key,
                    value: start..stop,
                });
                i = skip(b, stop)?;
                if b.get(i) == Some(&b',') {
                    i = skip(b, i + 1)?;
                }
            }
            _ => return Err("expected a key or `}`".to_string()),
        }
    }
}

/// The value of a top-level key, as JSON, if the text has it.
pub fn get(text: &str, key: &str) -> Result<Option<serde_json::Value>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let obj = parse(text)?;
    obj.members
        .iter()
        .find(|m| m.key == key)
        .map(|m| serde_json::from_str(&text[m.value.clone()]).map_err(|e| e.to_string()))
        .transpose()
}

/// Sets top-level keys to JSON values, replacing values that exist and adding the rest at the
/// end of the object, after a `// comment` line. Everything else in the text stays as it is.
/// An empty text becomes a new object.
pub fn set(
    text: &str,
    entries: &[(&str, serde_json::Value)],
    comment: &str,
) -> Result<String, String> {
    let source = if text.trim().is_empty() {
        "{\n}\n"
    } else {
        text
    };
    let obj = parse(source)?;
    let b = source.as_bytes();
    // Indentation of the existing members, else two spaces.
    let indent = obj
        .members
        .first()
        .map(|m| {
            let line_start = source[..m.value.start].rfind('\n').map_or(0, |p| p + 1);
            source[line_start..]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "  ".to_string());
    let render = |v: &serde_json::Value| serde_json::to_string(v).expect("JSON values serialize");

    // Replacements of existing values, and insertions for new keys.
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut new: Vec<String> = Vec::new();
    for (key, value) in entries {
        match obj.members.iter().find(|m| m.key == *key) {
            Some(m) => edits.push((m.value.clone(), render(value))),
            None => new.push(format!(
                "{indent}{}: {}",
                serde_json::to_string(key).expect("strings serialize"),
                render(value)
            )),
        }
    }
    if !new.is_empty() {
        // After the last member: a comma after its value if it has none, and the new lines at
        // the end of that line (after a trailing `// comment`), or right after `{`.
        let after_last = match obj.members.last() {
            Some(last) => {
                let next = skip_inline(b, last.value.end);
                if b.get(next) == Some(&b',') {
                    next + 1
                } else {
                    edits.push((last.value.end..last.value.end, ",".to_string()));
                    last.value.end
                }
            }
            None => obj.open + 1,
        };
        let at = line_end_if_only_comment(b, after_last);
        let mut insert = String::from("\n");
        insert.push_str(&format!("{indent}// {comment}\n"));
        insert.push_str(&new.join(",\n"));
        // A newline before the closing brace if the text between has none.
        if !source[at..obj.close].contains('\n') {
            insert.push('\n');
        }
        edits.push((at..at, insert));
    }
    // From the end, so earlier ranges stay valid; at one position, the later edit first, so the
    // edits end up in the order they were made.
    let mut edits: Vec<_> = edits.into_iter().enumerate().collect();
    edits.sort_by_key(|(n, (r, _))| std::cmp::Reverse((r.start, *n)));
    let mut out = source.to_string();
    for (_, (range, with)) in edits {
        out.replace_range(range, &with);
    }
    // The result must still read, with the values set.
    let check = parse(&out)?;
    for (key, value) in entries {
        let m = check
            .members
            .iter()
            .find(|m| m.key == *key)
            .ok_or_else(|| format!("{key} was not written"))?;
        let got: serde_json::Value =
            serde_json::from_str(&out[m.value.clone()]).map_err(|e| e.to_string())?;
        if &got != value {
            return Err(format!("{key} was not written correctly"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn creates_a_file() {
        let out = set("", &[("a.b", json!("~/x"))], "Added by rok").unwrap();
        assert_eq!(out, "{\n  // Added by rok\n  \"a.b\": \"~/x\"\n}\n");
        assert_eq!(get(&out, "a.b").unwrap(), Some(json!("~/x")));
    }

    #[test]
    fn keeps_comments_and_other_settings() {
        let text = r#"{
    // My settings
    "editor.fontSize": 14, /* big */
    "files.exclude": { "**/.git": true, // keep
        "x": [1, 2] },
    "positron.r.interpreters.default": "/old/R"
}
"#;
        let out = set(
            text,
            &[
                ("positron.r.interpreters.default", json!("~/new/R")),
                (
                    "positron.r.customRootFolders",
                    json!(["~/.local/share/R/rok/r"]),
                ),
            ],
            "Added by rok",
        )
        .unwrap();
        assert_eq!(
            out,
            r#"{
    // My settings
    "editor.fontSize": 14, /* big */
    "files.exclude": { "**/.git": true, // keep
        "x": [1, 2] },
    "positron.r.interpreters.default": "~/new/R",
    // Added by rok
    "positron.r.customRootFolders": ["~/.local/share/R/rok/r"]
}
"#
        );
        // Setting the same values again changes nothing.
        let again = set(
            &out,
            &[("positron.r.interpreters.default", json!("~/new/R"))],
            "Added by rok",
        )
        .unwrap();
        assert_eq!(again, out);
    }

    #[test]
    fn adds_after_a_trailing_line_comment() {
        let out = set("{\n  \"a\": 1, // note\n}\n", &[("b", json!(2))], "c").unwrap();
        assert_eq!(out, "{\n  \"a\": 1, // note\n  // c\n  \"b\": 2\n}\n");
        let out = set("{\n  \"a\": 1 // note\n}\n", &[("b", json!(2))], "c").unwrap();
        assert_eq!(out, "{\n  \"a\": 1, // note\n  // c\n  \"b\": 2\n}\n");
    }

    #[test]
    fn handles_trailing_commas_and_empty_objects() {
        let out = set("{\n  \"a\": 1,\n}\n", &[("b", json!(true))], "c").unwrap();
        assert_eq!(out, "{\n  \"a\": 1,\n  // c\n  \"b\": true\n}\n");
        let out = set("{}", &[("b", json!(1))], "c").unwrap();
        assert_eq!(out, "{\n  // c\n  \"b\": 1\n}");
        assert!(set("[1]", &[("b", json!(1))], "c").is_err());
        assert!(set("{ \"a\": ", &[("b", json!(1))], "c").is_err());
    }
}
