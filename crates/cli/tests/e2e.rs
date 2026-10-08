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
    assert!(
        out.status.success(),
        "rok failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    let lib = std::fs::read_dir(proj.join(".rok/library"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let lib = std::fs::read_dir(&lib)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(
        lib.join("R6/DESCRIPTION").is_file(),
        "R6 is not in {}",
        lib.display()
    );

    // status: in sync (exit 0); why and tree read the lockfile.
    assert!(rok(&proj, &["status", "--check"]).status.success());
    let why = rok(&proj, &["why", "R6"]);
    assert_eq!(
        String::from_utf8_lossy(&why.stdout).trim(),
        "R6 2.6.1 (declared)"
    );
    let tree = rok(&proj, &["tree"]);
    assert!(String::from_utf8_lossy(&tree.stdout).starts_with("R6 "));

    // In sync: `--locked` passes. After a hand edit it fails without touching rok.lock.
    assert!(rok(&proj, &["sync", "--locked"]).status.success());
    let lock_before = std::fs::read_to_string(proj.join("rok.lock")).unwrap();
    let mut manifest = std::fs::read_to_string(proj.join("rok.toml")).unwrap();
    manifest.push_str("jsonlite = \"*\"\n");
    std::fs::write(proj.join("rok.toml"), manifest).unwrap();
    let locked = rok(&proj, &["sync", "--locked"]);
    assert!(!locked.status.success());
    assert!(String::from_utf8_lossy(&locked.stderr).contains("out of date"));
    assert_eq!(
        std::fs::read_to_string(proj.join("rok.lock")).unwrap(),
        lock_before
    );
    let synced = json(&rok(&proj, &["sync", "--json"]));
    assert_eq!(synced["changes"][0]["name"], "jsonlite");

    let removed = json(&rok(&proj, &["remove", "R6", "jsonlite", "--json"]));
    assert_eq!(removed["changes"].as_array().unwrap().len(), 2);
    assert!(!lib.join("R6").exists());
    assert!(proj.join(".rok/undo/rok.toml").is_file());

    let not_declared = rok(&proj, &["remove", "R6"]);
    assert!(!not_declared.status.success());

    // undo restores the state before `remove`.
    let undone = json(&rok(&proj, &["undo", "--json"]));
    assert_eq!(undone["changes"].as_array().unwrap().len(), 2);
    assert!(lib.join("R6/DESCRIPTION").is_file());
    assert!(
        !rok(&proj, &["undo"]).status.success(),
        "a second undo has nothing to undo"
    );
}

