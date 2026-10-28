use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use eyre::{Result, bail, eyre};
use serde::Serialize;
use serde_json::{Value, json};

use crate::entries::{self, Entry, Filter};
use crate::store::{Project, Store, default_name};
use crate::{
    AddArgs, Args, Command, ImportArgs, InitArgs, ListArgs, PickArgs, PruneArgs, RemoveArgs,
    ResolveArgs, SkillAction, SkillArgs, TagArgs, WorktreeAction, WorktreeArgs, git, skill, tui,
    zoxide,
};

/// Returns the process exit code.
pub fn run(args: Args) -> Result<u8> {
    let json = args.json;
    let Some(command) = args.command else {
        return Ok(0);
    };
    match command {
        Command::Add(a) => add(json, a),
        Command::List(a) => list(json, a),
        Command::Remove(a) => remove(json, a),
        Command::Tag(a) => tag(json, a),
        Command::Pick(a) => pick(json, a),
        Command::Resolve(a) => resolve(json, a),
        Command::Worktree(a) => worktree(json, a),
        Command::Import(a) => import(json, a),
        Command::Prune(a) => prune(json, a),
        Command::Init(a) => init(a),
        Command::Skill(a) => skill_cmd(json, a),
        Command::Completions(a) => {
            let shell = usage::complete::Shell::from_name(&a.shell)
                .ok_or_else(|| eyre!("unknown shell `{}`", a.shell))?;
            print!("{}", Args::completion_script(shell));
            Ok(0)
        }
        Command::Spec(_) => {
            print!("{}", Args::to_kdl());
            Ok(0)
        }
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let text = if std::io::stdout().is_terminal() {
        serde_json::to_string_pretty(value)?
    } else {
        serde_json::to_string(value)?
    };
    println!("{text}");
    Ok(())
}

/// Tags may be repeated or comma-separated.
fn split_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags.iter().flat_map(|t| t.split(',')).map(str::trim) {
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_owned());
        }
    }
    out
}

fn project_json(p: &Project, worktrees: Option<Vec<git::Worktree>>) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
    v["exists"] = json!(p.exists());
    if let Some(wts) = worktrees {
        v["worktrees"] = json!(wts);
    }
    v
}

fn project_line(p: &Project) -> String {
    let tags = if p.tags.is_empty() {
        String::new()
    } else {
        format!("  [{}]", p.tags.join(", "))
    };
    let flags = match (p.bare, p.exists()) {
        (_, false) => " (missing)",
        (true, _) => " (bare)",
        _ => "",
    };
    format!("{}{flags}  {}{tags}", p.name, p.path.display())
}

/// Normalizes `path` (linked worktrees map back to their main repository)
/// and builds the project to register.
fn project_for(path: &Path, name: Option<String>, tags: Vec<String>) -> Result<Project> {
    let path = fs::canonicalize(path)?;
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    let (root, bare) = match git::inspect(&path) {
        Some(info) if info.bare || info.linked => (info.root(), info.bare),
        _ => (path, false),
    };
    let name = name.unwrap_or_else(|| default_name(&root));
    let mut project = Project::new(name, root, bare);
    project.tags = tags;
    Ok(project)
}

fn add(json: bool, a: AddArgs) -> Result<u8> {
    let tags = split_tags(&a.tag);

    if !a.scan {
        let project = project_for(&a.path, a.name, tags)?;
        Store::modify(|store| store.add(project.clone()))?;
        if json {
            print_json(&project_json(&project, None))?;
        } else {
            println!("added {}", project_line(&project));
        }
        return Ok(0);
    }

    if a.name.is_some() {
        bail!("--name cannot be combined with --scan");
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(&a.path)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && git::is_repo_root(p))
        .collect();
    dirs.sort();

    let (added, skipped) = Store::modify(|store| {
        let (mut added, mut skipped) = (Vec::new(), Vec::new());
        for dir in dirs {
            let result = project_for(&dir, None, tags.clone()).and_then(|p| {
                store.add(p.clone())?;
                Ok(p)
            });
            match result {
                Ok(p) => added.push(p),
                Err(e) => skipped.push(json!({ "path": dir, "reason": e.to_string() })),
            }
        }
        Ok((added, skipped))
    })?;
    if json {
        let added: Vec<Value> = added.iter().map(|p| project_json(p, None)).collect();
        print_json(&json!({ "added": added, "skipped": skipped }))?;
    } else {
        for p in &added {
            println!("added {}", project_line(p));
        }
        for s in &skipped {
            eprintln!(
                "skipped {}: {}",
                s["path"].as_str().unwrap_or("?"),
                s["reason"].as_str().unwrap_or("?")
            );
        }
        println!("{} added, {} skipped", added.len(), skipped.len());
    }
    Ok(0)
}

