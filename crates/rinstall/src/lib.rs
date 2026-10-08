//! Installation of R itself (requirements chapter 6; V1 and V1b).
//!
//! R comes from Posit's builds. On Linux the portable build (`manylinux_2_34`: it bundles its
//! libraries and runs wherever it is unpacked, on glibc 2.34 or later) comes first. Where it
//! cannot be used, the build for the distribution is unpacked and the paths compiled into it
//! are rewritten. On Windows the portable build (a `.zip`) is unpacked; no installer runs and
//! nothing is written to the registry (V5). Nothing needs administrator rights; rok installs
//! into `<data>/r/<version>`, or on Windows `%LOCALAPPDATA%/Programs/R/R-<version>`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use rok_core::http::{Http, HttpError};
use rok_core::paths::UserDirs;
use rok_core::platform::{Arch, LinuxDistro, Os, Platform};
use rok_core::rdetect::{self, RInstallation, RKind};
use rok_core::version::Version;

/// Where Posit publishes its R builds.
pub const BUILDS_URL: &str = "https://cdn.posit.co/r";

/// How long the list of R releases is reused before it is downloaded again.
const RELEASES_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum RInstallError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("R {version} is already installed at {}", path.display())]
    AlreadyInstalled { version: Version, path: PathBuf },
    #[error("R {version} is not installed by rok (rok removes only the R versions it installed)")]
    NotManaged { version: Version },
    #[error("no R release matches `{0}`; run `rok r list` to see the available versions")]
    NoMatch(String),
    #[error("`{0}` is not an R version; use the form 4.6, 4.6.1 or latest")]
    BadRequest(String),
    #[error("R {version} is not available for this machine:\n{reasons}")]
    Unavailable { version: Version, reasons: String },
    #[error("installing R is not supported on {0} yet")]
    Unsupported(String),
    #[error("the installed R {version} does not work: {message}")]
    Broken { version: Version, message: String },
    #[error("unexpected response from {url}: {message}")]
    Response { url: String, message: String },
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> RInstallError + '_ {
    move |source| RInstallError::Io {
        path: path.to_path_buf(),
        source,
    }
}

// ---- which version ----

/// An R version as the user asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// The newest release.
    Latest,
    /// The newest patch of a minor version (`4.5`).
    Minor(String),
    /// Exactly this version (`4.5.1`).
    Exact(Version),
}

impl FromStr for Request {
    type Err = RInstallError;

    fn from_str(s: &str) -> Result<Request, RInstallError> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("latest") {
            return Ok(Request::Latest);
        }
        let bad = || RInstallError::BadRequest(s.to_string());
        let v: Version = s.parse().map_err(|_| bad())?;
        match v.parts().len() {
            2 => Ok(Request::Minor(v.minor())),
            3 => Ok(Request::Exact(v)),
            _ => Err(bad()),
        }
    }
}

impl Request {
    /// The release to install: the newest that matches, from `releases` (newest first).
    pub fn pick(&self, releases: &[Version]) -> Option<Version> {
        releases.iter().find(|v| self.matches(v)).cloned()
    }

    /// Whether an R version satisfies the request (any version for [`Request::Latest`]).
    pub fn matches(&self, v: &Version) -> bool {
        match self {
            Request::Latest => true,
            Request::Minor(m) => v.minor() == *m,
            Request::Exact(e) => v == e,
        }
    }
}

/// The R releases Posit builds, newest first.
#[derive(Debug, Clone)]
pub struct Releases {
    pub versions: Vec<Version>,
    /// The list could not be downloaded, so an older copy was used.
    pub stale: bool,
}

