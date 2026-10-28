//! Persistent project list (`projects.json` in the config dir).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use eyre::{Result, bail};
use serde::{Deserialize, Serialize};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `$POOH_CONFIG_DIR`, else `$XDG_CONFIG_HOME/pooh`, else `~/.config/pooh`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = env::var_os("POOH_CONFIG_DIR") {
        return dir.into();
    }
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("pooh");
    }
    let home = env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".config").join("pooh")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub bare: bool,
    #[serde(default)]
    pub added_at: u64,
    #[serde(default)]
    pub last_opened_at: u64,
    #[serde(default)]
    pub visits: u32,
}

impl Project {
    pub fn new(name: String, path: PathBuf, bare: bool) -> Self {
        let now = now();
        Self {
            name,
            path,
            tags: Vec::new(),
            bare,
            added_at: now,
            last_opened_at: now,
            visits: 0,
        }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn has_any_tag(&self, tags: &[String]) -> bool {
        tags.is_empty() || tags.iter().any(|t| self.tags.contains(t))
    }

    /// Visit count weighted by how recently it was opened (zoxide-style buckets).
    pub fn frecency(&self, now: u64) -> f64 {
        let age = now.saturating_sub(self.last_opened_at);
        let weight = match age {
            0..3_600 => 4.0,
            3_600..86_400 => 2.0,
            86_400..604_800 => 0.5,
            _ => 0.25,
        };
        (f64::from(self.visits) + 1.0) * weight
    }
}

/// Folder name, minus `.git`; `.bare` and `.git` folders are named after their parent.
pub fn default_name(path: &Path) -> String {
    let leaf = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    let name = match leaf(path) {
        Some(n) if n == ".bare" || n == ".git" => path.parent().and_then(leaf),
        other => other,
    };
    let name = name.unwrap_or_else(|| "unknown".into());
    match name.strip_suffix(".git") {
        Some(stem) if !stem.is_empty() => stem.to_owned(),
        _ => name,
    }
}

/// Names end up in labels (`project:branch`), Rex session names and shell
/// output, so keep them unambiguous and printable.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("project name cannot be empty");
    }
    if name != name.trim() {
        bail!("project name '{name}' has leading or trailing whitespace");
    }
    if name.contains(':') || name.chars().any(char::is_control) {
        bail!("project name '{name}' cannot contain ':' or control characters");
    }
    Ok(())
}

/// Advisory `flock`, released when dropped.
struct FileLock(fs::File);

impl FileLock {
    fn exclusive(path: &Path) -> Result<Self> {
        use std::os::fd::AsRawFd;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        // SAFETY: `file` owns a valid fd for the duration of the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self(file))
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: as above; closing the file would release it too.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    pub projects: Vec<Project>,
}

impl Store {
    pub fn load() -> Result<Self> {
        Self::load_from(config_dir().join("projects.json"))
    }

    pub fn load_from(path: PathBuf) -> Result<Self> {
        let projects = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| eyre::eyre!("corrupt {}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, projects })
    }

    /// Write atomically so a crash or a concurrent reader never sees a partial
    /// file. A symlinked `projects.json` (dotfiles) is written through.
    pub fn save(&self) -> Result<()> {
        let target = fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = target.with_extension(format!("json.{}.tmp", std::process::id()));
        fs::write(&tmp, serde_json::to_string_pretty(&self.projects)?)?;
        fs::rename(&tmp, &target).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })?;
        Ok(())
    }

    /// Load, change and save under an exclusive lock, so concurrent `pooh`
    /// runs (and a picker that has been open for a while) never overwrite
    /// each other's changes.
    pub fn modify<R>(f: impl FnOnce(&mut Store) -> Result<R>) -> Result<R> {
        Self::modify_at(config_dir().join("projects.json"), f)
    }

