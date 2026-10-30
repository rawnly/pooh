//! Optional zoxide integration. Every call is best effort: without zoxide on
//! `PATH` (or with `POOH_ZOXIDE=0`) it silently does nothing.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

/// On-disk format version we know how to read.
const DB_VERSION: u32 = 3;

fn disabled() -> bool {
    std::env::var("POOH_ZOXIDE").is_ok_and(|v| matches!(v.as_str(), "0" | "false" | "off"))
}

/// `(score, path)` pairs as zoxide ranks them. Empty when unavailable.
/// Reads zoxide's database directly; falls back to the CLI if that fails.
pub fn list() -> Vec<(f64, PathBuf)> {
    if disabled() {
        return Vec::new();
    }
    if let Some(entries) = read_db() {
        return entries;
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

/// `$_ZO_DATA_DIR/db.zo`, else the platform data dir zoxide itself uses.
pub fn db_path() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("_ZO_DATA_DIR") {
        return Some(PathBuf::from(dir).join("db.zo"));
    }
    let home = PathBuf::from(env::var_os("HOME")?);
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"))
    };
    Some(dir.join("zoxide").join("db.zo"))
}

fn read_db() -> Option<Vec<(f64, PathBuf)>> {
    let bytes = fs::read(db_path()?).ok()?;
    let mut entries = parse_db(&bytes, crate::store::now())?;
    // Like `zoxide query`, hide directories that no longer exist.
    entries.retain(|(_, path)| path.is_dir());
    entries.sort_by(|a, b| b.0.total_cmp(&a.0));
    Some(entries)
}

/// zoxide's `db.zo`: u32 version, then a bincode `Vec<Dir>` (fixed-width little
/// endian ints; `Dir` = path bytes, f64 rank, u64 last accessed). Scores are
/// the stored rank aged the way `zoxide query --score` does.
fn parse_db(bytes: &[u8], now: u64) -> Option<Vec<(f64, PathBuf)>> {
    fn take<'a>(buf: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        let (head, rest) = buf.split_at_checked(n)?;
        *buf = rest;
        Some(head)
    }
    fn u64_le(buf: &mut &[u8]) -> Option<u64> {
        Some(u64::from_le_bytes(take(buf, 8)?.try_into().ok()?))
    }

    let mut buf = bytes;
    if u32::from_le_bytes(take(&mut buf, 4)?.try_into().ok()?) != DB_VERSION {
        return None;
    }
    let count = usize::try_from(u64_le(&mut buf)?).ok()?;
    let mut out = Vec::with_capacity(count.min(buf.len() / 24));
    for _ in 0..count {
        let len = usize::try_from(u64_le(&mut buf)?).ok()?;
        let path = PathBuf::from(OsStr::from_bytes(take(&mut buf, len)?));
        let rank = f64::from_bits(u64_le(&mut buf)?);
        let last = u64_le(&mut buf)?;
        let weight = match now.saturating_sub(last) {
            0..3_600 => 4.0,
            3_600..86_400 => 2.0,
            86_400..604_800 => 0.5,
            _ => 0.25,
        };
        out.push((rank * weight, path));
    }
    buf.is_empty().then_some(out)
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

    fn db_entry(path: &str, rank: f64, last: u64) -> Vec<u8> {
        let mut e = (path.len() as u64).to_le_bytes().to_vec();
        e.extend(path.as_bytes());
        e.extend(rank.to_bits().to_le_bytes());
        e.extend(last.to_le_bytes());
        e
    }

    #[test]
    fn reads_database_and_ages_scores() {
        let now = 10_000_000;
        let mut db = 3u32.to_le_bytes().to_vec();
        db.extend(2u64.to_le_bytes());
        db.extend(db_entry("/a b", 2.0, now - 10));
        db.extend(db_entry("/old", 8.0, now - 1_000_000));
        assert_eq!(
            parse_db(&db, now),
            Some(vec![
                (8.0, PathBuf::from("/a b")),
                (2.0, PathBuf::from("/old"))
            ])
        );
    }

    #[test]
    fn rejects_unknown_or_truncated_database() {
        let mut db = 4u32.to_le_bytes().to_vec();
        db.extend(0u64.to_le_bytes());
        assert_eq!(parse_db(&db, 0), None);
        let mut db = 3u32.to_le_bytes().to_vec();
        db.extend(1u64.to_le_bytes());
        assert_eq!(parse_db(&db, 0), None);
    }

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
