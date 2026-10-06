//! Core of rok: versions and constraints, DESCRIPTION/PACKAGES parsing, the manifest and
//! lockfile formats, the machine and its R installations, and P3M.

pub mod cache;
pub mod constraint;
pub mod date;
pub mod dcf;
pub mod fsutil;
pub mod http;
pub mod install;
pub mod lockfile;
pub mod manifest;
pub mod ops;
pub mod p3m;
pub mod par;
pub mod paths;
pub mod platform;
pub mod project;
pub mod rdetect;
pub mod resolve;
pub mod rpkgs;
pub mod version;
