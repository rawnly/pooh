//! Optional zoxide integration. Every call is best effort: without zoxide on
//! `PATH` (or with `POOH_ZOXIDE=0`) it silently does nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn disabled() -> bool {
    std::env::var("POOH_ZOXIDE").is_ok_and(|v| matches!(v.as_str(), "0" | "false" | "off"))
}

/// `(score, path)` pairs as zoxide ranks them. Empty when unavailable.
pub fn list() -> Vec<(f64, PathBuf)> {
    if disabled() {
        return Vec::new();
    }
    let Ok(out) = Command::new("zoxide")
        .args(["query", "--list", "--score"])
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_list(&String::from_utf8_lossy(&out.stdout))
}

fn parse_list(text: &str) -> Vec<(f64, PathBuf)> {
    text.lines()
        .filter_map(|line| {
            let (score, path) = line.trim_start().split_once(char::is_whitespace)?;
            Some((score.parse().ok()?, PathBuf::from(path.trim_start())))
        })
        .collect()
}

pub fn scores() -> HashMap<PathBuf, f64> {
    list().into_iter().map(|(s, p)| (p, s)).collect()
}

/// Teach zoxide about a directory we just jumped to.
pub fn add(path: &Path) {
    if disabled() {
        return;
    }
    let _ = Command::new("zoxide")
        .arg("add")
        .arg("--")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scored_listing() {
        let parsed = parse_list("  12.5 /home/me/a b\n 3.0 /tmp/x\nnot a line\n");
        assert_eq!(
            parsed,
            vec![
                (12.5, PathBuf::from("/home/me/a b")),
                (3.0, PathBuf::from("/tmp/x"))
            ]
        );
    }
}
