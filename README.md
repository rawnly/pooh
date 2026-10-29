# pooh

Installs two binaries: `pooh` and the short `po` (same program).

Project manager for the terminal. Register projects once, then jump to them (or to their git worktrees) from your shell, tmux, herdr or rex. Built on [usage-rs](https://usage.jdx.dev): completions, man page and a machine-readable spec come from the binary itself.

Every data command accepts `--json` (`init`, `completions`, `spec` and `skill show` print plain text). On failure it prints `{"error": "..."}` on stdout and exits 2, usage errors included; exit 1 means cancelled or no match. Project names cannot be empty, contain `:` or control characters, or have leading/trailing whitespace.

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

### Picker

`pick` opens an inline picker (a few rows under the cursor, no alternate screen) drawn on the tty, so stdout carries only the result. `--full-screen` uses the alternate screen instead (what the Rex plugin does, since it runs in a float). The inline view needs the terminal to answer a cursor-position query; if it doesn't, `pick` fails with a hint to use `--full-screen`.

| Key | Action |
| --- | --- |
| type | fuzzy filter (matches name, branch and tags) |
| `↑` `↓` `ctrl-p` `ctrl-n` `PgUp` `PgDn` | move |
| `tab` / `shift-tab` | cycle the tag filter |
| `ctrl-o` | cycle all / projects only / worktrees only |
| `ctrl-e` | edit the highlighted project's tags (comma-separated, `enter` saves) |
| `enter` | pick |
| `esc` `ctrl-c` | cancel (exit 1) |

Text editing: `←` `→` `home` `end` `ctrl-w` `ctrl-u`.

`--vim` (or `POOH_VIM=1`) makes the picker modal, like `bracco --vim`. It starts in normal mode and typing does not filter:

| Key (normal mode) | Action |
| --- | --- |
| `j` `k` | move |
| `g` `G` | top / bottom |
| `d` `u` `ctrl-d` `ctrl-u` | down / up (10 / half page) |
| `h` `l` `0` `$` | move the query cursor |
| `x` `D` | delete char / delete to end of query |
| `/` `i` `a` | search (`esc` goes back to normal mode) |
| `t` `T` | next / previous tag filter |
| `w` | cycle all / projects / worktrees |
| `e` | edit tags |
| `enter` | pick |
| `esc` | clear the query, or cancel when it is empty |
| `q` | cancel |

In the Rex plugin, pass it with `args = { "--vim" }`.

### Worktrees

Worktrees are read live from git, so anything created with plain `git worktree` shows up too. `pick` and `resolve` treat each worktree as `project:branch`.

```sh
pooh worktree list              # alias: wt; default project = the one containing cwd
pooh worktree add feat/login    # -b BASE, --path DIR, -p PROJECT
pooh worktree remove feat/login # branch, then path (relative ok), then folder name; -f to force
```

Adding a linked worktree with `pooh add` registers its main repository. Bare repos (including the `repo/.bare` layout) are supported; their root is not a jump target while they have worktrees.

### zoxide

- `pooh import zoxide` registers the git repos zoxide already knows (`--all`, `--min-score N`, `-n`).
- `pick` and `resolve` call `zoxide add`, so jumps feed zoxide.
- Ranking combines pooh's own frecency with zoxide's score.
- `POOH_ZOXIDE=0` turns all of it off.

### Shell

```sh
eval "$(pooh init zsh)"         # defines `pj [query]`: pick and cd, and completions for `po` (bash, zsh, fish)
eval "$(pooh init zsh --alias p)" # ...plus an extra `alias p=pooh` with completions (--function NAME renames `pj`)
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

`pick --json` prints `{label, project, path, project_path, tags, worktree}`; the UI is drawn on the tty, so it composes with `$(...)`.

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

Then `rex do amber_install` and `rex config reload` twice, as for any Amber plugin. For a local checkout add `path = "/abs/path/to/pooh"` to the spec (the `owner/repo` string stays).

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
| `mode` | `"session"` | `"session"`: switch to the pick's own session; `"window"`: tab in the current session. Both reuse the one a previous pick created (tabs are found through a `pooh:<name>` block label, so a tab you named by hand is not reused) |
| `session_label` | `"label"` | name sessions/tabs by `project[:branch]`, or `"path"` to use the directory |
| `full_screen` | `true` | run the picker with `--full-screen` so it fills the float |
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
