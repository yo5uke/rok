//! GitHub: resolving references to commits, reading DESCRIPTION, downloading sources.
//!
//! The REST API is used only where needed (resolving a reference, comparing commits), because
//! without a token it allows 60 requests an hour. Files come from raw.githubusercontent.com and
//! codeload.github.com, which do not count against that limit. A token is read from
//! `GITHUB_PAT`, `GITHUB_TOKEN` or `GH_TOKEN`, as pak does; it is sent only to GitHub.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::http::{Head, Http, HttpError};
use crate::manifest::GitRef;

const API: &str = "https://api.github.com";

#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("{owner}/{repo} has no tag, branch or commit `{reference}`.")]
    UnknownRef {
        owner: String,
        repo: String,
        reference: String,
    },
    #[error(
        "{owner}/{repo} was not found on GitHub, or it is private.\nFor a private repository, set GITHUB_PAT to a token that can read it."
    )]
    NotFound { owner: String, repo: String },
    /// The message says whether a token was used and when the limit resets.
    #[error("{message}")]
    RateLimited { message: String },
    #[error("{owner}/{repo}: GitHub refused the request: {message}")]
    Refused {
        owner: String,
        repo: String,
        message: String,
    },
    #[error("Unexpected response from {url}: {message}.")]
    Response { url: String, message: String },
}

pub struct GitHub<'a> {
    http: &'a Http,
    token: Option<String>,
    /// Where DESCRIPTION files at commits are kept (see [`GitHub::with_cache`]).
    cache: Option<PathBuf>,
}

