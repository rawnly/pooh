---
name: pooh
description: Find, register and jump between the user's projects and git worktrees with the `pooh` CLI. Use when the user names a project you must locate, asks to add or list projects, or wants a worktree created or removed for a branch.
---

# pooh

`pooh` is the user's project manager. It knows every registered project, its git worktrees, and (via zoxide) which directories the user visits most. Every command takes `--json`; always pass it and parse the result instead of scraping text.

`pooh pick` (also `po pick`) is an interactive TUI. Never run it from an agent: use `resolve` or `list`.

## Find a project

```bash
pooh resolve api --json            # best fuzzy match for "api"
pooh resolve api -p shop --json    # restrict to project "shop" (matches its worktrees)
pooh list --json                   # every project
pooh list --worktrees --json       # ...with git worktrees
pooh list --tag work --json
```

`resolve` prints one entry (exit 0) or, with no match, `{"error": ...}` (exit 1):

```json
{"label":"shop:feat-cart","project":"shop","path":"/w/shop-feat-cart","project_path":"/w/shop","tags":["work"],"worktree":{"name":"shop-feat-cart","branch":"feat-cart"}}
```

`path` is the directory to work in. `worktree` is `null` for the project root. `label` is `project` or `project:branch`.

Resolving counts as a visit and teaches zoxide the path. Use `list` when you only want to look.

## Worktrees

Without `--project` the project containing the current directory is used.

```bash
pooh worktree list --json
pooh worktree add feat/login --json           # new branch from HEAD
pooh worktree add feat/login -b main --json   # new branch from main
pooh worktree add existing-branch --json      # checks out the existing branch
pooh worktree remove feat/login --json        # target: branch, name or path
```

`add` returns `{"project","branch","path"}`. The default location is a sibling of the repository (`<repo>-<branch-with-dashes>`); bare repos in a `.bare` layout get `<parent>/<branch>`. Override with `--path`. Removing refuses dirty trees unless `--force`: ask the user before forcing.

## Manage the list

```bash
pooh add [path] [--name N] [--tag a,b] --json    # register (default: cwd); linked worktrees map to their main repo
pooh add ~/code --scan --json                    # register every git repo directly inside
pooh tag <project> a b [--remove] --json
pooh remove <project> --json                     # only unregisters, never deletes files
pooh prune --dry-run --json                      # projects whose directory is gone
pooh import zoxide --dry-run --json              # bring in git repos zoxide already knows
```

## Errors

On failure `--json` prints `{"error":"..."}` on stdout and exits 2. Exit 1 means "no match". Do not retry blindly: read the message.

## Config

Data lives in `$POOH_CONFIG_DIR` (default `~/.config/pooh/projects.json`). Set `POOH_ZOXIDE=0` to disable zoxide.
