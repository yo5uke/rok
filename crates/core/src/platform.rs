//! The machine rok runs on: operating system, CPU architecture and Linux distribution, and how
//! P3M and R name them.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    MacOs,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn current() -> Arch {
        if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            Arch::X86_64
        }
    }

    /// The name R uses (`R.version$arch`).
    pub fn r_name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

/// The fields of `/etc/os-release` that rok uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxDistro {
    /// `ID`, for example `ubuntu`.
    pub id: String,
    /// `VERSION_ID`, for example `24.04`.
    pub version_id: String,
    /// `ID_LIKE`, for example `["ubuntu", "debian"]`.
    pub id_like: Vec<String>,
    /// `UBUNTU_CODENAME`, set by Ubuntu and its derivatives (Linux Mint, Pop!_OS).
    pub ubuntu_codename: Option<String>,
    /// `PRETTY_NAME`, for messages.
    pub pretty_name: String,
}

impl LinuxDistro {
    /// Parses the contents of `/etc/os-release`.
    pub fn parse_os_release(text: &str) -> LinuxDistro {
        let mut d = LinuxDistro::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string();
            match key.trim() {
                "ID" => d.id = value.to_ascii_lowercase(),
                "VERSION_ID" => d.version_id = value,
                "ID_LIKE" => d.id_like = value.split_whitespace().map(str::to_string).collect(),
                "UBUNTU_CODENAME" if !value.is_empty() => d.ubuntu_codename = Some(value),
                "PRETTY_NAME" => d.pretty_name = value,
                _ => {}
            }
        }
        d
    }

    /// Reads `/etc/os-release` (or `/usr/lib/os-release`).
    pub fn detect() -> Option<LinuxDistro> {
        ["/etc/os-release", "/usr/lib/os-release"]
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
            .map(|t| LinuxDistro::parse_os_release(&t))
    }

    /// The distribution's name in P3M's Linux binary URLs (`__linux__/<name>/`), if P3M
    /// builds binaries for it. The table follows P3M's `/__api__/status` (checked in V3).
    pub fn p3m_name(&self) -> Option<&'static str> {
        let major = self.version_id.split('.').next().unwrap_or("");
        let by_id = match (self.id.as_str(), self.version_id.as_str()) {
            ("ubuntu", "22.04") => Some("jammy"),
            ("ubuntu", "24.04") => Some("noble"),
            ("ubuntu", "26.04") => Some("resolute"),
            ("debian", "12") => Some("bookworm"),
            ("debian", "13") => Some("trixie"),
            ("rhel" | "rocky" | "almalinux" | "centos", _) => match major {
                "8" => Some("centos8"),
                "9" => Some("rhel9"),
                "10" => Some("rhel10"),
                _ => None,
            },
            ("opensuse-leap" | "sles", "15.6") => Some("opensuse156"),
            _ => None,
        };
        // Ubuntu derivatives report the Ubuntu release they are based on.
        by_id.or(match self.ubuntu_codename.as_deref() {
            Some("jammy") => Some("jammy"),
            Some("noble") => Some("noble"),
            Some("resolute") => Some("resolute"),
            _ => None,
        })
    }
}

/// The platform: OS, architecture and, on Linux, the distribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
    pub distro: Option<LinuxDistro>,
}

impl Platform {
    pub fn detect() -> Platform {
        let os = Os::current();
        Platform {
            os,
            arch: Arch::current(),
            distro: if os == Os::Linux {
                LinuxDistro::detect()
            } else {
                None
            },
        }
    }

