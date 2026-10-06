//! GitHub: resolving references to commits, reading DESCRIPTION, downloading sources.
//!
//! The REST API is used only where needed (resolving a reference, comparing commits), because
//! without a token it allows 60 requests an hour. Files come from raw.githubusercontent.com and
//! codeload.github.com, which do not count against that limit. A token is read from
//! `GITHUB_PAT`, `GITHUB_TOKEN` or `GH_TOKEN`, as pak does; it is sent only to GitHub.

use crate::http::{Http, HttpError};
use crate::manifest::GitRef;

const API: &str = "https://api.github.com";

#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("{owner}/{repo}: cannot find `{reference}` (not a tag, a branch or a commit)")]
    UnknownRef {
        owner: String,
        repo: String,
        reference: String,
    },
    #[error("{owner}/{repo}: not found, or private (set GITHUB_PAT to a token that can read it)")]
    NotFound { owner: String, repo: String },
    #[error(
        "{owner}/{repo}: GitHub's API rate limit was reached; set GITHUB_PAT to a token to raise it"
    )]
    RateLimited { owner: String, repo: String },
    #[error("unexpected response from {url}: {message}")]
    Response { url: String, message: String },
}

pub struct GitHub<'a> {
    http: &'a Http,
    token: Option<String>,
}

impl<'a> GitHub<'a> {
    /// Uses a token from the environment, if any.
    pub fn new(http: &'a Http) -> GitHub<'a> {
        let token = ["GITHUB_PAT", "GITHUB_TOKEN", "GH_TOKEN"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
            .map(|t| t.trim().to_string());
        GitHub { http, token }
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
        self.http.get_with(&url, &headers).map_err(|e| match e {
            HttpError::Status {
                status: 403 | 429, ..
            } => GitHubError::RateLimited {
                owner: owner.to_string(),
                repo: repo.to_string(),
            },
            other => other.into(),
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
                if sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
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

    /// The DESCRIPTION file at a commit.
    pub fn description(
        &self,
        owner: &str,
        repo: &str,
        commit: &str,
    ) -> Result<String, GitHubError> {
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
        Ok(String::from_utf8_lossy(&body).into_owned())
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
    }
}
