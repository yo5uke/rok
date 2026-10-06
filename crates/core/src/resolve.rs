//! Dependency resolution with PubGrub.
//!
//! The graph has three kinds of nodes: the project (the root), R itself (a single version, the
//! project's R) and packages. Making R a node lets explanations say "fixest needs R >= 4.5".
//! Each package's candidate versions come from a [`CandidateSource`], normally the version in the
//! project's snapshot and the locked version. A preferred (locked) version is chosen whenever it
//! is allowed, so an operation changes as little as possible (requirements, chapter 5).
//! When there is no solution, the explanation is a chain of dependencies in English.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Bound;
use std::rc::Rc;

use pubgrub::{
    DefaultStringReporter, Dependencies, DependencyConstraints, DependencyProvider, DerivationTree,
    Derived, External, Map, PackageResolutionStatistics, PubGrubError, Ranges, ReportFormatter,
    Reporter, Term,
};

use crate::constraint::{Constraint, Op};
use crate::dcf::Dependency;
use crate::lockfile::name_order;
use crate::p3m::{Index, IndexEntry};
use crate::platform::Os;
use crate::rpkgs::is_base;
use crate::version::Version;

/// A node of the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    Project,
    R,
    Package(String),
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Project => f.write_str("your project"),
            Node::R => f.write_str("R"),
            Node::Package(name) => f.write_str(name),
        }
    }
}

/// One available version of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub version: Version,
    /// The snapshot date this version is taken from.
    pub date: String,
    pub r_constraint: Constraint,
    pub dependencies: Vec<Dependency>,
    pub linking_to: Vec<Dependency>,
    pub os_type: Option<String>,
}

impl Candidate {
    pub fn from_index(entry: &IndexEntry, date: &str) -> Candidate {
        Candidate {
            version: entry.version.clone(),
            date: date.to_string(),
            r_constraint: entry.r_constraint.clone(),
            dependencies: entry.dependencies.clone(),
            linking_to: entry.linking_to.clone(),
            os_type: entry.os_type.clone(),
        }
    }
}

/// Versions of a package with the snapshot dates they come from.
type Versions = Rc<Vec<(Version, String)>>;
/// Loads the index of a snapshot date.
type LoadIndex<'a> = Box<dyn Fn(&str) -> Result<Index, SourceError> + 'a>;
/// Fetches a package's releases with their publication times.
type FetchHistory<'a> = Box<dyn Fn(&str) -> Result<Vec<(Version, String)>, SourceError> + 'a>;

/// Error from a [`CandidateSource`], such as a failed index download.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SourceError(pub String);

/// Provides the candidate versions of packages.
pub trait CandidateSource {
    /// Every version of `name` that may be used, with the snapshot date it comes from, in any
    /// order. Empty if the package is unknown. This should be cheap: details are fetched only
    /// for the versions the solver looks at.
    fn versions(&self, name: &str) -> Result<Vec<(Version, String)>, SourceError>;

    /// The details of one version returned by [`CandidateSource::versions`].
    fn details(&self, name: &str, version: &Version, date: &str) -> Result<Candidate, SourceError>;
}

/// What to resolve.
#[derive(Debug, Clone)]
pub struct Request {
    /// The project's R version.
    pub r_version: Version,
    pub os: Os,
    /// Declared packages and their constraints.
    pub requirements: Vec<(String, Constraint)>,
    /// Versions to keep when allowed, usually those in the lockfile.
    pub preferred: HashMap<String, Version>,
    /// Whether `LinkingTo` packages are part of the result (needed to build from source).
    pub include_linking_to: bool,
}

/// A package in the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub name: String,
    pub version: Version,
    pub date: String,
    /// Names of the packages it needs (base packages excluded), sorted.
    pub dependencies: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("No solution was found for the dependencies.\n{0}")]
    NoSolution(String),
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// Resolves `request`. The result is sorted by package name.
pub fn resolve(
    source: &dyn CandidateSource,
    request: &Request,
) -> Result<Vec<Resolved>, ResolveError> {
    let provider = Provider {
        source,
        request,
        versions: RefCell::new(HashMap::new()),
        details: RefCell::new(HashMap::new()),
    };
    match pubgrub::resolve(&provider, Node::Project, root_version()) {
        Ok(selected) => {
            let mut out = Vec::new();
            for (node, version) in selected {
                let Node::Package(name) = node else { continue };
                let cand = provider.details(&name, &version)?;
                let mut deps: Vec<String> = provider
                    .used_dependencies(&cand)
                    .map(|d| d.name.clone())
                    .collect();
                deps.sort_by(|a, b| name_order(a, b));
                deps.dedup();
                out.push(Resolved {
                    name,
                    version,
                    date: cand.date.clone(),
                    dependencies: deps,
                });
            }
            out.sort_by(|a, b| name_order(&a.name, &b.name));
            Ok(out)
        }
        Err(PubGrubError::NoSolution(mut tree)) => {
            collapse_no_versions(&mut tree);
            let formatter = Formatter {
                r_version: request.r_version.clone(),
            };
            Err(ResolveError::NoSolution(
                DefaultStringReporter::report_with_formatter(&tree, &formatter),
            ))
        }
        Err(PubGrubError::ErrorRetrievingDependencies { source, .. })
        | Err(PubGrubError::ErrorChoosingVersion { source, .. }) => Err(source.into()),
        Err(PubGrubError::ErrorInShouldCancel(e)) => Err(e.into()),
    }
}

