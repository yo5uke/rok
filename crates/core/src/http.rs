//! HTTP access.
//!
//! A thin wrapper over a blocking `ureq` agent. Requests follow redirects (P3M redirects
//! downloads to a CDN), honour the usual proxy variables, and return errors that name the URL.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Largest response body rok reads into memory (package indexes are a few MB).
const MAX_BODY: u64 = 512 * 1024 * 1024;
/// Largest file rok downloads to disk (R itself is about 110 MB).
const MAX_DOWNLOAD: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("Could not get {url} (HTTP {status}).")]
    Status { url: String, status: u16 },
    #[error(
        "Could not reach {url}: {source}\nCheck the network connection, and HTTPS_PROXY if you use a proxy."
    )]
    Transport {
        url: String,
        #[source]
        source: Box<ureq::Error>,
    },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The result of a HEAD request: status and the headers rok looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    headers: Vec<(String, String)>,
}

impl Head {
    /// A header value; names are case-insensitive.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A shareable HTTP client.
#[derive(Debug, Clone)]
pub struct Http {
    agent: ureq::Agent,
    /// The same settings, but redirects are returned instead of followed.
    no_redirect: ureq::Agent,
}

impl Default for Http {
    fn default() -> Self {
        Http::new()
    }
}

impl Http {
    pub fn new() -> Http {
        let config = |max_redirects: u32| {
            ureq::Agent::config_builder()
                .user_agent(format!("rok/{}", env!("CARGO_PKG_VERSION")))
                .timeout_connect(Some(Duration::from_secs(30)))
                .timeout_global(Some(Duration::from_secs(600)))
                .proxy(ureq::Proxy::try_from_env())
                .http_status_as_error(false)
                .max_redirects(max_redirects)
                .build()
        };
        Http {
            agent: config(10).into(),
            no_redirect: config(0).into(),
        }
    }

    /// GET `url` and return the body. `user_agent` replaces the default User-Agent.
    pub fn get_bytes(&self, url: &str, user_agent: Option<&str>) -> Result<Vec<u8>, HttpError> {
        match user_agent {
            Some(ua) => self.get_with(url, &[("User-Agent", ua)]),
            None => self.get_with(url, &[]),
        }
    }

    /// GET `url` with extra headers (such as `Authorization`) and return the body.
    pub fn get_with(&self, url: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>, HttpError> {
        let transport = |source| HttpError::Transport {
            url: url.to_string(),
            source: Box::new(source),
        };
        let mut req = self.agent.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let mut resp = req.call().map_err(transport)?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(HttpError::Status {
                url: url.to_string(),
                status,
            });
        }
        resp.body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(transport)
    }

    /// GET `url` into the file `path` without holding the body in memory. Returns the size.
    pub fn download(&self, url: &str, path: &Path) -> Result<u64, HttpError> {
        let mut resp = self
            .agent
            .get(url)
            .call()
            .map_err(|source| HttpError::Transport {
                url: url.to_string(),
                source: Box::new(source),
            })?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(HttpError::Status {
                url: url.to_string(),
                status,
            });
        }
        let io = |source| HttpError::Io {
            path: path.to_path_buf(),
            source,
        };
        let mut file = std::fs::File::create(path).map_err(io)?;
        let mut reader = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
        let size = std::io::copy(&mut reader, &mut file).map_err(io)?;
        file.sync_all().map_err(io)?;
        Ok(size)
    }

    /// HEAD `url`. Any status is returned as a [`Head`]; only transport failures are errors.
    pub fn head(&self, url: &str, user_agent: Option<&str>) -> Result<Head, HttpError> {
        let mut req = self.agent.head(url);
        if let Some(ua) = user_agent {
            req = req.header("User-Agent", ua);
        }
        Self::head_of(url, req.call())
    }

    /// GET `url` without following a redirect, returning the status and headers only. P3M
    /// answers a package request with a redirect whose headers say whether the file is a
    /// binary; this reads them in one round trip.
    pub fn get_headers(&self, url: &str, user_agent: Option<&str>) -> Result<Head, HttpError> {
        let mut req = self.no_redirect.get(url);
        if let Some(ua) = user_agent {
            req = req.header("User-Agent", ua);
        }
        Self::head_of(url, req.call())
    }

    fn head_of(
        url: &str,
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Head, HttpError> {
        let resp = result.map_err(|source| HttpError::Transport {
            url: url.to_string(),
            source: Box::new(source),
        })?;
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
            .collect();
        Ok(Head {
            status: resp.status().as_u16(),
            headers,
        })
    }
}