/// The R releases (from Posit's `versions.json`, cached for a day). Development builds
/// (`devel`, `next`) are left out.
pub fn releases(http: &Http, dirs: &UserDirs) -> Result<Releases, RInstallError> {
    let path = dirs.cache.join("r").join("versions.json");
    let cached = std::fs::read(&path).ok();
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok());
    let url = format!("{BUILDS_URL}/versions.json");
    let (body, stale) = match (&cached, age) {
        (Some(b), Some(age)) if age < RELEASES_MAX_AGE => (b.clone(), false),
        _ => match http.get_bytes(&url, None) {
            Ok(b) => {
                rok_core::fsutil::write_atomic(&path, &b).map_err(io_err(&path))?;
                (b, false)
            }
            Err(e) => (cached.ok_or(e)?, true),
        },
    };
    #[derive(serde::Deserialize)]
    struct List {
        r_versions: Vec<String>,
    }
    let list: List = serde_json::from_slice(&body).map_err(|e| RInstallError::Response {
        url: url.clone(),
        message: e.to_string(),
    })?;
    let mut versions: Vec<Version> = list
        .r_versions
        .iter()
        .filter_map(|v| v.parse::<Version>().ok())
        .filter(|v| v.parts().len() == 3)
        .collect();
    versions.sort_by(|a, b| b.cmp(a));
    Ok(Releases { versions, stale })
}

// ---- which build ----

/// A kind of R build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildKind {
    /// Posit's portable build: libraries bundled, runs where it is unpacked.
    Portable,
    /// The build for a distribution (`ubuntu-2404`); its paths are rewritten after unpacking.
    Distribution(String),
}

impl std::fmt::Display for BuildKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildKind::Portable => f.write_str("portable build"),
            BuildKind::Distribution(d) => write!(f, "{d} build"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub kind: BuildKind,
    pub url: String,
}

/// The glibc version, from `getconf GNU_LIBC_VERSION` (`glibc 2.39`). `None` on other C
/// libraries (musl).
pub fn glibc_version() -> Option<(u32, u32)> {
    let out = Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .output()
        .ok()?;
    parse_glibc(&String::from_utf8_lossy(&out.stdout))
}