/// The root's version. It is never shown to users.
fn root_version() -> Version {
    "0.0".parse().expect("a valid version")
}

/// The PubGrub version set for a constraint.
fn ranges(c: &Constraint) -> Ranges<Version> {
    c.clauses().iter().fold(Ranges::full(), |acc, clause| {
        let v = clause.version.clone();
        let r = match clause.op {
            Op::Lt => Ranges::strictly_lower_than(v),
            Op::Le => Ranges::lower_than(v),
            Op::Gt => Ranges::strictly_higher_than(v),
            Op::Ge => Ranges::higher_than(v),
            Op::Eq => Ranges::singleton(v),
            Op::Ne => Ranges::singleton(v).complement(),
        };
        acc.intersection(&r)
    })
}

struct Provider<'a> {
    source: &'a dyn CandidateSource,
    request: &'a Request,
    /// Versions per package, newest first, with their snapshot dates.
    versions: RefCell<HashMap<String, Versions>>,
    details: RefCell<HashMap<(String, Version), Rc<Candidate>>>,
}

impl Provider<'_> {
    fn versions(&self, name: &str) -> Result<Versions, SourceError> {
        if let Some(v) = self.versions.borrow().get(name) {
            return Ok(v.clone());
        }
        let mut list = self.source.versions(name)?;
        list.sort_by(|a, b| b.0.cmp(&a.0));
        list.dedup_by(|a, b| a.0 == b.0);
        let list = Rc::new(list);
        self.versions
            .borrow_mut()
            .insert(name.to_string(), list.clone());
        Ok(list)
    }

    fn details(&self, name: &str, version: &Version) -> Result<Rc<Candidate>, SourceError> {
        let key = (name.to_string(), version.clone());
        if let Some(c) = self.details.borrow().get(&key) {
            return Ok(c.clone());
        }
        let versions = self.versions(name)?;
        let date = versions
            .iter()
            .find(|(v, _)| v == version)
            .map(|(_, d)| d.as_str())
            .ok_or_else(|| SourceError(format!("{name} {version} is not a candidate")))?;
        let cand = Rc::new(self.source.details(name, version, date)?);
        self.details.borrow_mut().insert(key, cand.clone());
        Ok(cand)
    }

    /// The dependencies of a candidate that take part in resolution (base packages excluded).
    fn used_dependencies<'c>(&self, cand: &'c Candidate) -> impl Iterator<Item = &'c Dependency> {
        let linking: &[Dependency] = if self.request.include_linking_to {
            &cand.linking_to
        } else {
            &[]
        };
        cand.dependencies
            .iter()
            .chain(linking)
            .filter(|d| !is_base(&d.name))
    }
}

/// Collects constraints, merging repeated packages (for example in both Imports and LinkingTo).
fn constraints<'d>(
    items: impl Iterator<Item = (Node, &'d Constraint)>,
) -> DependencyConstraints<Node, Ranges<Version>> {
    let mut merged: Vec<(Node, Ranges<Version>)> = Vec::new();
    for (node, c) in items {
        let r = ranges(c);
        match merged.iter_mut().find(|(n, _)| *n == node) {
            Some((_, existing)) => *existing = existing.intersection(&r),
            None => merged.push((node, r)),
        }
    }
    merged.into_iter().collect()
}

