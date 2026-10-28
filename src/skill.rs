//! The agent skill, embedded so `pooh skill` works from any install.

use std::path::PathBuf;
use std::{env, fs};

use eyre::{Result, eyre};

pub const SKILL: &str = include_str!("../skills/pooh/SKILL.md");

fn home() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| eyre!("$HOME is not set"))
}

pub fn default_dir(claude: bool) -> Result<PathBuf> {
    Ok(home()?.join(if claude {
        ".claude/skills"
    } else {
        ".agents/skills"
    }))
}

/// Writes `<dir>/pooh/SKILL.md` and returns its path.
pub fn install(dir: PathBuf) -> Result<PathBuf> {
    let target = dir.join("pooh");
    fs::create_dir_all(&target)?;
    let file = target.join("SKILL.md");
    fs::write(&file, SKILL)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_frontmatter() {
        assert!(SKILL.starts_with("---\nname: pooh\n"));
    }

    #[test]
    fn installs_into_named_folder() {
        let dir = tempfile::tempdir().unwrap();
        let file = install(dir.path().to_path_buf()).unwrap();
        assert_eq!(file, dir.path().join("pooh/SKILL.md"));
        assert_eq!(fs::read_to_string(file).unwrap(), SKILL);
    }
}