fn parse_glibc(text: &str) -> Option<(u32, u32)> {
    let v = text.trim().strip_prefix("glibc ")?;
    let mut parts = v.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Posit's name for a distribution's builds (`ubuntu-2404`, `debian-12`, `rhel-9`).
pub fn distribution_name(d: &LinuxDistro) -> Option<String> {
    let major = d.version_id.split('.').next().unwrap_or("");
    match d.id.as_str() {
        "ubuntu" => Some(format!("ubuntu-{}", d.version_id.replace('.', ""))),
        "debian" => Some(format!("debian-{major}")),
        "rhel" | "rocky" | "almalinux" | "centos" => match major {
            "7" | "8" => Some(format!("centos-{major}")),
            "" => None,
            m => Some(format!("rhel-{m}")),
        },
        "opensuse-leap" | "sles" => Some(format!("opensuse-{}", d.version_id.replace('.', ""))),
        "fedora" => Some(format!("fedora-{major}")),
        // Ubuntu derivatives use the builds of the Ubuntu release they are based on.
        _ => match d.ubuntu_codename.as_deref()? {
            "focal" => Some("ubuntu-2004".into()),
            "jammy" => Some("ubuntu-2204".into()),
            "noble" => Some("ubuntu-2404".into()),
            "resolute" => Some("ubuntu-2604".into()),
            _ => None,
        },
    }
}

/// The builds of R `version` that can run on this machine, best first.
pub fn builds(
    platform: &Platform,
    glibc: Option<(u32, u32)>,
    version: &Version,
) -> Result<Vec<Build>, RInstallError> {
    match (platform.os, platform.arch) {
        (Os::Linux, _) => {}
        (Os::Windows, Arch::X86_64) => {
            return Ok(vec![Build {
                kind: BuildKind::Portable,
                url: format!("{BUILDS_URL}/windows/R-{version}-windows.zip"),
            }]);
        }
        (Os::MacOs, arch) => {
            let arm = if arch == Arch::Aarch64 { "-arm64" } else { "" };
            return Ok(vec![Build {
                kind: BuildKind::Portable,
                url: format!("{BUILDS_URL}/macos/R-{version}-macos{arm}.tar.gz"),
            }]);
        }
        _ => return Err(RInstallError::Unsupported(platform.to_string())),
    }
    let arm = if platform.arch == Arch::Aarch64 {
        "-arm64"
    } else {
        ""
    };
    let mut out = Vec::new();
    if glibc.is_some_and(|v| v >= (2, 34)) {
        out.push(Build {
            kind: BuildKind::Portable,
            url: format!("{BUILDS_URL}/manylinux_2_34/R-{version}-manylinux_2_34{arm}.tar.gz"),
        });
    }
    if let Some(name) = platform.distro.as_ref().and_then(distribution_name) {
        out.push(Build {
            url: format!("{BUILDS_URL}/{name}/R-{version}-{name}{arm}.tar.gz"),
            kind: BuildKind::Distribution(name),
        });
    }
    Ok(out)
}

/// Commands that install R `version` system-wide into `/opt/R/<version>` with the
/// distribution's package manager. rok only shows them: they need administrator rights.
pub fn system_install_commands(platform: &Platform, version: &Version) -> Option<Vec<String>> {
    let d = platform.distro.as_ref()?;
    let name = distribution_name(d)?;
    let like = |x: &str| d.id == x || d.id_like.iter().any(|l| l == x);
    if like("debian") || like("ubuntu") {
        let arch = if platform.arch == Arch::Aarch64 {
            "arm64"
        } else {
            "amd64"
        };
        let file = format!("r-{version}_1_{arch}.deb");
        Some(vec![
            format!("curl -fLO {BUILDS_URL}/{name}/pkgs/{file}"),
            format!("sudo apt-get install -y ./{file}"),
        ])
    } else if like("rhel") || like("fedora") || like("centos") {
        let arch = platform.arch.r_name();
        Some(vec![format!(
            "sudo dnf install -y {BUILDS_URL}/{name}/pkgs/R-{version}-1-1.{arch}.rpm"
        )])
    } else {
        None
    }
}

// ---- installing ----

/// An R installed by rok.
#[derive(Debug, Clone)]
pub struct Installed {
    pub installation: RInstallation,
    pub build: BuildKind,
}

/// The directory of a managed R version: `<version>` on Linux, `R-<version>` on Windows (as
/// CRAN's installer names it) and macOS (as Posit's builds unpack).
pub fn install_dir(dirs: &UserDirs, version: &Version) -> PathBuf {
    if cfg!(target_os = "linux") {
        dirs.r_installs().join(version.as_str())
    } else {
        dirs.r_installs().join(format!("R-{version}"))
    }
}

/// R `version` as rok installs it (whether or not it is installed yet): R_HOME is
/// [`install_dir`] itself on Windows and macOS, and its `lib/R` on Linux.
pub fn managed_installation(dirs: &UserDirs, version: &Version) -> RInstallation {
    let dir = install_dir(dirs, version);
    RInstallation {
        version: version.clone(),
        r_home: if cfg!(target_os = "linux") {
            dir.join("lib").join("R")
        } else {
            dir.clone()
        },
        executable: dir.join("bin").join(rdetect::exe_name("R")),
        kind: RKind::Managed,
    }
}

/// Whether rok installed R `version` (on Windows, R installed by CRAN's installer may sit in
/// the same directory).
pub fn is_managed(dirs: &UserDirs, version: &Version) -> bool {
    let dir = install_dir(dirs, version);
    dir.join("bin").is_dir() && (!cfg!(windows) || dir.join(rdetect::MANAGED_MARK).is_file())
}

/// CRAN's installers of past R releases for Windows.
pub const WINDOWS_OLD_RELEASES: &str = "https://cloud.r-project.org/bin/windows/base/old/";

/// CRAN's installers of R releases for macOS.
pub const MACOS_RELEASES: &str = "https://cloud.r-project.org/bin/macosx/";

/// Where to get R `version` by hand when Posit has no portable build of it.
fn installer_advice(os: Os, version: &Version) -> String {
    match os {
        Os::MacOs => format!(
            "  • Install it with CRAN's installer from {MACOS_RELEASES} (older releases are linked there); rok then finds it"
        ),
        _ => format!(
            "  • Install it with CRAN's installer from {WINDOWS_OLD_RELEASES}{version}/; rok then finds it"
        ),
    }
}

/// Where to find R installers by hand on Windows or macOS.
pub fn installers_url(os: Os) -> &'static str {
    if os == Os::MacOs {
        MACOS_RELEASES
    } else {
        WINDOWS_OLD_RELEASES
    }
}

