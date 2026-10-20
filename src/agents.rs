//! Read-only agent profile configuration and launch command rendering.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use toml_edit::{DocumentMut, TableLike};

use crate::domain::{
    normalize_thread, resolve_assignee, resolve_on, thread_refusal_message, Assignee, BlockKind,
    BlockOn,
};
use crate::ui::queue::{GroupBy, SortBy};

const CONFIG_FILE: &str = "config.toml";
pub const CONFIG_TEMP_PREFIX: &str = ".config.toml.tmp.";
static CONFIG_TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The commented `config.toml` a full board open seeds. Every profile is commented out, and
/// the quoted default prompt must stay equal to [`DEFAULT_PROMPT`] (pinned by a test).
pub const STARTER_CONFIG: &str = r##"# tsk configuration
#
# Agent profiles let you assign a task to an agent (`@` or `!a name`).
# In Herdr, starting an assigned project task (ctrl+s) dispatches it: tsk
# creates a git worktree for the task, opens a Herdr workspace there, and runs
# the profile's command. Outside Herdr, or on a desk task, a start only sets
# the status.
#
# Any agent that runs in a terminal works, including ones not listed here.
# A profile needs only:
#
#   [agent.<name>]                     name: lowercase letters, digits, - and .
#   command = ["program", "arg", ...]  the program and its arguments
#
# tsk adds the prompt as the LAST argument, so the agent must accept its first
# message as a trailing argument (`claude "…"`, `pi "…"`, `codex "…"`).
#
# Optional:
#   prompt = "…"                       replaces the default prompt below
#   [agent.<name>.env]                 extra environment variables
#   KEY = "value"
#
# Placeholders, usable in `command` and `prompt`:
#   {number}    task number, e.g. 12
#   {title}     task title
#   {notes}     task notes
#   {steps}     task steps, one per line as [ ] or [x]
#   {worktree}  path of the task's worktree
#   {branch}    the task's branch
#   {base}      the branch the work starts from, e.g. main
#
# The default prompt, used when a profile has no `prompt`:
#
#   You were dispatched to T{number} ({title}) in worktree {worktree} on
#   branch {branch}, based on {base}.
#
#   1. Run `tsk guide`, then `tsk list {number} --json`. The task notes are
#      your brief.
#   2. Read the repo's agent instructions (AGENTS.md or CLAUDE.md) if present.
#   3. Work only on {branch}. Run the project's checks before saying you are
#      done.
#   4. Push and open a pull request into {base}. Never merge it.
#   5. Set the task to review with what you did and what to check:
#      `tsk status {number} review --done "…" --check "…"`, one `--check` per
#      thing to verify, adding `--next` for what comes after. When you need a
#      human, block it with your question: `tsk status {number} blocked
#      --why "…"`, adding `--needs` and one `--option` per choice, then stop.
#   6. On a relaunch, first read whichever record closed last (`closed_at`)
#      in `tsk list {number} --json`: the `replies` of the last `past_blocks`
#      entry, or the `feedback` and failed `checks` of the last `past_reviews`
#      entry when it was `sent_back`.
#   7. A message `[tsk T{number} sent back] …` is review feedback, with any
#      failed checks: address it, then set review again.
#
# Remove the leading # from a block below to use it.

# --- Examples using the default prompt -------------------------------------

# Flags follow the program, for example a model: ["codex", "--model", "<model>"].

# [agent.claude]
# command = ["claude"]

# [agent.codex]
# command = ["codex"]

# [agent.pi]
# command = ["pi"]

# --- Examples with their own prompt -----------------------------------------

# A quick fixer for small tasks, with the task text inlined.
# [agent.grok]
# command = ["grok"]
# prompt = """
# Fix T{number}: {title}
#
# {notes}
#
# {steps}
#
# Keep the change small, commit on {branch}, open a pull request into {base},
# then set the task to review.
# """

# Any other terminal agent: its program and flags go in `command`. `env` sets
# variables for that agent only, here to mark its commits as the agent's.
# Do not put API keys here: this file is plain text.
# [agent.my-agent]
# command = ["my-agent", "--some-flag"]
# [agent.my-agent.env]
# GIT_AUTHOR_NAME = "my-agent (via tsk)"
# GIT_COMMITTER_NAME = "my-agent (via tsk)"

# --- On Windows (preview) ---------------------------------------------------

# tsk starts the agent through Windows PowerShell, so `command` may name an
# .exe or the .ps1 shim npm installs (pi, codex). Write Windows paths as
# literal strings ('...') so their backslashes stay as typed.
# [agent.claude-win]
# command = ['C:\Users\you\.local\bin\claude.exe']

# --- Merging a task's pull request ------------------------------------------

# `m` on the cleanup card (or `tsk status T12 done --merge`) merges the task's
# open pull request with the GitHub CLI (`gh`), then marks the task done and
# cleans up. `method` is "merge" (the default), "squash", or "rebase".
# [merge]
# method = "squash"

# --- How the board opens ----------------------------------------------------

# The group and sort a fresh board starts from (`g` and `s` change them for
# the session only). `group` and `sort` set the desk; `project_group` and
# `project_sort` set every project board. `messages` hides the agent message
# lines under board rows (`z` toggles them for the session). `me` is the name
# the board shows for you, such as `@sam` on a task assigned to you, instead
# of `you`.
#   group          "none", "project", "thread", or "assignee"
#   sort           "recent", "project", "thread", "title", or "assignee"
#   project_group  "none", "thread", or "assignee"
#   project_sort   "recent", "thread", "title", or "assignee"
#   messages       "shown" or "hidden"
#   me             lowercase letters, digits, - and .; not an agent's name
# [board]
# group = "project"
# sort = "recent"
# project_group = "thread"
# project_sort = "title"
# messages = "hidden"
# me = "sam"

# --- Backups ------------------------------------------------------------------

# Saves copy tsk.json and this file (without `env` values) to a git
# repository, using your own `git` and its credentials. `tsk backup setup`
# makes a private GitHub repository with `gh`; `tsk backup add <git-url>` adds
# any other. Both write an entry like the one below. `tsk backup list` shows
# how each went, and `tsk restore` puts a version back. Backups hold your task
# notes: keep the repository private.
#   every  "save" (the default) or "daily"
# [[backup]]
# url = "git@github.com:you/tsk-backup.git"
# every = "save"
"##;

