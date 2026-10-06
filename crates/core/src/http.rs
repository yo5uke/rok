//! HTTP access.
//!
//! A thin wrapper over a blocking `ureq` agent. Requests follow redirects (P3M redirects
//! downloads to a CDN), honour the usual proxy variables, and return errors that name the URL.

use std::time::Duration;

/// Largest response body rok reads into memory (package indexes are a few MB).
const MAX_BODY: u64 = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("{url}: HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("{url}: {source}")]
    Transport {
        url: String,
        #[source]
        source: ureq::Error,
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
}

impl Default for Http {
    fn default() -> Self {
        Http::new()
    }
}

impl Http {
    pub fn new() -> Http {
        let config = ureq::Agent::config_builder()
            .user_agent(format!("rok/{}", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(600)))
            .proxy(ureq::Proxy::try_from_env())
            .http_status_as_error(false)
            .build();
        Http {
            agent: config.into(),
        }
    }

    /// GET `url` and return the body. `user_agent` replaces the default User-Agent.
    pub fn get_bytes(&self, url: &str, user_agent: Option<&str>) -> Result<Vec<u8>, HttpError> {
        let transport = |source| HttpError::Transport {
            url: url.to_string(),
            source,
        };
        let mut req = self.agent.get(url);
        if let Some(ua) = user_agent {
            req = req.header("User-Agent", ua);
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

    /// HEAD `url`. Any status is returned as a [`Head`]; only transport failures are errors.
    pub fn head(&self, url: &str, user_agent: Option<&str>) -> Result<Head, HttpError> {
        let mut req = self.agent.head(url);
        if let Some(ua) = user_agent {
            req = req.header("User-Agent", ua);
        }
        let resp = req.call().map_err(|source| HttpError::Transport {
            url: url.to_string(),
            source,
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