impl DependencyProvider for Provider<'_> {
    type P = Node;
    type V = Version;
    type VS = Ranges<Version>;
    type M = String;
    type Priority = (u32, Reverse<usize>);
    type Err = SourceError;

    fn prioritize(
        &self,
        package: &Node,
        range: &Ranges<Version>,
        stats: &PackageResolutionStatistics,
    ) -> Self::Priority {
        match package {
            Node::Project | Node::R => (u32::MAX, Reverse(0)),
            Node::Package(name) => {
                // Decide packages in conflict first, then those with the fewest choices.
                let choices = self
                    .versions(name)
                    .map(|c| c.iter().filter(|(v, _)| range.contains(v)).count())
                    .unwrap_or(0);
                (stats.conflict_count(), Reverse(choices))
            }
        }
    }

    fn choose_version(
        &self,
        package: &Node,
        range: &Ranges<Version>,
    ) -> Result<Option<Version>, SourceError> {
        match package {
            Node::Project => Ok(Some(root_version())),
            Node::R => Ok(range
                .contains(&self.request.r_version)
                .then(|| self.request.r_version.clone())),
            Node::Package(name) => {
                let versions = self.versions(name)?;
                let allowed = |v: &Version| range.contains(v);
                if let Some(pref) = self.request.preferred.get(name)
                    && allowed(pref)
                    && versions.iter().any(|(v, _)| v == pref)
                {
                    return Ok(Some(pref.clone()));
                }
                Ok(versions
                    .iter()
                    .map(|(v, _)| v)
                    .find(|v| allowed(v))
                    .cloned())
            }
        }
    }

    fn get_dependencies(
        &self,
        package: &Node,
        version: &Version,
    ) -> Result<Dependencies<Node, Ranges<Version>, String>, SourceError> {
        match package {
            Node::Project => Ok(Dependencies::Available(constraints(
                self.request
                    .requirements
                    .iter()
                    .filter(|(name, _)| !is_base(name))
                    .map(|(name, c)| (Node::Package(name.clone()), c)),
            ))),
            Node::R => Ok(Dependencies::Available(DependencyConstraints::default())),
            Node::Package(name) => {
                let cand = self.details(name, version)?;
                let os = match self.request.os {
                    Os::Windows => "windows",
                    Os::Linux | Os::MacOs => "unix",
                };
                if let Some(only) = &cand.os_type
                    && only != os
                {
                    return Ok(Dependencies::Unavailable(format!(
                        "it works only on {only}"
                    )));
                }
                let r = (!cand.r_constraint.is_any()).then_some((Node::R, &cand.r_constraint));
                let pkgs = self
                    .used_dependencies(&cand)
                    .map(|d| (Node::Package(d.name.clone()), &d.constraint));
                Ok(Dependencies::Available(constraints(
                    r.into_iter().chain(pkgs),
                )))
            }
        }
    }
}

/// Candidates from P3M snapshots: the version in the project's snapshot, the locked version
/// (taken from the snapshot date recorded in the lockfile) when it differs, and, for packages
/// whose constraint excludes the snapshot's version, older releases (requirements, chapter 7).
pub struct SnapshotSource<'a> {
    date: String,
    load: LoadIndex<'a>,
    indexes: RefCell<HashMap<String, Rc<Index>>>,
    locked: HashMap<String, (Version, String)>,
    history: Option<History<'a>>,
    /// A later snapshot whose versions are also offered (`rok update <package>`).
    newer: Option<String>,
}

struct History<'a> {
    dates: Vec<String>,
    packages: HashSet<String>,
    fetch: FetchHistory<'a>,
}

impl<'a> SnapshotSource<'a> {
    /// `load` returns the index of a snapshot date; `locked` maps a package to its locked
    /// version and the date it came from.
    pub fn new(
        date: &str,
        load: impl Fn(&str) -> Result<Index, SourceError> + 'a,
        locked: HashMap<String, (Version, String)>,
    ) -> Self {
        SnapshotSource {
            date: date.to_string(),
            load: Box::new(load),
            indexes: RefCell::new(HashMap::new()),
            locked,
            history: None,
            newer: None,
        }
    }

    /// Also offers each package's version in the snapshot of `date` (later than the project's).
    /// With the other packages' locked versions preferred, only what must move moves.
    pub fn with_newer(mut self, date: &str) -> Self {
        self.newer = Some(date.to_string());
        self
    }

    /// Also offers the older releases of `packages`. `fetch` returns a package's releases with
    /// their publication times; `dates` are the published snapshot dates (sorted).
    pub fn with_history(
        mut self,
        dates: Vec<String>,
        packages: HashSet<String>,
        fetch: impl Fn(&str) -> Result<Vec<(Version, String)>, SourceError> + 'a,
    ) -> Self {
        self.history = Some(History {
            dates,
            packages,
            fetch: Box::new(fetch),
        });
        self
    }

    fn index(&self, date: &str) -> Result<Rc<Index>, SourceError> {
        if let Some(i) = self.indexes.borrow().get(date) {
            return Ok(i.clone());
        }
        let index = Rc::new((self.load)(date)?);
        self.indexes
            .borrow_mut()
            .insert(date.to_string(), index.clone());
        Ok(index)
    }
}

