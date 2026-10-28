use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

struct Env {
    root: TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let p = self.root.path().join(name);
        std::fs::create_dir_all(&p).unwrap();
        p.canonicalize().unwrap()
    }

    fn pooh(&self, cwd: &Path, args: &[&str]) -> (i32, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_pooh"))
            .args(args)
            .current_dir(cwd)
            .env("POOH_CONFIG_DIR", self.root.path().join("config"))
            .env("POOH_ZOXIDE", "0")
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8(out.stdout).unwrap(),
        )
    }

    fn json(&self, cwd: &Path, args: &[&str]) -> Value {
        let (code, out) = self.pooh(cwd, args);
        assert!(code == 0 || code == 1, "{args:?} exited {code}: {out}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed");
}

fn repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
}

#[test]
fn add_list_tag_remove() {
    let e = Env::new();
    let app = e.dir("app");
    repo(&app);

    let added = e.json(&app, &["add", "--json", "-t", "work,rust"]);
    assert_eq!(added["name"], "app");
    assert_eq!(added["tags"], serde_json::json!(["work", "rust"]));
    assert_eq!(e.pooh(&app, &["add", "--json"]).0, 2, "duplicate add fails");

    e.json(&app, &["tag", "app", "extra", "--json"]);
    e.json(&app, &["tag", "app", "work", "--remove", "--json"]);
    let list = e.json(&app, &["list", "--json", "--tag", "extra"]);
    assert_eq!(list[0]["tags"], serde_json::json!(["rust", "extra"]));
    assert_eq!(
        e.json(&app, &["list", "--json", "-t", "nope"]),
        serde_json::json!([])
    );

    e.json(&app, &["rm", "app", "--json"]);
    assert_eq!(e.json(&app, &["list", "--json"]), serde_json::json!([]));
}

#[test]
fn errors_are_json_in_json_mode() {
    let e = Env::new();
    let (code, out) = e.pooh(e.root.path(), &["remove", "ghost", "--json"]);
    assert_eq!(code, 2);
    assert!(serde_json::from_str::<Value>(&out).unwrap()["error"].is_string());
}

#[test]
fn worktrees_resolve_and_remove() {
    let e = Env::new();
    let app = e.dir("app");
    repo(&app);
    e.json(&app, &["add", "--json"]);

    let wt = e.json(&app, &["wt", "add", "feat/x", "--json"]);
    let wt_path = PathBuf::from(wt["path"].as_str().unwrap());
    assert!(wt_path.ends_with("app-feat-x") && wt_path.exists());

    let listed = e.json(&app, &["worktree", "list", "--json"]);
    assert_eq!(listed["worktrees"].as_array().unwrap().len(), 2);

    let hit = e.json(&app, &["resolve", "feat", "--json"]);
    assert_eq!(hit["label"], "app:feat/x");
    assert_eq!(hit["worktree"]["branch"], "feat/x");
    assert_eq!(hit["project_path"], app.to_str().unwrap());

    let root = e.json(&app, &["resolve", "app", "--no-worktrees", "--json"]);
    assert!(root["worktree"].is_null());

    // Running from inside the worktree still targets the same project.
    let inside = e.json(&wt_path, &["worktree", "list", "--json"]);
    assert_eq!(inside["project"], "app");

    e.json(&app, &["worktree", "remove", "feat/x", "--json"]);
    assert!(!wt_path.exists());
    assert_eq!(e.pooh(&app, &["resolve", "zzzz", "--json"]).0, 1);
}

#[test]
fn adding_a_linked_worktree_registers_the_main_repo() {
    let e = Env::new();
    let app = e.dir("app");
    repo(&app);
    let wt = e.dir("app-wt");
    std::fs::remove_dir(&wt).unwrap();
    git(
        &app,
        &["worktree", "add", "-q", "-b", "b", wt.to_str().unwrap()],
    );

    let added = e.json(&wt, &["add", "--json"]);
    assert_eq!(added["name"], "app");
    assert_eq!(added["path"], app.to_str().unwrap());
}

#[test]
fn bare_repo_layout() {
    let e = Env::new();
    let src = e.dir("src");
    repo(&src);
    let proj = e.dir("proj");
    let bare = proj.join(".bare");
    git(
        &proj,
        &[
            "clone",
            "-q",
            "--bare",
            src.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );

    let added = e.json(&proj, &["add", bare.to_str().unwrap(), "--json"]);
    assert_eq!(added["name"], "proj");
    assert_eq!(added["bare"], true);

    let wt = e.json(&proj, &["wt", "add", "main", "-p", "proj", "--json"]);
    assert_eq!(wt["path"], proj.join("main").to_str().unwrap());

    // The bare root is not a jump target while worktrees exist.
    let hit = e.json(&proj, &["resolve", "proj", "--json"]);
    assert_eq!(hit["label"], "proj:main");
}

#[test]
fn scan_registers_child_repos() {
    let e = Env::new();
    let code = e.dir("code");
    for name in ["one", "two"] {
        repo(&e.dir(&format!("code/{name}")));
    }
    e.dir("code/plain");
    let out = e.json(&code, &["add", ".", "--scan", "--json"]);
    assert_eq!(out["added"].as_array().unwrap().len(), 2);
}

#[test]
fn prune_drops_missing_directories() {
    let e = Env::new();
    let gone = e.dir("gone");
    e.json(&gone, &["add", "--json"]);
    std::fs::remove_dir(&gone).unwrap();
    let cwd = e.root.path().to_path_buf();

    let dry = e.json(&cwd, &["prune", "--dry-run", "--json"]);
    assert_eq!(dry["removed"].as_array().unwrap().len(), 1);
    assert_eq!(
        e.json(&cwd, &["list", "--json"]).as_array().unwrap().len(),
        1
    );
    e.json(&cwd, &["prune", "--json"]);
    assert_eq!(e.json(&cwd, &["list", "--json"]), serde_json::json!([]));
}

#[test]
fn skill_installs_and_spec_prints() {
    let e = Env::new();
    let skills = e.root.path().join("skills");
    let out = e.json(
        e.root.path(),
        &[
            "skill",
            "install",
            "--dir",
            skills.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(Path::new(out["installed"].as_str().unwrap()).exists());
    assert!(
        e.pooh(e.root.path(), &["skill", "show"])
            .1
            .contains("name: pooh")
    );
    assert!(e.pooh(e.root.path(), &["spec"]).1.contains("bin pooh"));
    assert!(e.pooh(e.root.path(), &["init", "zsh"]).1.contains("pj()"));
}