    pub fn modify_at<R>(path: PathBuf, f: impl FnOnce(&mut Store) -> Result<R>) -> Result<R> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let _lock = FileLock::exclusive(&path.with_extension("json.lock"))?;
        let mut store = Self::load_from(path)?;
        let out = f(&mut store)?;
        store.save()?;
        Ok(out)
    }

    pub fn add(&mut self, project: Project) -> Result<()> {
        validate_name(&project.name)?;
        if self.find(&project.name).is_some() {
            bail!("a project named '{}' already exists", project.name);
        }
        if let Some(p) = self.projects.iter().find(|p| p.path == project.path) {
            bail!("{} is already registered as '{}'", p.path.display(), p.name);
        }
        self.projects.push(project);
        Ok(())
    }

    pub fn find(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.name == name)
    }

    pub fn get(&self, name: &str) -> Result<&Project> {
        self.find(name)
            .ok_or_else(|| eyre::eyre!("project '{name}' not found"))
    }

    pub fn remove(&mut self, name: &str) -> Result<Project> {
        let Some(i) = self.projects.iter().position(|p| p.name == name) else {
            bail!("project '{name}' not found");
        };
        Ok(self.projects.remove(i))
    }

    pub fn touch(&mut self, name: &str) {
        if let Some(p) = self.find_mut(name) {
            p.visits += 1;
            p.last_opened_at = now();
        }
    }

    /// The registered project that owns `dir`: the repository it belongs to
    /// (linked worktrees included) if that is registered, else the deepest
    /// registered ancestor directory.
    pub fn find_containing(&self, dir: &Path) -> Option<&Project> {
        let dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if let Some(root) = crate::git::inspect(&dir).map(|i| i.root())
            && let Some(p) = self.projects.iter().find(|p| p.path == root)
        {
            return Some(p);
        }
        self.projects
            .iter()
            .filter(|p| dir.starts_with(&p.path))
            .max_by_key(|p| p.path.components().count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::load_from(dir.path().join("projects.json")).unwrap();
        (dir, store)
    }

    fn project(name: &str, path: &str) -> Project {
        Project::new(name.into(), path.into(), false)
    }

    #[test]
    fn rejects_duplicate_name_and_path() {
        let (_d, mut s) = store();
        s.add(project("a", "/a")).unwrap();
        assert!(s.add(project("a", "/b")).is_err());
        assert!(s.add(project("b", "/a")).is_err());
    }

    #[test]
    fn roundtrips_through_disk() {
        let (d, mut s) = store();
        s.add(project("a", "/a")).unwrap();
        s.save().unwrap();
        let again = Store::load_from(d.path().join("projects.json")).unwrap();
        assert_eq!(again.projects, s.projects);
    }

    #[test]
    fn deepest_containing_project_wins() {
        let (_d, mut s) = store();
        s.add(project("outer", "/w")).unwrap();
        s.add(project("inner", "/w/inner")).unwrap();
        assert_eq!(
            s.find_containing(Path::new("/w/inner/src")).unwrap().name,
            "inner"
        );
        assert_eq!(
            s.find_containing(Path::new("/w/other")).unwrap().name,
            "outer"
        );
    }

    #[test]
    fn rejects_unusable_names() {
        let (_d, mut s) = store();
        for bad in ["", " a", "a ", "a:b", "a\tb"] {
            assert!(s.add(project(bad, "/x")).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn concurrent_modifications_do_not_lose_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.json");
        std::thread::scope(|scope| {
            for i in 0..16 {
                let path = path.clone();
                scope.spawn(move || {
                    Store::modify_at(path, |s| {
                        s.add(project(&format!("p{i}"), &format!("/p{i}")))
                    })
                    .unwrap();
                });
            }
        });
        assert_eq!(Store::load_from(path).unwrap().projects.len(), 16);
    }

    #[test]
    fn save_writes_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        let link = dir.path().join("projects.json");
        std::fs::write(&real, "[]").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        Store::modify_at(link.clone(), |s| s.add(project("a", "/a"))).unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(std::fs::read_to_string(real).unwrap().contains("\"a\""));
    }

    #[test]
    fn names_from_paths() {
        assert_eq!(default_name(Path::new("/w/proj/.git")), "proj");
        assert_eq!(default_name(Path::new("/w/app")), "app");
        assert_eq!(default_name(Path::new("/w/app.git")), "app");
        assert_eq!(default_name(Path::new("/w/app/.bare")), "app");
    }

    #[test]
    fn recent_visits_rank_higher() {
        let mut p = project("a", "/a");
        p.last_opened_at = 1_000;
        let fresh = p.frecency(1_100);
        let stale = p.frecency(1_000 + 30 * 86_400);
        assert!(fresh > stale);
    }
}
