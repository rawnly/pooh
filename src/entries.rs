//! Pickable targets: every project plus its linked worktrees, ranked.

use std::path::PathBuf;

use serde::Serialize;

use crate::store::{Store, now};
use crate::{git, zoxide};

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeRef {
    pub name: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub label: String,
    pub project: String,
    /// Directory to jump to (the worktree path when `worktree` is set).
    pub path: PathBuf,
    pub project_path: PathBuf,
    pub tags: Vec<String>,
    pub worktree: Option<WorktreeRef>,
    #[serde(skip)]
    pub rank: f64,
}

impl Entry {
    pub fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.label,
            self.tags.join(" "),
            self.path.display()
        )
    }
}

#[derive(Default)]
pub struct Filter {
    pub tags: Vec<String>,
    pub project: Option<String>,
    pub worktrees: bool,
    pub include_missing: bool,
}

pub fn build(store: &Store, filter: &Filter) -> Vec<Entry> {
    let zoxide = zoxide::scores();
    let now = now();
    let mut entries = Vec::new();

    for p in &store.projects {
        if !p.has_any_tag(&filter.tags)
            || filter.project.as_deref().is_some_and(|n| n != p.name)
            || !(filter.include_missing || p.exists())
        {
            continue;
        }
        let base = p.frecency(now);
        let rank = |path: &PathBuf| base + zoxide.get(path).copied().unwrap_or(0.0);
        let worktrees =
            if filter.worktrees && p.exists() && git::may_have_linked_worktrees(&p.path, p.bare) {
                git::list_worktrees(&p.path).unwrap_or_default()
            } else {
                Vec::new()
            };

        // A bare repo has no checkout of its own to jump to.
        if !p.bare || worktrees.is_empty() {
            entries.push(Entry {
                label: p.name.clone(),
                project: p.name.clone(),
                path: p.path.clone(),
                project_path: p.path.clone(),
                tags: p.tags.clone(),
                worktree: None,
                rank: rank(&p.path) + 0.5,
            });
        }
        for wt in worktrees.into_iter().filter(|w| w.path != p.path) {
            let shown = wt.branch.clone().unwrap_or_else(|| wt.name.clone());
            entries.push(Entry {
                label: format!("{}:{shown}", p.name),
                project: p.name.clone(),
                rank: rank(&wt.path),
                path: wt.path,
                project_path: p.path.clone(),
                tags: p.tags.clone(),
                worktree: Some(WorktreeRef {
                    name: wt.name,
                    branch: wt.branch,
                }),
            });
        }
    }
    entries.sort_by(|a, b| {
        b.rank
            .total_cmp(&a.rank)
            .then_with(|| a.label.cmp(&b.label))
    });
    entries
}

/// Best entry for a free-form query: exact label first, then fuzzy score, then rank.
pub fn best_match<'a>(entries: &'a [Entry], query: &str) -> Option<&'a Entry> {
    entries
        .iter()
        .filter_map(|e| {
            let fuzzy = crate::fuzzy::score(query, &e.search_text())?;
            let branch = e.worktree.as_ref().and_then(|w| w.branch.as_deref());
            let exact = if e.label == query || branch == Some(query) {
                10_000
            } else {
                0
            };
            Some((e, fuzzy + exact))
        })
        // `max_by_key` keeps the last of equal keys; entries are rank-sorted, so keep the first.
        .fold(None, |best: Option<(&Entry, i64)>, cur| match best {
            Some(b) if b.1 >= cur.1 => Some(b),
            _ => Some(cur),
        })
        .map(|(e, _)| e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, rank: f64) -> Entry {
        Entry {
            label: label.into(),
            project: label.into(),
            path: format!("/{label}").into(),
            project_path: format!("/{label}").into(),
            tags: vec![],
            worktree: None,
            rank,
        }
    }

    #[test]
    fn exact_label_beats_fuzzy() {
        let entries = [entry("api-gateway", 9.0), entry("api", 1.0)];
        assert_eq!(best_match(&entries, "api").unwrap().label, "api");
    }

    #[test]
    fn exact_branch_beats_a_fuzzier_project_name() {
        let mut wt = entry("app:feat", 1.0);
        wt.worktree = Some(WorktreeRef {
            name: "app-feat".into(),
            branch: Some("feat".into()),
        });
        let entries = [entry("feature-flags", 9.0), wt];
        assert_eq!(best_match(&entries, "feat").unwrap().label, "app:feat");
    }

    #[test]
    fn ties_go_to_the_higher_ranked() {
        let entries = [entry("web-a", 9.0), entry("web-b", 1.0)];
        assert_eq!(best_match(&entries, "web").unwrap().label, "web-a");
    }

    #[test]
    fn no_match_is_none() {
        assert!(best_match(&[entry("a", 1.0)], "zzz").is_none());
    }
}
