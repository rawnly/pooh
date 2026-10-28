# pooh

Project manager for the terminal. Register projects once, then jump to them (or to their git worktrees) from your shell, tmux, herdr or rex. Built on [usage-rs](https://usage.jdx.dev): completions, man page and a machine-readable spec come from the binary itself.

Every command accepts `--json`. On failure it prints `{"error": "..."}` and exits 2; exit 1 means cancelled or no match.

## Usage

```sh
pooh add [path]                 # register (default: cwd); -n NAME, -t tag,tag
pooh add ~/code --scan          # register every git repo directly inside
pooh list [-t tag] [-w]         # -w includes worktrees
pooh pick [query]               # fuzzy picker, prints the chosen path
pooh resolve <query>            # non-interactive best match
pooh tag <project> a b [-r]
pooh remove <project>           # alias: rm
pooh prune [-n]                 # drop projects whose directory is gone
```

### Worktrees

Worktrees are read live from git, so anything created with plain `git worktree` shows up too. `pick` and `resolve` treat each worktree as `project:branch`.

```sh
pooh worktree list              # alias: wt; default project = the one containing cwd
pooh worktree add feat/login    # -b BASE, --path DIR, -p PROJECT
pooh worktree remove feat/login # branch, name or path; -f to force
```

Adding a linked worktree with `pooh add` registers its main repository. Bare repos (including the `repo/.bare` layout) are supported; their root is not a jump target while they have worktrees.

### zoxide

- `pooh import zoxide` registers the git repos zoxide already knows (`--all`, `--min-score N`, `-n`).
- `pick` and `resolve` call `zoxide add`, so jumps feed zoxide.
- Ranking combines pooh's own frecency with zoxide's score.
- `POOH_ZOXIDE=0` turns all of it off.

### Shell

```sh
eval "$(pooh init zsh)"         # defines `pj [query]`: pick and cd (bash, zsh, fish)
pooh completions zsh            # also bash, fish, elvish, nu, powershell
pooh spec | usage generate manpage -f - -o pooh.1
```

### Agents

```sh
pooh skill install              # ~/.agents/skills/pooh/SKILL.md (--claude for ~/.claude/skills, --dir DIR)
pooh skill show
```

The skill source lives in [`skills/pooh/SKILL.md`](skills/pooh/SKILL.md).

## Integrations

`pick --json` prints `{label, project, path, project_path, tags, worktree}`; the UI is on stderr, so it composes with `$(...)`.

**tmux** (`~/.tmux.conf`): open the pick in a new window.

```tmux
bind-key f display-popup -E -w 60% -h 60% \
  'dir=$(pooh pick) && tmux new-window -c "$dir" -n "$(basename "$dir")"'
```

**herdr**: run `pooh pick` inside a popup pane and open the result at that `cwd`. See bracco's `plugin/bracco.sh` for the same pattern with a file picker.

**Rex** ([`rex/lua/pooh`](rex/lua/pooh/init.lua)): registers a `pooh_pick` action that opens the picker in a floating pane, then switches to a Rex session for the pick (created on first use, named `project` or `project:branch`).

With [Amber](https://github.com/rawnly/amber), in `~/.config/rex/init.lua`:

```lua
amber.setup({
  plugins = {
    { "rawnly/pooh", main = "pooh", opts = { key = "ctrl+a>p" } },
  },
})
```

Then `rex do amber_install` and `rex config reload` twice, as for any Amber plugin. For a local checkout use `path = "/abs/path/to/pooh"` instead of the repo name.

Without Amber, clone the repo anywhere and load the module yourself:

```lua
local pooh_dir = os.getenv("HOME") .. "/Developer/pooh" -- your checkout
package.path = pooh_dir .. "/rex/lua/?.lua;" .. pooh_dir .. "/rex/lua/?/init.lua;" .. package.path
require("pooh").setup({ key = "ctrl+a>p" })
```

Or skip the key and run `rex do pooh_pick`. Options (all optional):

| Option | Default | Meaning |
| --- | --- | --- |
| `bin` | `"pooh"` | pooh executable |
| `mode` | `"session"` | `"session"`: switch to the pick's own session; `"window"`: new tab in the current session |
| `session_label` | `"label"` | name sessions/tabs by `project[:branch]`, or `"path"` to use the directory |
| `args` | `{}` | extra `pooh pick` flags, e.g. `{ "--tag", "work" }` |
| `bounds` | `{0.2, 0.2, 0.6, 0.6}` | float geometry as window fractions |
| `key` | none | key binding for the action |

Needs `pooh` on the Rex server's `PATH`.

## Config

`$POOH_CONFIG_DIR/projects.json` (default `~/.config/pooh/`). Removing a project never touches files on disk.

## Develop

```sh
mise run check   # fmt, clippy, tests
```
