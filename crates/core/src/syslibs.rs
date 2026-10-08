//! System libraries on Linux (requirements chapter 7, V3b): which shared libraries an ELF file
//! needs (`DT_NEEDED`), whether the system has them, and which apt packages provide the
//! missing ones.
//!
//! Only link-time dependencies are visible this way; libraries loaded with `dlopen` and
//! external commands (such as `gdal-bin`) are not. Nothing here uses the network or runs the
//! files that are inspected.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::platform::Arch;

/// The shared libraries an ELF file needs, and where it asks the loader to look.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElfDeps {
    pub needed: Vec<String>,
    /// `DT_RUNPATH` or `DT_RPATH` entries, with `$ORIGIN` still in them.
    pub search: Vec<String>,
}

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const DT_NEEDED: u64 = 1;
const DT_STRTAB: u64 = 5;
const DT_STRSZ: u64 = 10;
const DT_RPATH: u64 = 15;
const DT_RUNPATH: u64 = 29;

fn read_at(f: &mut std::fs::File, offset: u64, len: usize) -> Option<Vec<u8>> {
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Reads the dynamic section of a 64-bit little-endian ELF file (x86_64 and aarch64 Linux).
/// `None` if the file is not such an ELF file or has no dynamic section.
pub fn elf_deps(path: &Path) -> Option<ElfDeps> {
    let mut f = std::fs::File::open(path).ok()?;
    let header = read_at(&mut f, 0, 64)?;
    if header[..4] != *b"\x7fELF" || header[4] != 2 || header[5] != 1 {
        return None;
    }
    let (phoff, phentsize, phnum) = (
        u64_at(&header, 0x20),
        u16_at(&header, 0x36) as usize,
        u16_at(&header, 0x38) as usize,
    );
    if phentsize < 56 || phnum > 4096 {
        return None;
    }
    let table = read_at(&mut f, phoff, phentsize * phnum)?;
    let mut loads = Vec::new();
    let mut dynamic = None;
    for i in 0..phnum {
        let ph = &table[i * phentsize..i * phentsize + 56];
        let (kind, offset, vaddr, filesz) =
            (u32_at(ph, 0), u64_at(ph, 8), u64_at(ph, 16), u64_at(ph, 32));
        match kind {
            PT_LOAD => loads.push((vaddr, offset, filesz)),
            PT_DYNAMIC => dynamic = Some((offset, filesz)),
            _ => {}
        }
    }
    let (offset, size) = dynamic?;
    let entries = read_at(&mut f, offset, usize::try_from(size.min(1 << 20)).ok()?)?;
    let (mut strtab, mut strsz) = (None, 0u64);
    let (mut needed, mut search) = (Vec::new(), Vec::new());
    for e in entries.as_chunks::<16>().0 {
        let (tag, val) = (u64_at(e, 0), u64_at(e, 8));
        match tag {
            0 => break,
            DT_NEEDED => needed.push(val),
            DT_STRTAB => strtab = Some(val),
            DT_STRSZ => strsz = val,
            DT_RPATH | DT_RUNPATH => search.push(val),
            _ => {}
        }
    }
    // The string table is given as a virtual address; find it in the file.
    let strtab = strtab?;
    let file_offset = loads
        .iter()
        .find(|(vaddr, _, filesz)| (*vaddr..vaddr + filesz).contains(&strtab))
        .map(|(vaddr, offset, _)| strtab - vaddr + offset)?;
    let strings = read_at(
        &mut f,
        file_offset,
        usize::try_from(strsz.min(1 << 22)).ok()?,
    )?;
    let string = |at: u64| -> Option<String> {
        let rest = strings.get(usize::try_from(at).ok()?..)?;
        let end = rest.iter().position(|b| *b == 0)?;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    };
    Some(ElfDeps {
        needed: needed.into_iter().filter_map(string).collect(),
        search: search
            .into_iter()
            .filter_map(string)
            .flat_map(|s| s.split(':').map(str::to_string).collect::<Vec<_>>())
            .filter(|s| !s.is_empty())
            .collect(),
    })
}

/// The libraries the dynamic loader can find without extra search paths: the entries of
/// `ld.so.cache` for this architecture (from `ldconfig -p`) and the default directories.
#[derive(Debug, Clone, Default)]
pub struct SystemLibraries {
    cached: HashSet<String>,
    dirs: Vec<PathBuf>,
}

impl SystemLibraries {
    /// Reads the loader cache. `None` where it cannot be read (no `ldconfig`, as on musl).
    pub fn detect(arch: Arch) -> Option<SystemLibraries> {
        let ldconfig = ["/sbin/ldconfig", "/usr/sbin/ldconfig", "ldconfig"]
            .into_iter()
            .find_map(|exe| {
                Command::new(exe)
                    .arg("-p")
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
            })?;
        let (tag, triplet) = match arch {
            Arch::X86_64 => ("x86-64", "x86_64-linux-gnu"),
            Arch::Aarch64 => ("AArch64", "aarch64-linux-gnu"),
        };
        Some(SystemLibraries {
            cached: parse_ldconfig(&String::from_utf8_lossy(&ldconfig.stdout), tag),
            dirs: [
                format!("/lib/{triplet}"),
                format!("/usr/lib/{triplet}"),
                "/lib64".to_string(),
                "/usr/lib64".to_string(),
                "/lib".to_string(),
                "/usr/lib".to_string(),
            ]
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        })
    }

    fn has(&self, soname: &str) -> bool {
        self.cached.contains(soname) || self.dirs.iter().any(|d| d.join(soname).exists())
    }

    /// The libraries that the ELF files in `files` need but that cannot be found, sorted.
    /// `extra` are directories searched as well (R's `lib`, for `libR.so`).
    pub fn missing(&self, files: &[PathBuf], extra: &[PathBuf]) -> Vec<String> {
        let mut out = BTreeSet::new();
        for file in files {
            let Some(deps) = elf_deps(file) else { continue };
            let origin = file.parent().unwrap_or(Path::new("/"));
            let search: Vec<PathBuf> = deps
                .search
                .iter()
                .map(|s| {
                    PathBuf::from(
                        s.replace("${ORIGIN}", &origin.to_string_lossy())
                            .replace("$ORIGIN", &origin.to_string_lossy()),
                    )
                })
                .chain(extra.iter().cloned())
                .collect();
            for lib in deps.needed {
                let found = self.has(&lib) || search.iter().any(|d| d.join(&lib).exists());
                if !found {
                    out.insert(lib);
                }
            }
        }
        out.into_iter().collect()
    }
}

/// The sonames in `ldconfig -p` output whose entry is for the architecture `tag`
/// (`x86-64`, `AArch64`).
fn parse_ldconfig(text: &str, tag: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.trim().split_once(' ')?;
            let flags = rest.trim().strip_prefix('(')?.split_once(')')?.0;
            flags
                .split(',')
                .any(|f| f.trim() == tag)
                .then(|| name.to_string())
        })
        .collect()
}

