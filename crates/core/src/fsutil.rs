//! File system helpers.

use std::path::{Path, PathBuf};

/// Writes `bytes` to `path` through a temporary file in the same directory, so that readers
/// never see a partial file. Creates the directory if needed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    // Unique per process and per call, so concurrent writers never share a temporary file.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(
        ".{}.{}-{n}.tmp",
        path.file_name().and_then(|f| f.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `std::fs::canonicalize`, without the `\\?\` prefix Windows adds to ordinary drive paths
/// (R and other programs do not accept it).
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(simplify)
}

/// Drops Windows' `\\?\` prefix from a drive path (`\\?\C:\x` → `C:\x`). Other paths are
/// returned as they are.
pub fn simplify(path: PathBuf) -> PathBuf {
    if cfg!(windows)
        && let Some(rest) = path.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        && rest.as_bytes().get(1) == Some(&b':')
        && rest.as_bytes().get(2) == Some(&b'\\')
    {
        return PathBuf::from(rest);
    }
    path
}

/// Creates a link at `link` to the directory `target` (an absolute path): a symbolic link on
/// Unix, a junction on Windows, which needs neither administrator rights nor developer mode
/// (V11).
pub fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        junction::create(target, link)
    }
}

/// Where a link made by [`link_dir`] points, whether or not that still exists. `None` if
/// `path` is not such a link.
pub fn link_target(path: &Path) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::fs::read_link(path).ok()
    }
    #[cfg(windows)]
    {
        junction::get_target(path).ok().map(simplify)
    }
}

/// Removes a link made by [`link_dir`], never what it points to. OneDrive marks junctions in
/// the folders it syncs read-only, which blocks their removal, so that mark is cleared first
/// (V11).
pub fn remove_link(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::remove_file(path)
    }
    #[cfg(windows)]
    {
        match std::fs::remove_dir(path) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                let mut perms = std::fs::symlink_metadata(path)?.permissions();
                #[expect(clippy::permissions_set_readonly_false, reason = "Windows only")]
                perms.set_readonly(false);
                std::fs::set_permissions(path, perms)?;
                std::fs::remove_dir(path)
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_directories_and_removes_only_the_link() {
        let t = tempfile::tempdir().unwrap();
        let target = canonicalize(t.path()).unwrap().join("cache/pkg");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("DESCRIPTION"), "Package: pkg\n").unwrap();
        let link = t.path().join("lib/pkg");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        link_dir(&target, &link).unwrap();
        assert!(link.join("DESCRIPTION").is_file());
        assert_eq!(link_target(&link), Some(target.clone()));
        assert_eq!(link_target(&target), None, "a directory is not a link");
        // OneDrive marks junctions read-only; removal still works.
        if cfg!(windows) {
            let mut perms = std::fs::symlink_metadata(&link).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&link, perms).unwrap();
        }
        remove_link(&link).unwrap();
        assert!(!link.exists() && link_target(&link).is_none());
        assert!(target.join("DESCRIPTION").is_file(), "the target stays");
        // A link whose target is gone still reads as a link.
        link_dir(&target, &link).unwrap();
        std::fs::remove_dir_all(&target).unwrap();
        assert_eq!(link_target(&link), Some(target));
        assert!(!link.join("DESCRIPTION").exists());
        remove_link(&link).unwrap();
    }

    #[test]
    fn simplifies_verbatim_drive_paths_on_windows() {
        let p = PathBuf::from(r"\\?\C:\Users\me");
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\Users\me")
        } else {
            p.clone()
        };
        assert_eq!(simplify(p), expected);
        assert_eq!(
            simplify(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
    }
}
