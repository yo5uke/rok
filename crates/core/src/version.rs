//! R package and R versions.
//!
//! R versions are sequences of at least two non-negative integers separated by `.` or `-`
//! (for example `1.0`, `0.12.1`, `1.3-32`, `1.0.0.9000`). They compare component by
//! component as numbers, `.` and `-` are equivalent, and a version that is a prefix of
//! another is smaller (`1.0` < `1.0.0`). This matches `utils::compareVersion()` and
//! `package_version()` in R.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

/// A version of an R package or of R itself.
///
/// Equality, ordering and hashing use only the numeric components, so `1.0-1` equals
/// `1.0.1`. The original text is kept for display and for building file names such as
/// `pkg_1.0-1.tar.gz`.
#[derive(Clone)]
pub struct Version {
    parts: Vec<u64>,
    text: String,
}

/// Error returned when a string is not a valid R version.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid version `{0}`: expected numbers separated by `.` or `-`, such as `1.0` or `1.3-2`"
)]
pub struct ParseVersionError(pub String);

impl Version {
    /// The numeric components.
    pub fn parts(&self) -> &[u64] {
        &self.parts
    }

    /// The version as written, for example `1.3-32`.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The first two components joined by `.`, for example `4.4` for R `4.4.2`.
    pub fn minor(&self) -> String {
        format!("{}.{}", self.parts[0], self.parts[1])
    }
}

impl FromStr for Version {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseVersionError(s.to_string());
        let text = s.trim();
        let parts = text
            .split(['.', '-'])
            .map(|p| {
                if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(err());
                }
                p.parse::<u64>().map_err(|_| err())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if parts.len() < 2 {
            return Err(err());
        }
        Ok(Version {
            parts,
            text: text.to_string(),
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl fmt::Debug for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Version({})", self.text)
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.parts == other.parts
    }
}

impl Eq for Version {}

impl Hash for Version {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.parts.hash(state);
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lexicographic comparison of the numeric components; a prefix is smaller.
        self.parts.cmp(&other.parts)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn parses_r_style_versions() {
        assert_eq!(v("1.3-32").parts(), &[1, 3, 32]);
        assert_eq!(v("1.0.0.9000").parts(), &[1, 0, 0, 9000]);
        assert_eq!(v(" 0.12.1 ").as_str(), "0.12.1");
        assert_eq!(v("2023.10.01").parts(), &[2023, 10, 1]);
    }

    #[test]
    fn rejects_invalid_versions() {
        for s in [
            "", "1", "1.", ".1", "1..2", "1.a", "v1.0", "1.0 beta", "-1.0", "1,0",
        ] {
            assert!(s.parse::<Version>().is_err(), "{s:?} should be rejected");
        }
    }

    #[test]
    fn compares_like_r() {
        assert!(v("0.9") < v("0.10"));
        assert!(v("1.0") < v("1.0.0"));
        assert!(v("1.0-1") < v("1.0.2"));
        assert!(v("1.18.6.1") > v("1.18.6"));
        assert_eq!(v("1.0-1"), v("1.0.1"));
        assert_eq!(v("1.01"), v("1.1"));
    }

    #[test]
    fn keeps_original_text_and_minor() {
        let r = v("4.4.2");
        assert_eq!(r.to_string(), "4.4.2");
        assert_eq!(r.minor(), "4.4");
        assert_eq!(v("1.3-32").to_string(), "1.3-32");
    }
}
