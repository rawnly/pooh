mod commands;
mod entries;
mod fuzzy;
mod git;
mod picker;
mod skill;
mod store;
mod zoxide;

use std::path::PathBuf;
use std::process::ExitCode;

use usage::Cli;

/// Project manager with git worktree and zoxide support.
///
/// Register projects once, then jump to them (or to their worktrees) from a
/// shell, tmux, herdr or rex. Every command accepts --json for scripts and agents.
/// Exit code: 0 ok, 1 cancelled / no match, 2 error.
#[derive(Debug, Cli)]
#[usage(bin = "pooh", version = env!("CARGO_PKG_VERSION"), unknown_flags = "error", completion, arg_required_else_help)]
pub struct Args {
    /// Print machine-readable JSON instead of text
    #[usage(long, global)]
    pub json: bool,

    #[usage(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, usage::Subcommands)]
pub enum Command {
    /// Register a project (default: current directory)
    Add(AddArgs),
    /// List registered projects
    List(ListArgs),
    /// Unregister a project
    #[usage(alias("rm"))]
    Remove(RemoveArgs),
    /// Add or remove tags on a project
    Tag(TagArgs),
    /// Fuzzy-pick a project or worktree and print its path
    Pick(PickArgs),
    /// Print the path of the best match for a query (non-interactive)
    Resolve(ResolveArgs),
    /// Inspect and manage git worktrees of a project
    #[usage(alias("wt"))]
    Worktree(WorktreeArgs),
    /// Import projects from another tool
    Import(ImportArgs),
    /// Unregister projects whose directory no longer exists
    Prune(PruneArgs),
    /// Print a shell function that cd's into a picked project
    Init(InitArgs),
    /// Print or install the agent skill
    Skill(SkillArgs),
    /// Print a shell completion script
    Completions(CompletionsArgs),
    /// Print the usage spec (input for `usage generate manpage|markdown`)
    Spec(SpecArgs),
}

#[derive(Debug, usage::Args)]
pub struct AddArgs {
    /// Directory to register
    #[usage(default = ".", value_hint = usage::ValueHint::DirPath)]
    pub path: PathBuf,

    /// Project name (default: folder name)
    #[usage(short, long)]
    pub name: Option<String>,

    /// Tag to attach (repeatable, or comma-separated)
    #[usage(short, long)]
    pub tag: Vec<String>,

    /// Register every git repository directly inside PATH instead of PATH itself
    #[usage(short, long)]
    pub scan: bool,
}

#[derive(Debug, usage::Args)]
pub struct ListArgs {
    /// Only projects with one of these tags (repeatable, or comma-separated)
    #[usage(short, long)]
    pub tag: Vec<String>,

    /// Include each project's git worktrees
    #[usage(short, long)]
    pub worktrees: bool,

    /// Only projects whose directory is missing
    #[usage(long)]
    pub broken: bool,
}

#[derive(Debug, usage::Args)]
pub struct RemoveArgs {
    /// Project name
    pub name: String,
}

#[derive(Debug, usage::Args)]
pub struct TagArgs {
    /// Project name
    pub project: String,

    /// Tags to add
    pub tags: Vec<String>,

    /// Remove the tags instead of adding them
    #[usage(short, long)]
    pub remove: bool,
}

#[derive(Debug, usage::Args)]
pub struct PickArgs {
    /// Initial filter
    pub query: Option<String>,

    /// Only projects with one of these tags (repeatable, or comma-separated)
    #[usage(short, long)]
    pub tag: Vec<String>,

    /// Only this project (shows its worktrees)
    #[usage(short, long)]
    pub project: Option<String>,

    /// Do not list worktrees, only projects
    #[usage(long)]
    pub no_worktrees: bool,
}

#[derive(Debug, usage::Args)]
pub struct ResolveArgs {
    /// Name to look for (fuzzy)
    pub query: String,

    /// Only projects with one of these tags (repeatable, or comma-separated)
    #[usage(short, long)]
    pub tag: Vec<String>,

    /// Only this project (matches its worktrees)
    #[usage(short, long)]
    pub project: Option<String>,

