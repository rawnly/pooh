//! Thin wrappers over the `git` CLI. Worktrees are always read live from git,
//! never cached in the project store.

use std::path::{Path, PathBuf};
use std::process::Command;

use eyre::{Result, bail};
use serde::Serialize;

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
}

#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub common_dir: PathBuf,
    /// The repository itself is bare (worktrees live elsewhere).
    pub bare: bool,
    /// `path` is a linked worktree, not the main checkout.
    pub linked: bool,
}

impl RepoInfo {
    /// Directory that should identify the repository in the project list.
    pub fn root(&self) -> PathBuf {
        if !self.bare && self.common_dir.file_name().is_some_and(|n| n == ".git") {
            return self.common_dir.parent().unwrap_or(&self.common_dir).into();
        }
        self.common_dir.clone()
    }
}

/// `None` when `path` is not inside a git repository.
pub fn inspect(path: &Path) -> Option<RepoInfo> {
    let out = git(
        path,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ],
    )
    .ok()?;
    let mut lines = out.lines();
    let git_dir = PathBuf::from(lines.next()?);
    let common_dir = PathBuf::from(lines.next()?);
    let bare = Command::new("git")
        .arg("--git-dir")
        .arg(&common_dir)
        .args(["config", "--bool", "core.bare"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "true");
    Some(RepoInfo {
        linked: git_dir != common_dir,
        common_dir,
        bare,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Worktree {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    /// The main checkout (always the project path for non-bare repos).
    pub main: bool,
}

/// Cheap check, no process spawned: git keeps one `worktrees/<name>` dir per
/// linked worktree. Conservative: `true` whenever the layout is unfamiliar.
pub fn may_have_linked_worktrees(repo: &Path, bare: bool) -> bool {
    let common = if bare {
        repo.to_path_buf()
    } else {
        repo.join(".git")
    };
    if !common.is_dir() {
        return !bare; // `.git` file or missing: let git decide
    }
    std::fs::read_dir(common.join("worktrees")).is_ok_and(|mut d| d.next().is_some())
}

pub fn list_worktrees(repo: &Path) -> Result<Vec<Worktree>> {
    Ok(parse_worktrees(&git(
        repo,
        &["worktree", "list", "--porcelain"],
    )?))
}

fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    for (i, block) in porcelain.split("\n\n").enumerate() {
        let mut path = None;
        let mut branch = None;
        let mut head = None;
        let mut bare = false;
        for line in block.lines() {
            match line.split_once(' ') {
                Some(("worktree", p)) => path = Some(PathBuf::from(p)),
                Some(("branch", b)) => {
                    branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_owned())
                }
                Some(("HEAD", h)) => head = Some(h.chars().take(10).collect()),
                None if line == "bare" => bare = true,
                _ => {}
            }
        }
        let Some(path) = path.filter(|_| !bare) else {
            continue;
        };
        out.push(Worktree {
            name: path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into(),
            ),
            path,
            branch,
            head,
            main: i == 0,
        });
    }
    out
}

fn slug(branch: &str) -> String {
    branch.replace('/', "-")
}

/// Where `worktree add` puts a branch when no explicit path is given.
pub fn default_worktree_path(repo: &Path, bare: bool, branch: &str) -> PathBuf {
    let name = repo
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into());
    let parent = repo.parent().unwrap_or(repo);
    let slug = slug(branch);
    match (bare, name.as_str()) {
        (true, ".bare") => parent.join(slug),
        (true, _) => parent.join(format!("{}-{slug}", name.trim_end_matches(".git"))),
        (false, _) => parent.join(format!("{name}-{slug}")),
    }
}

pub fn add_worktree(repo: &Path, branch: &str, path: &Path, base: Option<&str>) -> Result<()> {
    let exists = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok();
    let path = path.to_string_lossy();
    if exists {
        git(repo, &["worktree", "add", &path, branch])?;
    } else {
        let mut args = vec!["worktree", "add", "-b", branch, &path];
        args.extend(base);
        git(repo, &args)?;
    }
    Ok(())
}

pub fn remove_worktree(repo: &Path, path: &Path, force: bool) -> Result<()> {
    let path = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path);
    git(repo, &args)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_and_skips_bare() {
        let text = "worktree /r/.bare\nbare\n\nworktree /r/main\nHEAD abcdef1234567890\nbranch refs/heads/main\n\nworktree /r/feat\nHEAD 1234567890abcdef\nbranch refs/heads/feat/x\n";
        let wts = parse_worktrees(text);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].name, "main");
        assert_eq!(wts[1].branch.as_deref(), Some("feat/x"));
        assert_eq!(wts[1].head.as_deref(), Some("1234567890"));
    }

    #[test]
    fn default_paths() {
        let p = |r, bare, b| default_worktree_path(Path::new(r), bare, b);
        assert_eq!(p("/w/app", false, "feat/x"), Path::new("/w/app-feat-x"));
        assert_eq!(
            p("/w/app/.bare", true, "feat/x"),
            Path::new("/w/app/feat-x")
        );
        assert_eq!(p("/w/app.git", true, "dev"), Path::new("/w/app-dev"));
    }
}
