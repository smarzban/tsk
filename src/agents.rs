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

use crate::domain::{normalize_thread, thread_refusal_message};

const CONFIG_FILE: &str = "config.toml";
pub const CONFIG_TEMP_PREFIX: &str = ".config.toml.tmp.";
static CONFIG_TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The commented `config.toml` a full board open seeds. Every profile is commented out, and
/// the quoted default prompt must stay equal to [`DEFAULT_PROMPT`] (pinned by a test).
pub const STARTER_CONFIG: &str = r##"# tsk configuration
#
# Agent profiles let you assign a task to an agent (`!a name` or `@`).
# Starting an assigned task (ctrl+s) launches it: tsk creates a git worktree
# for the task, opens a Herdr workspace there, and runs the profile's command.
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
#   {number}    task number, e.g. 158
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
#   6. On a relaunch, read the answers to your last question first: a
#      relaunch closes the block, so they are the `replies` of the last
#      `past_blocks` entry in `tsk list {number} --json`.
#   7. A message `[tsk T{number} sent back] …` is review feedback, with any
#      failed checks: address it, then set review again.
#
# Remove the leading # from a block below to use it.

# --- Examples using the default prompt -------------------------------------

# [agent.claude]
# command = ["claude"]

# [agent.sol]
# command = ["codex", "--model", "gpt-6.1-sol"]

# [agent.pi-opus]
# command = ["pi", "--model", "anthropic/claude-opus-5-5", "--thinking", "high"]

# --- Examples with their own prompt -----------------------------------------

# A quick fixer for small tasks, with the task text inlined.
# [agent.grok]
# command = ["grok", "--model", "grok-4.7"]
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

# Any other terminal agent: put its program and flags in `command`.
# `env` sets environment variables for that agent only. For example, mark
# the agent's commits so they are easy to tell apart from yours.
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

# [agent.pi-win]
# command = ["pi", "--model", "anthropic/claude-opus-5-5", "--thinking", "high"]
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

/// Prompt used by a profile that does not define its own template. It assumes a git
/// worktree, which dispatch guarantees (non-git projects refuse before rendering).
pub const DEFAULT_PROMPT: &str = "You were dispatched to T{number} ({title}) in worktree {worktree} on branch {branch}, based on {base}.

1. Run `tsk guide`, then `tsk list {number} --json`. The task notes are your brief.
2. Read the repo's agent instructions (AGENTS.md or CLAUDE.md) if present.
3. Work only on {branch}. Run the project's checks before saying you are done.
4. Push and open a pull request into {base}. Never merge it.
5. Set the task to review with what you did and what to check: `tsk status {number} review --done \"…\" --check \"…\"`, one `--check` per thing to verify, adding `--next` for what comes after. When you need a human, block it with your question: `tsk status {number} blocked --why \"…\"`, adding `--needs` and one `--option` per choice, then stop.
6. On a relaunch, read the answers to your last question first: a relaunch closes the block, so they are the `replies` of the last `past_blocks` entry in `tsk list {number} --json`.
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
}

/// One named launch profile from `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub command: Vec<String>,
    pub prompt: Option<String>,
    pub env: BTreeMap<String, String>,
}

impl AgentProfile {
    /// Substitute this profile for one task and quote it as one login-shell command.
    pub fn render(&self, context: &RenderContext<'_>) -> RenderedLaunch {
        let prompt = render_template(self.prompt.as_deref().unwrap_or(DEFAULT_PROMPT), context);
        let mut argv = self
            .command
            .iter()
            .map(|argument| render_template(argument, context))
            .collect::<Vec<_>>();
        argv.push(prompt);
        let mut shell_parts = Vec::new();
        if !self.env.is_empty() {
            shell_parts.push(shell_quote("env"));
            shell_parts.extend(
                self.env
                    .iter()
                    .map(|(key, value)| shell_quote(&format!("{key}={value}"))),
            );
        }
        shell_parts.extend(argv.iter().map(|argument| shell_quote(argument)));
        let shell_argv = shell_parts.join(" ");
        RenderedLaunch {
            command: format!("$SHELL -lc {}", shell_quote(&shell_argv)),
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
    pub command: String,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl RenderedLaunch {
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

fn base64_text(value: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(value.as_bytes())
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

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
        assert!(rendered.command.starts_with("$SHELL -lc "));
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

    // Runs the rendered POSIX line under /bin/sh; Windows launches through a PowerShell script.
    #[cfg(unix)]
    #[test]
    fn rendering_shell_quotes_a_prompt_containing_a_single_quote() {
        let profile = AgentProfile {
            command: vec!["printf".into()],
            prompt: Some("Don't; printf hacked".into()),
            env: Default::default(),
        };

        let rendered = profile.render(&context());
        // `SHELL=/bin/sh $SHELL ...` would expand `$SHELL` before the assignment applies and run
        // the ambient shell. Pin the shell by substituting the token in the rendered line instead.
        let pinned = rendered
            .command
            .strip_prefix("$SHELL ")
            .map(|rest| format!("/bin/sh {rest}"))
            .expect("rendered command starts with $SHELL");
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(pinned)
            .env_remove("SHELL")
            .output()
            .expect("run rendered command");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).expect("utf-8"),
            "Don't; printf hacked"
        );
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
6. On a relaunch, read the answers to your last question first: a relaunch closes the block, so they are the `replies` of the last `past_blocks` entry in `tsk list 101 --json`.
7. A message `[tsk T101 sent back] …` is review feedback, with any failed checks: address it, \
then set review again."
        );
    }

    // Runs the rendered POSIX line under /bin/sh; Windows launches through a PowerShell script.
    #[cfg(unix)]
    #[test]
    fn rendering_the_builtin_prompt_survives_shell_quoting_of_a_hostile_title() {
        let profile = AgentProfile {
            command: vec!["printf".into(), "%s".into()],
            prompt: None,
            env: Default::default(),
        };
        let context = RenderContext {
            title: "Don't run `rm -rf $HOME`; echo \"hi\"",
            ..context()
        };

        let rendered = profile.render(&context);
        let pinned = rendered
            .command
            .strip_prefix("$SHELL ")
            .map(|rest| format!("/bin/sh {rest}"))
            .expect("rendered command starts with $SHELL");
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(pinned)
            .env_remove("SHELL")
            .output()
            .expect("run rendered command");
        assert!(output.status.success());
        let printed = String::from_utf8(output.stdout).expect("utf-8");
        assert_eq!(&printed, rendered.argv.last().expect("appended prompt"));
        assert!(printed.starts_with(
            "You were dispatched to T101 (Don't run `rm -rf $HOME`; echo \"hi\") in worktree"
        ));
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
            [
                "claude",
                "claude-win",
                "grok",
                "my-agent",
                "pi-opus",
                "pi-win",
                "sol"
            ]
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
        assert!(profiles.get("sol").expect("sol").prompt.is_none());
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
        assert!(rendered.command.contains("'env'"), "{}", rendered.command);
        assert!(
            rendered.command.contains("'PI_MODEL=anthropic/claude'"),
            "{}",
            rendered.command
        );
    }
}
