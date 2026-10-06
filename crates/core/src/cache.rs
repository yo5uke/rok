//! The global package cache: installed packages shared by all projects.
//!
//! Layout: `<cache>/packages/<name>/<version>/<key>/<name>/`, where `key` identifies the build:
//! P3M's binary tag (`4.6-noble`, `4.6-noble-arm64`) for binaries, or
//! `<R minor>-<platform>-source[-<env>]` for packages built here. Project libraries link to
//! these directories. Entries are created in a temporary directory and renamed into place, so
//! a reader never sees a partial package, and two processes adding the same package do not
//! clash (the second rename finds the first one's result).

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::dcf;
use crate::paths::UserDirs;
use crate::version::Version;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{name} {version}: {reason}")]
    BadPackage {
        name: String,
        version: String,
        reason: String,
    },
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> CacheError + '_ {
    move |source| CacheError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone)]
pub struct PackageCache {
    root: PathBuf,
}

impl PackageCache {
    pub fn new(dirs: &UserDirs) -> PackageCache {
        PackageCache {
            root: dirs.cache.join("packages"),
        }
    }

    /// The directory that holds every cached package.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where `name` `version` built as `key` lives (whether or not it exists).
    pub fn path(&self, name: &str, version: &Version, key: &str) -> PathBuf {
        self.root
            .join(name)
            .join(version.as_str())
            .join(key)
            .join(name)
    }

    /// The cached package, if present.
    pub fn get(&self, name: &str, version: &Version, key: &str) -> Option<PathBuf> {
        let p = self.path(name, version, key);
        p.join("DESCRIPTION").is_file().then_some(p)
    }

    /// A temporary directory inside the cache (same file system, so renames are cheap).
    pub fn temp_dir(&self) -> Result<tempfile::TempDir, CacheError> {
        let tmp = self.root.join(".tmp");
        std::fs::create_dir_all(&tmp).map_err(io_err(&tmp))?;
        tempfile::Builder::new()
            .prefix("rok-")
            .tempdir_in(&tmp)
            .map_err(io_err(&tmp))
    }

    /// Extracts a binary package (`.tar.gz`) into the cache and returns its directory. The
    /// archive must contain `<name>/DESCRIPTION` with the expected name and version, and a
    /// `Built` field (which source packages lack).
    pub fn insert_binary(
        &self,
        name: &str,
        version: &Version,
        key: &str,
        tar_gz: &[u8],
    ) -> Result<PathBuf, CacheError> {
        let tmp = self.temp_dir()?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tar_gz));
        // `unpack` refuses entries that would land outside the target directory.
        archive
            .unpack(tmp.path())
            .map_err(|e| CacheError::BadPackage {
                name: name.to_string(),
                version: version.to_string(),
                reason: format!("cannot extract the archive ({e})"),
            })?;
        let built = check_package(&tmp.path().join(name), name, version)?;
        if !built {
            return Err(CacheError::BadPackage {
                name: name.to_string(),
                version: version.to_string(),
                reason: "expected a binary package, got a source package".to_string(),
            });
        }
        self.insert_dir(name, version, key, &tmp.path().join(name))
    }

    /// Moves an installed package directory (for example built by `R CMD INSTALL`) into the
    /// cache and returns its new location. `dir` must be on the cache's file system.
    pub fn insert_dir(
        &self,
        name: &str,
        version: &Version,
        key: &str,
        dir: &Path,
    ) -> Result<PathBuf, CacheError> {
        check_package(dir, name, version)?;
        let dest = self.path(name, version, key);
        let parent = dest.parent().expect("cache paths have a parent");
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        match std::fs::rename(dir, &dest) {
            Ok(()) => Ok(dest),
            // Another process cached it first; use theirs.
            Err(_) if dest.join("DESCRIPTION").is_file() => Ok(dest),
            Err(e) => Err(io_err(&dest)(e)),
        }
    }
}

/// Checks `<dir>/DESCRIPTION` and returns whether the package is installed (has `Built`).
fn check_package(dir: &Path, name: &str, version: &Version) -> Result<bool, CacheError> {
    let bad = |reason: String| CacheError::BadPackage {
        name: name.to_string(),
        version: version.to_string(),
        reason,
    };
    let path = dir.join("DESCRIPTION");
    let mut text = String::new();
    std::fs::File::open(&path)
        .and_then(|mut f| f.read_to_string(&mut text))
        .map_err(|e| bad(format!("no readable DESCRIPTION ({e})")))?;
    let records = dcf::parse(&text).map_err(|e| bad(e.to_string()))?;
    let rec = records
        .first()
        .ok_or_else(|| bad("empty DESCRIPTION".to_string()))?;
    if rec.get("Package") != Some(name) {
        return Err(bad(format!(
            "DESCRIPTION names package {:?}",
            rec.get("Package").unwrap_or("")
        )));
    }
    let found: Option<Version> = rec.get("Version").and_then(|v| v.parse().ok());
    if found.as_ref() != Some(version) {
        return Err(bad(format!(
            "DESCRIPTION has version {:?}",
            rec.get("Version").unwrap_or("")
        )));
    }
    Ok(rec.get("Built").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, content.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn cache(t: &tempfile::TempDir) -> PackageCache {
        PackageCache::new(&UserDirs {
            data: t.path().join("data"),
            cache: t.path().join("cache"),
        })
    }

    #[test]
    fn inserts_and_finds_binary_packages() {
        let t = tempfile::tempdir().unwrap();
        let c = cache(&t);
        let v: Version = "2.6.1".parse().unwrap();
        assert_eq!(c.get("R6", &v, "4.6-noble"), None);
        let bin = tar_gz(&[
            (
                "R6/DESCRIPTION",
                "Package: R6\nVersion: 2.6.1\nBuilt: R 4.6.0; ; 2026-01-01; unix\n",
            ),
            ("R6/R/R6", ""),
        ]);
        let path = c.insert_binary("R6", &v, "4.6-noble", &bin).unwrap();
        assert!(path.ends_with("packages/R6/2.6.1/4.6-noble/R6"));
        assert_eq!(c.get("R6", &v, "4.6-noble"), Some(path.clone()));
        // Inserting again (as a racing process would) keeps the existing entry.
        assert_eq!(c.insert_binary("R6", &v, "4.6-noble", &bin).unwrap(), path);
        // Temporary directories are cleaned up.
        assert_eq!(std::fs::read_dir(c.root().join(".tmp")).unwrap().count(), 0);
    }

    #[test]
    fn rejects_unexpected_archives() {
        let t = tempfile::tempdir().unwrap();
        let c = cache(&t);
        let v: Version = "2.6.1".parse().unwrap();
        let source = tar_gz(&[(
            "R6/DESCRIPTION",
            "Package: R6\nVersion: 2.6.1\nPackaged: 2026-01-01\n",
        )]);
        let err = c
            .insert_binary("R6", &v, "k", &source)
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected a binary package"), "{err}");
        let wrong = tar_gz(&[("R6/DESCRIPTION", "Package: R6\nVersion: 2.5.0\nBuilt: R\n")]);
        assert!(
            c.insert_binary("R6", &v, "k", &wrong)
                .unwrap_err()
                .to_string()
                .contains("version")
        );
        assert!(c.insert_binary("R6", &v, "k", b"not gzip").is_err());
        assert_eq!(c.get("R6", &v, "k"), None);
    }
}