/// Seed the commented profile examples on a full board open.
///
/// The temporary file is complete and synced before one atomic hard-link creates the target.
/// A target that already exists wins without being changed, including an empty file.
pub fn seed_on_open(state_dir: &Path) -> io::Result<bool> {
    let target = state_dir.join(CONFIG_FILE);
    // The common case is an existing file: answer without writing anything. The hard
    // link below still decides the race when two opens find it missing at once.
    if target.exists() {
        return Ok(false);
    }
    crate::fsperm::ensure_private_dir(state_dir)?;
    let tmp = unique_tmp_path(state_dir);
    let write_result = (|| -> io::Result<bool> {
        let mut temp_file = create_private_temp(&tmp)?;
        temp_file.write_all(STARTER_CONFIG.as_bytes())?;
        temp_file.sync_all()?;
        drop(temp_file);
        match fs::hard_link(&tmp, &target) {
            Ok(()) => {
                fs::remove_file(&tmp)?;
                fs::File::open(state_dir)?.sync_all()?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                fs::remove_file(&tmp)?;
                Ok(false)
            }
            Err(error) => Err(error),
        }
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

fn create_private_temp(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn unique_tmp_path(dir: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = CONFIG_TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(
        "{CONFIG_TEMP_PREFIX}{}.{nanos}.{sequence}",
        std::process::id()
    ))
}

/// `<state_dir>/config.toml`'s text, `None` when the file does not exist.
pub(crate) fn read_config(state_dir: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(state_dir.join(CONFIG_FILE)) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Replace `<state_dir>/config.toml` with `content`: a private, synced temp file renamed over
/// the target, so a reader sees the old file or the new one, never a torn one.
pub(crate) fn write_config(state_dir: &Path, content: &str) -> io::Result<()> {
    crate::fsperm::ensure_private_dir(state_dir)?;
    let tmp = unique_tmp_path(state_dir);
    let result = (|| -> io::Result<()> {
        let mut temp_file = create_private_temp(&tmp)?;
        temp_file.write_all(content.as_bytes())?;
        temp_file.sync_all()?;
        drop(temp_file);
        crate::fsperm::replace_file(&tmp, &state_dir.join(CONFIG_FILE))?;
        #[cfg(not(windows))]
        fs::File::open(state_dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Prompt used by a profile that does not define its own template. It assumes a git
/// worktree, which dispatch guarantees (non-git projects refuse before rendering).
pub const DEFAULT_PROMPT: &str = "You were dispatched to T{number} ({title}) in worktree {worktree} on branch {branch}, based on {base}.

1. Run `tsk guide`, then `tsk list {number} --json`. The task notes are your brief.
2. Read the repo's agent instructions (AGENTS.md or CLAUDE.md) if present.
3. Work only on {branch}. Run the project's checks before saying you are done.
4. Push and open a pull request into {base}. Never merge it.
5. Set the task to review with what you did and what to check: `tsk status {number} review --done \"…\" --check \"…\"`, one `--check` per thing to verify, adding `--next` for what comes after. When you need a human, block it with your question: `tsk status {number} blocked --why \"…\"`, adding `--needs` and one `--option` per choice, then stop.
6. On a relaunch, first read whichever record closed last (`closed_at`) in `tsk list {number} --json`: the `replies` of the last `past_blocks` entry, or the `feedback` and failed `checks` of the last `past_reviews` entry when it was `sent_back`.
7. A message `[tsk T{number} sent back] …` is review feedback, with any failed checks: address it, then set review again.";

/// All agent profiles loaded from one state directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentProfiles {
    profiles: BTreeMap<String, AgentProfile>,
}

impl AgentProfiles {
    /// Load profiles from `<state_dir>/config.toml`. A missing file is an empty profile set.
    pub fn load(state_dir: impl AsRef<Path>) -> Result<Self, AgentLoadError> {
        let path = state_dir.as_ref().join(CONFIG_FILE);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(AgentLoadError::Io { path, source }),
        };
        Self::parse(&content)
    }

    fn parse(content: &str) -> Result<Self, AgentLoadError> {
        let document: DocumentMut =
            content
                .parse()
                .map_err(|source| AgentLoadError::MalformedToml {
                    source: Box::new(source),
                })?;
        // Other top-level keys are settings for other features, possibly from a newer
        // binary: ignore them so an older tsk still reads its profiles.
        let Some(agents) = document.get("agent") else {
            return Ok(Self::default());
        };
        let agents = agents
            .as_table_like()
            .ok_or_else(|| AgentLoadError::InvalidDocument {
                message: "expected [agent.<name>] tables".into(),
            })?;

        let mut profiles = BTreeMap::new();
        for (name, item) in agents.iter() {
            validate_name(name)?;
            let table = item
                .as_table_like()
                .ok_or_else(|| invalid_profile(name, "profile must be a [agent.<name>] table"))?;
            profiles.insert(name.to_string(), parse_profile(name, table)?);
        }
        Ok(Self { profiles })
    }

    /// Whether `content` parses as profiles (tests of other modules).
    #[cfg(test)]
    pub(crate) fn parse_for_tests(content: &str) -> bool {
        Self::parse(content).is_ok()
    }

    pub fn get(&self, name: &str) -> Option<&AgentProfile> {
        self.profiles.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// Normalize a supplied name, then require one exact profile match.
    pub fn resolve_name(&self, input: &str) -> Result<String, String> {
        let normalized = normalize_thread(input)
            .map_err(|error| thread_refusal_message(error).replacen("thread", "agent name", 1))?;
        if self.profiles.contains_key(&normalized) {
            Ok(normalized)
        } else {
            Err(format!("unknown agent {normalized}"))
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.profiles.keys().map(String::as_str)
    }

    /// Read a typed assignee by the name rule against these profiles: you, a profile (an
    /// agent), or any other valid name (a person).
    pub fn resolve_assignee(&self, input: &str) -> Result<Assignee, String> {
        resolve_assignee(input, |name| self.profiles.contains_key(name))
            .map_err(|error| thread_refusal_message(error).replacen("thread", "assignee", 1))
    }

    /// Read a typed waiting-on value by the name rule against these profiles. Task addresses
    /// count only on a block.
    pub fn resolve_on(&self, input: &str, kind: BlockKind) -> BlockOn {
        resolve_on(input, kind, |name| self.profiles.contains_key(name))
    }
}

/// `[board] me` in `config.toml`: the name your handle shows instead of `you`. It never names
/// an agent profile, and it changes no stored data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnerName {
    pub name: Option<String>,
    /// Why the setting was not used: a bad name, or one an agent profile already has.
    pub error: Option<String>,
}

impl OwnerName {
    /// Read `[board] me` from `<state_dir>/config.toml`, checked against `profiles`. A missing
    /// file, table, or key is no name; a refused name keeps `you` and says why.
    pub fn load(state_dir: impl AsRef<Path>, profiles: &AgentProfiles) -> Self {
        let path = state_dir.as_ref().join(CONFIG_FILE);
        match fs::read_to_string(&path) {
            Ok(content) => Self::parse(&content, profiles),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(source) => Self {
                name: None,
                error: Some(AgentLoadError::Io { path, source }.to_string()),
            },
        }
    }

    fn parse(content: &str, profiles: &AgentProfiles) -> Self {
        let refuse = |message: String| Self {
            name: None,
            error: Some(
                AgentLoadError::InvalidDocument {
                    message: format!("[board] {message}"),
                }
                .to_string(),
            ),
        };
        let Ok(document) = content.parse::<DocumentMut>() else {
            // The profile loader reports a malformed file.
            return Self::default();
        };
        let Some(item) = document
            .get("board")
            .and_then(|board| board.as_table_like())
            .and_then(|board| board.get("me"))
        else {
            return Self::default();
        };
        let Some(value) = item.as_str() else {
            return refuse("me must be a string".into());
        };
        let name = match normalize_thread(value.trim()) {
            Ok(name) => name,
            Err(error) => {
                return refuse(format!(
                    "me {value:?}: {}",
                    thread_refusal_message(error).replacen("thread", "name", 1)
                ));
            }
        };
        if profiles.get(&name).is_some() {
            return refuse(format!("me {name:?} is an agent profile"));
        }
        Self {
            name: Some(name),
            error: None,
        }
    }
}

/// One named launch profile from `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub command: Vec<String>,
    pub prompt: Option<String>,
    pub env: BTreeMap<String, String>,
}

impl AgentProfile {
    /// Substitute this profile for one task.
    pub fn render(&self, context: &RenderContext<'_>) -> RenderedLaunch {
        let prompt = render_template(self.prompt.as_deref().unwrap_or(DEFAULT_PROMPT), context);
        let mut argv = self
            .command
            .iter()
            .map(|argument| render_template(argument, context))
            .collect::<Vec<_>>();
        argv.push(prompt);
        RenderedLaunch {
            argv,
            env: self.env.clone(),
        }
    }
}

/// Task and worktree values available to command and prompt templates.
#[derive(Debug, Clone, Copy)]
pub struct RenderContext<'a> {
    pub number: u64,
    pub title: &'a str,
    pub notes: &'a str,
    pub steps: &'a str,
    pub worktree: &'a str,
    pub branch: &'a str,
    pub base: &'a str,
}

/// Fully substituted values ready for a launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedLaunch {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl RenderedLaunch {
    /// The launch as a POSIX `sh` script at `launcher`, for `/bin/sh "<launcher>"` in the pane.
    /// The pane's shell parses only that fixed line, never task text or `env` values, whatever
    /// shell it is. The script runs twice: first it asks the user's login shell (`$SHELL -l`)
    /// for the fixed command `/bin/sh "<launcher>" login`, so the agent gets the login
    /// environment; that second run deletes the script and starts the argv. Every value is
    /// single-quoted for `/bin/sh` alone, where `'\''` is exact by POSIX. `launcher` must be
    /// literal inside double quotes in any shell ([`crate::dispatch::sh_launch_line`] checks).
    pub fn shell_script(&self, launcher: &str) -> String {
        let login = format!("/bin/sh \"{launcher}\" login");
        let mut script = format!(
            "# tsk dispatch launcher: runs once in your login shell, then deletes itself.\n\
             if [ \"${{1-}}\" != login ]; then\n    \
             exec \"${{SHELL:-/bin/sh}}\" -l -c {}\n\
             fi\n\
             rm -f -- \"$0\"\n\
             exec",
            shell_quote(&login)
        );
        if !self.env.is_empty() {
            script.push_str(" env");
            for (key, value) in &self.env {
                script.push(' ');
                script.push_str(&shell_quote(&format!("{key}={value}")));
            }
        }
        for argument in &self.argv {
            script.push(' ');
            script.push_str(&shell_quote(argument));
        }
        script.push('\n');
        script
    }

    /// The same launch as a Windows PowerShell 5.1 script, for `powershell -File`. Every value
    /// is embedded base64-encoded UTF-8, so the script is plain ASCII and no task text is ever
    /// parsed as PowerShell. The script deletes itself before it starts the agent.
    ///
    /// Windows PowerShell passes arguments to programs (and to the `.ps1` shims npm installs,
    /// which forward them to `node.exe`) without escaping embedded double quotes. Each argument
    /// is pre-escaped for the program's command-line parser so it arrives verbatim. A script
    /// that reads `$args` itself sees that escaping: whether a script forwards to a program
    /// cannot be known from outside it, so the supported script is a forwarding shim.
    ///
    /// A `.cmd` or `.bat` file would hand the arguments to cmd.exe, which no escaping makes
    /// safe for task text, so the program resolves only to an executable or a PowerShell
    /// script; a name that finds nothing else refuses in the pane and starts nothing.
    pub fn powershell_script(&self) -> String {
        let text = |value: &str| format!("(TskText '{}')", base64_text(value));
        let mut script = String::from(POWERSHELL_PRELUDE);
        for (key, value) in &self.env {
            script.push_str(&format!(
                "[Environment]::SetEnvironmentVariable({}, {}, 'Process')\n",
                text(key),
                text(value)
            ));
        }
        let (program, arguments) = self.argv.split_first().expect("argv has a program");
        script.push_str(&format!("$TskCommand = TskProgram {}\n", text(program)));
        script.push_str("$TskArguments = @(\n");
        for argument in arguments {
            script.push_str(&format!("    (TskArgument {})\n", text(argument)));
        }
        script.push_str(")\n& $TskCommand @TskArguments\nexit $LASTEXITCODE\n");
        script
    }
}

/// The fixed head of every PowerShell launcher. `TskArgument` escapes one argument the way
/// the Microsoft C runtime parses a command line: backslashes before a quote double and the
/// quote is escaped; trailing backslashes double when PowerShell will wrap the argument in
/// quotes (it does so for any argument containing whitespace); an empty argument becomes `""`,
/// which PowerShell would otherwise drop.
const POWERSHELL_PRELUDE: &str = r#"# tsk dispatch launcher: runs once, then deletes itself.
$ErrorActionPreference = 'Stop'
Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
function TskText([string]$Value) {
    [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Value))
}
function TskArgument([string]$Value) {
    if ($Value.Length -eq 0) { return '""' }
    $Value = [regex]::Replace($Value, '(\\*)"', '$1$1\"')
    if ($Value -match '\s') { $Value = [regex]::Replace($Value, '(\\+)$', '$1$1') }
    $Value
}
function TskProgram([string]$Name) {
    $Found = @(Get-Command -Name $Name -All -CommandType Application, ExternalScript -ErrorAction SilentlyContinue)
    $Safe = @($Found | Where-Object { $_.Path -notmatch '\.(cmd|bat)$' })
    if ($Safe.Count -gt 0) { return $Safe[0] }
    if ($Found.Count -gt 0) {
        [Console]::Error.WriteLine("tsk: not starting ${Name}: it resolves only to the batch file $($Found[0].Path), and cmd.exe would run task text as commands. Point the profile at the agent's .exe or .ps1.")
    } else {
        [Console]::Error.WriteLine("tsk: not starting ${Name}: no program or PowerShell script has that name.")
    }
    exit 1
}
"#;

/// The batch file `program` resolves to when `TskProgram` would find only batch files for it,
/// so dispatch can refuse before it creates anything. It searches like `Get-Command`: the
/// `;`-separated `path`, the name as given and under each `pathext` extension and `.ps1`, or
/// an absolute path itself. `None` when it finds an executable or a PowerShell script, when
/// it finds nothing, or when the name is relative to a directory only the pane knows: the
/// launcher still refuses those.
pub fn batch_only_program(
    program: &str,
    path: Option<&std::ffi::OsStr>,
    pathext: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let pathext = pathext
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    let mut names = vec![program.to_string()];
    names.extend(
        pathext
            .split(';')
            .chain([".ps1"])
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!("{program}{extension}")),
    );
    let directories: Vec<PathBuf> = if program.contains(['\\', '/']) {
        if !Path::new(program).is_absolute() {
            return None;
        }
        vec![PathBuf::new()]
    } else {
        path.map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default()
            .split(';')
            .map(|directory| directory.trim().trim_matches('"'))
            .filter(|directory| !directory.is_empty())
            .map(PathBuf::from)
            .collect()
    };
    let found: Vec<PathBuf> = directories
        .iter()
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .filter(|candidate| candidate.is_file())
        .collect();
    let batch = |candidate: &PathBuf| {
        candidate
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
            })
    };
    match found.first() {
        Some(first) if found.iter().all(batch) => Some(first.clone()),
        _ => None,
    }
}