impl CandidateSource for SnapshotSource<'_> {
    fn versions(&self, name: &str) -> Result<Vec<(Version, String)>, SourceError> {
        let mut out = Vec::new();
        if let Some(entry) = self.index(&self.date)?.get(name) {
            out.push((entry.version.clone(), self.date.clone()));
        }
        if let Some((version, date)) = self.locked.get(name)
            && out.iter().all(|(v, _)| v != version)
        {
            out.push((version.clone(), date.clone()));
        }
        if let Some(newer) = &self.newer
            && let Some(entry) = self.index(newer)?.get(name)
            && out.iter().all(|(v, _)| v != &entry.version)
        {
            out.push((entry.version.clone(), newer.clone()));
        }
        if let Some(h) = &self.history
            && h.packages.contains(name)
        {
            for (v, d) in release_snapshots(&(h.fetch)(name)?, &h.dates, &self.date) {
                if out.iter().all(|(known, _)| known != &v) {
                    out.push((v, d));
                }
            }
        }
        Ok(out)
    }

    fn details(&self, name: &str, version: &Version, date: &str) -> Result<Candidate, SourceError> {
        let index = self.index(date)?;
        match index.get(name).filter(|e| &e.version == version) {
            Some(entry) => Ok(Candidate::from_index(entry, date)),
            // A locked version that its snapshot does not have would otherwise be replaced
            // silently, so it is an error.
            None if self.locked.get(name) == Some(&(version.clone(), date.to_string())) => {
                Err(SourceError(format!(
                    "rok.lock records {name} {version} from the {date} snapshot, but that snapshot does not have it"
                )))
            }
            None => Err(SourceError(format!(
                "the {date} snapshot does not have {name} {version}"
            ))),
        }
    }
}

/// For each release, the snapshot date to take it from: the first snapshot after its
/// publication, provided the next release had not been published by then (V10), and no later
/// than `until`. `history` holds (version, publication time) pairs; `dates` must be sorted.
pub fn release_snapshots(
    history: &[(Version, String)],
    dates: &[String],
    until: &str,
) -> Vec<(Version, String)> {
    let mut releases: Vec<&(Version, String)> = history.iter().collect();
    releases.sort_by(|a, b| a.1.cmp(&b.1));
    let day = |t: &str| t.get(..10).unwrap_or(t).to_string();
    let mut out = Vec::new();
    for (i, (version, published)) in releases.iter().enumerate() {
        // A snapshot taken at 00:00 UTC on day T includes releases published before day T.
        let published_day = day(published);
        let first = dates.partition_point(|d| d.as_str() <= published_day.as_str());
        let Some(date) = dates.get(first) else {
            continue;
        };
        let superseded = releases
            .get(i + 1)
            .is_some_and(|(_, next)| day(next) < *date);
        if !superseded && date.as_str() <= until {
            out.push((version.clone(), date.clone()));
        }
    }
    out
}

// ---- explanations ----

type Tree = DerivationTree<Node, Ranges<Version>, String>;

/// Removes "no version of X matches (all versions except the one we have)" steps from the
/// explanation, like pubgrub's `DerivationTree::collapse_no_versions`, but keeps two cases that
/// are often the actual reason: R not satisfying a requirement, and a package that does not
/// exist at all.
fn collapse_no_versions(tree: &mut Tree) {
    let Tree::Derived(derived) = tree else { return };
    // Kept: R not satisfying a requirement, a package that does not exist at all, and a
    // requirement that no version of a package satisfies (merging would drop the reason).
    let keep = |p: &Node, r: &Ranges<Version>, other: &Tree| {
        *p == Node::R
            || *r == Ranges::full()
            || matches!(other, Tree::External(External::FromDependencyOf(_, _, dep, wanted)) if dep == p && wanted.subset_of(r))
    };
    let c1 = std::sync::Arc::make_mut(&mut derived.cause1);
    let c2 = std::sync::Arc::make_mut(&mut derived.cause2);
    let collapsed = match (c1, c2) {
        (Tree::External(External::NoVersions(p, r)), other)
        | (other, Tree::External(External::NoVersions(p, r)))
            if !keep(p, r, other) =>
        {
            collapse_no_versions(other);
            merge_no_versions(other.clone(), p, r)
        }
        (c1, c2) => {
            collapse_no_versions(c1);
            collapse_no_versions(c2);
            None
        }
    };
    if let Some(t) = collapsed {
        *tree = t;
    }
}

/// Folds "no other version of `package` in `set`" into an external dependency step.
fn merge_no_versions(tree: Tree, package: &Node, set: &Ranges<Version>) -> Option<Tree> {
    match tree {
        Tree::Derived(_) => Some(tree),
        Tree::External(External::FromDependencyOf(p1, r1, p2, r2)) => {
            Some(Tree::External(if p1 == *package {
                External::FromDependencyOf(p1, r1.union(set), p2, r2)
            } else {
                External::FromDependencyOf(p1, r1, p2, r2.union(set))
            }))
        }
        // rok's only custom reason is a package limited to another OS. If no other versions
        // exist, the reason covers the package as a whole.
        Tree::External(External::Custom(p, r, reason)) if p == *package => {
            Some(Tree::External(External::Custom(p, r.union(set), reason)))
        }
        // The reason may not match, so these are kept as they are.
        Tree::External(External::NoVersions(..) | External::Custom(..) | External::NotRoot(..)) => {
            None
        }
    }
}

/// Formats explanations for rok: the root is "your project", versions are written the way
/// rok writes constraints (`>= 1.0, < 2.0`), and a single version reads `fixest 0.12.1`.
/// The structure follows pubgrub's `DefaultStringReportFormatter`.
struct Formatter {
    r_version: Version,
}

