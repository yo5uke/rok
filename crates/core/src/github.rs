//! GitHub: resolving references to commits, reading DESCRIPTION, downloading sources.
//!
//! GitHub's REST API allows 60 requests an hour without a token, and the limit can be used up
//! by others on the same address (as on CI runners). So references are resolved from the list
//! that Git itself reads (`git ls-remote`), files come from raw.githubusercontent.com and
//! codeload.github.com, and none of these count against that limit. The REST API is left for
//! what only it can do: abbreviated commit SHAs, private sources, releases, and comparing
//! commits. A token is read from `GITHUB_PAT`, `GITHUB_TOKEN` or `GH_TOKEN`, as pak does; it is
//! sent only to GitHub.

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
                Refusal::Primary { limit, resets_in } => {
                    return Err(self.rate_limited(owner, repo, limit, resets_in));
                }
                Refusal::Secondary { retry_after } => {
                    return Err(self.rate_limited(owner, repo, None, Some(retry_after)));
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

    /// `limit` is the number of requests an hour GitHub allowed: 60 means it took the requests
    /// as anonymous, even with a token.
    fn rate_limited(
        &self,
        owner: &str,
        repo: &str,
        limit: Option<u64>,
        resets_in: Option<u64>,
    ) -> GitHubError {
        let limit = match limit {
            Some(n) => format!(" ({n} requests an hour)"),
            None => String::new(),
        };
        let when = match resets_in {
            Some(s) => format!(" (try again in {} min)", s.div_ceil(60).max(1)),
            None => String::new(),
        };
        let message = if self.has_token() {
            format!(
                "{owner}/{repo}: GitHub's API rate limit{limit} for the token in GITHUB_PAT (or GITHUB_TOKEN, GH_TOKEN) was reached{when}."
            )
        } else {
            format!(
                "{owner}/{repo}: GitHub's API rate limit{limit} was reached{when}.\nSet GITHUB_PAT to a token to raise the limit."
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

    /// The commit a reference points to (the default branch for [`GitRef::DefaultBranch`]). A
    /// full commit SHA is taken as it is: reading DESCRIPTION there checks it.
    pub fn resolve(
        &self,
        owner: &str,
        repo: &str,
        reference: &GitRef,
    ) -> Result<String, GitHubError> {
        if let GitRef::Rev(r) = reference
            && is_sha(r)
        {
            return Ok(r.to_ascii_lowercase());
        }
        let refs = self.refs(owner, repo)?;
        let (found, name) = match reference {
            GitRef::DefaultBranch => (refs.head.clone(), "HEAD"),
            GitRef::Branch(b) => (refs.branch(b), b.as_str()),
            GitRef::Tag(t) => (refs.tag(t), t.as_str()),
            GitRef::Rev(r) => return self.expand_sha(owner, repo, r),
        };
        found.ok_or_else(|| GitHubError::UnknownRef {
            owner: owner.to_string(),
            repo: repo.to_string(),
            reference: name.to_string(),
        })
    }

    /// The full SHA of an abbreviated one, which only the REST API can tell.
    fn expand_sha(&self, owner: &str, repo: &str, rev: &str) -> Result<String, GitHubError> {
        let url = format!("{API}/repos/{owner}/{repo}/commits/{rev}");
        match self.api(
            owner,
            repo,
            &format!("/commits/{rev}"),
            "application/vnd.github.sha",
        ) {
            Ok(body) => {
                let sha = String::from_utf8_lossy(&body).trim().to_string();
                if is_sha(&sha) {
                    Ok(sha.to_ascii_lowercase())
                } else {
                    Err(GitHubError::Response {
                        url,
                        message: "not a commit SHA".to_string(),
                    })
                }
            }
            Err(GitHubError::Http(HttpError::Status {
                status: 404 | 422, ..
            })) => Err(GitHubError::UnknownRef {
                owner: owner.to_string(),
                repo: repo.to_string(),
                reference: rev.to_string(),
            }),
            Err(e) => Err(e),
        }
    }

    /// The references of a repository, as `git ls-remote` lists them: Git's HTTP protocol, which
    /// the REST API's rate limit does not apply to. GitHub answers 401 for a repository that
    /// does not exist or that the token (if any) cannot read.
    fn refs(&self, owner: &str, repo: &str) -> Result<Refs, GitHubError> {
        let url =
            format!("https://github.com/{owner}/{repo}.git/info/refs?service=git-upload-pack");
        // Git sends a token as the password of Basic authentication.
        let auth = self
            .token
            .as_ref()
            .map(|t| format!("Basic {}", base64(format!("x-access-token:{t}").as_bytes())));
        let mut response = self.http.get_any(
            &url,
            &auth
                .iter()
                .map(|a| ("Authorization", a.as_str()))
                .collect::<Vec<_>>(),
        )?;
        // A token GitHub does not accept fails even for a public repository: try without.
        if response.0.status == 401 && auth.is_some() {
            response = self.http.get_any(&url, &[])?;
        }
        let (head, body) = response;
        match head.status {
            200 => Refs::parse(&body).ok_or(GitHubError::Response {
                url,
                message: "not a list of Git references".to_string(),
            }),
            401 | 403 | 404 => Err(GitHubError::NotFound {
                owner: owner.to_string(),
                repo: repo.to_string(),
            }),
            status => Err(HttpError::Status { url, status }.into()),
        }
    }

    /// Whether `reference` (from `user/repo@reference`) is a tag, a branch or a commit.
    pub fn classify(
        &self,
        owner: &str,
        repo: &str,
        reference: &str,
    ) -> Result<GitRef, GitHubError> {
        let refs = self.refs(owner, repo)?;
        if refs.tag(reference).is_some() {
            return Ok(GitRef::Tag(reference.to_string()));
        }
        if refs.branch(reference).is_some() {
            return Ok(GitRef::Branch(reference.to_string()));
        }
        let hex =
            (7..=40).contains(&reference.len()) && reference.bytes().all(|b| b.is_ascii_hexdigit());
        if hex {
            if !is_sha(reference) {
                self.expand_sha(owner, repo, reference)?;
            }
            return Ok(GitRef::Rev(reference.to_string()));
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
        let body = match self.http.get_with(&url, &headers) {
            // A token GitHub does not accept gets 404 even for a public repository: try without.
            Err(HttpError::Status { status: 404, .. }) if auth.is_some() => {
                self.http.get_with(&url, &[])
            }
            other => other,
        };
        let body = body.map_err(|e| match e {
            HttpError::Status { status: 404, .. } => GitHubError::Response {
                url: url.clone(),
                message: format!(
                    "no DESCRIPTION at the top of the repository at commit {} (the commit may not exist; R packages in a subdirectory are not supported yet)",
                    &commit[..commit.len().min(7)]
                ),
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

    /// Downloads the source at a commit as a `.tar.gz` (one top directory). It comes from
    /// codeload.github.com; a private repository's, which needs the token, from the REST API.
    pub fn tarball(&self, owner: &str, repo: &str, commit: &str) -> Result<Vec<u8>, GitHubError> {
        let url = format!("https://codeload.github.com/{owner}/{repo}/tar.gz/{commit}");
        match self.http.get_bytes(&url, None) {
            Err(HttpError::Status { status: 404, .. }) if self.has_token() => self.api(
                owner,
                repo,
                &format!("/tarball/{commit}"),
                "application/vnd.github+json",
            ),
            other => Ok(other?),
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

/// A repository's references from Git's HTTP protocol (`info/refs`).
#[derive(Debug, Default, PartialEq, Eq)]
struct Refs {
    /// The commit of the default branch.
    head: Option<String>,
    /// Full names (`refs/heads/main`, `refs/tags/v1.0`, and `refs/tags/v1.0^{}` for the commit
    /// an annotated tag points to) and their SHAs.
    refs: std::collections::HashMap<String, String>,
}

impl Refs {
    /// Reads the "smart" reference advertisement: lines in Git's pkt-line format, each a SHA
    /// and a name (the first also lists the server's capabilities after a NUL).
    fn parse(body: &[u8]) -> Option<Refs> {
        let mut out = Refs::default();
        let mut rest = body;
        while rest.len() >= 4 {
            let len = usize::from_str_radix(std::str::from_utf8(&rest[..4]).ok()?, 16).ok()?;
            if len == 0 {
                rest = &rest[4..];
                continue;
            }
            if len < 4 || len > rest.len() {
                return None;
            }
            let line = std::str::from_utf8(&rest[4..len]).ok()?;
            rest = &rest[len..];
            let line = line.split('\0').next()?.trim_end_matches('\n');
            if line.starts_with('#') {
                continue;
            }
            let (sha, name) = line.split_once(' ')?;
            if !is_sha(sha) {
                return None;
            }
            let sha = sha.to_ascii_lowercase();
            if name == "HEAD" {
                out.head = Some(sha);
            } else {
                out.refs.insert(name.to_string(), sha);
            }
        }
        Some(out)
    }

    fn branch(&self, name: &str) -> Option<String> {
        self.refs.get(&format!("refs/heads/{name}")).cloned()
    }

    /// The commit a tag points to (through an annotated tag's object).
    fn tag(&self, name: &str) -> Option<String> {
        let tag = format!("refs/tags/{name}");
        self.refs
            .get(&format!("{tag}^{{}}"))
            .or_else(|| self.refs.get(&tag))
            .cloned()
    }
}

/// Standard base64, for Basic authentication.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = byte(0) << 16 | byte(1) << 8 | byte(2);
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize])
            } else {
                '='
            });
        }
    }
    out
}

/// The longest wait for a secondary rate limit before trying again.
const MAX_WAIT: u64 = 60;

/// Why GitHub's API refused a request (403 or 429), as GitHub documents its rate limits.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// The hourly limit (`limit` requests, if GitHub said) was used up; it resets in
    /// `resets_in` seconds, if GitHub said.
    Primary {
        limit: Option<u64>,
        resets_in: Option<u64>,
    },
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
        let number = |name: &str| head.header(name).and_then(|v| v.trim().parse::<u64>().ok());
        return Refusal::Primary {
            limit: number("x-ratelimit-limit"),
            resets_in: number("x-ratelimit-reset").map(|reset| reset.saturating_sub(now)),
        };
    }
    // Without the headers, GitHub's advice is to wait at least a minute.
    if lower.contains("secondary rate limit") || head.status == 429 {
        return Refusal::Secondary { retry_after: 60 };
    }
    if lower.contains("rate limit") {
        return Refusal::Primary {
            limit: None,
            resets_in: None,
        };
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
    fn reads_git_reference_lists() {
        let pkt = |s: &str| format!("{:04x}{s}", s.len() + 4);
        let (a, b, c, d) = (
            "a".repeat(40),
            "b".repeat(40),
            "c".repeat(40),
            "d".repeat(40),
        );
        let body = [
            pkt("# service=git-upload-pack\n"),
            "0000".to_string(),
            pkt(&format!(
                "{a} HEAD\0multi_ack symref=HEAD:refs/heads/main\n"
            )),
            pkt(&format!("{a} refs/heads/main\n")),
            pkt(&format!("{b} refs/tags/v1.0\n")),
            pkt(&format!("{c} refs/tags/v2.0\n")),
            pkt(&format!("{d} refs/tags/v2.0^{{}}\n")),
            "0000".to_string(),
        ]
        .concat();
        let refs = Refs::parse(body.as_bytes()).unwrap();
        assert_eq!(refs.head.as_deref(), Some(a.as_str()));
        assert_eq!(refs.branch("main").as_deref(), Some(a.as_str()));
        assert_eq!(refs.branch("dev"), None);
        // A lightweight tag is the commit; an annotated one points to it.
        assert_eq!(refs.tag("v1.0").as_deref(), Some(b.as_str()));
        assert_eq!(refs.tag("v2.0").as_deref(), Some(d.as_str()));
        // An empty repository lists nothing; anything else is not a list.
        assert_eq!(Refs::parse(b"0000").unwrap(), Refs::default());
        assert_eq!(Refs::parse(b"<html>"), None);
        assert_eq!(base64(b"x-access-token:abc"), "eC1hY2Nlc3MtdG9rZW46YWJj");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"a"), "YQ==");
    }

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
                        ("x-ratelimit-limit", "60"),
                        ("x-ratelimit-remaining", "0"),
                        ("x-ratelimit-reset", "1300")
                    ]
                ),
                &body("API rate limit exceeded for 1.2.3.4."),
                1000
            ),
            Refusal::Primary {
                limit: Some(60),
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
