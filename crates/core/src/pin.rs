//! Changing the project's R version (`rok r pin`, requirements chapter 6): whether P3M has
//! binaries of the locked packages for the new R, and the candidates when some are missing.
//!
//! Whether P3M has a binary depends on the package version, the R minor version and the
//! distribution (or Windows), not on the snapshot date (V4). Answers are therefore cached forever, and a
//! search asks P3M only about versions it has not seen. One operation sends at most
//! [`MAX_REQUESTS`] requests, 16 at a time.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::lockfile::Lockfile;
use crate::ops::Env;
use crate::par;
use crate::platform::{Arch, Os};
use crate::resolve::release_snapshots;
use crate::version::Version;

/// The most requests one operation sends to P3M.
pub const MAX_REQUESTS: usize = 2000;
/// How far the date search goes from the project's snapshot, in months each way.
const SEARCH_MONTHS: i64 = 24;

#[derive(Debug, thiserror::Error)]
pub enum PinError {
    #[error(
        "stopped after {MAX_REQUESTS} requests to P3M; the search found no snapshot date where every package has a binary"
    )]
    Budget,
    #[error("{0}")]
    P3m(String),
}

/// A package version and the snapshot date it is taken from.
pub type Item = (String, Version, String);

/// Answers whether P3M has a binary of a package version for one R minor version on this
/// machine's distribution, or on Windows.
pub struct BinaryChecker<'a> {
    env: &'a Env,
    /// P3M's name of the Linux distribution; `None` on Windows.
    distro: Option<&'static str>,
    r_minor: String,
    user_agent: String,
    path: PathBuf,
    known: Mutex<BTreeMap<String, bool>>,
    sent: AtomicUsize,
}

