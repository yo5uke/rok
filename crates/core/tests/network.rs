//! Tests against the real P3M and this machine. They need the network, so they are ignored by
//! default; run them with `cargo test -p rok-core -- --ignored`.

use std::time::Instant;

use rok_core::http::Http;
use rok_core::p3m::{DEFAULT_URL, P3m};
use rok_core::paths::UserDirs;
use rok_core::platform::Platform;
use rok_core::rdetect;

fn temp_dirs() -> (tempfile::TempDir, UserDirs) {
    let t = tempfile::tempdir().unwrap();
    let dirs = UserDirs {
        data: t.path().join("data"),
        cache: t.path().join("cache"),
    };
    (t, dirs)
}

#[test]
#[ignore = "needs the network"]
fn resolves_snapshots_and_reads_an_index() {
    let (_t, dirs) = temp_dirs();
    let p3m = P3m::new(DEFAULT_URL, Http::new(), &dirs);

    let (latest, dates) = p3m.resolve_snapshot(None).unwrap();
    assert_eq!(dates.dates.first().map(String::as_str), Some("2017-10-10"));
    assert_eq!(&latest, dates.dates.last().unwrap());
    // 2017-10-11 is not a snapshot date; it resolves to the one before (V3).
    assert_eq!(
        p3m.resolve_snapshot(Some("2017-10-11")).unwrap().0,
        "2017-10-10"
    );

    let start = Instant::now();
    let index = p3m.index("2026-10-01").unwrap();
    let download = start.elapsed();
    let start = Instant::now();
    let cached = p3m.index("2026-10-01").unwrap();
    let from_cache = start.elapsed();
    eprintln!(
        "index 2026-10-01: {} packages, {} skipped; download+parse {download:?}, cached parse {from_cache:?}",
        index.len(),
        index.skipped.len()
    );
    assert!(index.len() > 20_000);
    assert_eq!(cached.len(), index.len());
    assert_eq!(
        index.get("data.table").unwrap().version.as_str(),
        "1.18.6.1"
    );
    assert_eq!(index.get("boot").unwrap().version.as_str(), "1.3-32");
}

#[test]
#[ignore = "needs the network"]
fn p3m_serves_a_binary_for_this_r_user_agent() {
    let platform = Platform::detect();
    let Some(distro) = platform.p3m_linux_name() else {
        eprintln!("no P3M binaries for {platform}; skipping");
        return;
    };
    let ua = platform.r_user_agent(&"4.4.2".parse().unwrap());
    let url = format!(
        "{DEFAULT_URL}/cran/__linux__/{distro}/2026-10-01/src/contrib/data.table_1.18.6.1.tar.gz"
    );
    let head = Http::new().head(&url, Some(&ua)).unwrap();
    assert_eq!(head.header("x-package-type"), Some("binary"));
    assert_eq!(
        head.header("X-Package-Binary-Tag")
            .map(|t| t.starts_with("4.4-")),
        Some(true)
    );
}

#[test]
#[ignore = "inspects this machine"]
fn detects_this_machine() {
    let platform = Platform::detect();
    eprintln!(
        "platform: {platform}, P3M name: {:?}",
        platform.p3m_linux_name()
    );
    let dirs = UserDirs::from_env().unwrap();
    eprintln!(
        "data: {}\ncache: {}",
        dirs.data.display(),
        dirs.cache.display()
    );
    let start = Instant::now();
    let found = rdetect::find_installations(&dirs, std::env::var_os("PATH"));
    eprintln!(
        "found {} R installation(s) in {:?}",
        found.len(),
        start.elapsed()
    );
    for r in &found {
        eprintln!("  R {} ({:?}) at {}", r.version, r.kind, r.r_home.display());
    }
}

#[test]
#[ignore = "needs the network"]
fn resolves_the_benchmark_projects() {
    use rok_core::platform::Os;
    use rok_core::resolve::{Request, SnapshotSource, SourceError, resolve};
    use std::collections::HashMap;

    let (_t, dirs) = temp_dirs();
    let p3m = P3m::new(DEFAULT_URL, Http::new(), &dirs);
    p3m.index("2026-10-01").unwrap(); // download once, outside the timing
    for (project, roots) in [
        ("small", &["fixest", "modelsummary"][..]),
        ("medium", &["tidyverse"][..]),
        ("gis", &["sf", "terra", "tmap"][..]),
    ] {
        let start = Instant::now();
        let source = SnapshotSource::new(
            "2026-10-01",
            |d: &str| p3m.index(d).map_err(|e| SourceError(e.to_string())),
            HashMap::new(),
        );
        let request = Request {
            r_version: "4.6.1".parse().unwrap(),
            os: Os::Linux,
            requirements: roots
                .iter()
                .map(|r| (r.to_string(), Default::default()))
                .collect(),
            preferred: HashMap::new(),
            include_linking_to: true,
        };
        let result = resolve(&source, &request).unwrap();
        let elapsed = start.elapsed();
        let mut without_linking = request.clone();
        without_linking.include_linking_to = false;
        let fewer = resolve(&source, &without_linking).unwrap().len();
        let recommended = result
            .iter()
            .filter(|r| rok_core::rpkgs::is_recommended(&r.name))
            .count();
        eprintln!(
            "{project}: {} packages ({recommended} recommended), {fewer} without LinkingTo; index load + resolve {elapsed:?}",
            result.len()
        );
        assert!(result.iter().any(|r| r.name == roots[0]));
    }
}
