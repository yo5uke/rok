//! CRAN-like repositories other than CRAN, declared in `[repositories]` (R-multiverse,
//! r-universe). Their indexes are not dated (R-multiverse's Production snapshots excepted), so
//! they are cached for an hour (V12).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::http::{Http, HttpError};
use crate::lockfile::Remote;
use crate::p3m::{Index, IndexEntry};
use crate::paths::UserDirs;
use crate::version::Version;

const MAX_AGE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{url} is not a gzip-compressed PACKAGES file.")]
    BadIndex { url: String },
}

/// Loaded indexes by `contrib` URL (`None`: the repository has no such index).
type Loaded = HashMap<String, Option<Arc<Index>>>;

/// Indexes of CRAN-like repositories. Each index is read once, even when many packages are
/// looked up in parallel.
pub struct Repositories<'a> {
    http: &'a Http,
    cache: PathBuf,
    loaded: Mutex<Loaded>,
}

impl<'a> Repositories<'a> {
    pub fn new(http: &'a Http, dirs: &UserDirs) -> Repositories<'a> {
        Repositories {
            http,
            cache: dirs.cache.join("repos"),
            loaded: Mutex::new(HashMap::new()),
        }
    }

    /// The source index (`<url>/src/contrib/PACKAGES.gz`).
    pub fn source_index(&self, url: &str) -> Result<Arc<Index>, RepoError> {
        let contrib = format!("{url}/src/contrib");
        self.index(&contrib, url)?
            .ok_or_else(|| RepoError::BadIndex { url: contrib })
    }

    /// The binary index in `contrib` ([`linux_contrib`] or [`windows_contrib`] of `url`), or
    /// `None` if the repository has none.
    pub fn binary_index(&self, url: &str, contrib: &str) -> Result<Option<Arc<Index>>, RepoError> {
        self.index(contrib, url)
    }

    fn index(&self, contrib: &str, label: &str) -> Result<Option<Arc<Index>>, RepoError> {
        // Held while loading, so parallel lookups wait for one download instead of repeating it.
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = loaded.get(contrib) {
            return Ok(i.clone());
        }
        let index = self.load(contrib, label)?.map(Arc::new);
        loaded.insert(contrib.to_string(), index.clone());
        Ok(index)
    }

    fn load(&self, contrib: &str, label: &str) -> Result<Option<Index>, RepoError> {
        let path = self.cache.join(cache_name(contrib)).join("PACKAGES.gz");
        let age = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok());
        let cached = std::fs::read(&path).ok();
        let bytes = match (&cached, age) {
            (Some(b), Some(age)) if age < MAX_AGE => b.clone(),
            _ => match self.http.get_bytes(&format!("{contrib}/PACKAGES.gz"), None) {
                Ok(b) => {
                    crate::fsutil::write_atomic(&path, &b).map_err(|source| RepoError::Io {
                        path: path.clone(),
                        source,
                    })?;
                    b
                }
                Err(HttpError::Status { status: 404, .. }) => return Ok(None),
                // Offline: an older copy is better than nothing.
                Err(e) => cached.ok_or(e)?,
            },
        };
        let mut text = String::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .read_to_string(&mut text)
            .map_err(|_| RepoError::BadIndex {
                url: format!("{contrib}/PACKAGES.gz"),
            })?;
        Ok(Some(Index::parse_repository(label, &text)))
    }
}

/// `<url>/bin/linux/<distro>-<arch>/<minor>/src/contrib` (r-universe's layout).
pub fn linux_contrib(url: &str, distro: &str, arch: &str, r_minor: &str) -> String {
    format!("{url}/bin/linux/{distro}-{arch}/{r_minor}/src/contrib")
}

/// `<url>/bin/windows/contrib/<minor>` (CRAN's layout, which r-universe follows).
pub fn windows_contrib(url: &str, r_minor: &str) -> String {
    format!("{url}/bin/windows/contrib/{r_minor}")
}

/// The URL of a package file (`ext`: `tar.gz`, or `zip` for Windows binaries) in a `contrib`
/// directory, honouring the index's `Path` (`<contrib>/<Path>/<name>_<version>.<ext>`, as R
/// builds it).
pub fn file_url(
    contrib: &str,
    name: &str,
    version: &Version,
    path: Option<&str>,
    ext: &str,
) -> String {
    match path {
        Some(p) => format!("{contrib}/{p}/{name}_{version}.{ext}"),
        None => format!("{contrib}/{name}_{version}.{ext}"),
    }
}

/// The entry for `name` `version` in an index, if it has it.
pub fn entry<'i>(index: &'i Index, name: &str, version: &Version) -> Option<&'i IndexEntry> {
    index.get(name).filter(|e| &e.version == version)
}

