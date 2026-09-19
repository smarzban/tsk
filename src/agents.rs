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

const AGENTS_FILE: &str = "agents.toml";
const AGENTS_TEMP_PREFIX: &str = ".agents.toml.tmp.";
static AGENTS_TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

const STARTER_AGENTS: &str = "# tsk agent profiles. Assign with `!a name`, dispatch with ctrl+g.\n\
# Placeholders in command and prompt: {number} {title} {notes} {steps} {worktree} {branch}\n\
# The prompt is appended to the command as its last argument. Omit `prompt` for the default:\n\
#   You were dispatched to T{number} in this worktree. Run `tsk guide`, then `tsk list {number}`.\n\
#   Set the task to review when done, or blocked when a human is needed.\n\
\n\
# [agent.grok]\n\
# command = [\"pi\", \"--model\", \"xai/grok-4.6\", \"--thinking\", \"high\"]\n\
\n\
# [agent.opus]\n\
# command = [\"claude\", \"--model\", \"opus\", \"--effort\", \"high\"]\n\
# prompt = \"Review the branch for T{number}: {title}. Leave findings as steps on the task, then set review.\"\n\
\n\
# [agent.fable]\n\
# command = [\"fable\"]\n";

/// Seed the commented profile examples on a full board open.
///
/// The temporary file is complete and synced before one atomic hard-link creates the target.
/// A target that already exists wins without being changed, including an empty file.
pub fn seed_on_open(state_dir: &Path) -> io::Result<bool> {
    crate::fsperm::ensure_private_dir(state_dir)?;
    let target = state_dir.join(AGENTS_FILE);
    let tmp = unique_tmp_path(state_dir);
    let write_result = (|| -> io::Result<bool> {
        let mut temp_file = create_private_temp(&tmp)?;
        temp_file.write_all(STARTER_AGENTS.as_bytes())?;
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
    let sequence = AGENTS_TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(
        "{AGENTS_TEMP_PREFIX}{}.{nanos}.{sequence}",
        std::process::id()
    ))
}

/// Prompt used by a profile that does not define its own template.
pub const DEFAULT_PROMPT: &str = "You were dispatched to T{number} in this worktree. Run `tsk guide`, then `tsk list {number}`. Set the task to review when done, or blocked when a human is needed.";

/// All agent profiles loaded from one state directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentProfiles {
    profiles: BTreeMap<String, AgentProfile>,
}

impl AgentProfiles {
    /// Load `<state_dir>/agents.toml`. A missing file is an empty profile set.
    pub fn load(state_dir: impl AsRef<Path>) -> Result<Self, AgentLoadError> {
        let path = state_dir.as_ref().join(AGENTS_FILE);
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
        if document.is_empty() {
            return Ok(Self::default());
        }
        if document.len() != 1 {
            return Err(AgentLoadError::InvalidDocument {
                message: "only [agent.<name>] tables are allowed".into(),
            });
        }
        let agents = document
            .get("agent")
            .and_then(|item| item.as_table_like())
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

    pub fn iter(&self) -> impl Iterator<Item = (&str, &AgentProfile)> {
        self.profiles
            .iter()
            .map(|(name, profile)| (name.as_str(), profile))
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

/// One named launch profile from `agents.toml`.
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
        let shell_argv = argv
            .iter()
            .map(|argument| shell_quote(argument))
            .collect::<Vec<_>>()
            .join(" ");
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
}

/// Fully substituted values ready for a launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedLaunch {
    pub command: String,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// Typed failures from reading or validating `agents.toml`.
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
                write!(formatter, "agents.toml is malformed: {source}")
            }
            Self::InvalidDocument { message } => write!(formatter, "agents.toml: {message}"),
            Self::InvalidName { name, message } => {
                write!(
                    formatter,
                    "agents.toml: invalid agent name {name:?}: {message}"
                )
            }
            Self::InvalidProfile { name, message } => {
                write!(formatter, "agents.toml: agent {name:?}: {message}")
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
            fs::write(self.0.join("agents.toml"), content).expect("write agents.toml");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn context<'a>() -> RenderContext<'a> {
        RenderContext {
            number: 101,
            title: "Load profiles",
            notes: "First line\nSecond line",
            steps: "[ ] parse\n[x] validate",
            worktree: "/tmp/tsk-t101",
            branch: "tsk/t101-load-profiles",
        }
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
        assert_eq!(profiles.len(), 2);
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
        assert!(error.to_string().contains("agents.toml"));
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
            ["fable", DEFAULT_PROMPT.replace("{number}", "101").as_str()]
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
        let prompt = rendered.argv.last().expect("appended prompt");
        assert_eq!(prompt, &DEFAULT_PROMPT.replace("{number}", "101"));
        assert!(prompt.contains("tsk guide"));
        assert!(prompt.contains("tsk list 101"));
        assert!(prompt.contains("review"));
        assert!(prompt.contains("blocked"));
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
    }
}