/// Why dispatch refuses a program that resolves only to `batch`: the launcher's own words.
pub fn batch_only_refusal(program: &str, batch: &Path) -> String {
    format!(
        "not starting {program}: it resolves only to the batch file {}, {BATCH_ONLY_ADVICE}",
        batch.display()
    )
}

/// The end of the refusal, shared with [`POWERSHELL_PRELUDE`] (pinned by a test).
const BATCH_ONLY_ADVICE: &str =
    "and cmd.exe would run task text as commands. Point the profile at the agent's .exe or .ps1.";

fn base64_text(value: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(value.as_bytes())
}

/// How `m` on the cleanup card and `--merge` merge a task's pull request: `[merge] method`
/// in `config.toml`. Without the setting, a merge commit, as GitHub's own default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    /// The setting's value, and the `gh pr merge` flag without its dashes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }

    /// Read `[merge] method` from `<state_dir>/config.toml`. A missing file or table is the
    /// default; only the `[merge]` table is validated, so a broken profile never blocks it.
    pub fn load(state_dir: impl AsRef<Path>) -> Result<Self, AgentLoadError> {
        let path = state_dir.as_ref().join(CONFIG_FILE);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(AgentLoadError::Io { path, source }),
        };
        Self::parse(&content)
    }

    fn parse(content: &str) -> Result<Self, AgentLoadError> {
        let document: DocumentMut =
            content
                .parse()
                .map_err(|source| AgentLoadError::MalformedToml {
                    source: Box::new(source),
                })?;
        let invalid = |message: String| AgentLoadError::InvalidDocument {
            message: format!("[merge] {message}"),
        };
        let Some(merge) = document.get("merge") else {
            return Ok(Self::default());
        };
        let table = merge
            .as_table_like()
            .ok_or_else(|| invalid("must be a table".into()))?;
        if let Some(key) = table
            .iter()
            .map(|(key, _)| key)
            .find(|key| *key != "method")
        {
            return Err(invalid(format!("unknown key {key:?}")));
        }
        match table.get("method").map(|item| item.as_str()) {
            None => Ok(Self::default()),
            Some(Some("merge")) => Ok(Self::Merge),
            Some(Some("squash")) => Ok(Self::Squash),
            Some(Some("rebase")) => Ok(Self::Rebase),
            Some(Some(other)) => Err(invalid(format!(
                "method {other:?} is not merge, squash, or rebase"
            ))),
            Some(None) => Err(invalid("method must be a string".into())),
        }
    }
}

