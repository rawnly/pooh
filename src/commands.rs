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
    ResolveArgs, SkillAction, SkillArgs, TagArgs, WorktreeAction, WorktreeArgs, git, picker, skill,
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
    let mut store = Store::load()?;

    if !a.scan {
        let project = project_for(&a.path, a.name, tags)?;
        store.add(project.clone())?;
        store.save()?;
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
        .filter(|p| p.is_dir() && git::inspect(p).is_some())
        .collect();
    dirs.sort();

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
    store.save()?;
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
    let mut store = Store::load()?;
    let project = store.remove(&a.name)?;
    store.save()?;
    if json {
        print_json(&json!({ "removed": [project] }))?;
    } else {
        println!("removed {}", project.name);
    }
    Ok(0)
}

fn tag(json: bool, a: TagArgs) -> Result<u8> {
    let mut store = Store::load()?;
    let tags = split_tags(&a.tags);
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
    let project = project.clone();
    store.save()?;
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
fn visit(store: &mut Store, entry: &Entry) -> Result<()> {
    store.touch(&entry.project);
    store.save()?;
    zoxide::add(&entry.path);
    Ok(())
}

fn print_entry(json: bool, entry: &Entry) -> Result<()> {
    if json {
        print_json(entry)
    } else {
        println!("{}", entry.path.display());
        Ok(())
    }
}

fn pick(json: bool, a: PickArgs) -> Result<u8> {
    let mut store = Store::load()?;
    let entries = entries::build(&store, &filter(&a.tag, a.project, a.no_worktrees));
    if entries.is_empty() {
        bail!("no projects to pick from (add one with `pooh add`)");
    }
    let Some(entry) = picker::pick(entries, a.query.as_deref())? else {
        return Ok(1);
    };
    visit(&mut store, &entry)?;
    print_entry(json, &entry)?;
    Ok(0)
}

fn resolve(json: bool, a: ResolveArgs) -> Result<u8> {
    let mut store = Store::load()?;
    let entries = entries::build(&store, &filter(&a.tag, a.project, a.no_worktrees));
    let Some(entry) = entries::best_match(&entries, &a.query).cloned() else {
        if json {
            print_json(&json!({ "error": format!("no match for '{}'", a.query) }))?;
        } else {
            eprintln!("pooh: no match for '{}'", a.query);
        }
        return Ok(1);
    };
    visit(&mut store, &entry)?;
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
            let path = w
                .path
                .map(|p| std::path::absolute(&p))
                .transpose()?
                .unwrap_or_else(|| {
                    git::default_worktree_path(&project.path, project.bare, &w.branch)
                });
            git::add_worktree(&project.path, &w.branch, &path, w.base.as_deref())?;
            zoxide::add(&path);
            if json {
                print_json(&json!({ "project": project.name, "branch": w.branch, "path": path }))?;
            } else {
                println!("{}", path.display());
            }
        }
        WorktreeAction::Remove(w) => {
            let project = project_arg(&store, w.project.as_deref())?;
            let target = Path::new(&w.target);
            let wt = git::list_worktrees(&project.path)?
                .into_iter()
                .find(|wt| {
                    !wt.main
                        && (wt.branch.as_deref() == Some(&w.target)
                            || wt.name == w.target
                            || wt.path == target)
                })
                .ok_or_else(|| eyre!("no linked worktree matches '{}'", w.target))?;
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

fn import(json: bool, a: ImportArgs) -> Result<u8> {
    // `source` is restricted to "zoxide" by the spec.
    let mut store = Store::load()?;
    let known: std::collections::HashSet<PathBuf> =
        store.projects.iter().map(|p| p.path.clone()).collect();
    let (mut added, mut skipped) = (Vec::new(), 0usize);
    for (score, path) in zoxide::list() {
        let low = a.min_score.is_some_and(|min| score < min);
        let usable = path.is_dir() && !known.contains(&path) && !low;
        let candidate = usable
            .then(|| match git::inspect(&path) {
                // Subdirectories and linked worktrees belong to a repository we already list.
                Some(info) if info.linked || info.root() != path && !info.bare => None,
                Some(_) => Some(true),
                None => a.all.then_some(false),
            })
            .flatten();
        let Some(_) = candidate else {
            skipped += 1;
            continue;
        };
        let project = project_for(&path, None, vec!["zoxide".into()])?;
        if store.add(project.clone()).is_ok() {
            added.push(project);
        } else {
            skipped += 1;
        }
    }
    if !a.dry_run {
        store.save()?;
    }
    if json {
        let items: Vec<Value> = added.iter().map(|p| project_json(p, None)).collect();
        print_json(&json!({ "dry_run": a.dry_run, "added": items, "skipped": skipped }))?;
    } else {
        let verb = if a.dry_run { "would add" } else { "added" };
        for p in &added {
            println!("{verb} {}", project_line(p));
        }
        println!("{} {verb}, {skipped} skipped", added.len());
    }
    Ok(0)
}

fn prune(json: bool, a: PruneArgs) -> Result<u8> {
    let mut store = Store::load()?;
    let (gone, kept): (Vec<_>, Vec<_>) = store.projects.drain(..).partition(|p| !p.exists());
    store.projects = if a.dry_run {
        gone.iter().cloned().chain(kept).collect()
    } else {
        kept
    };
    if !a.dry_run {
        store.save()?;
    }
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
    let code = match a.shell.as_str() {
        "fish" => {
            "function pj\n    set -l dir (command pooh pick $argv)\n    and test -n \"$dir\"\n    and cd $dir\nend"
        }
        _ => {
            "pj() {\n  local dir\n  dir=$(command pooh pick \"$@\") || return\n  [ -n \"$dir\" ] && cd \"$dir\"\n}"
        }
    };
    println!("{code}");
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
