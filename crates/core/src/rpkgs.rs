//! Packages that come with R itself.

/// Base packages: part of every R installation and never installed from a repository.
pub const BASE_PACKAGES: [&str; 14] = [
    "base",
    "compiler",
    "datasets",
    "graphics",
    "grDevices",
    "grid",
    "methods",
    "parallel",
    "splines",
    "stats",
    "stats4",
    "tcltk",
    "tools",
    "utils",
];

/// Recommended packages: shipped with R, but also released on CRAN.
pub const RECOMMENDED_PACKAGES: [&str; 15] = [
    "boot",
    "class",
    "cluster",
    "codetools",
    "foreign",
    "KernSmooth",
    "lattice",
    "MASS",
    "Matrix",
    "mgcv",
    "nlme",
    "nnet",
    "rpart",
    "spatial",
    "survival",
];

/// Whether `name` is a base package (or `R` itself, as it appears in `Depends`).
pub fn is_base(name: &str) -> bool {
    name == "R" || BASE_PACKAGES.contains(&name)
}

/// Whether `name` is a recommended package.
pub fn is_recommended(name: &str) -> bool {
    RECOMMENDED_PACKAGES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_packages() {
        assert!(is_base("R"));
        assert!(is_base("grDevices"));
        assert!(!is_base("Matrix"));
        assert!(is_recommended("Matrix"));
        assert!(!is_recommended("fixest"));
    }
}