/// The board's starting group, sort, and message lines: `[board]` in `config.toml`. A fresh
/// board opens with these; changes made in the session are never written back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoardDefaults {
    /// The desk (`group`, `sort`).
    pub group: GroupBy,
    pub sort: SortBy,
    /// Every project board (`project_group`, `project_sort`).
    pub project_group: GroupBy,
    pub project_sort: SortBy,
    /// `messages = "hidden"`: board rows start without their `└─` message lines.
    pub messages_hidden: bool,
}

impl BoardDefaults {
    /// Read `[board]` from `<state_dir>/config.toml`. A missing file, table, or key is the
    /// default (ungrouped, recent); only the `[board]` table is validated, so a broken
    /// profile never blocks it.
    pub fn load(state_dir: impl AsRef<Path>) -> Result<Self, AgentLoadError> {
        let path = state_dir.as_ref().join(CONFIG_FILE);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(AgentLoadError::Io { path, source }),
        };
        Self::parse(&content)
    }

    fn parse(content: &str) -> Result<Self, AgentLoadError> {
        let document: DocumentMut =
            content
                .parse()
                .map_err(|source| AgentLoadError::MalformedToml {
                    source: Box::new(source),
                })?;
        let invalid = |message: String| AgentLoadError::InvalidDocument {
            message: format!("[board] {message}"),
        };
        let mut defaults = Self::default();
        let Some(board) = document.get("board") else {
            return Ok(defaults);
        };
        let table = board
            .as_table_like()
            .ok_or_else(|| invalid("must be a table".into()))?;
        // A project board never groups or sorts by project: every row shares it.
        let desk_groups = GroupBy::ALL.to_vec();
        let project_groups: Vec<GroupBy> = GroupBy::ALL
            .into_iter()
            .filter(|group| *group != GroupBy::Project)
            .collect();
        let desk_sorts = SortBy::ALL.to_vec();
        let project_sorts: Vec<SortBy> = SortBy::ALL
            .into_iter()
            .filter(|sort| *sort != SortBy::Project)
            .collect();
        for (key, item) in table.iter() {
            // `me` is read on its own ([`OwnerName`]), so a bad name never resets the layout.
            if key == "me" {
                continue;
            }
            if !matches!(
                key,
                "group" | "sort" | "project_group" | "project_sort" | "messages"
            ) {
                return Err(invalid(format!("unknown key {key:?}")));
            }
            let words = |choices: &[&str]| -> String {
                match choices {
                    [] => String::new(),
                    [only] => (*only).to_string(),
                    [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
                }
            };
            let value = item
                .as_str()
                .ok_or_else(|| invalid(format!("{key} must be a string")))?;
            let pick_group = |choices: &[GroupBy]| {
                choices
                    .iter()
                    .copied()
                    .find(|group| group.word() == value)
                    .ok_or_else(|| {
                        let names: Vec<&str> = choices.iter().map(|group| group.word()).collect();
                        invalid(format!("{key} {value:?} is not {}", words(&names)))
                    })
            };
            let pick_sort = |choices: &[SortBy]| {
                choices
                    .iter()
                    .copied()
                    .find(|sort| sort.word() == value)
                    .ok_or_else(|| {
                        let names: Vec<&str> = choices.iter().map(|sort| sort.word()).collect();
                        invalid(format!("{key} {value:?} is not {}", words(&names)))
                    })
            };
            match key {
                "messages" => {
                    defaults.messages_hidden = match value {
                        "shown" => false,
                        "hidden" => true,
                        _ => {
                            return Err(invalid(format!(
                                "messages {value:?} is not shown or hidden"
                            )))
                        }
                    }
                }
                "group" => defaults.group = pick_group(&desk_groups)?,
                "sort" => defaults.sort = pick_sort(&desk_sorts)?,
                "project_group" => defaults.project_group = pick_group(&project_groups)?,
                _ => defaults.project_sort = pick_sort(&project_sorts)?,
            }
        }
        Ok(defaults)
    }
}

/// Typed failures from reading or validating `config.toml`.
#[derive(Debug)]
pub enum AgentLoadError {
    Io { path: PathBuf, source: io::Error },
    MalformedToml { source: Box<toml_edit::TomlError> },
    InvalidDocument { message: String },
    InvalidName { name: String, message: String },
    InvalidProfile { name: String, message: String },
}

impl fmt::Display for AgentLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::MalformedToml { source } => {
                write!(formatter, "config.toml is malformed: {source}")
            }
            Self::InvalidDocument { message } => write!(formatter, "config.toml: {message}"),
            Self::InvalidName { name, message } => {
                write!(
                    formatter,
                    "config.toml: invalid agent name {name:?}: {message}"
                )
            }
            Self::InvalidProfile { name, message } => {
                write!(formatter, "config.toml: agent {name:?}: {message}")
            }
        }
    }
}

impl Error for AgentLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::MalformedToml { source } => Some(source.as_ref()),
            Self::InvalidDocument { .. }
            | Self::InvalidName { .. }
            | Self::InvalidProfile { .. } => None,
        }
    }
}

fn validate_name(name: &str) -> Result<(), AgentLoadError> {
    match normalize_thread(name) {
        Ok(normalized) if normalized == name => Ok(()),
        Ok(normalized) => Err(AgentLoadError::InvalidName {
            name: name.into(),
            message: format!("name must already be normalized as {normalized:?}"),
        }),
        Err(error) => Err(AgentLoadError::InvalidName {
            name: name.into(),
            message: thread_refusal_message(error).replacen("thread", "agent name", 1),
        }),
    }
}

fn invalid_profile(name: &str, message: impl Into<String>) -> AgentLoadError {
    AgentLoadError::InvalidProfile {
        name: name.into(),
        message: message.into(),
    }
}

fn parse_profile(name: &str, table: &dyn TableLike) -> Result<AgentProfile, AgentLoadError> {
    if let Some(key) = table
        .iter()
        .map(|(key, _)| key)
        .find(|key| !matches!(*key, "command" | "prompt" | "env"))
    {
        return Err(invalid_profile(name, format!("unknown key {key:?}")));
    }

    let command = table
        .get("command")
        .and_then(|item| item.as_array())
        .filter(|array| !array.is_empty())
        .ok_or_else(|| invalid_profile(name, "needs a non-empty command array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid_profile(name, "command entries must be strings"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let prompt = table
        .get("prompt")
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid_profile(name, "prompt must be a string"))
        })
        .transpose()?;

    let env = match table.get("env") {
        None => BTreeMap::new(),
        Some(item) => item
            .as_table_like()
            .ok_or_else(|| invalid_profile(name, "env must be a string table"))?
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.to_string(), value.to_string()))
                    .ok_or_else(|| invalid_profile(name, "env values must be strings"))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?,
    };

    Ok(AgentProfile {
        command,
        prompt,
        env,
    })
}