impl<'a> GitHub<'a> {
    /// Uses a token from the environment, if any.
    pub fn new(http: &'a Http) -> GitHub<'a> {
        let token = ["GITHUB_PAT", "GITHUB_TOKEN", "GH_TOKEN"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
            .map(|t| t.trim().to_string());
        GitHub {
            http,
            token,
            cache: None,
        }
    }

    /// Keeps the DESCRIPTION files read at commits in `dir`: they never change.
    pub fn with_cache(mut self, dir: PathBuf) -> GitHub<'a> {
        self.cache = Some(dir);
        self
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn api(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        accept: &str,
    ) -> Result<Vec<u8>, GitHubError> {
        let url = format!("{API}/repos/{owner}/{repo}{path}");
        let auth = self.token.as_ref().map(|t| format!("Bearer {t}"));
        let mut headers = vec![("Accept", accept), ("X-GitHub-Api-Version", "2022-11-28")];
        if let Some(a) = &auth {
            headers.push(("Authorization", a.as_str()));
        }
        let mut waited = false;
        loop {
            let (head, body) = self.http.get_any(&url, &headers)?;
            let status = head.status;
            if (200..300).contains(&status) {
                return Ok(body);
            }
            if !matches!(status, 403 | 429) {
                return Err(HttpError::Status { url, status }.into());
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            match refusal(&head, &body, now) {
                // A secondary limit asks to wait a little: once, then try again.
                Refusal::Secondary { retry_after } if !waited && retry_after <= MAX_WAIT => {
                    std::thread::sleep(Duration::from_secs(retry_after));
                    waited = true;
                }
                Refusal::Primary { resets_in } => {
                    return Err(self.rate_limited(owner, repo, resets_in));
                }
                Refusal::Secondary { retry_after } => {
                    return Err(self.rate_limited(owner, repo, Some(retry_after)));
                }
                Refusal::Other(message) => {
                    return Err(GitHubError::Refused {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                        message,
                    });
                }
            }
        }
    }

    fn rate_limited(&self, owner: &str, repo: &str, resets_in: Option<u64>) -> GitHubError {
        let when = match resets_in {
            Some(s) => format!(" (try again in {} min)", s.div_ceil(60).max(1)),
            None => String::new(),
        };
        let message = if self.has_token() {
            format!(
                "{owner}/{repo}: GitHub's API rate limit for the token in GITHUB_PAT (or GITHUB_TOKEN, GH_TOKEN) was reached{when}."
            )
        } else {
            format!(
                "{owner}/{repo}: GitHub's API rate limit was reached{when}.\nSet GITHUB_PAT to a token to raise the limit."
            )
        };
        GitHubError::RateLimited { message }
    }

    /// The repository's releases (newest first, as GitHub lists them), as JSON.
    pub fn releases(&self, owner: &str, repo: &str) -> Result<serde_json::Value, GitHubError> {
        let body = self.api(
            owner,
            repo,
            "/releases?per_page=30",
            "application/vnd.github+json",
        )?;
        serde_json::from_slice(&body).map_err(|e| GitHubError::Response {
            url: format!("{API}/repos/{owner}/{repo}/releases"),
            message: e.to_string(),
        })
    }

    /// The commit a reference points to (the default branch for [`GitRef::DefaultBranch`]).
    pub fn resolve(
        &self,
        owner: &str,
        repo: &str,
        reference: &GitRef,
    ) -> Result<String, GitHubError> {
        let r = match reference {
            GitRef::DefaultBranch => "HEAD",
            GitRef::Branch(b) => b,
            GitRef::Tag(t) => t,
            GitRef::Rev(r) => r,
        };
        match self.api(
            owner,
            repo,
            &format!("/commits/{r}"),
            "application/vnd.github.sha",
        ) {
            Ok(body) => {
                let sha = String::from_utf8_lossy(&body).trim().to_string();
                if is_sha(&sha) {
                    Ok(sha.to_ascii_lowercase())
                } else {
                    Err(GitHubError::Response {
                        url: format!("{API}/repos/{owner}/{repo}/commits/{r}"),
                        message: "not a commit SHA".to_string(),
                    })
                }
            }
            Err(GitHubError::Http(HttpError::Status {
                status: 404 | 422, ..
            })) => {
                // 404 for an unknown repository, 422 for an unknown reference.
                if self
                    .api(owner, repo, "", "application/vnd.github+json")
                    .is_err()
                {
                    Err(GitHubError::NotFound {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                    })
                } else {
                    Err(GitHubError::UnknownRef {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                        reference: r.to_string(),
                    })
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Whether `reference` (from `user/repo@reference`) is a tag, a branch or a commit.
    pub fn classify(
        &self,
        owner: &str,
        repo: &str,
        reference: &str,
    ) -> Result<GitRef, GitHubError> {
        let exists =
            |path: String| match self.api(owner, repo, &path, "application/vnd.github+json") {
                Ok(_) => Ok(true),
                Err(GitHubError::Http(HttpError::Status { status: 404, .. })) => Ok(false),
                Err(e) => Err(e),
            };
        if exists(format!("/git/ref/tags/{reference}"))? {
            return Ok(GitRef::Tag(reference.to_string()));
        }
        if exists(format!("/branches/{reference}"))? {
            return Ok(GitRef::Branch(reference.to_string()));
        }
        let hex =
            (7..=40).contains(&reference.len()) && reference.bytes().all(|b| b.is_ascii_hexdigit());
        if hex {
            self.resolve(owner, repo, &GitRef::Rev(reference.to_string()))?;
            return Ok(GitRef::Rev(reference.to_string()));
        }
        if !exists(String::new())? {
            return Err(GitHubError::NotFound {
                owner: owner.to_string(),
                repo: repo.to_string(),
            });
        }
        Err(GitHubError::UnknownRef {
            owner: owner.to_string(),
            repo: repo.to_string(),
            reference: reference.to_string(),
        })
    }

    /// The DESCRIPTION file at a commit, from the cache when it is a full commit SHA read
    /// before.
    pub fn description(
        &self,
        owner: &str,
        repo: &str,
        commit: &str,
    ) -> Result<String, GitHubError> {
        let cached = self
            .cache
            .as_ref()
            .filter(|_| is_sha(commit) && !owner.starts_with('.') && !repo.starts_with('.'))
            .map(|d| d.join(owner).join(repo).join(commit).join("DESCRIPTION"));
        if let Some(text) = cached
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
        {
            return Ok(text);
        }
        let url = format!("https://raw.githubusercontent.com/{owner}/{repo}/{commit}/DESCRIPTION");
        let auth = self.token.as_ref().map(|t| format!("Bearer {t}"));
        let headers: Vec<(&str, &str)> =
            auth.iter().map(|a| ("Authorization", a.as_str())).collect();
        let body = self.http.get_with(&url, &headers).map_err(|e| match e {
            HttpError::Status { status: 404, .. } => GitHubError::Response {
                url: url.clone(),
                message: "no DESCRIPTION at the top of the repository (R packages in a subdirectory are not supported yet)".to_string(),
            },
            other => other.into(),
        })?;
        let text = String::from_utf8_lossy(&body).into_owned();
        if let Some(p) = &cached {
            // Only a cache: reading it again from GitHub works too.
            let _ = crate::fsutil::write_atomic(p, text.as_bytes());
        }
        Ok(text)
    }

    /// Downloads the source at a commit as a `.tar.gz` (top directory `<repo>-<commit>`).
    pub fn tarball(&self, owner: &str, repo: &str, commit: &str) -> Result<Vec<u8>, GitHubError> {
        match &self.token {
            Some(_) => self.api(
                owner,
                repo,
                &format!("/tarball/{commit}"),
                "application/vnd.github+json",
            ),
            None => Ok(self.http.get_bytes(
                &format!("https://codeload.github.com/{owner}/{repo}/tar.gz/{commit}"),
                None,
            )?),
        }
    }

    /// How many commits `head` is ahead of `base`, if GitHub can tell. Needs a token, so that
    /// a routine update does not use up the anonymous rate limit.
    pub fn ahead_by(&self, owner: &str, repo: &str, base: &str, head: &str) -> Option<u64> {
        if !self.has_token() {
            return None;
        }
        let body = self
            .api(
                owner,
                repo,
                &format!("/compare/{base}...{head}"),
                "application/vnd.github+json",
            )
            .ok()?;
        let v: serde_json::Value = serde_json::from_slice(&body).ok()?;
        v.get("ahead_by")?.as_u64()
    }
}

/// The longest wait for a secondary rate limit before trying again.
const MAX_WAIT: u64 = 60;

/// Why GitHub's API refused a request (403 or 429), as GitHub documents its rate limits.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// The hourly limit was used up; it resets in this many seconds, if GitHub said.
    Primary { resets_in: Option<u64> },
    /// A secondary limit (too many requests at once, for example): try again after this many
    /// seconds.
    Secondary { retry_after: u64 },
    /// Another reason, in GitHub's words.
    Other(String),
}

/// Reads a refusal: the `retry-after` and `x-ratelimit-*` headers, and the message in the body.
/// `now` is the time in seconds since the Unix epoch.
fn refusal(head: &Head, body: &[u8], now: u64) -> Refusal {
    let message = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("message")?.as_str().map(str::to_string))
        .unwrap_or_default();
    let lower = message.to_ascii_lowercase();
    if let Some(retry_after) = head
        .header("retry-after")
        .and_then(|v| v.trim().parse().ok())
    {
        return Refusal::Secondary { retry_after };
    }
    if head.header("x-ratelimit-remaining").map(str::trim) == Some("0") {
        let resets_in = head
            .header("x-ratelimit-reset")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(|reset| reset.saturating_sub(now));
        return Refusal::Primary { resets_in };
    }
    // Without the headers, GitHub's advice is to wait at least a minute.
    if lower.contains("secondary rate limit") || head.status == 429 {
        return Refusal::Secondary { retry_after: 60 };
    }
    if lower.contains("rate limit") {
        return Refusal::Primary { resets_in: None };
    }
    Refusal::Other(if message.is_empty() {
        format!("HTTP {}", head.status)
    } else {
        message
    })
}

/// Whether `s` is a full commit SHA.
fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The owner and repository of a GitHub URL (`https://github.com/owner/repo[.git]`).
pub fn parse_url(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let rest = rest.trim_end_matches('/');
    crate::manifest::parse_github(rest.strip_suffix(".git").unwrap_or(rest))
}

/// Splits `owner/repo` or `owner/repo@ref`.
pub fn parse_spec(spec: &str) -> Option<(String, String, Option<String>)> {
    let (repo_part, reference) = match spec.split_once('@') {
        Some((r, reference)) if !reference.is_empty() => (r, Some(reference.to_string())),
        Some(_) => return None,
        None => (spec, None),
    };
    let (owner, repo) = crate::manifest::parse_github(repo_part)?;
    Some((owner, repo, reference))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_why_the_api_refused() {
        let head = |status: u16, headers: &[(&str, &str)]| {
            Head::new(
                status,
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
        };
        let body = |m: &str| format!(r#"{{"message": "{m}"}}"#).into_bytes();
        assert_eq!(
            refusal(
                &head(
                    403,
                    &[
                        ("x-ratelimit-remaining", "0"),
                        ("x-ratelimit-reset", "1300")
                    ]
                ),
                &body("API rate limit exceeded for 1.2.3.4."),
                1000
            ),
            Refusal::Primary {
                resets_in: Some(300)
            }
        );
        assert_eq!(
            refusal(&head(403, &[("Retry-After", "30")]), b"", 0),
            Refusal::Secondary { retry_after: 30 }
        );
        assert_eq!(
            refusal(
                &head(403, &[]),
                &body("You have exceeded a secondary rate limit."),
                0
            ),
            Refusal::Secondary { retry_after: 60 }
        );
        assert_eq!(
            refusal(
                &head(403, &[("x-ratelimit-remaining", "4999")]),
                &body("Resource not accessible by integration"),
                0
            ),
            Refusal::Other("Resource not accessible by integration".into())
        );
        assert_eq!(
            refusal(&head(403, &[]), b"<html>", 0),
            Refusal::Other("HTTP 403".into())
        );
    }

    #[test]
    fn reads_descriptions_at_commits_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let commit = "a".repeat(40);
        let path = dir.path().join("o/pkg").join(&commit).join("DESCRIPTION");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Package: pkg\nVersion: 1.0\n").unwrap();
        let http = Http::new();
        let github = GitHub::new(&http).with_cache(dir.path().to_path_buf());
        // No request is made: the repository does not exist.
        assert_eq!(
            github.description("o", "pkg", &commit).unwrap(),
            "Package: pkg\nVersion: 1.0\n"
        );
    }

    #[test]
    fn parses_specs() {
        assert_eq!(
            parse_spec("yo5uke/coresynth"),
            Some(("yo5uke".into(), "coresynth".into(), None))
        );
        assert_eq!(
            parse_spec("yo5uke/coresynth@v0.3.0"),
            Some(("yo5uke".into(), "coresynth".into(), Some("v0.3.0".into())))
        );
        assert_eq!(parse_spec("yo5uke/coresynth@"), None);
        assert_eq!(parse_spec("coresynth"), None);
        assert_eq!(parse_spec("a/b/c"), None);
        assert_eq!(
            parse_url("https://github.com/pola-rs/r-polars.git"),
            Some(("pola-rs".into(), "r-polars".into()))
        );
        assert_eq!(parse_url("https://gitlab.com/a/b"), None);
    }
}
