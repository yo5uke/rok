//! `rok why` and `rok tree`: the dependency graph of a lockfile, drawn as text.

use std::collections::HashSet;

use crate::lockfile::{Lockfile, name_order};

/// Why `name` is in the lockfile, from the package up to the declared packages that need it.
/// `None` if the lockfile does not have the package.
pub fn why(lock: &Lockfile, declared: &[String], name: &str) -> Option<Vec<String>> {
    let pkg = lock.package(name)?;
    let label = |n: &str| {
        let version = lock
            .package(n)
            .map(|p| format!(" {}", p.version))
            .unwrap_or_default();
        let mark = if declared.iter().any(|d| d == n) {
            " (declared)"
        } else {
            ""
        };
        format!("{n}{version}{mark}")
    };
    let dependents = |n: &str| -> Vec<&str> {
        let mut d: Vec<&str> = lock
            .packages
            .iter()
            .filter(|p| p.dependencies.iter().any(|x| x == n))
            .map(|p| p.name.as_str())
            .collect();
        d.sort_by(|a, b| name_order(a, b));
        d
    };
    let mut lines = vec![label(&pkg.name)];
    let mut seen = HashSet::from([pkg.name.clone()]);
    draw(
        &mut lines,
        &mut seen,
        "",
        &dependents(&pkg.name),
        &label,
        &dependents,
        None,
        1,
    );
    Some(lines)
}

/// The packages each package depends on, from `roots` (or the declared packages) down.
/// Subtrees already shown are marked `(*)`. `depth` limits how deep to go.
pub fn tree(lock: &Lockfile, roots: &[String], depth: Option<usize>) -> Vec<String> {
    let label = |n: &str| match lock.package(n) {
        Some(p) => format!("{n} {}", p.version),
        None => format!("{n} (not in rok.lock)"),
    };
    let deps = |n: &str| -> Vec<&str> {
        let mut d: Vec<&str> = lock
            .package(n)
            .map(|p| p.dependencies.iter().map(String::as_str).collect())
            .unwrap_or_default();
        d.sort_by(|a, b| name_order(a, b));
        d
    };
    let mut roots: Vec<&String> = roots.iter().collect();
    roots.sort_by(|a, b| name_order(a, b));
    let mut lines = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        lines.push(label(root));
        seen.insert(root.clone());
        draw(
            &mut lines,
            &mut seen,
            "",
            &deps(root),
            &label,
            &deps,
            depth,
            1,
        );
    }
    lines
}

/// Draws `children` below the current line with box-drawing characters.
#[allow(clippy::too_many_arguments)]
fn draw<'a>(
    lines: &mut Vec<String>,
    seen: &mut HashSet<String>,
    prefix: &str,
    children: &[&'a str],
    label: &dyn Fn(&str) -> String,
    next: &dyn Fn(&str) -> Vec<&'a str>,
    depth: Option<usize>,
    level: usize,
) {
    if depth.is_some_and(|d| level > d) {
        return;
    }
    for (i, child) in children.iter().enumerate() {
        let last = i + 1 == children.len();
        let (branch, indent) = if last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };
        let grandchildren = next(child);
        if !seen.insert((*child).to_string()) && !grandchildren.is_empty() {
            lines.push(format!("{prefix}{branch}{} (*)", label(child)));
            continue;
        }
        lines.push(format!("{prefix}{branch}{}", label(child)));
        draw(
            lines,
            seen,
            &format!("{prefix}{indent}"),
            &grandchildren,
            label,
            next,
            depth,
            level + 1,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock() -> Lockfile {
        let mut text = String::from(
            "version = 1\ngenerated-by = \"rok\"\n[r]\nversion = \"4.6.1\"\n[snapshot]\ndate = \"2026-10-01\"\nrepository = \"x\"\n",
        );
        for (n, deps) in [
            ("fixest", "\"dreamerr\", \"Rcpp\""),
            ("modelsummary", "\"data.table\", \"insight\""),
            ("insight", ""),
            ("dreamerr", "\"Formula\""),
            ("Formula", ""),
            ("Rcpp", ""),
            ("data.table", ""),
            ("sandwich", "\"zoo\""),
            ("zoo", ""),
        ] {
            text.push_str(&format!(
                "[[package]]\nname = \"{n}\"\nversion = \"1.0\"\nsource = {{ repository = \"cran\" }}\ndependencies = [{deps}]\n"
            ));
        }
        Lockfile::parse(&text).unwrap()
    }

    #[test]
    fn draws_trees() {
        let roots = ["fixest".to_string(), "modelsummary".to_string()];
        assert_eq!(
            tree(&lock(), &roots, None),
            [
                "fixest 1.0",
                "├── dreamerr 1.0",
                "│   └── Formula 1.0",
                "└── Rcpp 1.0",
                "modelsummary 1.0",
                "├── data.table 1.0",
                "└── insight 1.0",
            ]
        );
        assert_eq!(
            tree(&lock(), &["fixest".to_string()], Some(1)),
            ["fixest 1.0", "├── dreamerr 1.0", "└── Rcpp 1.0"]
        );
    }

    #[test]
    fn marks_repeated_subtrees() {
        let roots = ["dreamerr".to_string(), "fixest".to_string()];
        let lines = tree(&lock(), &roots, None);
        assert!(
            lines.contains(&"├── dreamerr 1.0 (*)".to_string()),
            "{lines:?}"
        );
    }

    #[test]
    fn explains_why_a_package_is_needed() {
        let declared = ["fixest".to_string(), "modelsummary".to_string()];
        assert_eq!(
            why(&lock(), &declared, "Formula").unwrap(),
            [
                "Formula 1.0",
                "└── dreamerr 1.0",
                "    └── fixest 1.0 (declared)"
            ]
        );
        assert_eq!(
            why(&lock(), &declared, "fixest").unwrap(),
            ["fixest 1.0 (declared)"]
        );
        assert_eq!(why(&lock(), &declared, "nope"), None);
    }
}