/// Whether Posit builds R `version` for this machine. Portable builds exist for R 3.6.3 and
/// 4.1.0 or later on Windows (V5), and 4.1.0 or later on macOS (V6); on Linux the release
/// list does not tell, so all count.
pub fn is_built_for(platform: &Platform, version: &Version) -> bool {
    let part = |i: usize| version.parts().get(i).copied().unwrap_or(0);
    let from_4_1 = (part(0), part(1)) >= (4, 1);
    match platform.os {
        Os::Windows => version.as_str() == "3.6.3" || from_4_1,
        Os::MacOs => from_4_1,
        Os::Linux => true,
    }
}

/// Installs R `version` into `<data>/r/<version>`. `progress` receives one line per step.
pub fn install(
    http: &Http,
    dirs: &UserDirs,
    platform: &Platform,
    version: &Version,
    progress: &dyn Fn(&str),
) -> Result<Installed, RInstallError> {
    let target = install_dir(dirs, version);
    if target.exists() {
        return Err(RInstallError::AlreadyInstalled {
            version: version.clone(),
            path: target,
        });
    }
    let candidates = builds(platform, glibc_version(), version)?;
    let mut reasons = Vec::new();
    let mut chosen = None;
    for b in candidates {
        match http.head(&b.url, None)? {
            h if h.status == 200 => {
                chosen = Some(b);
                break;
            }
            h => reasons.push(format!("  • {}: {} (HTTP {})", b.kind, b.url, h.status)),
        }
    }
    let Some(build) = chosen else {
        if platform.os != Os::Linux {
            reasons.push(installer_advice(platform.os, version));
        } else if reasons.is_empty() {
            reasons
                .push("  • no Posit build fits this Linux distribution and C library".to_string());
        }
        return Err(RInstallError::Unavailable {
            version: version.clone(),
            reasons: reasons.join("\n"),
        });
    };

    install_build(http, dirs, &build, version, progress)
}

/// Installs one particular build of R `version` into [`install_dir`].
pub fn install_build(
    http: &Http,
    dirs: &UserDirs,
    build: &Build,
    version: &Version,
    progress: &dyn Fn(&str),
) -> Result<Installed, RInstallError> {
    let target = install_dir(dirs, version);
    if target.exists() {
        return Err(RInstallError::AlreadyInstalled {
            version: version.clone(),
            path: target,
        });
    }
    let root = dirs.r_installs();
    std::fs::create_dir_all(&root).map_err(io_err(&root))?;
    // Work next to the target, so the final rename stays on one file system.
    let work = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&root)
        .map_err(io_err(&root))?;
    let zip = build.url.ends_with(".zip");
    let archive = work.path().join(if zip { "R.zip" } else { "R.tar.gz" });
    progress(&format!("Downloading R {version} ({})", build.kind));
    http.download(&build.url, &archive)?;
    progress(&format!("Unpacking R {version}"));
    let unpack = work.path().join("unpack");
    let file = std::fs::File::open(&archive).map_err(io_err(&archive))?;
    if zip {
        rok_core::archive::unzip(std::io::BufReader::new(file), &unpack).map_err(|message| {
            RInstallError::Broken {
                version: version.clone(),
                message: format!("cannot unpack {} ({message})", build.url),
            }
        })?;
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(&unpack)
            .map_err(io_err(&unpack))?;
    }
    // Windows and macOS builds unpack to `R-<version>/` with R_HOME at the top; Linux builds
    // to `<version>/` with R_HOME at `lib/R`.
    let macos = build.url.contains("/macos/");
    let (top, exe) = if zip {
        (unpack.join(format!("R-{version}")), "bin/R.exe")
    } else if macos {
        (unpack.join(format!("R-{version}")), "bin/R")
    } else {
        (unpack.join(version.as_str()), "bin/R")
    };
    if !top.join(exe).is_file() {
        return Err(RInstallError::Broken {
            version: version.clone(),
            message: format!("{} has no {exe}", build.url),
        });
    }
    if let BuildKind::Distribution(_) = &build.kind {
        relocate(&top, &format!("/opt/R/{version}"), &target)?;
    }
    if zip || macos {
        let mark = top.join(rdetect::MANAGED_MARK);
        std::fs::write(&mark, format!("Installed by rok from {}\n", build.url))
            .map_err(io_err(&mark))?;
    }
    std::fs::rename(&top, &target).map_err(io_err(&target))?;

    let r_home = if zip || macos {
        target.clone()
    } else {
        target.join("lib").join("R")
    };
    match rdetect::r_home_version(&r_home) {
        Some(v) if v == *version => {}
        found => {
            let _ = std::fs::remove_dir_all(&target);
            return Err(RInstallError::Broken {
                version: version.clone(),
                message: format!("its base package reports version {found:?}"),
            });
        }
    }
    Ok(Installed {
        installation: RInstallation {
            version: version.clone(),
            executable: target.join(exe),
            r_home,
            kind: RKind::Managed,
        },
        build: build.kind.clone(),
    })
}