fn list(json: bool, a: ListArgs) -> Result<u8> {
    let store = Store::load()?;
    let tags = split_tags(&a.tag);
    let now = crate::store::now();
    let mut projects: Vec<&Project> = store
        .projects
        .iter()
        .filter(|p| p.has_any_tag(&tags) && (!a.broken || !p.exists()))
        .collect();
    projects.sort_by(|a, b| b.frecency(now).total_cmp(&a.frecency(now)));

    if json {
        let items: Vec<Value> = projects
            .iter()
            .map(|p| {
                let wts = a
                    .worktrees
                    .then(|| git::list_worktrees(&p.path).unwrap_or_default());
                project_json(p, wts)
            })
            .collect();
        print_json(&items)?;
        return Ok(0);
    }
    for p in projects {
        println!("{}", project_line(p));
        if a.worktrees {
            for wt in git::list_worktrees(&p.path).unwrap_or_default() {
                println!("  {}", worktree_line(&wt));
            }
        }
    }
    Ok(0)
}

fn worktree_line(wt: &git::Worktree) -> String {
    let branch = wt
        .branch
        .as_deref()
        .map_or(String::new(), |b| format!(" [{b}]"));
    format!("{}{branch}  {}", wt.name, wt.path.display())
}

fn remove(json: bool, a: RemoveArgs) -> Result<u8> {
    let project = Store::modify(|store| store.remove(&a.name))?;
    if json {
        print_json(&json!({ "removed": [project] }))?;
    } else {
        println!("removed {}", project.name);
    }
    Ok(0)
}

fn tag(json: bool, a: TagArgs) -> Result<u8> {
    let tags = split_tags(&a.tags);
    let project = Store::modify(|store| {
        let project = store
            .find_mut(&a.project)
            .ok_or_else(|| eyre!("project '{}' not found", a.project))?;
        for t in tags {
            if a.remove {
                project.tags.retain(|x| *x != t);
            } else if !project.tags.contains(&t) {
                project.tags.push(t);
            }
        }
        Ok(project.clone())
    })?;
    if json {
        print_json(&project_json(&project, None))?;
    } else {
        println!("{}", project_line(&project));
    }
    Ok(0)
}

fn filter(tag: &[String], project: Option<String>, no_worktrees: bool) -> Filter {
    Filter {
        tags: split_tags(tag),
        project,
        worktrees: !no_worktrees,
        include_missing: false,
    }
}

/// Records the jump in the project store and in zoxide.
///
/// Best effort: a store that cannot be written must not swallow the answer.
fn visit(entry: &Entry) {
    // Reloaded under the lock: the picker may have been open for a while.
    let saved = Store::modify(|store| {
        store.touch(&entry.project);
        Ok(())
    });
    if let Err(e) = saved {
        eprintln!("pooh: could not record the visit: {e}");
    }
    zoxide::add(&entry.path);
}

fn print_entry(json: bool, entry: &Entry) -> Result<()> {
    if json {
        print_json(entry)
    } else {
        println!("{}", entry.path.display());
        Ok(())
    }
}

fn require_project(store: &Store, name: Option<&str>) -> Result<()> {
    name.map_or(Ok(()), |n| store.get(n).map(|_| ()))
}