fn cache_name(contrib: &str) -> String {
    let s = contrib.split_once("://").map_or(contrib, |(_, rest)| rest);
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Reads `RemoteUrl`, `RemoteSha` and `RemoteSubdir` from an installed package's DESCRIPTION.
pub fn remote_of(package_dir: &Path) -> Option<Remote> {
    let text = std::fs::read_to_string(package_dir.join("DESCRIPTION")).ok()?;
    let records = crate::dcf::parse(&text).ok()?;
    let rec = records.first()?;
    Some(Remote {
        url: rec.get("RemoteUrl")?.to_string(),
        sha: rec.get("RemoteSha")?.to_string(),
        subdir: rec
            .get("RemoteSubdir")
            .filter(|d| !d.is_empty() && *d != ".")
            .map(str::to_string),
        rebuilt: false,
    })
}

/// The host of an http(s) URL, in lower case and without a port.
fn host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Whether `url` is CRAN: its main site and mirrors (`cran.*`), or a CRAN repository of a
/// Posit Package Manager (`p3m_cran` is the one rok uses, `<server>/cran`).
pub fn is_cran_url(url: &str, p3m_cran: &str) -> bool {
    let Some(h) = host(url) else {
        return false;
    };
    let path = url.splitn(4, '/').nth(3).unwrap_or("");
    h.starts_with("cran.")
        || h == "cloud.r-project.org"
        || (matches!(
            h.as_str(),
            "packagemanager.posit.co" | "packagemanager.rstudio.com"
        ) && (path == "cran" || path.starts_with("cran/")))
        || url.trim_end_matches('/') == p3m_cran
        || url.starts_with(&format!("{p3m_cran}/"))
}

/// A name for the repository at `url` in `[repositories]`: the universe for r-universe
/// (`ropensci` for `https://ropensci.r-universe.dev`), `multiverse` for R-multiverse's
/// Community repository, or else the host. `taken` holds the names already declared (name →
/// URL); a taken name gets a number.
pub fn alias_for(url: &str, taken: &std::collections::BTreeMap<String, String>) -> String {
    let h = host(url).unwrap_or_else(|| "repo".to_string());
    let base = if let Some(universe) = h.strip_suffix(".r-universe.dev") {
        universe.to_string()
    } else if h == "community.r-multiverse.org" {
        "multiverse".to_string()
    } else {
        h
    };
    let free = |name: &str| name != "cran" && !taken.contains_key(name);
    if free(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| free(name))
        .expect("a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_cran_urls() {
        let p3m = "https://packagemanager.posit.co/cran";
        for url in [
            "https://cloud.r-project.org",
            "https://cran.r-project.org/",
            "https://cran.rstudio.com",
            "https://cran.ism.ac.jp/",
            "https://packagemanager.posit.co/cran/latest",
            "https://packagemanager.posit.co/cran/__linux__/noble/2026-10-01",
            "https://p3m.example.org/cran/2026-10-01",
        ] {
            assert!(
                is_cran_url(
                    url,
                    if url.contains("example") {
                        "https://p3m.example.org/cran"
                    } else {
                        p3m
                    }
                ),
                "{url}"
            );
        }
        for url in [
            "https://community.r-multiverse.org",
            "https://ropensci.r-universe.dev",
            "https://packagemanager.posit.co/bioconductor",
            "not a url",
        ] {
            assert!(!is_cran_url(url, p3m), "{url}");
        }
    }

    #[test]
    fn names_repositories() {
        let none = std::collections::BTreeMap::new();
        assert_eq!(
            alias_for("https://ropensci.r-universe.dev/", &none),
            "ropensci"
        );
        assert_eq!(
            alias_for("https://community.r-multiverse.org", &none),
            "multiverse"
        );
        assert_eq!(
            alias_for("https://Repo.Example.org:8080/r", &none),
            "repo.example.org"
        );
        let taken = std::collections::BTreeMap::from([
            ("multiverse".to_string(), "https://other.org".to_string()),
            ("multiverse-2".to_string(), "https://other2.org".to_string()),
        ]);
        assert_eq!(
            alias_for("https://community.r-multiverse.org", &taken),
            "multiverse-3"
        );
    }

    #[test]
    fn builds_urls() {
        let v: Version = "1.16.0".parse().unwrap();
        let contrib = "https://community.r-multiverse.org/src/contrib";
        assert_eq!(
            file_url(contrib, "polars", &v, None, "tar.gz"),
            format!("{contrib}/polars_1.16.0.tar.gz")
        );
        assert_eq!(
            file_url(
                contrib,
                "polars",
                &v,
                Some("polars_1.16.0.tar.gz?sha256=ab&file="),
                "tar.gz"
            ),
            format!("{contrib}/polars_1.16.0.tar.gz?sha256=ab&file=/polars_1.16.0.tar.gz")
        );
        assert_eq!(
            linux_contrib("https://x.r-universe.dev", "noble", "x86_64", "4.6"),
            "https://x.r-universe.dev/bin/linux/noble-x86_64/4.6/src/contrib"
        );
        assert_eq!(
            windows_contrib("https://x.r-universe.dev", "4.6"),
            "https://x.r-universe.dev/bin/windows/contrib/4.6"
        );
        assert_eq!(
            cache_name("https://production.r-multiverse.org/2026-09-15/src/contrib"),
            "production.r-multiverse.org_2026-09-15_src_contrib"
        );
    }
}
