//! rok's own releases on GitHub (requirements chapter 4): which file to download for this
//! machine, and which release is newer. Each release has `rok-<target>.tar.gz` (the binary)
//! with its `.sha256`, and the R package's source, `rok_<version>.tar.gz`.

use crate::platform::{Arch, Os};
use crate::version::Version;

/// The GitHub repository that publishes rok.
pub const OWNER: &str = "yo5uke";
pub const REPO: &str = "rok";

/// The Rust target whose binary runs on this machine (static musl builds on Linux).
pub fn target(os: Os, arch: Arch) -> &'static str {
    match (os, arch) {
        (Os::Linux, Arch::X86_64) => "x86_64-unknown-linux-musl",
        (Os::Linux, Arch::Aarch64) => "aarch64-unknown-linux-musl",
        (Os::MacOs, Arch::X86_64) => "x86_64-apple-darwin",
        (Os::MacOs, Arch::Aarch64) => "aarch64-apple-darwin",
        (Os::Windows, _) => "x86_64-pc-windows-msvc",
    }
}

/// The URL of a file of release `version`.
pub fn file_url(version: &Version, file: &str) -> String {
    format!("https://github.com/{OWNER}/{REPO}/releases/download/v{version}/{file}")
}

/// The binary's file for a target.
pub fn binary_file(target: &str) -> String {
    format!("rok-{target}.tar.gz")
}

/// The R package's source file of a version.
pub fn package_file(version: &Version) -> String {
    format!("rok_{version}.tar.gz")
}

/// A release newer than `current`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Newer {
    pub version: Version,
    pub prerelease: bool,
}

/// The newest release in GitHub's list that is newer than `current`, skipping drafts, and
/// pre-releases unless `prerelease`. Also returns the newest pre-release that was skipped, if
/// it is newer than what was picked (to mention it).
pub fn newest(
    releases: &serde_json::Value,
    current: &Version,
    prerelease: bool,
) -> (Option<Newer>, Option<Version>) {
    let mut all: Vec<Newer> = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            Some(Newer {
                version: tag.strip_prefix('v').unwrap_or(tag).parse().ok()?,
                prerelease: r["prerelease"].as_bool().unwrap_or(false),
            })
        })
        .filter(|r| r.version > *current)
        .collect();
    all.sort_by(|a, b| b.version.cmp(&a.version));
    let picked = all.iter().find(|r| prerelease || !r.prerelease).cloned();
    let skipped = all
        .iter()
        .find(|r| r.prerelease && picked.as_ref().is_none_or(|p| r.version > p.version))
        .map(|r| r.version.clone());
    (picked, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn picks_the_newest_release() {
        let list = json!([
            {"tag_name": "v0.3.0", "prerelease": true, "draft": false},
            {"tag_name": "v0.4.0", "prerelease": false, "draft": true},
            {"tag_name": "v0.2.0", "prerelease": false, "draft": false},
            {"tag_name": "v0.1.0", "prerelease": true, "draft": false}
        ]);
        let (picked, skipped) = newest(&list, &v("0.1.0"), false);
        assert_eq!(picked.unwrap().version, v("0.2.0"));
        assert_eq!(skipped, Some(v("0.3.0")));
        let (picked, skipped) = newest(&list, &v("0.1.0"), true);
        assert_eq!(
            picked,
            Some(Newer {
                version: v("0.3.0"),
                prerelease: true
            })
        );
        assert_eq!(skipped, None);
        assert_eq!(newest(&list, &v("0.3.0"), true), (None, None));
    }

    #[test]
    fn names_files() {
        assert_eq!(
            file_url(&v("0.1.0"), &binary_file(target(Os::Linux, Arch::X86_64))),
            "https://github.com/yo5uke/rok/releases/download/v0.1.0/rok-x86_64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(package_file(&v("0.1.0")), "rok_0.1.0.tar.gz");
        assert_eq!(target(Os::Windows, Arch::X86_64), "x86_64-pc-windows-msvc");
    }
}