fn pick(json: bool, a: PickArgs) -> Result<u8> {
    let store = Store::load()?;
    require_project(&store, a.project.as_deref())?;
    let entries = entries::build(&store, &filter(&a.tag, a.project, a.no_worktrees));
    if store.projects.is_empty() {
        bail!("no projects to pick from (add one with `pooh add`)");
    }
    if entries.is_empty() {
        // The store has projects; the filters just excluded all of them.
        if json {
            print_json(&json!({ "error": "no projects match the filters" }))?;
        } else {
            eprintln!("pooh: no projects match the filters");
        }
        return Ok(1);
    }
    let Some(tty) = tui::open_tty() else {
        bail!("pick needs a controlling terminal; use `resolve` or `list --json` instead");
    };
    // Tag edits are written as they happen, reloaded under the lock.
    let mut save_tags = |project: &str, tags: &[String]| {
        Store::modify(|store| {
            if let Some(p) = store.find_mut(project) {
                p.tags = tags.to_vec();
            }
            Ok(())
        })
    };
    let options = tui::Options {
        full_screen: a.full_screen,
        vim: a.vim,
    };
    let query = a.query.as_deref().unwrap_or("");
    let entry = match tui::run(tty, entries, query, &mut save_tags, options)? {
        tui::Outcome::Picked(entry) => *entry,
        tui::Outcome::Interrupted => bail!("interrupted"),
        tui::Outcome::Cancelled => {
            if json {
                print_json(&json!({ "cancelled": true }))?;
            }
            return Ok(1);
        }
        tui::Outcome::Unsupported => {
            bail!("the terminal did not report its cursor position; try `pick --full-screen`")
        }
    };
    visit(&entry);
    print_entry(json, &entry)?;
    Ok(0)
}

fn resolve(json: bool, a: ResolveArgs) -> Result<u8> {
    let store = Store::load()?;
    require_project(&store, a.project.as_deref())?;
    let entries = entries::build(&store, &filter(&a.tag, a.project, a.no_worktrees));
    let Some(entry) = entries::best_match(&entries, &a.query).cloned() else {
        if json {
            print_json(&json!({ "error": format!("no match for '{}'", a.query) }))?;
        } else {
            eprintln!("pooh: no match for '{}'", a.query);
        }
        return Ok(1);
    };
    visit(&entry);
    print_entry(json, &entry)?;
    Ok(0)
}

fn project_arg<'a>(store: &'a Store, name: Option<&str>) -> Result<&'a Project> {
    match name {
        Some(n) => store.get(n),
        None => {
            let cwd = std::env::current_dir()?;
            store.find_containing(&cwd).ok_or_else(|| {
                eyre!(
                    "{} is not inside a registered project; pass --project",
                    cwd.display()
                )
            })
        }
    }
}

