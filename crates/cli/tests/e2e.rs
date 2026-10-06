//! End-to-end tests of the `rok` binary. They need the network and an installed R, so they are
//! ignored by default; run them with `cargo test -p rok-cli -- --ignored`.

use std::path::Path;
use std::process::{Command, Output};

fn rok(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rok"))
        .args(args)
        .current_dir(home)
        .env("ROK_CACHE_DIR", home.join("cache"))
        .env("ROK_DATA_DIR", home.join("data"))
        .env("NO_COLOR", "1")
        .output()
        .expect("rok runs")
}

fn json(out: &Output) -> serde_json::Value {
    assert!(out.status.success(), "rok failed:\n{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("JSON on stdout")
}

#[test]
#[ignore = "needs the network and R"]
fn init_add_sync_remove() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();

    let init = json(&rok(home, &["init", "proj", "--json"]));
    assert_eq!(init["name"], "proj");
    let proj = home.join("proj");
    for f in ["rok.toml", "rok.lock", ".Rprofile", ".rok/activate.R"] {
        assert!(proj.join(f).is_file(), "{f} was not created");
    }

    let added = json(&rok(home, &["--project", "proj", "add", "R6", "--json"]));
    assert_eq!(added["changes"][0]["name"], "R6");
    let lib = std::fs::read_dir(proj.join(".rok/library")).unwrap().next().unwrap().unwrap().path();
    let lib = std::fs::read_dir(&lib).unwrap().next().unwrap().unwrap().path();
    assert!(lib.join("R6/DESCRIPTION").is_file(), "R6 is not in {}", lib.display());

    // In sync: `--locked` passes. After a hand edit it fails without touching rok.lock.
    assert!(rok(&proj, &["sync", "--locked"]).status.success());
    let lock_before = std::fs::read_to_string(proj.join("rok.lock")).unwrap();
    let mut manifest = std::fs::read_to_string(proj.join("rok.toml")).unwrap();
    manifest.push_str("jsonlite = \"*\"\n");
    std::fs::write(proj.join("rok.toml"), manifest).unwrap();
    let locked = rok(&proj, &["sync", "--locked"]);
    assert!(!locked.status.success());
    assert!(String::from_utf8_lossy(&locked.stderr).contains("out of date"));
    assert_eq!(std::fs::read_to_string(proj.join("rok.lock")).unwrap(), lock_before);
    let synced = json(&rok(&proj, &["sync", "--json"]));
    assert_eq!(synced["changes"][0]["name"], "jsonlite");

    let removed = json(&rok(&proj, &["remove", "R6", "jsonlite", "--json"]));
    assert_eq!(removed["changes"].as_array().unwrap().len(), 2);
    assert!(!lib.join("R6").exists());
    assert!(proj.join(".rok/undo/rok.toml").is_file());

    let not_declared = rok(&proj, &["remove", "R6"]);
    assert!(!not_declared.status.success());
}