impl<'a> BinaryChecker<'a> {
    /// `None` where P3M builds no binaries for this machine.
    pub fn new(env: &'a Env, r_version: &Version) -> Option<BinaryChecker<'a>> {
        let distro = match (env.platform.os, env.platform.arch) {
            (Os::Windows, Arch::X86_64) => None,
            _ => Some(env.platform.p3m_linux_name()?),
        };
        let path = env.p3m.cache_dir().join("binaries").join(format!(
            "{}-{}-R{}.json",
            distro.unwrap_or("windows"),
            env.platform.arch.r_name(),
            r_version.minor()
        ));
        let known = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Some(BinaryChecker {
            env,
            distro,
            r_minor: r_version.minor(),
            user_agent: env.platform.r_user_agent(r_version),
            path,
            known: Mutex::new(known),
            sent: AtomicUsize::new(0),
        })
    }

    /// Requests sent so far.
    pub fn requests(&self) -> usize {
        self.sent.load(Ordering::Relaxed)
    }

    /// Whether each item has a binary. Unknown versions are asked about in parallel.
    pub fn check(&self, items: &[Item]) -> Result<Vec<bool>, PinError> {
        let key = |(n, v, _): &Item| format!("{n} {v}");
        let unknown: Vec<&Item> = {
            let known = self.known.lock().unwrap_or_else(|e| e.into_inner());
            let mut seen = HashSet::new();
            items
                .iter()
                .filter(|i| !known.contains_key(&key(i)) && seen.insert(key(i)))
                .collect()
        };
        if self.requests() + unknown.len() > MAX_REQUESTS {
            return Err(PinError::Budget);
        }
        let answers = par::map(&unknown, self.env.jobs, |(n, v, date)| {
            self.sent.fetch_add(1, Ordering::Relaxed);
            // On Linux P3M picks the binary for the R in the User-Agent; Windows binaries have
            // their own URLs (404 when there is none).
            let (url, user_agent) = match self.distro {
                Some(d) => (
                    self.env.p3m.linux_package_url(d, date, n, v),
                    Some(self.user_agent.as_str()),
                ),
                None => (
                    self.env.p3m.windows_package_url(date, &self.r_minor, n, v),
                    None,
                ),
            };
            self.env
                .http
                .get_headers(&url, user_agent)
                .map(|h| match h.status {
                    200..=399 => Some(h.header("x-package-type") == Some("binary")),
                    // Not in that snapshot: no answer to remember.
                    _ => None,
                })
                .map_err(|e| PinError::P3m(e.to_string()))
        });
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        let mut fresh = HashMap::new();
        for (item, answer) in unknown.iter().zip(answers) {
            match answer? {
                Some(binary) => {
                    known.insert(key(item), binary);
                }
                None => {
                    fresh.insert(key(item), false);
                }
            }
        }
        let out = items
            .iter()
            .map(|i| {
                let k = key(i);
                known.get(&k).or(fresh.get(&k)).copied().unwrap_or(false)
            })
            .collect();
        if let Ok(bytes) = serde_json::to_vec(&*known) {
            let _ = crate::fsutil::write_atomic(&self.path, &bytes);
        }
        Ok(out)
    }
}

/// The CRAN packages of a lockfile, with their versions and snapshot dates.
pub fn cran_items(lock: &Lockfile) -> Vec<Item> {
    lock.packages
        .iter()
        .filter_map(|p| {
            Some((
                p.name.clone(),
                p.version.clone(),
                p.source.snapshot()?.to_string(),
            ))
        })
        .collect()
}

/// The packages of `items` without a binary.
pub fn missing_binaries(checker: &BinaryChecker, items: &[Item]) -> Result<Vec<String>, PinError> {
    let binary = checker.check(items)?;
    Ok(items
        .iter()
        .zip(binary)
        .filter(|(_, b)| !b)
        .map(|((n, _, _), _)| n.clone())
        .collect())
}

/// The release history of each package, as (version, snapshot date to take it from), for
/// the snapshot dates up to `until`.
fn releases(
    env: &Env,
    names: &[String],
    dates: &[String],
    until: &str,
) -> Result<HashMap<String, Vec<(Version, String)>>, PinError> {
    let histories = par::map(names, env.jobs, |n| env.p3m.history(n));
    names
        .iter()
        .zip(histories)
        .map(|(n, h)| {
            let h = h.map_err(|e| PinError::P3m(e.to_string()))?;
            Ok((n.clone(), release_snapshots(&h, dates, until)))
        })
        .collect()
}

/// For each package without a binary, the version nearest to the locked one (in release
/// order) that has a binary, with the snapshot date to take it from. `allowed` filters versions
/// (declared constraints). Packages for which none is found are left out.
pub fn nearest_versions(
    env: &Env,
    checker: &BinaryChecker,
    items: &[Item],
    missing: &[String],
    dates: &[String],
    allowed: &dyn Fn(&str, &Version) -> bool,
    prefer_newer: bool,
) -> Result<BTreeMap<String, (Version, String)>, PinError> {
    let latest = dates.last().cloned().unwrap_or_default();
    let all = releases(env, missing, dates, &latest)?;
    let mut out = BTreeMap::new();
    for name in missing {
        let Some((_, locked, _)) = items.iter().find(|(n, _, _)| n == name) else {
            continue;
        };
        let rel = &all[name];
        // Releases ordered by distance from the locked one; ties go the way R moves.
        let at = rel
            .iter()
            .position(|(v, _)| v >= locked)
            .unwrap_or(rel.len());
        let exact = rel.get(at).is_some_and(|(v, _)| v == locked);
        let mut order: Vec<(usize, usize)> = rel
            .iter()
            .enumerate()
            .filter(|(_, (v, _))| v != locked && allowed(name, v))
            .map(|(i, _)| {
                let distance = match (exact, i >= at) {
                    (true, _) => i.abs_diff(at),
                    (false, true) => i - at + 1,
                    (false, false) => at - i,
                };
                let wrong_way = (i >= at) != prefer_newer;
                (distance * 2 + usize::from(wrong_way), i)
            })
            .collect();
        order.sort();
        for chunk in order.chunks(8) {
            let batch: Vec<Item> = chunk
                .iter()
                .map(|(_, i)| (name.clone(), rel[*i].0.clone(), rel[*i].1.clone()))
                .collect();
            let binary = checker.check(&batch)?;
            if let Some(((_, v, d), _)) = batch.into_iter().zip(binary).find(|(_, b)| *b) {
                out.insert(name.clone(), (v, d));
                break;
            }
        }
    }
    Ok(out)
}

/// The snapshot date nearest to `from` at which every package of `names` has a binary, up to
/// [`SEARCH_MONTHS`] months either way: first month by month, then day by day at the boundary
/// (requirements chapter 6). Each package is taken at the version current on the date (V10).
/// `dates` are the published snapshot dates, sorted. Later dates are tried first when
/// `prefer_later` (moving to a newer R).
pub fn nearest_date(
    env: &Env,
    checker: &BinaryChecker,
    names: &[String],
    from: &str,
    dates: &[String],
    prefer_later: bool,
) -> Result<Option<String>, PinError> {
    let Some(latest) = dates.last() else {
        return Ok(None);
    };
    let all = releases(env, names, dates, latest)?;
    // Whether every package has a binary at the snapshot `dates[i]`; `None` if a package did
    // not exist yet.
    let complete = |i: usize| -> Result<Option<bool>, PinError> {
        let day = &dates[i];
        let mut items = Vec::new();
        for n in names {
            let current = all[n].iter().rev().find(|(_, d)| d <= day);
            let Some((v, d)) = current else {
                return Ok(None);
            };
            items.push((n.clone(), v.clone(), d.clone()));
        }
        Ok(Some(checker.check(&items)?.into_iter().all(|b| b)))
    };
    let start = dates
        .partition_point(|d| d.as_str() <= from)
        .saturating_sub(1);
    let index_of = |day: &str| dates.partition_point(|d| d.as_str() <= day).checked_sub(1);
    let sides: [i64; 2] = if prefer_later { [1, -1] } else { [-1, 1] };
    let mut last = [start; 2];
    for k in 1..=SEARCH_MONTHS {
        for (s, sign) in sides.iter().enumerate() {
            let Some(day) = crate::date::add_days(from, sign * k * 30) else {
                continue;
            };
            let Some(i) = index_of(&day) else { continue };
            if i == last[s] || (*sign > 0 && i <= start) || (*sign < 0 && i >= start) {
                continue;
            }
            if complete(i)? == Some(true) {
                // Narrow down between the last date that failed on this side and this one.
                let (mut near, mut far) = (last[s], i);
                while near.abs_diff(far) > 1 {
                    let mid = (near + far) / 2;
                    if complete(mid)? == Some(true) {
                        far = mid;
                    } else {
                        near = mid;
                    }
                }
                return Ok(Some(dates[far].clone()));
            }
            last[s] = i;
        }
    }
    Ok(None)
}