fn render_template(template: &str, context: &RenderContext<'_>) -> String {
    let number = context.number.to_string();
    let replacements = [
        ("{number}", number.as_str()),
        ("{title}", context.title),
        ("{notes}", context.notes),
        ("{steps}", context.steps),
        ("{worktree}", context.worktree),
        ("{branch}", context.branch),
        ("{base}", context.base),
    ];
    let mut rendered = String::with_capacity(template.len());
    let mut remaining = template;
    while !remaining.is_empty() {
        if let Some((placeholder, value)) = replacements
            .iter()
            .find(|(placeholder, _)| remaining.starts_with(placeholder))
        {
            rendered.push_str(value);
            remaining = &remaining[placeholder.len()..];
        } else {
            let character = remaining.chars().next().expect("remaining is not empty");
            rendered.push(character);
            remaining = &remaining[character.len_utf8()..];
        }
    }
    rendered
}

/// One `/bin/sh` single-quoted word. NUL is dropped: no program argument or variable can
/// carry it.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\0', "").replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "tsk-agents-{label}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, content: &str) {
            fs::write(self.0.join("config.toml"), content).expect("write config.toml");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn seeding_an_existing_file_writes_nothing_to_the_state_dir() {
        let dir = TempDir::new("seed-existing");
        dir.write("");
        let before = fs::metadata(dir.path().join("config.toml"))
            .expect("metadata")
            .modified()
            .expect("mtime");

        assert!(!seed_on_open(dir.path()).expect("seed"));

        let names: Vec<String> = fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, vec!["config.toml".to_string()], "{names:?}");
        let after = fs::metadata(dir.path().join("config.toml"))
            .expect("metadata")
            .modified()
            .expect("mtime");
        assert_eq!(before, after);
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml")).expect("read"),
            ""
        );
    }

    fn context<'a>() -> RenderContext<'a> {
        RenderContext {
            number: 101,
            title: "Load profiles",
            notes: "First line\nSecond line",
            steps: "[ ] parse\n[x] validate",
            worktree: "/tmp/tsk-t101",
            branch: "tsk/t101-load-profiles",
            base: "dispatch",
        }
    }

    #[test]
    fn base_placeholder_uses_the_target_branch() {
        let profiles = AgentProfiles::parse("[agent.builder]\ncommand = [\"echo\", \"{base}\"]\nprompt = \"Open the PR into {base}\"\n").unwrap();
        let rendered = profiles.get("builder").unwrap().render(&context());
        assert_eq!(
            rendered.argv,
            vec!["echo", "dispatch", "Open the PR into dispatch"]
        );
    }

    #[test]
    fn missing_file_loads_an_empty_profile_set() {
        let dir = TempDir::new("missing");
        let profiles = AgentProfiles::load(dir.path()).expect("missing file is valid");
        assert!(profiles.is_empty());
    }

    #[test]
    fn valid_file_loads_multiple_profiles() {
        let dir = TempDir::new("multiple");
        dir.write(
            r#"
[agent.implementer]
command = ["pi", "--model", "anthropic/claude"]
prompt = "Work on T{number}: {title}"

[agent.reviewer]
command = ["claude", "--print"]
"#,
        );

        let profiles = AgentProfiles::load(dir.path()).expect("valid profiles");
        assert_eq!(profiles.names().count(), 2);
        assert_eq!(
            profiles.get("implementer").expect("implementer").command,
            ["pi", "--model", "anthropic/claude"]
        );
        assert_eq!(
            profiles
                .get("implementer")
                .expect("implementer")
                .prompt
                .as_deref(),
            Some("Work on T{number}: {title}")
        );
        assert_eq!(
            profiles.get("reviewer").expect("reviewer").command,
            ["claude", "--print"]
        );
    }

    #[test]
    fn malformed_toml_is_a_typed_load_error_with_a_human_message() {
        let dir = TempDir::new("malformed");
        dir.write("[agent.implementer\ncommand = [\"pi\"]");

        let error = AgentProfiles::load(dir.path()).expect_err("malformed file refused");
        assert!(matches!(error, AgentLoadError::MalformedToml { .. }));
        assert!(error.to_string().contains("config.toml"));
    }

    #[test]
    fn unknown_top_level_settings_are_ignored_beside_profiles() {
        let dir = TempDir::new("unknown-top-level");
        dir.write(
            r#"
theme = "future"

[board]
wide = true

[agent.implementer]
command = ["pi"]
"#,
        );

        let profiles = AgentProfiles::load(dir.path()).expect("unknown settings ignored");
        assert_eq!(profiles.names().collect::<Vec<_>>(), vec!["implementer"]);

        dir.write("[board]\nwide = true\n");
        let profiles = AgentProfiles::load(dir.path()).expect("settings without profiles");
        assert!(profiles.is_empty());
    }

    #[test]
    fn merge_method_defaults_to_merge_and_reads_each_value() {
        let dir = TempDir::new("merge-method");
        assert_eq!(MergeMethod::load(dir.path()).unwrap(), MergeMethod::Merge);
        dir.write("[agent.x]\ncommand = [\"x\"]\n");
        assert_eq!(MergeMethod::load(dir.path()).unwrap(), MergeMethod::Merge);
        for (value, method) in [
            ("merge", MergeMethod::Merge),
            ("squash", MergeMethod::Squash),
            ("rebase", MergeMethod::Rebase),
        ] {
            dir.write(&format!("[merge]\nmethod = \"{value}\"\n"));
            assert_eq!(MergeMethod::load(dir.path()).unwrap(), method);
            assert_eq!(method.name(), value);
        }
        // A broken profile never blocks the merge setting, and the reverse.
        dir.write("[merge]\nmethod = \"squash\"\n[agent.x]\nprompt = 1\n");
        assert_eq!(MergeMethod::load(dir.path()).unwrap(), MergeMethod::Squash);
        dir.write("[merge]\nmethod = \"fast-forward\"\n[agent.x]\ncommand = [\"x\"]\n");
        assert_eq!(AgentProfiles::load(dir.path()).unwrap().names().count(), 1);
    }

    #[test]
    fn an_unknown_merge_method_or_key_is_a_clear_config_error() {
        let dir = TempDir::new("merge-method-bad");
        for (content, needle) in [
            (
                "[merge]\nmethod = \"fast-forward\"\n",
                "method \"fast-forward\" is not merge, squash, or rebase",
            ),
            ("[merge]\nmethod = 1\n", "method must be a string"),
            ("[merge]\nmetod = \"squash\"\n", "unknown key \"metod\""),
            ("merge = \"squash\"\n", "[merge] must be a table"),
        ] {
            dir.write(content);
            let error = MergeMethod::load(dir.path()).expect_err(content);
            assert!(
                error.to_string().starts_with("config.toml: [merge]"),
                "{error}"
            );
            assert!(error.to_string().contains(needle), "{error}");
        }
    }

    #[test]
    fn board_defaults_read_each_key_and_default_to_ungrouped_recent() {
        let dir = TempDir::new("board-defaults");
        assert_eq!(
            BoardDefaults::load(dir.path()).unwrap(),
            BoardDefaults::default()
        );
        dir.write("[agent.x]\ncommand = [\"x\"]\n");
        assert_eq!(
            BoardDefaults::load(dir.path()).unwrap(),
            BoardDefaults::default()
        );
        dir.write(
            "[board]\ngroup = \"assignee\"\nsort = \"title\"\n\
             project_group = \"thread\"\nproject_sort = \"assignee\"\n",
        );
        assert_eq!(
            BoardDefaults::load(dir.path()).unwrap(),
            BoardDefaults {
                group: GroupBy::Assignee,
                sort: SortBy::Title,
                project_group: GroupBy::Thread,
                project_sort: SortBy::Assignee,
                messages_hidden: false,
            }
        );
        for group in GroupBy::ALL {
            dir.write(&format!("[board]\ngroup = \"{}\"\n", group.word()));
            assert_eq!(BoardDefaults::load(dir.path()).unwrap().group, group);
        }
        for sort in SortBy::ALL {
            dir.write(&format!("[board]\nsort = \"{}\"\n", sort.word()));
            assert_eq!(BoardDefaults::load(dir.path()).unwrap().sort, sort);
        }
        for (word, hidden) in [("shown", false), ("hidden", true)] {
            dir.write(&format!("[board]\nmessages = \"{word}\"\n"));
            assert_eq!(
                BoardDefaults::load(dir.path()).unwrap().messages_hidden,
                hidden
            );
        }
        // A broken profile never blocks the board defaults, and the reverse.
        dir.write("[board]\ngroup = \"thread\"\n[agent.x]\nprompt = 1\n");
        assert_eq!(
            BoardDefaults::load(dir.path()).unwrap().group,
            GroupBy::Thread
        );
        dir.write("[board]\ngroup = \"status\"\n[agent.x]\ncommand = [\"x\"]\n");
        assert_eq!(AgentProfiles::load(dir.path()).unwrap().names().count(), 1);
    }

    #[test]
    fn an_unknown_board_key_or_value_is_a_clear_config_error() {
        let dir = TempDir::new("board-defaults-bad");
        for (content, needle) in [
            (
                "[board]\ngroup = \"status\"\n",
                "group \"status\" is not none, project, thread, or assignee",
            ),
            (
                "[board]\nsort = \"age\"\n",
                "sort \"age\" is not recent, project, thread, title, or assignee",
            ),
            (
                "[board]\nproject_group = \"project\"\n",
                "project_group \"project\" is not none, thread, or assignee",
            ),
            (
                "[board]\nproject_sort = \"project\"\n",
                "project_sort \"project\" is not recent, thread, title, or assignee",
            ),
            (
                "[board]\nmessages = \"off\"\n",
                "messages \"off\" is not shown or hidden",
            ),
            ("[board]\ngroup = 1\n", "group must be a string"),
            ("[board]\nwide = true\n", "unknown key \"wide\""),
            ("board = \"project\"\n", "[board] must be a table"),
        ] {
            dir.write(content);
            let error = BoardDefaults::load(dir.path()).expect_err(content);
            assert!(
                error.to_string().starts_with("config.toml: [board]"),
                "{error}"
            );
            assert!(error.to_string().contains(needle), "{error}");
        }
    }

    /// `[board] me` is read on its own: a valid name shows for you, a bad one or a profile's
    /// name keeps `you` and says why, and none of it touches the layout defaults.
    #[test]
    fn board_me_names_you_unless_it_is_bad_or_a_profile() {
        let profiles = AgentProfiles::parse("[agent.pi]\ncommand = [\"pi\"]\n").expect("profiles");
        let read = |content: &str| OwnerName::parse(content, &profiles);
        assert_eq!(read(""), OwnerName::default());
        assert_eq!(
            read("[board]\nme = \"Saeed\"\n"),
            OwnerName {
                name: Some("saeed".into()),
                error: None
            }
        );
        for (content, needle) in [
            ("[board]\nme = \"pi\"\n", "me \"pi\" is an agent profile"),
            ("[board]\nme = \"sam smith\"\n", "use letters, numbers"),
            ("[board]\nme = 7\n", "me must be a string"),
        ] {
            let owner = read(content);
            assert_eq!(owner.name, None, "{content}");
            let error = owner.error.expect("refused");
            assert!(error.starts_with("config.toml: [board] "), "{error}");
            assert!(error.contains(needle), "{error}");
        }
        assert_eq!(
            BoardDefaults::parse("[board]\nme = \"pi\"\ngroup = \"thread\"\n")
                .expect("me never breaks the layout")
                .group,
            GroupBy::Thread
        );
    }

    #[test]
    fn starter_board_example_parses_once_uncommented() {
        assert_eq!(
            BoardDefaults::parse(STARTER_CONFIG).expect("starter parses"),
            BoardDefaults::default()
        );
        let mut in_block = false;
        let uncommented = STARTER_CONFIG
            .lines()
            .map(|line| {
                in_block = line == "# [board]" || (in_block && !line.is_empty());
                if in_block {
                    line.strip_prefix("# ").unwrap_or(line)
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            BoardDefaults::parse(&uncommented).expect("uncommented example parses"),
            BoardDefaults {
                group: GroupBy::Project,
                sort: SortBy::Recent,
                project_group: GroupBy::Thread,
                project_sort: SortBy::Title,
                messages_hidden: true,
            }
        );
        assert_eq!(
            OwnerName::parse(&uncommented, &AgentProfiles::default()).name,
            Some("sam".to_string())
        );
    }

    #[test]
    fn unknown_keys_inside_a_profile_still_refuse() {
        let dir = TempDir::new("unknown-profile-key");
        dir.write(
            "[board]\nwide = true\n\n[agent.implementer]\ncommand = [\"pi\"]\ncomand = [\"x\"]\n",
        );

        let error = AgentProfiles::load(dir.path()).expect_err("profile typo refused");
        assert!(matches!(
            error,
            AgentLoadError::InvalidProfile { ref name, .. } if name == "implementer"
        ));
        assert!(error.to_string().contains("config.toml"), "{error}");
        assert!(error.to_string().contains("comand"), "{error}");
    }

    #[test]
    fn a_non_table_agent_key_still_refuses() {
        let dir = TempDir::new("agent-scalar");
        dir.write("agent = \"pi\"\n");

        let error = AgentProfiles::load(dir.path()).expect_err("scalar agent refused");
        assert!(matches!(error, AgentLoadError::InvalidDocument { .. }));
    }

    #[test]
    fn profile_key_must_already_be_normalized() {
        let dir = TempDir::new("bad-key");
        dir.write(
            r#"
[agent.Implementer]
command = ["pi"]
"#,
        );

        let error = AgentProfiles::load(dir.path()).expect_err("mixed-case key refused");
        assert!(matches!(
            error,
            AgentLoadError::InvalidName { ref name, .. } if name == "Implementer"
        ));
        assert!(error.to_string().contains("Implementer"));
    }

    #[test]
    fn command_is_required_and_must_not_be_empty() {
        for (label, content) in [
            ("missing-command", "[agent.implementer]\nprompt = \"Go\"\n"),
            ("empty-command", "[agent.implementer]\ncommand = []\n"),
        ] {
            let dir = TempDir::new(label);
            dir.write(content);
            let error = AgentProfiles::load(dir.path()).expect_err("command refused");
            assert!(matches!(
                error,
                AgentLoadError::InvalidProfile { ref name, .. } if name == "implementer"
            ));
            assert!(error.to_string().contains("non-empty command"));
        }
    }

    #[test]
    fn rendering_substitutes_known_placeholders_including_multiline_text() {
        let profile = AgentProfile {
            command: vec![
                "runner".into(),
                "{number}".into(),
                "{title}".into(),
                "{notes}".into(),
                "{steps}".into(),
                "{worktree}".into(),
                "{branch}".into(),
                "{prompt}".into(),
                "{unknown}".into(),
            ],
            prompt: Some("Task T{number}\n{notes}\n{steps}".into()),
            env: Default::default(),
        };

        let rendered = profile.render(&context());
        assert_eq!(
            rendered.argv,
            [
                "runner",
                "101",
                "Load profiles",
                "First line\nSecond line",
                "[ ] parse\n[x] validate",
                "/tmp/tsk-t101",
                "tsk/t101-load-profiles",
                "{prompt}",
                "{unknown}",
                "Task T101\nFirst line\nSecond line\n[ ] parse\n[x] validate",
            ]
        );
    }

    #[test]
    fn rendering_appends_the_default_prompt_when_command_has_no_placeholders() {
        let profile = AgentProfile {
            command: vec!["fable".into()],
            prompt: None,
            env: Default::default(),
        };

        let rendered = profile.render(&context());
        assert_eq!(
            rendered.argv,
            [
                "fable",
                render_template(DEFAULT_PROMPT, &context()).as_str()
            ]
        );
    }

    #[test]
    fn rendering_appends_a_profile_prompt_instead_of_the_default() {
        let profile = AgentProfile {
            command: vec!["pi".into(), "--model".into(), "opus".into()],
            prompt: Some("Review T{number}: {title}".into()),
            env: Default::default(),
        };

        let rendered = profile.render(&context());
        assert_eq!(
            rendered.argv,
            ["pi", "--model", "opus", "Review T101: Load profiles"]
        );
        assert!(!rendered.argv.last().expect("prompt").contains("tsk guide"));
    }

    #[test]
    fn rendering_uses_the_builtin_prompt_when_profile_omits_one() {
        let profile = AgentProfile {
            command: vec!["pi".into()],
            prompt: None,
            env: Default::default(),
        };

        let rendered = profile.render(&context());
        assert_eq!(
            rendered.argv.last().expect("appended prompt"),
            "You were dispatched to T101 (Load profiles) in worktree /tmp/tsk-t101 on branch \
tsk/t101-load-profiles, based on dispatch.

1. Run `tsk guide`, then `tsk list 101 --json`. The task notes are your brief.
2. Read the repo's agent instructions (AGENTS.md or CLAUDE.md) if present.
3. Work only on tsk/t101-load-profiles. Run the project's checks before saying you are done.
4. Push and open a pull request into dispatch. Never merge it.
5. Set the task to review with what you did and what to check: `tsk status 101 review --done \
\"…\" --check \"…\"`, one `--check` per thing to verify, adding `--next` for what comes after. \
When you need a human, block it with your question: `tsk status 101 blocked --why \"…\"`, adding \
`--needs` and one `--option` per choice, then stop.
6. On a relaunch, first read whichever record closed last (`closed_at`) in `tsk list 101 --json`: \
the `replies` of the last `past_blocks` entry, or the `feedback` and failed `checks` of the last \
`past_reviews` entry when it was `sent_back`.
7. A message `[tsk T101 sent back] …` is review feedback, with any failed checks: address it, \
then set review again."
        );
    }

    /// Run a Unix launcher the way a pane does: `pane -c '<typed line>'`, with `SHELL` set to
    /// `login`. A fake agent records its arguments and whether `TSK_TEST_KEY` arrived.
    #[cfg(unix)]
    fn run_unix_launcher(
        dir: &TempDir,
        pane: &str,
        login: &str,
        title: &str,
    ) -> (Vec<String>, String) {
        use std::os::unix::fs::PermissionsExt;
        let agent = dir.path().join("agent");
        fs::write(
            &agent,
            "#!/bin/sh\nfor argument; do printf '%s\\0' \"$argument\"; done > \"$TSK_TEST_OUT\"\n\
             printf '%s' \"${TSK_TEST_KEY+set}\" > \"$TSK_TEST_OUT.env\"\n",
        )
        .expect("agent");
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).expect("chmod");
        let profile = AgentProfile {
            command: vec![agent.to_string_lossy().into_owned(), "{title}".into()],
            prompt: Some("Do T{number}: {title}".into()),
            env: [("TSK_TEST_KEY".to_string(), "secret value".to_string())].into(),
        };
        let rendered = profile.render(&RenderContext { title, ..context() });
        let launchers = dir.path().join("launchers");
        fs::create_dir_all(&launchers).expect("launchers");
        let launcher = launchers.join("dispatch-w1.sh");
        let launcher_text = launcher.to_string_lossy().into_owned();
        fs::write(&launcher, rendered.shell_script(&launcher_text)).expect("launcher");
        let out = dir.path().join("out");
        let _ = fs::remove_file(&out);
        let status = Command::new(pane)
            .arg("-c")
            .arg(format!("/bin/sh \"{launcher_text}\""))
            .current_dir(dir.path())
            .env("SHELL", login)
            .env("HOME", dir.path())
            .env("ZDOTDIR", dir.path())
            .env("TSK_TEST_OUT", &out)
            .env_remove("TSK_TEST_KEY")
            .status()
            .expect("run pane shell");
        assert!(status.success(), "{pane} / {login}: {status}");
        assert!(!launcher.exists(), "the launcher deletes itself");
        let received = fs::read(&out).expect("agent ran");
        let arguments = String::from_utf8(received)
            .expect("utf-8")
            .split_terminator('\0')
            .map(str::to_string)
            .collect();
        let env = fs::read_to_string(dir.path().join("out.env")).expect("env mark");
        (arguments, env)
    }

    #[cfg(unix)]
    const HOSTILE_TITLES: &[&str] = &[
        "Don't; touch pwned",
        "x\\'; touch pwned; echo '",
        "\\'(touch pwned)\\'",
        "$(touch pwned) and `touch pwned` and ${HOME}",
        "say \"hi\"; touch pwned",
        "two\nlines; touch pwned",
        "del\u{7f}\u{7f}\u{7f}\u{7f}touch pwned",
        "ctrl \u{15}\u{3}\u{1b}[A touch pwned",
    ];

    /// Every POSIX shell on the machine, as the pane and as the login shell: task text and
    /// `env` values reach the agent byte for byte and nothing in them runs.
    #[cfg(unix)]
    #[test]
    fn a_unix_launcher_hands_hostile_titles_to_the_agent_verbatim_under_every_shell() {
        let shells: Vec<&str> = [
            "/bin/sh",
            "/bin/bash",
            "/bin/dash",
            "/bin/zsh",
            "/usr/bin/zsh",
            "/opt/homebrew/bin/zsh",
        ]
        .into_iter()
        .filter(|shell| Path::new(shell).exists())
        .collect();
        assert!(shells.len() >= 2, "{shells:?}");
        for shell in shells {
            for title in HOSTILE_TITLES {
                let dir = TempDir::new("launch");
                let (arguments, env) = run_unix_launcher(&dir, shell, shell, title);
                assert_eq!(
                    arguments,
                    [title.to_string(), format!("Do T101: {title}")],
                    "{shell}"
                );
                assert_eq!(env, "set", "{shell}: the env value arrives");
                assert!(!dir.path().join("pwned").exists(), "{shell}: {title:?}");
            }
        }
    }

    /// The login shell, whatever it is (fish, nushell…), is handed only a fixed command
    /// naming the launcher: it never parses task text or `env` values. A recording shell
    /// stands in for one that is not POSIX.
    #[cfg(unix)]
    #[test]
    fn the_login_shell_parses_only_a_fixed_command_without_task_text() {
        use std::os::unix::fs::PermissionsExt;
        for title in HOSTILE_TITLES {
            let dir = TempDir::new("login");
            let login = dir.path().join("login-shell");
            fs::write(
                &login,
                "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$TSK_TEST_OUT.login\"\n\
                 [ \"$1\" = -l ] && [ \"$2\" = -c ] || exit 9\nexec /bin/sh -c \"$3\"\n",
            )
            .expect("login shell");
            fs::set_permissions(&login, fs::Permissions::from_mode(0o755)).expect("chmod");
            let (arguments, env) =
                run_unix_launcher(&dir, "/bin/sh", &login.to_string_lossy(), title);
            assert_eq!(arguments[0], *title);
            assert_eq!(env, "set");
            let recorded = fs::read_to_string(dir.path().join("out.login")).expect("recorded");
            let launcher = dir.path().join("launchers").join("dispatch-w1.sh");
            assert_eq!(
                recorded,
                format!("-l\0-c\0/bin/sh \"{}\" login\0", launcher.display())
            );
        }
    }

    #[test]
    fn a_unix_launcher_keeps_env_values_and_task_text_off_its_typed_line() {
        let profile = AgentProfile {
            command: vec!["agent".into()],
            prompt: Some("{title}".into()),
            env: [("API_TOKEN".to_string(), "it's secret".to_string())].into(),
        };
        let rendered = profile.render(&context());
        let script = rendered.shell_script("/state/launchers/dispatch-w1.sh");
        let (head, start) = script.rsplit_once("\nexec ").expect("final exec");
        assert!(
            !head.contains("secret") && !head.contains("Load profiles"),
            "{head}"
        );
        assert_eq!(
            start,
            "env 'API_TOKEN=it'\\''s secret' 'agent' 'Load profiles'\n"
        );
        assert!(
            head.contains("-l -c '/bin/sh \"/state/launchers/dispatch-w1.sh\" login'"),
            "{head}"
        );
        assert_eq!(
            rendered.argv,
            ["agent", "Load profiles"],
            "no env in the argv"
        );
    }

    /// Like `TskProgram`: only a name whose every match is a batch file is batch-only, in
    /// `PATH` order; an executable or `.ps1` anywhere, or no match at all, is left to the pane.
    #[test]
    fn batch_only_program_finds_a_name_that_resolves_only_to_batch_files() {
        let first = TempDir::new("batch-first");
        let second = TempDir::new("batch-second");
        let touch = |dir: &TempDir, name: &str| {
            fs::write(dir.path().join(name), "").expect("touch");
            dir.path().join(name)
        };
        let path = std::ffi::OsString::from(format!(
            "{};\"{}\"",
            first.path().display(),
            second.path().display()
        ));
        let pathext = std::ffi::OsString::from(".exe;.bat;.cmd");
        let resolve = |program: &str| batch_only_program(program, Some(&path), Some(&pathext));

        let cmd = touch(&first, "agent.cmd");
        touch(&second, "agent.bat");
        assert_eq!(resolve("agent"), Some(cmd.clone()));
        assert_eq!(resolve("agent.cmd"), Some(cmd.clone()));
        touch(&second, "agent.exe");
        assert_eq!(resolve("agent"), None, "an executable later in PATH runs");
        touch(&first, "shim.cmd");
        touch(&first, "shim.ps1");
        assert_eq!(resolve("shim"), None, "a PowerShell script runs");
        assert_eq!(resolve("missing"), None, "nothing found: the pane says so");
        assert_eq!(
            resolve(&cmd.to_string_lossy()),
            Some(cmd.clone()),
            "an absolute path is checked itself"
        );
        assert_eq!(resolve(r"tools\agent"), None, "relative to the worktree");
        assert_eq!(
            batch_only_program("agent", None, Some(&pathext)),
            None,
            "no PATH"
        );

        let refusal = batch_only_refusal("agent", &cmd);
        assert!(refusal.starts_with(&format!(
            "not starting agent: it resolves only to the batch file {}, ",
            cmd.display()
        )));
        assert!(refusal.ends_with(BATCH_ONLY_ADVICE));
        assert!(
            POWERSHELL_PRELUDE.contains(&format!("$($Found[0].Path), {BATCH_ONLY_ADVICE}\")")),
            "the launcher refuses in the same words"
        );
    }

    #[test]
    fn powershell_launcher_embeds_env_program_and_arguments_encoded_in_order() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let profile = AgentProfile {
            command: vec!["claude".into(), "--add-dir".into(), "{worktree}".into()],
            prompt: Some("Don't \"quote\" $HOME %PATH% `x`; T{number}\ncafé".into()),
            env: [(
                "GIT_AUTHOR_NAME".to_string(),
                "a \"b\" (via tsk)".to_string(),
            )]
            .into(),
        };
        let rendered = profile.render(&RenderContext {
            worktree: r"C:\Users\Some One\.herdr\worktrees\app\tsk-t101",
            ..context()
        });
        let script = rendered.powershell_script();

        assert!(script.is_ascii(), "{script}");
        assert!(script.starts_with(POWERSHELL_PRELUDE));
        let values = script
            .split("(TskText '")
            .skip(1)
            .map(|rest| {
                let encoded = &rest[..rest.find('\'').expect("closing quote")];
                String::from_utf8(STANDARD.decode(encoded).expect("base64")).expect("utf-8")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            [
                "GIT_AUTHOR_NAME",
                "a \"b\" (via tsk)",
                "claude",
                "--add-dir",
                r"C:\Users\Some One\.herdr\worktrees\app\tsk-t101",
                "Don't \"quote\" $HOME %PATH% `x`; T101\ncafé",
            ]
        );
        assert_eq!(
            script.matches("(TskArgument ").count(),
            3,
            "program is not an argument"
        );
        assert!(script.ends_with("& $TskCommand @TskArguments\nexit $LASTEXITCODE\n"));
    }

    /// The default prompt as the starter file quotes it, unwrapped: `#   ` prefixes stripped,
    /// a bare `#` is a paragraph break, a `N. ` line starts a new line, and any other line
    /// continues the one before it.
    fn starter_quoted_default_prompt() -> String {
        let lines = STARTER_CONFIG
            .lines()
            .skip_while(|line| !line.starts_with("# The default prompt, used when"))
            .skip(2)
            .take_while(|line| !line.starts_with("# Remove the leading #"))
            .collect::<Vec<_>>();
        let mut prompt = String::new();
        for line in lines {
            if line == "#" {
                prompt.push_str("\n\n");
                continue;
            }
            let text = line.strip_prefix("#   ").expect("quoted prompt line");
            let starts_item = text.split_once(". ").is_some_and(|(number, _)| {
                !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
            });
            if prompt.is_empty() || prompt.ends_with('\n') {
                prompt.push_str(text);
            } else if starts_item {
                prompt.push('\n');
                prompt.push_str(text);
            } else {
                prompt.push(' ');
                prompt.push_str(text.trim_start());
            }
        }
        prompt.trim_end().to_string()
    }

    #[test]
    fn starter_file_quotes_the_default_prompt_verbatim() {
        assert_eq!(starter_quoted_default_prompt(), DEFAULT_PROMPT);
    }

    #[test]
    fn starter_file_has_no_active_profiles_and_every_example_parses() {
        assert!(AgentProfiles::parse(STARTER_CONFIG)
            .expect("starter parses")
            .is_empty());

        // Uncomment each block from its `# [agent.` header to the next blank line, as the
        // header tells the user to; prose comments around the blocks stay comments.
        let mut in_block = false;
        let examples = STARTER_CONFIG
            .lines()
            .map(|line| {
                in_block = line.starts_with("# [agent.") || (in_block && !line.is_empty());
                if in_block {
                    line.strip_prefix("# ")
                        .unwrap_or(line.trim_start_matches('#'))
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let profiles = AgentProfiles::parse(&examples).expect("uncommented examples parse");
        assert_eq!(
            profiles.names().collect::<Vec<_>>(),
            ["claude", "claude-win", "codex", "grok", "my-agent", "pi"]
        );
        assert_eq!(
            profiles.get("claude-win").expect("windows example").command,
            [r"C:\Users\you\.local\bin\claude.exe"]
        );
        let grok = profiles.get("grok").expect("grok example");
        assert!(grok
            .prompt
            .as_deref()
            .expect("grok prompt")
            .starts_with("Fix T{number}: {title}\n\n{notes}"));
        assert_eq!(
            profiles.get("my-agent").expect("generic example").env["GIT_AUTHOR_NAME"],
            "my-agent (via tsk)"
        );
        assert!(profiles.get("codex").expect("codex").prompt.is_none());
    }

    #[test]
    fn environment_is_loaded_and_passed_through_when_rendering() {
        let dir = TempDir::new("env");
        dir.write(
            r#"
[agent.implementer]
command = ["pi"]

[agent.implementer.env]
PI_MODEL = "anthropic/claude"
LITERAL = "{branch}"
"#,
        );

        let profiles = AgentProfiles::load(dir.path()).expect("valid environment");
        let rendered = profiles
            .get("implementer")
            .expect("implementer")
            .render(&context());
        assert_eq!(rendered.env["PI_MODEL"], "anthropic/claude");
        assert_eq!(rendered.env["LITERAL"], "{branch}");
        let script = rendered.shell_script("/state/launchers/dispatch-w1.sh");
        assert!(
            script.contains("exec env 'LITERAL={branch}' 'PI_MODEL=anthropic/claude' 'pi'"),
            "{script}"
        );
    }
}
