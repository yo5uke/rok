//! File system helpers.

use std::path::Path;

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