#[test]
#[ignore = "needs the network (GitHub, R-multiverse) and R"]
fn github_and_repository_packages() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    json(&rok(home, &["init", "proj", "--json"]));
    let proj = home.join("proj");
    let read = |f: &str| std::fs::read_to_string(proj.join(f)).unwrap();

    // A GitHub package: declared with its repository, locked at a commit, built from source.
    let added = json(&rok(
        &proj,
        &["add", "gaborcsardi/praise", "--yes", "--json"],
    ));
    assert_eq!(added["changes"][0]["name"], "praise");
    assert!(
        added["changes"][0]["note"]
            .as_str()
            .unwrap()
            .starts_with("gaborcsardi/praise@")
    );
    assert!(read("rok.toml").contains(r#"praise = { github = "gaborcsardi/praise" }"#));
    assert!(read("rok.lock").contains(r#"source = { github = "gaborcsardi/praise", commit = ""#));

    // A package from a CRAN-like repository: the URL and its Git origin are locked.
    let mut manifest = read("rok.toml");
    manifest.push_str(
        "rcheology = { repo = \"multiverse\" }\n\n[repositories]\nmultiverse = \"https://community.r-multiverse.org\"\n",
    );
    std::fs::write(proj.join("rok.toml"), manifest).unwrap();
    let synced = json(&rok(&proj, &["sync", "--yes", "--json"]));
    assert_eq!(synced["changes"][0]["name"], "rcheology");
    assert!(read("rok.lock").contains(
        r#"repository = "multiverse", url = "https://community.r-multiverse.org", remote-url = ""#
    ));
    assert!(rok(&proj, &["sync", "--locked"]).status.success());
    assert!(rok(&proj, &["status", "--check"]).status.success());

    // GitHub packages can be named by their repository.
    let removed = json(&rok(&proj, &["remove", "gaborcsardi/praise", "--json"]));
    assert_eq!(removed["changes"][0]["name"], "praise");
}

#[test]
#[ignore = "needs the network and R"]
fn runs_scripts_with_the_project_library() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    json(&rok(home, &["init", "proj", "--json"]));
    let proj = home.join("proj");
    json(&rok(&proj, &["add", "R6", "--json"]));
    std::fs::write(
        proj.join("s.R"),
        "library(R6)\ncat(.libPaths()[1], commandArgs(TRUE), sep = '\\n')\nquit(status = 3)\n",
    )
    .unwrap();

    // The script sees the project library first, gets its arguments, and its exit status
    // becomes rok's.
    let out = rok(&proj, &["run", "s.R", "--flag", "x"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("/.rok/library/R-"), "{stdout}");
    assert!(stdout.contains("--flag\nx"), "{stdout}");

    let list = json(&rok(&proj, &["r", "list", "--json"]));
    assert!(!list["installed"].as_array().unwrap().is_empty());
    assert!(!list["available"].as_array().unwrap().is_empty());
}

#[test]
#[ignore = "needs the network and R"]
fn hands_questions_back_to_programs() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    json(&rok(home, &["init", "proj", "--json"]));
    let proj = home.join("proj");

    // A nested project needs confirmation: with --json and no terminal, rok changes nothing
    // and returns the question with exit status 2 (the R package asks it, then runs again).
    let asked = rok(&proj, &["init", "sub", "--json"]);
    assert_eq!(asked.status.code(), Some(2));
    let needs: serde_json::Value = serde_json::from_slice(&asked.stdout).unwrap();
    assert_eq!(needs["needs"]["id"], "init-nested");
    assert_eq!(needs["needs"]["default"], false);
    assert!(!proj.join("sub/rok.toml").exists());

    let answered = json(&rok(
        &proj,
        &["init", "sub", "--json", "--confirmed", "init-nested"],
    ));
    assert_eq!(answered["name"], "sub");
    assert!(proj.join("sub/rok.toml").is_file());
}

#[test]
#[ignore = "needs the network and R"]
fn activates_projects_at_r_startup() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    json(&rok(home, &["init", "proj", "--json"]));
    let proj = home.join("proj");
    json(&rok(&proj, &["add", "R6", "--json"]));
    let r_home = String::from_utf8(
        Command::new("Rscript")
            .args(["-e", "cat(R.home())"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let activate = |extra: &[&str]| {
        let mut args = vec!["activate", "--r-home", r_home.as_str()];
        args.extend_from_slice(extra);
        rok(&proj, &args)
    };

    // In sync: one line for people, the library for R.
    let out = activate(&["--interactive"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("library=") && stdout.contains("/.rok/library/R-"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("rok: proj (R "));

    // Out of sync in a non-interactive session: a warning, nothing synced (the default).
    let lib = stdout.trim().strip_prefix("library=").unwrap().to_string();
    std::fs::remove_file(std::path::Path::new(&lib).join("R6")).unwrap();
    let out = activate(&[]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not synced"));
    assert!(!std::path::Path::new(&lib).join("R6").exists());

    // Interactive: a light sync happens by itself.
    let out = activate(&["--interactive"]);
    assert!(out.status.success());
    assert!(std::path::Path::new(&lib).join("R6/DESCRIPTION").is_file());

    // Strict mode stops R (exit status 3).
    let mut manifest = std::fs::read_to_string(proj.join("rok.toml")).unwrap();
    manifest.push_str("\n[sync]\nnoninteractive = \"error\"\n");
    std::fs::write(proj.join("rok.toml"), manifest).unwrap();
    assert_eq!(
        activate(&[]).status.code(),
        Some(0),
        "in sync: nothing to stop for"
    );
    std::fs::remove_file(std::path::Path::new(&lib).join("R6")).unwrap();
    assert_eq!(activate(&[]).status.code(), Some(3));
}

#[test]
#[ignore = "needs the network and R"]
fn scans_the_code_for_packages() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    json(&rok(home, &["init", "proj", "--json"]));
    let proj = home.join("proj");
    json(&rok(&proj, &["add", "R6", "--json"]));
    std::fs::create_dir_all(proj.join("code")).unwrap();
    std::fs::write(
        proj.join("code/a.R"),
        "library(R6)\n# library(commented)\np <- ggplot2::ggplot() + geom_sf()\n",
    )
    .unwrap();

    // status: the undeclared packages, with where; nothing about the comment.
    let status = json(&rok(&proj, &["status", "--json"]));
    let undeclared = status["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["code"] == "undeclared")
        .expect("undeclared packages reported");
    let details: Vec<&str> = undeclared["details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert_eq!(
        details,
        [
            "ggplot2 (code/a.R:3)",
            "sf (for geom_sf()/coord_sf() in code/a.R:3)"
        ]
    );

    // why: where the code uses it.
    let why = rok(&proj, &["why", "R6"]);
    assert!(String::from_utf8_lossy(&why.stdout).contains("code/a.R:1"));

    // remove: a package the code uses needs an answer, and the default keeps it.
    let asked = rok(&proj, &["remove", "R6", "--json"]);
    assert_eq!(asked.status.code(), Some(2));
    let needs: serde_json::Value = serde_json::from_slice(&asked.stdout).unwrap();
    assert_eq!(needs["needs"]["id"], "remove-used");
    assert_eq!(needs["needs"]["default"], false);
    assert!(
        std::fs::read_to_string(proj.join("rok.toml"))
            .unwrap()
            .contains("R6")
    );
    json(&rok(
        &proj,
        &["remove", "R6", "--json", "--confirmed", "remove-used"],
    ));
    assert!(
        !std::fs::read_to_string(proj.join("rok.toml"))
            .unwrap()
            .contains("R6")
    );
}
