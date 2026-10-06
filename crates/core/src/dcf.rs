//! Debian Control File (DCF) format, used by DESCRIPTION and PACKAGES files.
//!
//! A file is a sequence of records separated by blank lines. Each record has `Field: value`
//! lines; a line starting with whitespace continues the previous value. Some indexes put the
//! whole value on a continuation line (R-multiverse writes `SHA256:` followed by an indented
//! hash), so an empty first line is allowed.

use crate::constraint::{Clause, Constraint};

/// One record: fields in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    fields: Vec<(String, String)>,
}

impl Record {
    /// The value of a field, if present. Continuation lines are joined with single spaces.
    pub fn get(&self, field: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == field)
            .map(|(_, v)| v.as_str())
    }

    pub fn fields(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// Error returned for malformed DCF text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseDcfError {
    #[error("line {line}: expected `Field: value`, found `{text}`")]
    NotAField { line: usize, text: String },
    #[error("line {line}: continuation line without a preceding field")]
    DanglingContinuation { line: usize },
}

/// Parses DCF text into records.
pub fn parse(text: &str) -> Result<Vec<Record>, ParseDcfError> {
    let mut records = Vec::new();
    let mut current = Record::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim().is_empty() {
            if !current.fields.is_empty() {
                records.push(std::mem::take(&mut current));
            }
            continue;
        }
        if line.starts_with([' ', '\t']) {
            let Some((_, value)) = current.fields.last_mut() else {
                return Err(ParseDcfError::DanglingContinuation { line: i + 1 });
            };
            let more = line.trim();
            if value.is_empty() {
                value.push_str(more);
            } else {
                value.push(' ');
                value.push_str(more);
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(ParseDcfError::NotAField {
                line: i + 1,
                text: line.to_string(),
            });
        };
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err(ParseDcfError::NotAField {
                line: i + 1,
                text: line.to_string(),
            });
        }
        current
            .fields
            .push((key.to_string(), value.trim().to_string()));
    }
    if !current.fields.is_empty() {
        records.push(current);
    }
    Ok(records)
}

/// One entry of a `Depends`, `Imports` or `LinkingTo` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    pub constraint: Constraint,
}

/// Error returned for a malformed dependency field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid dependency `{0}`")]
pub struct ParseDependencyError(pub String);

/// Parses a field such as `R (>= 3.5.0), stats, Rcpp (>= 1.0.5)`.
///
/// The entry for `R` itself is returned like any other; callers decide how to use it.
pub fn parse_dependencies(value: &str) -> Result<Vec<Dependency>, ParseDependencyError> {
    value
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|entry| {
            let err = || ParseDependencyError(entry.to_string());
            let (name, constraint) = match entry.split_once('(') {
                None => (entry, Constraint::any()),
                Some((name, rest)) => {
                    let inner = rest.trim_end().strip_suffix(')').ok_or_else(err)?;
                    let clause: Clause = Constraint::parse_clause(inner).map_err(|_| err())?;
                    (name.trim(), Constraint::from(clause))
                }
            };
            if !is_package_name(name) {
                return Err(err());
            }
            Ok(Dependency {
                name: name.to_string(),
                constraint,
            })
        })
        .collect()
}

/// Whether `s` is a valid R package name: letters, digits and `.`, starting with a letter,
/// not ending with `.`. `R` itself is accepted.
pub fn is_package_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
        && !s.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_records_and_continuations() {
        let text = "Package: fixest\r\nVersion: 0.14.2\nImports: stats, graphics,\n  Rcpp (>= 1.0.5)\n\n\n\
                    Package: polars\nSHA256:\n        f263f133\nNeedsCompilation: yes\n";
        let recs = parse(text).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].get("Version"), Some("0.14.2"));
        assert_eq!(
            recs[0].get("Imports"),
            Some("stats, graphics, Rcpp (>= 1.0.5)")
        );
        assert_eq!(recs[1].get("SHA256"), Some("f263f133"));
        assert_eq!(recs[1].get("Missing"), None);
        let names: Vec<_> = recs[1].fields().map(|(k, _)| k).collect();
        assert_eq!(names, ["Package", "SHA256", "NeedsCompilation"]);
    }

    #[test]
    fn accepts_unusual_field_names_and_colons_in_values() {
        let recs =
            parse("Authors@R: c(person(\"A\"))\nConfig/testthat/edition: 3\nURL: https://x.org\n")
                .unwrap();
        assert_eq!(recs[0].get("Config/testthat/edition"), Some("3"));
        assert_eq!(recs[0].get("URL"), Some("https://x.org"));
    }

    #[test]
    fn rejects_malformed_text() {
        assert_eq!(
            parse("  dangling\n"),
            Err(ParseDcfError::DanglingContinuation { line: 1 })
        );
        assert!(matches!(
            parse("Package fixest\n"),
            Err(ParseDcfError::NotAField { line: 1, .. })
        ));
        assert!(matches!(
            parse("Bad Key: x\n"),
            Err(ParseDcfError::NotAField { .. })
        ));
    }

    #[test]
    fn parses_dependency_fields() {
        let deps =
            parse_dependencies("R (>= 3.5.0), stats,\tRcpp (>=1.0.5) , data.table,").unwrap();
        let names: Vec<_> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["R", "stats", "Rcpp", "data.table"]);
        assert_eq!(deps[2].constraint.to_string(), ">= 1.0.5");
        assert!(deps[1].constraint.is_any());
        assert!(parse_dependencies("").unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_dependencies() {
        for s in [
            "Rcpp (>= 1.0",
            "Rcpp (1.0",
            "1pkg",
            "pkg.",
            "a b",
            "Rcpp (>= x)",
        ] {
            assert!(parse_dependencies(s).is_err(), "{s:?} should be rejected");
        }
    }
}
