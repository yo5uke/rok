//! `rok self update` (requirements chapter 5): replaces this binary with the newest release of
//! rok, and updates the copies of the R package that projects load (one per R minor version,
//! under rok's data directory).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use serde_json::json;

use rok_core::github::GitHub;
use rok_core::ops::{self, Env};
use rok_core::version::Version;
use rok_core::{archive, fsutil, install, rdetect, release};

use crate::ui::Ui;

pub fn update(ui: &Ui, prerelease: bool) -> anyhow::Result<()> {
    let env = Env::from_env()?;
    let exe = fsutil::canonicalize(&std::env::current_exe()?)?;
    // A binary replaced on Windows is left beside the new one until the next update.
    let _ = std::fs::remove_file(old_exe(&exe));
    let current: Version = env!("CARGO_PKG_VERSION").parse()?;
    let releases = GitHub::new(&env.http).releases(release::OWNER, release::REPO)?;
    let (newer, skipped) = release::newest(&releases, &current, prerelease);
    let Some(newer) = newer else {
        ui.success(&format!("rok {current} is the newest release."));
        if let Some(v) = skipped {
            ui.info(&format!(
                "rok {v} is a pre-release; `rok self update --prerelease` installs it."
            ));
        }
        ui.result(json!({ "command": "self update", "from": current.to_string(), "to": null }));
        return Ok(());
    };
    let to = &newer.version;
    if !ui.confirm(
        "self-update",
        &format!(
            "Update rok {current} to {to}{} ({})?",
            if newer.prerelease {
                " (a pre-release)"
            } else {
                ""
            },
            exe.display()
        ),
        true,
    )? {
        bail!("Cancelled. Nothing was changed.");
    }

    // Download and check everything before replacing anything.
    let file = release::binary_file(release::target(env.platform.os, env.platform.arch));
    let url = release::file_url(to, &file);
    ui.step(&format!("Downloading rok {to}"));
    let bytes = env.http.get_bytes(&url, None)?;
    let sum = String::from_utf8(env.http.get_bytes(&format!("{url}.sha256"), None)?)?;
    let expected = sum
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if install::sha256_hex(&bytes) != expected {
        bail!("The download of {url} is damaged (its SHA-256 does not match).");
    }
    let dir = exe.parent().context("the rok binary has no directory")?;
    let work = dir.join(format!(".rok-update-{}", std::process::id()));
    let result = replace(&work, &bytes, &exe, to);
    let _ = std::fs::remove_dir_all(&work);
    result?;

    let updated = update_r_packages(ui, &env, to)?;
    ui.success(&format!("Updated rok to {to}."));
    ui.info(
        "The rok package in your own R library is not changed; update it the way you installed it.",
    );
    ui.result(json!({
        "command": "self update",
        "from": current.to_string(),
        "to": to.to_string(),
        "r_packages": updated,
    }));
    Ok(())
}

/// Unpacks the new binary next to `exe`, checks that it runs and reports `version`, and
/// swaps it in.
fn replace(work: &Path, archive_bytes: &[u8], exe: &Path, version: &Version) -> anyhow::Result<()> {
    std::fs::create_dir_all(work).with_context(|| {
        format!(
            "rok cannot write in {}",
            work.parent().unwrap_or(work).display()
        )
    })?;
    archive::unpack(archive_bytes, work).map_err(|e| anyhow::anyhow!("cannot unpack rok: {e}"))?;
    let new = work.join(rdetect::exe_name("rok"));
    if !new.is_file() {
        bail!("the release has no {}", rdetect::exe_name("rok"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    }
    let out = Command::new(&new).arg("--version").output()?;
    let reported = String::from_utf8_lossy(&out.stdout);
    if reported.trim() != format!("rok {version}") {
        bail!(
            "the downloaded binary reports `{}`, not rok {version}",
            reported.trim()
        );
    }
    if cfg!(windows) {
        // A running program cannot be replaced on Windows, but it can be renamed.
        let old = old_exe(exe);
        std::fs::rename(exe, &old).with_context(|| format!("cannot replace {}", exe.display()))?;
        if let Err(e) = std::fs::rename(&new, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(e).with_context(|| format!("cannot replace {}", exe.display()));
        }
    } else {
        std::fs::rename(&new, exe).with_context(|| format!("cannot replace {}", exe.display()))?;
    }
    Ok(())
}

fn old_exe(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Installs the R package of `version` into each library under rok's data directory that
/// holds a copy of it (`library/R-<minor>`), with an installed R of that minor version.
/// Returns the minor versions updated.
fn update_r_packages(ui: &Ui, env: &Env, version: &Version) -> anyhow::Result<Vec<String>> {
    let libraries: Vec<(String, PathBuf)> = std::fs::read_dir(env.dirs.data.join("library"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("rok").join("DESCRIPTION").is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            Some((name.strip_prefix("R-")?.to_string(), e.path()))
        })
        .collect();
    if libraries.is_empty() {
        return Ok(Vec::new());
    }
    let file = release::package_file(version);
    ui.step(&format!("Downloading the rok R package {version}"));
    let bytes = env
        .http
        .get_bytes(&release::file_url(version, &file), None)?;
    let tarball = env.dirs.cache.join(&file);
    fsutil::write_atomic(&tarball, &bytes)?;
    let installs = env.r_installations();
    let mut updated = Vec::new();
    for (minor, lib) in libraries {
        let Some(r) = ops::select_r(&installs, &minor, None) else {
            ui.warn(&format!(
                "R {minor} is not installed, so its copy of the rok package was not updated; run `rok::setup()` in R {minor}."
            ));
            continue;
        };
        let status = Command::new(r.r_home.join("bin").join(rdetect::exe_name("R")))
            .env_remove("R_HOME")
            .args(["CMD", "INSTALL", "--no-test-load", "-l"])
            .arg(&lib)
            .arg(&tarball)
            .output()?;
        if status.status.success() {
            updated.push(minor);
        } else {
            ui.warn(&format!(
                "The rok package for R {minor} could not be updated:\n{}",
                String::from_utf8_lossy(&status.stderr).trim()
            ));
        }
    }
    let _ = std::fs::remove_file(&tarball);
    Ok(updated)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A release archive whose `rok` is a script that reports `version`.
    fn archive(version: &str) -> Vec<u8> {
        let script = format!("#!/bin/sh\necho 'rok {version}'\n");
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(script.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "rok", script.as_bytes())
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn replaces_the_binary_only_with_the_expected_version() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("rok");
        std::fs::write(&exe, "old").unwrap();
        let v: Version = "9.9.9".parse().unwrap();
        // A binary that reports another version is refused, and nothing changes.
        let err = replace(&t.path().join("w1"), &archive("9.9.8"), &exe, &v).unwrap_err();
        assert!(err.to_string().contains("reports `rok 9.9.8`"), "{err}");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        replace(&t.path().join("w2"), &archive("9.9.9"), &exe, &v).unwrap();
        let out = Command::new(&exe).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "rok 9.9.9");
        assert_eq!(old_exe(&exe), t.path().join("rok.old"));
    }
}
