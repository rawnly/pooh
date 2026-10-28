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

/// Folder name, minus `.git` and with `.bare` replaced by its parent.
pub fn default_name(path: &Path) -> String {
    let leaf = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    let name = match leaf(path) {
        Some(n) if n == ".bare" => path.parent().and_then(leaf),
        other => other,
    };
    let name = name.unwrap_or_else(|| "unknown".into());
    name.strip_suffix(".git")
        .map_or(name.clone(), str::to_owned)
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

    /// Write atomically so a crash or a concurrent reader never sees a partial file.
    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(&self.projects)?)?;
        fs::rename(tmp, &self.path)?;
        Ok(())
    }

    pub fn add(&mut self, project: Project) -> Result<()> {
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

    /// The registered project that contains `dir` (the deepest match wins).
    /// Linked worktrees of a project count as inside it.
    pub fn find_containing(&self, dir: &Path) -> Option<&Project> {
        let direct = self
            .projects
            .iter()
            .filter(|p| dir.starts_with(&p.path))
            .max_by_key(|p| p.path.components().count());
        direct.or_else(|| {
            let root = crate::git::inspect(dir)?.root();
            self.projects.iter().find(|p| p.path == root)
        })
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
    fn names_from_paths() {
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