    /// The P3M Linux distribution name, if binaries are available for this machine.
    pub fn p3m_linux_name(&self) -> Option<&'static str> {
        self.distro.as_ref().and_then(LinuxDistro::p3m_name)
    }

    /// The name of the platform's directory in a project library, for example
    /// `ubuntu-24.04-x86_64`. It uses `ID` and `VERSION_ID` from `/etc/os-release` as they are,
    /// so the R side (`activate.R`) can compute the same name without a lookup table.
    pub fn library_tag(&self) -> String {
        let arch = self.arch.r_name();
        match (self.os, &self.distro) {
            (Os::Linux, Some(d)) if !d.id.is_empty() && !d.version_id.is_empty() => {
                format!("{}-{}-{arch}", d.id, d.version_id)
            }
            (Os::Linux, Some(d)) if !d.id.is_empty() => format!("{}-{arch}", d.id),
            (Os::Linux, _) => format!("linux-{arch}"),
            (Os::Windows, _) => format!("windows-{arch}"),
            (Os::MacOs, _) => format!("macos-{arch}"),
        }
    }

    /// `R.version$platform`, for example `x86_64-pc-linux-gnu`.
    pub fn r_platform(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Linux, Arch::X86_64) => "x86_64-pc-linux-gnu",
            (Os::Linux, Arch::Aarch64) => "aarch64-unknown-linux-gnu",
            (Os::Windows, _) => "x86_64-w64-mingw32",
            (Os::MacOs, Arch::X86_64) => "x86_64-apple-darwin20",
            (Os::MacOs, Arch::Aarch64) => "aarch64-apple-darwin20",
        }
    }

    /// `R.version$os`, for example `linux-gnu`.
    fn r_os(&self) -> &'static str {
        match self.os {
            Os::Linux => "linux-gnu",
            Os::Windows => "mingw32",
            Os::MacOs => "darwin20",
        }
    }

    /// The User-Agent R sends, which P3M uses to choose a Linux binary for that R version:
    /// `R (4.4.2 x86_64-pc-linux-gnu x86_64 linux-gnu)`. P3M needs the full patch version (V3).
    pub fn r_user_agent(&self, r_version: &crate::version::Version) -> String {
        format!(
            "R ({} {} {} {})",
            r_version,
            self.r_platform(),
            self.arch.r_name(),
            self.r_os()
        )
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.os, &self.distro) {
            (Os::Linux, Some(d)) if !d.pretty_name.is_empty() => {
                write!(f, "{} ({})", d.pretty_name, self.arch.r_name())
            }
            (Os::Linux, _) => write!(f, "Linux ({})", self.arch.r_name()),
            (Os::Windows, _) => write!(f, "Windows ({})", self.arch.r_name()),
            (Os::MacOs, _) => write!(f, "macOS ({})", self.arch.r_name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = "PRETTY_NAME=\"Ubuntu 24.04.5 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\nUBUNTU_CODENAME=noble\n";

    fn distro(id: &str, version: &str) -> LinuxDistro {
        LinuxDistro {
            id: id.into(),
            version_id: version.into(),
            ..Default::default()
        }
    }

    #[test]
    fn parses_os_release() {
        let d = LinuxDistro::parse_os_release(UBUNTU);
        assert_eq!(d.id, "ubuntu");
        assert_eq!(d.version_id, "24.04");
        assert_eq!(d.id_like, ["debian"]);
        assert_eq!(d.ubuntu_codename.as_deref(), Some("noble"));
        assert_eq!(d.pretty_name, "Ubuntu 24.04.5 LTS");
        assert_eq!(d.p3m_name(), Some("noble"));
    }

    #[test]
    fn maps_distributions_to_p3m_names() {
        assert_eq!(distro("ubuntu", "22.04").p3m_name(), Some("jammy"));
        assert_eq!(distro("debian", "13").p3m_name(), Some("trixie"));
        assert_eq!(distro("rocky", "9.4").p3m_name(), Some("rhel9"));
        assert_eq!(distro("rhel", "8.10").p3m_name(), Some("centos8"));
        assert_eq!(distro("sles", "15.6").p3m_name(), Some("opensuse156"));
        assert_eq!(distro("ubuntu", "20.04").p3m_name(), None);
        assert_eq!(distro("arch", "").p3m_name(), None);
        let mint = LinuxDistro::parse_os_release(
            "ID=linuxmint\nVERSION_ID=\"22\"\nUBUNTU_CODENAME=noble\n",
        );
        assert_eq!(mint.p3m_name(), Some("noble"));
    }

    #[test]
    fn builds_r_user_agent() {
        let p = Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
            distro: Some(LinuxDistro::parse_os_release(UBUNTU)),
        };
        let v = "4.4.2".parse().unwrap();
        assert_eq!(
            p.r_user_agent(&v),
            "R (4.4.2 x86_64-pc-linux-gnu x86_64 linux-gnu)"
        );
        let arm = Platform {
            arch: Arch::Aarch64,
            ..p.clone()
        };
        assert_eq!(
            arm.r_user_agent(&v),
            "R (4.4.2 aarch64-unknown-linux-gnu aarch64 linux-gnu)"
        );
        assert_eq!(p.to_string(), "Ubuntu 24.04.5 LTS (x86_64)");
        assert_eq!(p.library_tag(), "ubuntu-24.04-x86_64");
        assert_eq!(
            Platform {
                distro: None,
                ..p.clone()
            }
            .library_tag(),
            "linux-x86_64"
        );
    }
}
