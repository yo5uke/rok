//! Core of rok: versions and constraints, DESCRIPTION/PACKAGES parsing, the manifest and
//! lockfile formats, the machine and its R installations, and P3M.

pub mod cache;
pub mod constraint;
pub mod date;
pub mod dcf;
pub mod fsutil;
pub mod github;
pub mod graph;
pub mod http;
pub mod install;
pub mod lockfile;
pub mod manifest;
pub mod ops;
pub mod p3m;
pub mod par;
pub mod paths;
pub mod pin;
pub mod platform;
pub mod project;
pub mod rdetect;
pub mod renv;
pub mod repo;
pub mod resolve;
pub mod rpkgs;
pub mod scan;
pub mod status;
pub mod syslibs;
pub mod version;