/// Pick the worktree a user means by `target`: exact branch, then path, then folder
/// name. Ambiguity at the winning level is an error rather than a guess.
fn find_worktree<'a>(wts: &'a [git::Worktree], target: &str) -> Result<&'a git::Worktree> {
    let path = fs::canonicalize(target).ok();
    let linked = || wts.iter().filter(|wt| !wt.main);
    let levels: [Vec<&git::Worktree>; 3] = [
        linked()
            .filter(|wt| wt.branch.as_deref() == Some(target))
            .collect(),
        linked()
            .filter(|wt| {
                path.as_deref()
                    .is_some_and(|p| fs::canonicalize(&wt.path).is_ok_and(|w| w == p))
            })
            .collect(),
        linked().filter(|wt| wt.name == target).collect(),
    ];
    for hits in levels {
        match hits.as_slice() {
            [] => {}
            [one] => return Ok(one),
            many => bail!(
                "'{target}' matches several worktrees: {}; use the path",
                many.iter()
                    .map(|w| w.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    bail!("no linked worktree matches '{target}'")
}

/// `base`, `base-2`, `base-3`, ... until it does not exist.
fn unused_path(base: PathBuf) -> PathBuf {
    if !base.exists() {
        return base;
    }
    let name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (2..)
        .map(|n| base.with_file_name(format!("{name}-{n}")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

fn worktree(json: bool, a: WorktreeArgs) -> Result<u8> {
    let store = Store::load()?;
    match a.action {
        WorktreeAction::List(l) => {
            let project = project_arg(&store, l.project.as_deref())?;
            let wts = git::list_worktrees(&project.path)?;
            if json {
                print_json(&json!({ "project": project.name, "worktrees": wts }))?;
            } else {
                for wt in &wts {
                    println!("{}", worktree_line(wt));
                }
            }
        }
        WorktreeAction::Add(w) => {
            let project = project_arg(&store, w.project.as_deref())?;
            let path = match w.path {
                Some(p) => std::path::absolute(&p)?,
                None => unused_path(git::default_worktree_path(
                    &project.path,
                    project.bare,
                    &w.branch,
                )),
            };
            let branch = git::add_worktree(&project.path, &w.branch, &path, w.base.as_deref())?;
            // Report the resolved path so it can be fed back to `worktree remove`.
            let path = fs::canonicalize(&path).unwrap_or(path);
            zoxide::add(&path);
            if json {
                print_json(&json!({ "project": project.name, "branch": branch, "path": path }))?;
            } else {
                println!("{}", path.display());
            }
        }
        WorktreeAction::Remove(w) => {
            let project = project_arg(&store, w.project.as_deref())?;
            let wts = git::list_worktrees(&project.path)?;
            let wt = find_worktree(&wts, &w.target)?.clone();
            git::remove_worktree(&project.path, &wt.path, w.force)?;
            if json {
                print_json(&json!({ "project": project.name, "removed": wt }))?;
            } else {
                println!("removed {}", worktree_line(&wt));
            }
        }
    }
    Ok(0)
}

/// Why `import` passed over a zoxide entry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Skip {
    Registered,
    Missing,
    LowScore,
    NotGit,
    Subdirectory,
    LinkedWorktree,
}

impl Skip {
    fn key(self) -> &'static str {
        match self {
            Skip::Registered => "already_registered",
            Skip::Missing => "missing",
            Skip::LowScore => "below_min_score",
            Skip::NotGit => "not_git",
            Skip::Subdirectory => "inside_a_repository",
            Skip::LinkedWorktree => "linked_worktree",
        }
    }
}

fn skip_reason(
    path: &Path,
    score: f64,
    known: &std::collections::HashSet<PathBuf>,
    a: &ImportArgs,
) -> Option<Skip> {
    if known.contains(path) {
        return Some(Skip::Registered);
    }
    if !path.is_dir() {
        return Some(Skip::Missing);
    }
    if a.min_score.is_some_and(|min| score < min) {
        return Some(Skip::LowScore);
    }
    match git::inspect(path) {
        // Linked worktrees belong to a repository we already list (or will).
        Some(info) if info.linked => Some(Skip::LinkedWorktree),
        Some(info) if !info.bare && git::toplevel(path).is_none_or(|top| top != path) => {
            Some(Skip::Subdirectory)
        }
        Some(_) => None,
        None => (!a.all).then_some(Skip::NotGit),
    }
}

/// `name`, else `<parent>-name`, else `name-2`, `name-3`, ...
fn unique_name(store: &Store, name: &str, path: &Path) -> String {
    if store.find(name).is_none() {
        return name.to_owned();
    }
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned());
    let candidates = parent.map(|p| format!("{p}-{name}")).into_iter();
    let numbered = (2..).map(|n| format!("{name}-{n}"));
    candidates
        .chain(numbered)
        .find(|c| store.find(c).is_none())
        .expect("unbounded range")
}

fn import(json: bool, a: ImportArgs) -> Result<u8> {
    // `source` is restricted to "zoxide" by the spec.
    let entries = zoxide::list();
    let run =
        |store: &mut Store| -> Result<(Vec<Project>, std::collections::BTreeMap<Skip, usize>)> {
            let known: std::collections::HashSet<PathBuf> =
                store.projects.iter().map(|p| p.path.clone()).collect();
            let mut added = Vec::new();
            let mut skipped: std::collections::BTreeMap<Skip, usize> = Default::default();
            for (score, path) in &entries {
                // zoxide keeps logical paths (symlinks, /tmp); projects use real ones.
                let Ok(path) = fs::canonicalize(path) else {
                    *skipped.entry(Skip::Missing).or_default() += 1;
                    continue;
                };
                if let Some(reason) = skip_reason(&path, *score, &known, &a) {
                    *skipped.entry(reason).or_default() += 1;
                    continue;
                }
                let mut project = project_for(&path, None, vec!["zoxide".into()])?;
                project.name = unique_name(store, &project.name, &project.path);
                if store.add(project.clone()).is_ok() {
                    added.push(project);
                } else {
                    // Normalizes to a project we already have (e.g. a second path to one repo).
                    *skipped.entry(Skip::Registered).or_default() += 1;
                }
            }
            Ok((added, skipped))
        };
    let (added, skipped) = if a.dry_run {
        run(&mut Store::load()?)?
    } else {
        Store::modify(run)?
    };
    let total: usize = skipped.values().sum();
    if json {
        let items: Vec<Value> = added.iter().map(|p| project_json(p, None)).collect();
        let reasons: serde_json::Map<String, Value> = skipped
            .iter()
            .map(|(k, n)| (k.key().to_owned(), json!(n)))
            .collect();
        print_json(&json!({
            "dry_run": a.dry_run,
            "added": items,
            "skipped": total,
            "skipped_by_reason": reasons,
        }))?;
    } else {
        let verb = if a.dry_run { "would add" } else { "added" };
        for p in &added {
            println!("{verb} {}", project_line(p));
        }
        println!("{} {verb}, {total} skipped", added.len());
        for (reason, n) in &skipped {
            println!("  {n:>4}  {}", reason.key());
        }
    }
    Ok(0)
}

fn prune(json: bool, a: PruneArgs) -> Result<u8> {
    let find_gone = |store: &mut Store| {
        let (gone, kept): (Vec<_>, Vec<_>) = store.projects.drain(..).partition(|p| !p.exists());
        store.projects = if a.dry_run {
            gone.iter().cloned().chain(kept).collect()
        } else {
            kept
        };
        Ok(gone)
    };
    let gone = if a.dry_run {
        find_gone(&mut Store::load()?)?
    } else {
        Store::modify(find_gone)?
    };
    if json {
        print_json(&json!({ "dry_run": a.dry_run, "removed": gone }))?;
    } else {
        let verb = if a.dry_run { "would remove" } else { "removed" };
        for p in &gone {
            println!("{verb} {}", project_line(p));
        }
        println!("{} {verb}", gone.len());
    }
    Ok(0)
}

fn init(a: InitArgs) -> Result<u8> {
    let valid = |n: &str| {
        !n.is_empty()
            && n.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    };
    for name in a.alias.iter().chain([&a.function]) {
        if !valid(name) {
            bail!("'{name}' is not a valid shell command name");
        }
    }
    let f = &a.function;
    let mut out = match a.shell.as_str() {
        "fish" => format!(
            "function {f}\n    set -l dir (command pooh pick $argv)\n    and test -n \"$dir\"\n    and cd $dir\nend"
        ),
        _ => format!(
            "{f}() {{\n  local dir\n  dir=$(command pooh pick \"$@\") || return\n  [ -n \"$dir\" ] && cd \"$dir\"\n}}"
        ),
    };
    // `po` is the short binary: complete it like `pooh`.
    out.push_str(match a.shell.as_str() {
        "fish" => "\ncomplete -c po -w pooh",
        "zsh" => "\n(( $+functions[compdef] )) && compdef po=pooh",
        _ => "\ncomplete -p pooh >/dev/null 2>&1 && eval \"$(complete -p pooh | sed 's/ pooh$/ po/')\"",
    });
    if let Some(alias) = &a.alias {
        out.push_str(&match a.shell.as_str() {
            "fish" => format!("\nalias {alias} pooh\ncomplete -c {alias} -w pooh"),
            "zsh" => format!("\nalias {alias}=pooh\n(( $+functions[compdef] )) && compdef {alias}=pooh"),
            _ => format!("\nalias {alias}=pooh\ncomplete -p pooh >/dev/null 2>&1 && eval \"$(complete -p pooh | sed 's/ pooh$/ {alias}/')\""),
        });
    }
    println!("{out}");
    Ok(0)
}

fn skill_cmd(json: bool, a: SkillArgs) -> Result<u8> {
    match a.action {
        SkillAction::Show(_) => print!("{}", skill::SKILL),
        SkillAction::Install(i) => {
            let dir = match i.dir {
                Some(d) => d,
                None => skill::default_dir(i.claude)?,
            };
            let file = skill::install(dir)?;
            if json {
                print_json(&json!({ "installed": file }))?;
            } else {
                println!("installed {}", file.display());
            }
        }
    }
    Ok(0)
}
