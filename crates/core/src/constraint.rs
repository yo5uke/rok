//! Version constraints.
//!
//! The manifest (`rok.toml`) writes constraints as `"*"`, `"0.12.1"` (exact), `"< 0.13"` or
//! several clauses separated by commas (`">= 1.0, < 2.0"`). DESCRIPTION files write a single
//! clause in parentheses, such as `Rcpp (>= 1.0.5)`; the part inside the parentheses uses the
//! same clause syntax. Versions follow R's rules, except that the manifest also accepts a single
//! number as a bound (`< 2` means `< 2.0`); DESCRIPTION files need at least two components.

use std::fmt;
use std::str::FromStr;

use crate::version::Version;

/// A comparison operator in a version clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Op {
    fn as_str(self) -> &'static str {
        match self {
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Eq => "==",
            Op::Ne => "!=",
        }
    }
}

/// One comparison, such as `>= 1.0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    pub op: Op,
    pub version: Version,
}

impl Clause {
    pub fn matches(&self, v: &Version) -> bool {
        match self.op {
            Op::Lt => v < &self.version,
            Op::Le => v <= &self.version,
            Op::Gt => v > &self.version,
            Op::Ge => v >= &self.version,
            Op::Eq => v == &self.version,
            Op::Ne => v != &self.version,
        }
    }
}

impl fmt::Display for Clause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.op.as_str(), self.version)
    }
}

/// A set of clauses that must all hold. An empty constraint allows any version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Constraint {
    clauses: Vec<Clause>,
}

/// Error returned when a constraint cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid version constraint `{0}`: expected `*`, a version such as `0.12.1`, or clauses such as `>= 1.0, < 2.0`"
)]
pub struct ParseConstraintError(pub String);

impl Constraint {
    /// A constraint that allows any version.
    pub fn any() -> Self {
        Self::default()
    }

    pub fn clauses(&self) -> &[Clause] {
        &self.clauses
    }

    pub fn is_any(&self) -> bool {
        self.clauses.is_empty()
    }

    pub fn matches(&self, v: &Version) -> bool {
        self.clauses.iter().all(|c| c.matches(v))
    }

    /// Parses the part of a DESCRIPTION dependency inside the parentheses, such as `>= 1.0`.
    pub fn parse_clause(s: &str) -> Result<Clause, ParseConstraintError> {
        Self::parse_clause_with(s, |v| v.parse())
    }

    /// Parses one clause, reading the version with `parse_version`.
    fn parse_clause_with(
        s: &str,
        parse_version: impl Fn(&str) -> Result<Version, crate::version::ParseVersionError>,
    ) -> Result<Clause, ParseConstraintError> {
        let err = || ParseConstraintError(s.to_string());
        let t = s.trim();
        let (op, rest) = [
            ("<=", Op::Le),
            (">=", Op::Ge),
            ("==", Op::Eq),
            ("!=", Op::Ne),
            ("<", Op::Lt),
            (">", Op::Gt),
        ]
        .into_iter()
        .find_map(|(sym, op)| t.strip_prefix(sym).map(|rest| (op, rest)))
        .unwrap_or((Op::Eq, t)); // a bare version means an exact version
        let version = parse_version(rest.trim()).map_err(|_| err())?;
        Ok(Clause { op, version })
    }
}

impl FromStr for Constraint {
    type Err = ParseConstraintError;

    /// Parses the manifest syntax: `*`, `0.12.1`, `< 0.13`, `>= 1.0, < 2.0`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        if t == "*" {
            return Ok(Self::any());
        }
        if t.is_empty() {
            return Err(ParseConstraintError(s.to_string()));
        }
        let clauses = t
            .split(',')
            .map(|c| {
                // The manifest also accepts a single number such as `< 2` (see
                // `Version::parse_bound`); DESCRIPTION clauses stay strict.
                Self::parse_clause_with(c, Version::parse_bound)
                    .map_err(|_| ParseConstraintError(s.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Constraint { clauses })
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.clauses.is_empty() {
            return f.write_str("*");
        }
        for (i, c) in self.clauses.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

impl From<Clause> for Constraint {
    fn from(c: Clause) -> Self {
        Constraint { clauses: vec![c] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    fn c(s: &str) -> Constraint {
        s.parse().unwrap()
    }

    #[test]
    fn parses_manifest_syntax() {
        assert!(c("*").is_any());
        assert_eq!(c("< 0.13").to_string(), "< 0.13");
        assert_eq!(c(">=1.0,<2.0").to_string(), ">= 1.0, < 2.0");
        // A single number is accepted in the manifest and means `<number>.0`.
        assert_eq!(c("< 2").to_string(), "< 2");
        assert!(c("< 2").matches(&v("1.99.9")));
        assert!(!c("< 2").matches(&v("2.0")));
        assert!(c(">= 2").matches(&v("2.0")));
        // DESCRIPTION clauses stay strict.
        assert!(Constraint::parse_clause(">= 2").is_err());
        assert_eq!(c("0.12.1").to_string(), "== 0.12.1");
        assert_eq!(c("!= 1.2-3").to_string(), "!= 1.2-3");
    }

    #[test]
    fn rejects_invalid_constraints() {
        for s in [
            "",
            " ",
            "<",
            ">= ",
            "~> 1.0",
            "1.0,",
            ">= 1.0, *",
            "=> 1.0",
            ">= one",
        ] {
            assert!(s.parse::<Constraint>().is_err(), "{s:?} should be rejected");
        }
    }

    #[test]
    fn matches_versions() {
        let r = c(">= 1.0, < 2.0");
        assert!(r.matches(&v("1.0")));
        assert!(r.matches(&v("1.99.9")));
        assert!(!r.matches(&v("0.9")));
        assert!(!r.matches(&v("2.0")));
        assert!(c("< 0.13").matches(&v("0.12.1")));
        assert!(!c("< 0.13").matches(&v("0.13.0")));
        assert!(c("0.12.1").matches(&v("0.12-1")));
        assert!(c("*").matches(&v("99.0")));
        assert!(!c("!= 1.0").matches(&v("1.0")));
    }

    #[test]
    fn parses_description_clause() {
        let cl = Constraint::parse_clause(" >= 1.0.5 ").unwrap();
        assert_eq!(cl.op, Op::Ge);
        assert_eq!(cl.version, v("1.0.5"));
        assert!(Constraint::parse_clause(">= ").is_err());
    }
}