/// The shared objects of an installed R package (`libs/*.so`, including per-architecture
/// subdirectories).
pub fn package_objects(package_dir: &Path) -> Vec<PathBuf> {
    shared_objects(&package_dir.join("libs"), 1)
}

/// The ELF files of an R installation that must load for R to start and work: the R
/// executable, its libraries and its modules.
pub fn r_objects(r_home: &Path) -> Vec<PathBuf> {
    let mut files = vec![r_home.join("bin/exec/R")];
    files.extend(shared_objects(&r_home.join("lib"), 0));
    files.extend(shared_objects(&r_home.join("modules"), 0));
    files
}

fn shared_objects(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() && depth > 0 {
            out.extend(shared_objects(&path, depth - 1));
        } else if path.extension().is_some_and(|e| e == "so") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// The runtime packages among the dependencies of `dev_packages` (from `apt-cache depends`):
/// names starting with `lib` and not ending in `-dev`. `None` when apt cannot tell, usually
/// because the package lists are absent (minimal container images).
pub fn apt_runtime_candidates(dev_packages: &[String]) -> Option<Vec<String>> {
    if dev_packages.is_empty() {
        return Some(Vec::new());
    }
    // One package at a time: apt-cache fails as a whole if any name is unknown.
    let mut found = false;
    let mut out = BTreeSet::new();
    for dev in dev_packages {
        if let Some(o) = Command::new("apt-cache")
            .args(["depends", dev])
            .output()
            .ok()
            .filter(|o| o.status.success())
        {
            found = true;
            out.extend(parse_apt_depends(&String::from_utf8_lossy(&o.stdout)));
        }
    }
    found.then(|| out.into_iter().collect())
}

fn parse_apt_depends(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let Some(dep) = line
            .trim_start()
            .trim_start_matches('|')
            .strip_prefix("Depends:")
        else {
            continue;
        };
        let name = dep.split_whitespace().next().unwrap_or("");
        if name.starts_with("lib") && !name.ends_with("-dev") && !name.starts_with('<') {
            out.insert(name.trim_end_matches(":any").to_string());
        }
    }
    out.into_iter().collect()
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The runtime package that provides `soname`, among `candidates`: `libgdal.so.34` matches
/// `libgdal34t64` (`gdal34` is in `gdal34t64`). The closest name wins, and cross-compilation,
/// debug and development packages are never chosen.
pub fn package_for_soname<'a>(soname: &str, candidates: &'a [String]) -> Option<&'a String> {
    let stem = soname
        .strip_prefix("lib")
        .unwrap_or(soname)
        .replacen(".so", "", 1);
    let key = normalize(&stem);
    candidates
        .iter()
        .filter(|c| {
            !["-dev", "-dbg", "-cross", "-doc"]
                .iter()
                .any(|s| c.contains(s))
        })
        .filter_map(|c| {
            let n = normalize(c.strip_prefix("lib").unwrap_or(c));
            n.contains(&key).then(|| (n.len() - key.len(), c))
        })
        .min()
        .map(|(_, c)| c)
}