/// A version set in rok's notation, or `None` when it allows any version.
fn fmt_set(set: &Ranges<Version>) -> Option<String> {
    if *set == Ranges::full() {
        return None;
    }
    let parts: Vec<String> = set
        .iter()
        .map(|(lo, hi)| match (lo, hi) {
            (Bound::Included(a), Bound::Included(b)) if a == b => a.to_string(),
            (lo, hi) => {
                let mut s = Vec::new();
                match lo {
                    Bound::Included(a) => s.push(format!(">= {a}")),
                    Bound::Excluded(a) => s.push(format!("> {a}")),
                    Bound::Unbounded => {}
                }
                match hi {
                    Bound::Included(b) => s.push(format!("<= {b}")),
                    Bound::Excluded(b) => s.push(format!("< {b}")),
                    Bound::Unbounded => {}
                }
                s.join(", ")
            }
        })
        .collect();
    Some(if parts.is_empty() {
        "(no version)".to_string()
    } else {
        parts.join(" or ")
    })
}

fn named(node: &Node, set: &Ranges<Version>) -> String {
    match (node, fmt_set(set)) {
        (Node::Project, _) | (_, None) => node.to_string(),
        (_, Some(s)) => format!("{node} {s}"),
    }
}

type Ext = External<Node, Ranges<Version>, String>;
type Der = Derived<Node, Ranges<Version>, String>;
type Terms = Map<Node, Term<Ranges<Version>>>;

impl ReportFormatter<Node, Ranges<Version>, String> for Formatter {
    type Output = String;

    fn format_external(&self, external: &Ext) -> String {
        match external {
            External::NotRoot(p, _) => format!("we are solving the dependencies of {p}"),
            External::NoVersions(Node::R, set) => format!(
                "R {} does not satisfy {}",
                self.r_version,
                fmt_set(set).unwrap_or_default()
            ),
            External::NoVersions(p, set) => match fmt_set(set) {
                None => format!("{p} is not available"),
                Some(s) => format!("no version of {p} matches {s}"),
            },
            External::FromDependencyOf(p, ps, d, ds) => {
                format!("{} depends on {}", named(p, ps), named(d, ds))
            }
            External::Custom(p, ps, reason) => format!("{} cannot be used: {reason}", named(p, ps)),
        }
    }

    fn format_terms(&self, terms: &Terms) -> String {
        let terms: Vec<_> = terms.iter().collect();
        match terms.as_slice() {
            [] => "version solving failed".into(),
            [(Node::Project, Term::Positive(_))] => {
                "your project's dependencies cannot be satisfied".into()
            }
            [(p, Term::Positive(r))] => format!("{} cannot be used", named(p, r)),
            [(p, Term::Negative(r))] => format!("{} is required", named(p, r)),
            [(p1, Term::Positive(r1)), (p2, Term::Negative(r2))] => self.format_external(
                &External::FromDependencyOf((*p1).clone(), r1.clone(), (*p2).clone(), r2.clone()),
            ),
            [(p1, Term::Negative(r1)), (p2, Term::Positive(r2))] => self.format_external(
                &External::FromDependencyOf((*p2).clone(), r2.clone(), (*p1).clone(), r1.clone()),
            ),
            slice => {
                let parts: Vec<String> = slice
                    .iter()
                    .map(|(p, t)| match t {
                        Term::Positive(r) => named(p, r),
                        Term::Negative(r) => format!("not {}", named(p, r)),
                    })
                    .collect();
                let list = match parts.split_last() {
                    Some((last, rest)) if !rest.is_empty() => {
                        format!("{} and {last}", rest.join(", "))
                    }
                    _ => parts.join(""),
                };
                format!("{list} cannot be used together")
            }
        }
    }

    fn explain_both_external(&self, e1: &Ext, e2: &Ext, current: &Terms) -> String {
        format!(
            "Because {} and {}, {}.",
            self.format_external(e1),
            self.format_external(e2),
            self.format_terms(current)
        )
    }

    fn explain_both_ref(
        &self,
        id1: usize,
        d1: &Der,
        id2: usize,
        d2: &Der,
        current: &Terms,
    ) -> String {
        format!(
            "Because {} ({id1}) and {} ({id2}), {}.",
            self.format_terms(&d1.terms),
            self.format_terms(&d2.terms),
            self.format_terms(current)
        )
    }

    fn explain_ref_and_external(&self, id: usize, d: &Der, e: &Ext, current: &Terms) -> String {
        format!(
            "Because {} ({id}) and {}, {}.",
            self.format_terms(&d.terms),
            self.format_external(e),
            self.format_terms(current)
        )
    }

    fn and_explain_external(&self, e: &Ext, current: &Terms) -> String {
        format!(
            "And because {}, {}.",
            self.format_external(e),
            self.format_terms(current)
        )
    }