/// The text files of a distribution build with its install prefix compiled in (V1).
const PREFIX_FILES: [&str; 4] = [
    "bin/R",
    "lib/R/bin/R",
    "lib/pkgconfig/libR.pc",
    "lib/R/etc/Makeconf",
];

/// Makes a distribution build work at `to` instead of `from` (`/opt/R/<version>`): rewrites the
/// prefix in its scripts, and replaces `Rscript`, whose R_HOME is compiled in, with wrappers
/// that pass `RHOME`. The original binary must not go to `bin/exec/`, which R CMD INSTALL
/// takes for sub-architectures (V1).
pub fn relocate(dir: &Path, from: &str, to: &Path) -> Result<(), RInstallError> {
    let to_str = to.to_string_lossy();
    for f in PREFIX_FILES {
        let path = dir.join(f);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        std::fs::write(&path, text.replace(from, &to_str)).map_err(io_err(&path))?;
    }
    let bin = dir.join("lib/R/bin");
    let original = bin.join("Rscript.orig");
    std::fs::rename(bin.join("Rscript"), &original).map_err(io_err(&original))?;
    let wrapper = format!(
        "#!/bin/sh\n# rok: Rscript has its original R_HOME compiled in, so pass RHOME.\nRHOME=\"{to_str}/lib/R\" exec \"{to_str}/lib/R/bin/Rscript.orig\" \"$@\"\n"
    );
    for path in [dir.join("bin/Rscript"), bin.join("Rscript")] {
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, &wrapper).map_err(io_err(&path))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .map_err(io_err(&path))?;
        }
    }
    Ok(())
}