/// Libraries of the compiler and R's toolchain that P3M lists no system requirements for, with
/// the packages that provide them on Debian and Ubuntu. Used when apt cannot be asked.
const TOOLCHAIN: [(&str, &str); 6] = [
    ("libgomp.so.1", "libgomp1"),
    ("libgfortran.so.5", "libgfortran5"),
    ("libquadmath.so.0", "libquadmath0"),
    ("libblas.so.3", "libblas3"),
    ("liblapack.so.3", "liblapack3"),
    ("libstdc++.so.6", "libstdc++6"),
];

/// Whether apt's package lists are present (they are removed in most container images).
pub fn apt_lists_present() -> bool {
    std::fs::read_dir("/var/lib/apt/lists")
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains("_Packages"))
}

/// Packages whose names start like the library (`libgomp` for `libgomp.so.1`), from apt.
fn apt_packages_named_like(soname: &str) -> Vec<String> {
    let prefix = soname.split(".so").next().unwrap_or(soname);
    Command::new("apt-cache")
        .args(["pkgnames", prefix])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// What to install for missing libraries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AptAdvice {
    /// Packages to install.
    pub packages: Vec<String>,
    /// Whether these are the `-dev` packages from P3M's system requirements (more than needed),
    /// because apt could not narrow them down.
    pub broad: bool,
    /// Whether apt's package lists are missing, so `apt-get update` must run first.
    pub needs_update: bool,
    /// Missing libraries no package could be found for.
    pub unknown: Vec<String>,
}

impl AptAdvice {
    /// The command to show (rok never runs it).
    pub fn command(&self) -> Option<String> {
        if self.packages.is_empty() {
            return None;
        }
        let install = format!("sudo apt-get install -y {}", self.packages.join(" "));
        Some(if self.needs_update {
            format!("sudo apt-get update && {install}")
        } else {
            install
        })
    }
}