    /// Do not match worktrees, only projects
    #[usage(long)]
    pub no_worktrees: bool,
}

#[derive(Debug, usage::Args)]
pub struct WorktreeArgs {
    #[usage(subcommand)]
    pub action: WorktreeAction,
}

#[derive(Debug, usage::Subcommands)]
pub enum WorktreeAction {
    /// List worktrees (default project: the one containing the cwd)
    List(WorktreeListArgs),
    /// Create a worktree for a branch (created from --base if it does not exist)
    Add(WorktreeAddArgs),
    /// Remove a worktree by branch, name or path
    #[usage(alias("rm"))]
    Remove(WorktreeRemoveArgs),
}

#[derive(Debug, usage::Args)]
pub struct WorktreeListArgs {
    /// Project name (default: the project containing the cwd)
    #[usage(short, long)]
    pub project: Option<String>,
}

#[derive(Debug, usage::Args)]
pub struct WorktreeAddArgs {
    /// Branch to check out (created if missing)
    pub branch: String,

    /// Project name (default: the project containing the cwd)
    #[usage(short, long)]
    pub project: Option<String>,

    /// Where to create it (default: next to the repository)
    #[usage(long, value_hint = usage::ValueHint::DirPath)]
    pub path: Option<PathBuf>,

    /// Start point for a new branch (default: HEAD)
    #[usage(short, long)]
    pub base: Option<String>,
}

#[derive(Debug, usage::Args)]
pub struct WorktreeRemoveArgs {
    /// Branch, worktree name or path
    pub target: String,

    /// Project name (default: the project containing the cwd)
    #[usage(short, long)]
    pub project: Option<String>,

    /// Remove even with uncommitted changes
    #[usage(short, long)]
    pub force: bool,
}

#[derive(Debug, usage::Args)]
pub struct ImportArgs {
    /// Where to import from
    #[usage(choices("zoxide"))]
    pub source: String,

    /// Also import directories that are not git repositories
    #[usage(long)]
    pub all: bool,

    /// Skip entries with a lower zoxide score
    #[usage(long)]
    pub min_score: Option<f64>,

    /// Show what would be imported without saving
    #[usage(short = 'n', long)]
    pub dry_run: bool,
}

#[derive(Debug, usage::Args)]
pub struct PruneArgs {
    /// Show what would be removed without saving
    #[usage(short = 'n', long)]
    pub dry_run: bool,
}

#[derive(Debug, usage::Args)]
pub struct InitArgs {
    /// Target shell
    #[usage(choices("bash", "zsh", "fish"))]
    pub shell: String,

    /// Also define a short alias for pooh (with completions), e.g. --alias p
    #[usage(long)]
    pub alias: Option<String>,

    /// Name of the pick-and-cd function
    #[usage(long, default = "pj")]
    pub function: String,
}

#[derive(Debug, usage::Args)]
pub struct SkillArgs {
    #[usage(subcommand)]
    pub action: SkillAction,
}

#[derive(Debug, usage::Subcommands)]
pub enum SkillAction {
    /// Print SKILL.md
    Show(SkillShowArgs),
    /// Write SKILL.md to <dir>/pooh/ (default dir: ~/.agents/skills)
    Install(SkillInstallArgs),
}

#[derive(Debug, usage::Args)]
pub struct SkillShowArgs {}

#[derive(Debug, usage::Args)]
pub struct SkillInstallArgs {
    /// Skills directory to install into
    #[usage(long, value_hint = usage::ValueHint::DirPath)]
    pub dir: Option<PathBuf>,

    /// Install into ~/.claude/skills instead
    #[usage(long)]
    pub claude: bool,
}

#[derive(Debug, usage::Args)]
pub struct CompletionsArgs {
    /// Target shell
    #[usage(choices("bash", "zsh", "fish", "elvish", "nu", "powershell"))]
    pub shell: String,
}

#[derive(Debug, usage::Args)]
pub struct SpecArgs {}

/// Entry point shared by the `pooh` and `po` binaries.
pub fn main() -> ExitCode {
    // Handles --help / --version / usage errors itself.
    let args = Args::parse();
    let json = args.json;
    match commands::run(args) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({ "error": e.to_string() }));
            } else {
                eprintln!("pooh: {e}");
            }
            ExitCode::from(2)
        }
    }
}