/// Removes an R installed by rok.
pub fn uninstall(dirs: &UserDirs, version: &Version) -> Result<PathBuf, RInstallError> {
    let target = install_dir(dirs, version);
    if !is_managed(dirs, version) {
        return Err(RInstallError::NotManaged {
            version: version.clone(),
        });
    }
    // Move it aside first, so a failure halfway leaves no half-removed R in the list.
    let trash = dirs
        .r_installs()
        .join(format!(".remove-{version}-{}", std::process::id()));
    std::fs::rename(&target, &trash).map_err(io_err(&target))?;
    std::fs::remove_dir_all(&trash).map_err(io_err(&trash))?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn parses_requests() {
        assert_eq!("latest".parse::<Request>().unwrap(), Request::Latest);
        assert_eq!(
            "4.5".parse::<Request>().unwrap(),
            Request::Minor("4.5".into())
        );
        assert_eq!(
            "4.5.1".parse::<Request>().unwrap(),
            Request::Exact(v("4.5.1"))
        );
        assert!("4".parse::<Request>().is_err());
        assert!("four".parse::<Request>().is_err());
        let releases = [v("4.6.1"), v("4.6.0"), v("4.5.3"), v("4.5.2")];
        assert_eq!(Request::Latest.pick(&releases), Some(v("4.6.1")));
        assert_eq!(
            Request::Minor("4.5".into()).pick(&releases),
            Some(v("4.5.3"))
        );
        assert_eq!(Request::Exact(v("4.5.2")).pick(&releases), Some(v("4.5.2")));
        assert_eq!(Request::Exact(v("4.5.9")).pick(&releases), None);
    }

    fn platform(os_release: &str, arch: Arch) -> Platform {
        Platform {
            os: Os::Linux,
            arch,
            distro: Some(LinuxDistro::parse_os_release(os_release)),
        }
    }

    #[test]
    fn chooses_builds() {
        let noble = platform("ID=ubuntu\nVERSION_ID=\"24.04\"\n", Arch::X86_64);
        let urls: Vec<String> = builds(&noble, Some((2, 39)), &v("4.6.1"))
            .unwrap()
            .into_iter()
            .map(|b| b.url)
            .collect();
        assert_eq!(
            urls,
            [
                "https://cdn.posit.co/r/manylinux_2_34/R-4.6.1-manylinux_2_34.tar.gz",
                "https://cdn.posit.co/r/ubuntu-2404/R-4.6.1-ubuntu-2404.tar.gz"
            ]
        );
        // Old glibc: only the distribution's build.
        let focal = platform("ID=ubuntu\nVERSION_ID=\"20.04\"\n", Arch::Aarch64);
        let b = builds(&focal, Some((2, 31)), &v("4.4.2")).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(
            b[0].url,
            "https://cdn.posit.co/r/ubuntu-2004/R-4.4.2-ubuntu-2004-arm64.tar.gz"
        );
        let mint = platform(
            "ID=linuxmint\nVERSION_ID=\"22\"\nUBUNTU_CODENAME=noble\n",
            Arch::X86_64,
        );
        assert_eq!(
            distribution_name(mint.distro.as_ref().unwrap()).as_deref(),
            Some("ubuntu-2404")
        );
        assert_eq!(parse_glibc("glibc 2.39\n"), Some((2, 39)));
        assert_eq!(parse_glibc("musl"), None);
    }

    #[test]
    fn chooses_windows_builds() {
        let windows = Platform {
            os: Os::Windows,
            arch: Arch::X86_64,
            distro: None,
        };
        let b = builds(&windows, None, &v("4.5.3")).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].kind, BuildKind::Portable);
        assert_eq!(
            b[0].url,
            "https://cdn.posit.co/r/windows/R-4.5.3-windows.zip"
        );
        let arm = Platform {
            arch: Arch::Aarch64,
            ..windows.clone()
        };
        assert!(builds(&arm, None, &v("4.5.3")).is_err());
        assert!(installer_advice(Os::Windows, &v("4.0.5")).contains("/base/old/4.0.5/"));
        let mac = Platform {
            os: Os::MacOs,
            arch: Arch::Aarch64,
            distro: None,
        };
        assert_eq!(
            builds(&mac, None, &v("4.5.3")).unwrap()[0].url,
            "https://cdn.posit.co/r/macos/R-4.5.3-macos-arm64.tar.gz"
        );
        let intel = Platform {
            arch: Arch::X86_64,
            ..mac.clone()
        };
        assert_eq!(
            builds(&intel, None, &v("4.5.3")).unwrap()[0].url,
            "https://cdn.posit.co/r/macos/R-4.5.3-macos.tar.gz"
        );
        assert!(!is_built_for(&mac, &v("4.0.5")) && is_built_for(&mac, &v("4.1.0")));
        for (version, built) in [
            ("4.6.1", true),
            ("4.1.0", true),
            ("4.0.5", false),
            ("3.6.3", true),
            ("3.6.2", false),
            ("5.0.0", true),
        ] {
            assert_eq!(is_built_for(&windows, &v(version)), built, "{version}");
        }
    }

    #[test]
    fn shows_system_install_commands() {
        let noble = platform(
            "ID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"24.04\"\n",
            Arch::X86_64,
        );
        assert_eq!(
            system_install_commands(&noble, &v("4.6.1")).unwrap(),
            [
                "curl -fLO https://cdn.posit.co/r/ubuntu-2404/pkgs/r-4.6.1_1_amd64.deb",
                "sudo apt-get install -y ./r-4.6.1_1_amd64.deb"
            ]
        );
        let rhel = platform(
            "ID=rocky\nID_LIKE=\"rhel centos fedora\"\nVERSION_ID=\"9.4\"\n",
            Arch::X86_64,
        );
        assert_eq!(
            system_install_commands(&rhel, &v("4.6.1")).unwrap(),
            ["sudo dnf install -y https://cdn.posit.co/r/rhel-9/pkgs/R-4.6.1-1-1.x86_64.rpm"]
        );
    }

    /// Posit's portable build for this machine, installed for real into a temporary place:
    /// it must start and report its version (V1b, V5, V6).
    #[test]
    #[ignore = "downloads R (about 100 MB)"]
    fn installs_the_portable_build() {
        let platform = Platform::detect();
        let t = tempfile::tempdir().unwrap();
        let dirs = UserDirs::under(t.path());
        let version = v("4.5.3");
        let installed = install(&Http::new(), &dirs, &platform, &version, &|_| {}).unwrap();
        assert_eq!(installed.build, BuildKind::Portable);
        let r = installed.installation;
        assert!(is_managed(&dirs, &version));
        let found = rdetect::find_installations(&dirs, None);
        assert!(
            found
                .iter()
                .any(|i| i.version == version && i.kind == RKind::Managed),
            "{found:?}"
        );
        let out = Command::new(r.rscript())
            .args(["-e", "cat(as.character(getRversion()))"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "4.5.3");
        uninstall(&dirs, &version).unwrap();
        assert!(!install_dir(&dirs, &version).exists());
    }

    /// The fallback for old glibc, tried for real: the distribution build, relocated.
    #[test]
    #[ignore = "downloads R (about 65 MB)"]
    fn installs_a_relocated_distribution_build() {
        let platform = Platform::detect();
        let Some(name) = platform.distro.as_ref().and_then(distribution_name) else {
            return;
        };
        let t = tempfile::tempdir().unwrap();
        let dirs = UserDirs::under(t.path());
        let version = v("4.4.2");
        let build = builds(&platform, None, &version).unwrap().remove(0);
        assert_eq!(build.kind, BuildKind::Distribution(name));
        let installed =
            install_build(&Http::new(), &dirs, &build, &version, &|m| eprintln!("{m}")).unwrap();
        let rscript = installed.installation.rscript();
        let out = Command::new(&rscript)
            .args(["-e", "cat(R.home(), R.version.string, sep = '\\n')"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(text.contains("R version 4.4.2"), "{text}");
        assert!(
            text.starts_with(&*installed.installation.r_home.to_string_lossy()),
            "{text}"
        );
    }

    #[test]
    fn relocates_a_distribution_build() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("4.4.2");
        for f in PREFIX_FILES {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "R_HOME_DIR=/opt/R/4.4.2/lib/R\n").unwrap();
        }
        std::fs::write(dir.join("lib/R/bin/Rscript"), b"\x7fELF").unwrap();
        std::fs::write(dir.join("bin/Rscript"), b"\x7fELF").unwrap();
        let to = Path::new("/home/me/.local/share/R/rok/r/4.4.2");
        relocate(&dir, "/opt/R/4.4.2", to).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("bin/R")).unwrap(),
            "R_HOME_DIR=/home/me/.local/share/R/rok/r/4.4.2/lib/R\n"
        );
        let wrapper = std::fs::read_to_string(dir.join("bin/Rscript")).unwrap();
        assert!(wrapper.contains(
            "RHOME=\"/home/me/.local/share/R/rok/r/4.4.2/lib/R\" exec \"/home/me/.local/share/R/rok/r/4.4.2/lib/R/bin/Rscript.orig\""
        ));
        assert_eq!(
            std::fs::read(dir.join("lib/R/bin/Rscript.orig")).unwrap(),
            b"\x7fELF"
        );
    }
}