/// Advice for the missing libraries of R packages. `missing` maps a package to its missing
/// sonames; `sysreqs` maps it to P3M's `-dev` packages for this distribution.
///
/// With apt's package lists, each library is matched to the runtime package that provides it
/// (among the dependencies of the `-dev` packages, then among packages named like it). Without
/// them, known toolchain libraries are named directly, and otherwise the `-dev` packages are
/// suggested, which is more than needed (V3b).
pub fn apt_advice(
    missing: &BTreeMap<String, Vec<String>>,
    sysreqs: &BTreeMap<String, Vec<String>>,
) -> AptAdvice {
    let lists = apt_lists_present();
    let mut advice = AptAdvice::default();
    let mut packages = BTreeSet::new();
    for (name, sonames) in missing {
        let dev = sysreqs.get(name).cloned().unwrap_or_default();
        let candidates = if lists {
            apt_runtime_candidates(&dev).unwrap_or_default()
        } else {
            Vec::new()
        };
        for so in sonames {
            let exact = package_for_soname(so, &candidates).cloned().or_else(|| {
                lists
                    .then(|| package_for_soname(so, &apt_packages_named_like(so)).cloned())
                    .flatten()
            });
            let known = || {
                TOOLCHAIN
                    .iter()
                    .find(|(lib, _)| lib == so)
                    .map(|(_, p)| p.to_string())
            };
            match exact.or_else(known) {
                Some(p) => {
                    packages.insert(p);
                }
                None if !dev.is_empty() => {
                    packages.extend(dev.iter().cloned());
                    advice.broad = true;
                }
                None => advice.unknown.push(so.clone()),
            }
        }
    }
    advice.packages = packages.into_iter().collect();
    advice.needs_update = !lists && !advice.packages.is_empty();
    advice.unknown.sort();
    advice.unknown.dedup();
    advice
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_needed_libraries_of_an_elf_file() {
        // Any dynamically linked executable will do; `ls` exists on every Linux system.
        let Some(deps) = ["/bin/ls", "/usr/bin/ls"]
            .iter()
            .find_map(|p| elf_deps(Path::new(p)))
        else {
            return;
        };
        assert!(
            deps.needed.iter().any(|n| n.starts_with("libc.so")),
            "{deps:?}"
        );
        assert_eq!(elf_deps(Path::new("/etc/hostname")), None);
    }

    #[test]
    fn parses_ldconfig_output() {
        let text = "123 libs found in cache `/etc/ld.so.cache'\n\
            \tlibz.so.1 (libc6,x86-64) => /lib/x86_64-linux-gnu/libz.so.1\n\
            \tlibz.so.1 (libc6) => /lib/i386-linux-gnu/libz.so.1\n\
            \tlibgdal.so.34 (libc6,AArch64) => /lib/aarch64-linux-gnu/libgdal.so.34\n";
        let x86 = parse_ldconfig(text, "x86-64");
        assert!(x86.contains("libz.so.1") && !x86.contains("libgdal.so.34"));
        assert!(parse_ldconfig(text, "AArch64").contains("libgdal.so.34"));
    }

    #[test]
    fn maps_sonames_to_runtime_packages() {
        let depends = "libgdal-dev\n  Depends: libgdal34t64 (= 3.8.4)\n  Depends: libarmadillo-dev\n |Depends: libgeos-c1t64\n  Depends: <libfoo>\n";
        let candidates = parse_apt_depends(depends);
        assert_eq!(candidates, ["libgdal34t64", "libgeos-c1t64"]);
        let find = |so| package_for_soname(so, &candidates).map(String::as_str);
        assert_eq!(find("libgdal.so.34"), Some("libgdal34t64"));
        assert_eq!(find("libgeos_c.so.1"), Some("libgeos-c1t64"));
        assert_eq!(find("libproj.so.25"), None);
        let gomp: Vec<String> = ["libgomp1-alpha-cross", "libgomp1", "libgomp-plugin-nvptx1"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            package_for_soname("libgomp.so.1", &gomp).map(String::as_str),
            Some("libgomp1")
        );
        let units = ["libudunits2-0".to_string()];
        assert_eq!(
            package_for_soname("libudunits2.so.0", &units).map(String::as_str),
            Some("libudunits2-0")
        );
    }

    #[test]
    fn builds_the_apt_command() {
        let advice = AptAdvice {
            packages: vec!["libgdal-dev".into()],
            broad: true,
            needs_update: true,
            unknown: Vec::new(),
        };
        assert_eq!(
            advice.command().unwrap(),
            "sudo apt-get update && sudo apt-get install -y libgdal-dev"
        );
        assert_eq!(AptAdvice::default().command(), None);
    }
}