    fn and_explain_ref(&self, id: usize, d: &Der, current: &Terms) -> String {
        format!(
            "And because {} ({id}), {}.",
            self.format_terms(&d.terms),
            self.format_terms(current)
        )
    }

    fn and_explain_prior_and_external(&self, prior: &Ext, e: &Ext, current: &Terms) -> String {
        format!(
            "And because {} and {}, {}.",
            self.format_external(prior),
            self.format_external(e),
            self.format_terms(current)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Candidates from a table: name → [(version, R constraint, dependencies, linking_to)].
    struct Table(HashMap<String, Vec<Candidate>>);

    impl Table {
        fn new(rows: &[(&str, &str, &str, &str, &str)]) -> Table {
            let mut map: HashMap<String, Vec<Candidate>> = HashMap::new();
            for (name, version, r, deps, linking) in rows {
                let parse = |s: &str| crate::dcf::parse_dependencies(s).unwrap();
                map.entry(name.to_string()).or_default().push(Candidate {
                    version: version.parse().unwrap(),
                    date: "2026-10-01".into(),
                    r_constraint: if r.is_empty() {
                        Constraint::any()
                    } else {
                        r.parse().unwrap()
                    },
                    dependencies: parse(deps),
                    linking_to: parse(linking),
                    os_type: None,
                });
            }
            Table(map)
        }
    }

    impl CandidateSource for Table {
        fn versions(&self, name: &str) -> Result<Vec<(Version, String)>, SourceError> {
            Ok(self
                .0
                .get(name)
                .map(|cs| {
                    cs.iter()
                        .map(|c| (c.version.clone(), c.date.clone()))
                        .collect()
                })
                .unwrap_or_default())
        }

        fn details(
            &self,
            name: &str,
            version: &Version,
            _date: &str,
        ) -> Result<Candidate, SourceError> {
            self.0[name]
                .iter()
                .find(|c| &c.version == version)
                .cloned()
                .ok_or_else(|| SourceError("not listed".into()))
        }
    }

    fn request(reqs: &[(&str, &str)]) -> Request {
        Request {
            r_version: "4.4.2".parse().unwrap(),
            os: Os::Linux,
            requirements: reqs
                .iter()
                .map(|(n, c)| (n.to_string(), c.parse().unwrap()))
                .collect(),
            preferred: HashMap::new(),
            include_linking_to: true,
        }
    }

    fn versions(result: &[Resolved]) -> Vec<String> {
        result
            .iter()
            .map(|r| format!("{} {}", r.name, r.version))
            .collect()
    }

    fn no_solution(source: &Table, req: &Request) -> String {
        match resolve(source, req) {
            Err(ResolveError::NoSolution(s)) => s,
            other => panic!("expected no solution, got {other:?}"),
        }
    }

    #[test]
    fn resolves_the_newest_allowed_versions() {
        let t = Table::new(&[
            (
                "fixest",
                "0.12.1",
                ">= 3.5.0",
                "stats, Rcpp (>= 1.0.5), dreamerr",
                "Rcpp",
            ),
            (
                "fixest",
                "0.14.2",
                ">= 3.5.0",
                "stats, Rcpp (>= 1.0.5), dreamerr",
                "Rcpp",
            ),
            ("Rcpp", "1.0.4", "", "methods, utils", ""),
            ("Rcpp", "1.1.2", "", "methods, utils", ""),
            ("dreamerr", "1.4.0", "", "", ""),
        ]);
        let result = resolve(&t, &request(&[("fixest", "< 0.13")])).unwrap();
        assert_eq!(
            versions(&result),
            ["dreamerr 1.4.0", "fixest 0.12.1", "Rcpp 1.1.2"]
        );
        assert_eq!(result[1].dependencies, ["dreamerr", "Rcpp"]);
        assert_eq!(result[1].date, "2026-10-01");
    }

    #[test]
    fn keeps_preferred_versions_when_allowed() {
        let t = Table::new(&[
            ("a", "1.0", "", "b (>= 1.0)", ""),
            ("b", "1.1", "", "", ""),
            ("b", "1.2", "", "", ""),
        ]);
        let mut req = request(&[("a", "*")]);
        req.preferred.insert("b".into(), "1.1".parse().unwrap());
        assert_eq!(versions(&resolve(&t, &req).unwrap()), ["a 1.0", "b 1.1"]);
        // A new requirement moves only what it must.
        req.requirements
            .push(("b".into(), ">= 1.2".parse().unwrap()));
        assert_eq!(versions(&resolve(&t, &req).unwrap()), ["a 1.0", "b 1.2"]);
    }

    #[test]
    fn handles_linking_to_and_base_packages() {
        let t = Table::new(&[
            ("a", "1.0", "", "stats, methods", "BH"),
            ("BH", "1.87.0", "", "", ""),
        ]);
        assert_eq!(
            versions(&resolve(&t, &request(&[("a", "*"), ("utils", "*")])).unwrap()),
            ["a 1.0", "BH 1.87.0"]
        );
        let mut req = request(&[("a", "*")]);
        req.include_linking_to = false;
        assert_eq!(versions(&resolve(&t, &req).unwrap()), ["a 1.0"]);
    }

    #[test]
    fn explains_an_r_version_conflict() {
        let t = Table::new(&[("fixest", "0.14.2", ">= 4.5.0", "", "")]);
        let msg = no_solution(&t, &request(&[("fixest", "*")]));
        assert!(msg.contains("fixest 0.14.2 depends on R >= 4.5.0"), "{msg}");
        assert!(msg.contains("R 4.4.2 does not satisfy >= 4.5.0"), "{msg}");
    }

    #[test]
    fn explains_a_dependency_conflict() {
        let t = Table::new(&[
            ("a", "1.0", "", "c (< 2.0)", ""),
            ("b", "1.0", "", "c (>= 2.0)", ""),
            ("c", "1.5", "", "", ""),
            ("c", "2.1", "", "", ""),
        ]);
        let msg = no_solution(&t, &request(&[("a", "*"), ("b", "*")]));
        // `a` has a single version, so the explanation names the package without a version.
        for part in [
            "a depends on c < 2.0",
            "b 1.0 depends on c >= 2.0",
            "your project depends on",
        ] {
            assert!(msg.contains(part), "missing `{part}` in:\n{msg}");
        }
        assert!(
            !msg.contains("0.0"),
            "the root's version must not appear:\n{msg}"
        );
    }

    #[test]
    fn explains_a_constraint_no_version_satisfies() {
        let t = Table::new(&[("data.table", "1.18.6.1", "", "", "")]);
        let msg = no_solution(&t, &request(&[("data.table", "< 1.17")]));
        assert!(
            msg.contains("no version of data.table matches < 1.17"),
            "{msg}"
        );
        assert!(
            msg.contains("your project depends on data.table < 1.17"),
            "{msg}"
        );
    }

    #[test]
    fn explains_missing_and_os_specific_packages() {
        let t = Table::new(&[("a", "1.0", "", "", "")]);
        let msg = no_solution(&t, &request(&[("nosuchpkg", "*")]));
        assert!(msg.contains("nosuchpkg is not available"), "{msg}");
        let mut t = Table::new(&[("winonly", "1.0", "", "", "")]);
        t.0.get_mut("winonly").unwrap()[0].os_type = Some("windows".into());
        let msg = no_solution(&t, &request(&[("winonly", "*")]));
        assert!(
            msg.contains("winonly cannot be used: it works only on windows"),
            "{msg}"
        );
        assert!(!msg.contains("no version of"), "{msg}");
    }

    #[test]
    fn snapshot_source_adds_the_locked_version() {
        let current = "Package: b\nVersion: 1.2\n\nPackage: a\nVersion: 1.0\nImports: b\n";
        let old = "Package: b\nVersion: 1.1\n";
        let load = |date: &str| -> Result<Index, SourceError> {
            match date {
                "2026-10-01" => Ok(Index::parse(date, current)),
                "2025-01-02" => Ok(Index::parse(date, old)),
                _ => Err(SourceError(format!("no index for {date}"))),
            }
        };
        let locked = HashMap::from([(
            "b".to_string(),
            ("1.1".parse().unwrap(), "2025-01-02".to_string()),
        )]);
        let source = SnapshotSource::new("2026-10-01", load, locked);
        let dates: Vec<String> = source
            .versions("b")
            .unwrap()
            .iter()
            .map(|(v, d)| format!("{v} {d}"))
            .collect();
        assert_eq!(dates, ["1.2 2026-10-01", "1.1 2025-01-02"]);
        let mut req = request(&[("a", "*")]);
        req.preferred.insert("b".into(), "1.1".parse().unwrap());
        let result = resolve(&source, &req).unwrap();
        assert_eq!(versions(&result), ["a 1.0", "b 1.1"]);
        assert_eq!(result[1].date, "2025-01-02");
        assert!(source.versions("unknown").unwrap().is_empty());
        // A locked version missing from its snapshot is reported, not replaced.
        let wrong = HashMap::from([(
            "b".to_string(),
            ("1.0".parse().unwrap(), "2025-01-02".to_string()),
        )]);
        let mut req = request(&[("a", "*")]);
        req.preferred.insert("b".into(), "1.0".parse().unwrap());
        let err = resolve(&SnapshotSource::new("2026-10-01", load, wrong), &req)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("rok.lock records b 1.0 from the 2025-01-02 snapshot"),
            "{err}"
        );
    }

    #[test]
    fn updates_one_package_and_only_the_dependencies_it_needs() {
        // Project snapshot: sf 1.0 (needs units >= 0.8), units 0.8, s2 1.0.
        // Newer snapshot: sf 1.1 (needs units >= 0.9), units 0.9, s2 1.1.
        let indexes = |date: &str| -> Result<Index, SourceError> {
            Ok(Index::parse(
                date,
                match date {
                    "2026-01-01" => {
                        "Package: sf\nVersion: 1.0\nImports: units (>= 0.8), s2\n\nPackage: units\nVersion: 0.8\n\nPackage: s2\nVersion: 1.0\n"
                    }
                    _ => {
                        "Package: sf\nVersion: 1.1\nImports: units (>= 0.9), s2\n\nPackage: units\nVersion: 0.9\n\nPackage: s2\nVersion: 1.1\n"
                    }
                },
            ))
        };
        let lock = |n: &str, v: &str| {
            (
                n.to_string(),
                (v.parse().unwrap(), "2026-01-01".to_string()),
            )
        };
        let locked = HashMap::from([lock("sf", "1.0"), lock("units", "0.8"), lock("s2", "1.0")]);
        let source = SnapshotSource::new("2026-01-01", indexes, locked).with_newer("2026-10-01");
        let mut req = request(&[("sf", "*")]);
        req.preferred = HashMap::from([
            ("units".into(), "0.8".parse().unwrap()),
            ("s2".into(), "1.0".parse().unwrap()),
        ]);
        let got: Vec<String> = resolve(&source, &req)
            .unwrap()
            .iter()
            .map(|r| format!("{} {} {}", r.name, r.version, r.date))
            .collect();
        // sf moves; units must move for it; s2 stays.
        assert_eq!(
            got,
            [
                "s2 1.0 2026-01-01",
                "sf 1.1 2026-10-01",
                "units 0.9 2026-10-01"
            ]
        );
    }

    fn release(v: &str, t: &str) -> (Version, String) {
        (v.parse().unwrap(), t.to_string())
    }

    #[test]
    fn finds_the_snapshot_of_each_release() {
        let dates: Vec<String> = [
            "2024-01-09",
            "2024-01-12",
            "2024-07-18",
            "2024-07-20",
            "2025-01-13",
        ]
        .iter()
        .map(|d| d.to_string())
        .collect();
        let history = [
            release("1.0.11", "2023-07-04T10:00:00Z"), // before the first listed date
            release("1.0.12", "2024-01-09T08:20:35Z"), // the 01-09 snapshot was taken before it
            release("1.0.13", "2024-07-17T15:50:06Z"),
            release("1.0.13-1", "2024-07-19T01:00:00Z"), // superseded? no: next is 2025
            release("1.0.14", "2025-01-12T16:10:02Z"),
        ];
        let got: Vec<String> = release_snapshots(&history, &dates, "2025-01-13")
            .iter()
            .map(|(v, d)| format!("{v} {d}"))
            .collect();
        assert_eq!(
            got,
            [
                "1.0.11 2024-01-09",
                "1.0.12 2024-01-12",
                "1.0.13 2024-07-18",
                "1.0.13-1 2024-07-20",
                "1.0.14 2025-01-13"
            ]
        );
        // A release replaced before any snapshot saw it is left out; `until` limits the dates.
        let quick = [
            release("2.0", "2024-07-18T01:00:00Z"),
            release("2.1", "2024-07-19T01:00:00Z"),
        ];
        let got = release_snapshots(&quick, &dates, "2024-12-31");
        assert_eq!(got, [release("2.1", "2024-07-20")]);
    }

    #[test]
    fn offers_older_releases_for_constrained_packages() {
        let indexes = |date: &str| -> Result<Index, SourceError> {
            Ok(Index::parse(
                date,
                match date {
                    "2026-10-01" => {
                        "Package: fixest\nVersion: 0.14.2\nImports: Rcpp\n\nPackage: Rcpp\nVersion: 1.1.2\n"
                    }
                    "2025-03-03" => "Package: fixest\nVersion: 0.12.1\nImports: Rcpp\n",
                    _ => "",
                },
            ))
        };
        let dates = vec!["2025-03-03".to_string(), "2026-10-01".to_string()];
        let history = |name: &str| -> Result<Vec<(Version, String)>, SourceError> {
            assert_eq!(name, "fixest", "only constrained packages are looked up");
            Ok(vec![
                release("0.12.1", "2025-03-01T00:00:00Z"),
                release("0.14.2", "2026-09-01T00:00:00Z"),
            ])
        };
        let source = SnapshotSource::new("2026-10-01", indexes, HashMap::new()).with_history(
            dates,
            HashSet::from(["fixest".to_string()]),
            history,
        );
        let result = resolve(&source, &request(&[("fixest", "< 0.13")])).unwrap();
        let got: Vec<String> = result
            .iter()
            .map(|r| format!("{} {} {}", r.name, r.version, r.date))
            .collect();
        assert_eq!(got, ["fixest 0.12.1 2025-03-03", "Rcpp 1.1.2 2026-10-01"]);
    }
}
