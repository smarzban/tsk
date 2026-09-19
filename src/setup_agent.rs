//! Install the embedded agent skill into a host skills directory.

use std::env;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};

use crate::cli::guide::SKILL_MD;

const SKILL_FOLDER: &str = "tsk-cli";
const SKILL_FILE: &str = "SKILL.md";

pub const USAGE: &str = "usage: tsk setup [herdr | agents | claude | pi | omp | cursor | grok | codex | opencode | --skill-dir <path>] [--yes] [--force] [--json]\n       tsk setup --detected-ids | --skill-states\n       tsk setup herdr --check";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Claude,
    Pi,
    Omp,
    Cursor,
    Grok,
    Codex,
    OpenCode,
    SkillDir(PathBuf),
}

impl Target {
    pub fn name(&self) -> &str {
        match self {
            Self::Claude => "claude",
            Self::Pi => "pi",
            Self::Omp => "omp",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::SkillDir(_) => "skill-dir",
        }
    }

    pub fn skills_root(&self) -> Result<PathBuf, Error> {
        match self {
            Self::SkillDir(path) => Ok(path.clone()),
            Self::Claude => Ok(home_dir()?.join(".claude/skills")),
            Self::Pi => Ok(home_dir()?.join(".pi/agent/skills")),
            Self::Omp => Ok(omp_agent_dir()?.join("skills")),
            Self::Cursor => Ok(home_dir()?.join(".cursor/skills")),
            Self::Grok => Ok(home_dir()?.join(".grok/skills")),
            Self::Codex => Ok(home_dir()?.join(".agents/skills")),
            // OpenCode-native global skills root per https://opencode.ai/docs/skills/
            // (project-local `.opencode/skills` is intentionally out of scope).
            Self::OpenCode => Ok(home_dir()?.join(".config/opencode/skills")),
        }
    }

    pub fn skill_path(&self) -> Result<PathBuf, Error> {
        Ok(self.skills_root()?.join(SKILL_FOLDER).join(SKILL_FILE))
    }

    fn named_agents() -> [Target; 7] {
        [
            Target::Claude,
            Target::Pi,
            Target::Omp,
            Target::Cursor,
            Target::Grok,
            Target::Codex,
            Target::OpenCode,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillState {
    Missing,
    Current,
    Outdated,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    pub id: String,
    pub skills_root: PathBuf,
    pub skill_path: PathBuf,
    pub present: bool,
    pub installed_version: Option<String>,
    pub state: SkillState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    Written(PathBuf),
    Updated {
        path: PathBuf,
        previous: Option<String>,
    },
}

impl InstallOutcome {
    pub fn path(&self) -> &Path {
        match self {
            Self::Written(path) => path,
            Self::Updated { path, .. } => path,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Written(_) => "written",
            Self::Updated { .. } => "updated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchResult {
    pub applied: Vec<(String, InstallOutcome)>,
    pub skipped_current: Vec<String>,
    pub blocked: Vec<String>,
    pub declined: bool,
    pub none_detected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    List {
        json: bool,
    },
    DetectedIds,
    /// Installer probe: one tab-separated line per detected agent with its skill state.
    SkillStates,
    /// Installer probe: whether both plugin commands are already bound in the Herdr config.
    HerdrCheck,
    Interactive {
        json: bool,
    },
    AgentsYes {
        json: bool,
        force: bool,
    },
    Herdr,
    Skill {
        target: Target,
        force: bool,
        json: bool,
    },
}

#[derive(Debug)]
pub enum Error {
    Usage(String),
    Exists(PathBuf),
    Symlink(PathBuf),
    Home,
    Io(String),
    InvalidOmpProfile(String),
    Ended,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(reason) => write!(f, "{reason}"),
            Self::Exists(_) => write!(f, "skill-exists"),
            Self::Symlink(path) => write!(f, "refusing symlink: {}", path.display()),
            Self::Home => write!(f, "HOME is not set"),
            Self::Io(detail) => write!(f, "{detail}"),
            Self::InvalidOmpProfile(profile) => write!(
                f,
                "invalid OMP profile {profile:?}; expected [a-z0-9][a-z0-9._-]{{0,63}}, not ending in a dot or using a reserved device name"
            ),
            Self::Ended => write!(f, "confirmation ended; no changes made"),
        }
    }
}

pub fn parse(args: &[String]) -> Result<Command, Error> {
    let tail = args.get(2..).unwrap_or(&[]);
    if tail.iter().any(|arg| arg == "--help") {
        return Ok(Command::Help);
    }

    let mut herdr = false;
    let mut agents_cmd = false;
    let mut yes = false;
    let mut force = false;
    let mut json = false;
    let mut detected_ids = false;
    let mut skill_states = false;
    let mut check = false;
    let mut agents: Vec<Target> = Vec::new();
    let mut skill_dir: Option<PathBuf> = None;
    let mut index = 0;
    while index < tail.len() {
        let arg = tail[index].as_str();
        match arg {
            "herdr" => {
                if herdr {
                    return Err(usage());
                }
                herdr = true;
            }
            "agents" => {
                if agents_cmd {
                    return Err(usage());
                }
                agents_cmd = true;
            }
            "claude" => push_agent(&mut agents, Target::Claude)?,
            "pi" => push_agent(&mut agents, Target::Pi)?,
            "omp" => push_agent(&mut agents, Target::Omp)?,
            "cursor" => push_agent(&mut agents, Target::Cursor)?,
            "grok" => push_agent(&mut agents, Target::Grok)?,
            "codex" => push_agent(&mut agents, Target::Codex)?,
            "opencode" => push_agent(&mut agents, Target::OpenCode)?,
            "--yes" => yes = true,
            "--force" => force = true,
            "--json" => json = true,
            "--detected-ids" => detected_ids = true,
            "--skill-states" => skill_states = true,
            "--check" => check = true,
            "--skill-dir" => {
                index += 1;
                let Some(value) = tail.get(index) else {
                    return Err(usage());
                };
                if value.is_empty() || value.starts_with('-') || skill_dir.is_some() {
                    return Err(usage());
                }
                skill_dir = Some(PathBuf::from(value));
            }
            flag if flag.starts_with("--skill-dir=") => {
                let value = &flag["--skill-dir=".len()..];
                if value.is_empty() || skill_dir.is_some() {
                    return Err(usage());
                }
                skill_dir = Some(PathBuf::from(value));
            }
            _ => return Err(usage()),
        }
        index += 1;
    }

    if detected_ids || skill_states {
        if herdr
            || agents_cmd
            || yes
            || force
            || json
            || check
            || !agents.is_empty()
            || skill_dir.is_some()
            || (detected_ids && skill_states)
        {
            return Err(usage());
        }
        return Ok(if detected_ids {
            Command::DetectedIds
        } else {
            Command::SkillStates
        });
    }

    let named = agents.len() + usize::from(skill_dir.is_some());
    if herdr {
        if named > 0 || force || json || yes || agents_cmd {
            return Err(usage());
        }
        return Ok(if check {
            Command::HerdrCheck
        } else {
            Command::Herdr
        });
    }
    if check {
        return Err(usage());
    }
    if agents_cmd {
        if named > 0 {
            return Err(usage());
        }
        if yes {
            return Ok(Command::AgentsYes { json, force });
        }
        // `--force` is an unattended intent: it only pairs with `--yes`.
        if force {
            return Err(usage());
        }
        return Ok(Command::Interactive { json });
    }
    if named > 1 {
        return Err(usage());
    }
    if named == 0 {
        if force || yes {
            return Err(usage());
        }
        return Ok(Command::List { json });
    }
    if yes {
        return Err(usage());
    }
    let target = if let Some(path) = skill_dir {
        Target::SkillDir(path)
    } else {
        agents.pop().expect("one named agent")
    };
    Ok(Command::Skill {
        target,
        force,
        json,
    })
}

/// Bare `tsk setup` becomes interactive on a TTY; otherwise lists guidance.
pub fn parse_bare(args: &[String], interactive: bool) -> Result<Command, Error> {
    let command = parse(args)?;
    match command {
        Command::List { json } if interactive && !json => Ok(Command::Interactive { json: false }),
        other => Ok(other),
    }
}

pub fn embedded_skill_version() -> String {
    frontmatter_version(SKILL_MD).expect("embedded SKILL.md must declare version")
}

pub fn frontmatter_version(md: &str) -> Option<String> {
    let rest = md.strip_prefix("---\n")?;
    let close = rest.find("\n---\n")?;
    let yaml = &rest[..close];
    for line in yaml.lines() {
        let line = line.trim();
        let Some(value) = line.strip_prefix("version:") else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        return normalize_version(value);
    }
    None
}

fn normalize_version(raw: &str) -> Option<String> {
    let value = raw.strip_prefix('v').unwrap_or(raw).trim();
    let mut parts = value.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if [major, minor, patch]
        .iter()
        .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some(format!("{major}.{minor}.{patch}"))
}

pub fn install(target: &Target, force: bool) -> Result<InstallOutcome, Error> {
    let root = target.skills_root()?;
    if root.as_os_str().is_empty() {
        return Err(usage());
    }
    let folder = root.join(SKILL_FOLDER);
    let dest = folder.join(SKILL_FILE);
    refuse_symlink(&root)?;
    refuse_symlink(&folder)?;
    refuse_symlink(&dest)?;

    let previous = if path_present(&dest)? {
        let existing = fs::read_to_string(&dest).map_err(io_error)?;
        let installed = frontmatter_version(&existing);
        if !force {
            if let Some(ref version) = installed {
                if version.as_str() == embedded_skill_version().as_str() {
                    return Err(Error::Exists(dest));
                }
            }
        }
        Some(installed)
    } else {
        None
    };

    let had_file = previous.is_some();
    let previous_version = previous.flatten();

    fs::create_dir_all(&folder).map_err(io_error)?;
    refuse_symlink(&root)?;
    refuse_symlink(&folder)?;
    refuse_symlink(&dest)?;
    write_replace(&folder, &dest, SKILL_MD)?;
    if had_file {
        Ok(InstallOutcome::Updated {
            path: dest,
            previous: previous_version,
        })
    } else {
        Ok(InstallOutcome::Written(dest))
    }
}

pub fn detect() -> Result<Vec<AgentStatus>, Error> {
    let home = home_dir()?;
    let embedded = embedded_skill_version();
    let mut out = Vec::new();
    for target in Target::named_agents() {
        let skills_root = match target.skills_root() {
            Ok(path) => path,
            Err(Error::InvalidOmpProfile(_)) if target == Target::Omp => continue,
            Err(error) => return Err(error),
        };
        let skill_path = skills_root.join(SKILL_FOLDER).join(SKILL_FILE);
        if !agent_present(&home, &target, &skills_root) {
            continue;
        }
        let (installed_version, state) = match skill_status(&skills_root, &skill_path)? {
            SkillStatusDetail::Blocked => (None, SkillState::Blocked),
            SkillStatusDetail::Missing => (None, SkillState::Missing),
            SkillStatusDetail::Ready { version } => {
                let state = match &version {
                    Some(v) if v.as_str() == embedded.as_str() => SkillState::Current,
                    _ => SkillState::Outdated,
                };
                (version, state)
            }
        };
        out.push(AgentStatus {
            id: target.name().to_string(),
            skills_root,
            skill_path,
            present: true,
            installed_version,
            state,
        });
    }
    Ok(out)
}

pub fn detected_ids() -> Result<Vec<String>, Error> {
    Ok(detect()?.into_iter().map(|agent| agent.id).collect())
}

/// Installer probe for `tsk update`: a tab-separated table a POSIX shell can read with
/// `awk -F'\t'`. The first line is `embedded\t<version>`; each following line is
/// `<id>\t<state>\t<installed version or ->\t<skill path>`. States are `missing`,
/// `current`, `outdated`, and `blocked-symlink`, the same words as `--json`.
pub fn skill_states_text() -> Result<String, Error> {
    let mut out = format!("embedded\t{}\n", embedded_skill_version());
    for agent in detect()? {
        let state = match agent.state {
            SkillState::Missing => "missing",
            SkillState::Current => "current",
            SkillState::Outdated => "outdated",
            SkillState::Blocked => "blocked-symlink",
        };
        // The path is display-only; a tab or newline inside it would forge a row.
        let path: String = agent
            .skill_path
            .display()
            .to_string()
            .chars()
            .map(|c| if c.is_control() { '?' } else { c })
            .collect();
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            agent.id,
            state,
            agent.installed_version.as_deref().unwrap_or("-"),
            path
        ));
    }
    Ok(out)
}

pub fn install_detected(force: bool) -> Result<BatchResult, Error> {
    let detected = detect()?;
    if detected.is_empty() {
        return Ok(BatchResult {
            applied: Vec::new(),
            skipped_current: Vec::new(),
            blocked: Vec::new(),
            declined: false,
            none_detected: true,
        });
    }
    let mut applied = Vec::new();
    let mut skipped_current = Vec::new();
    let mut blocked = Vec::new();
    for status in detected {
        if status.state == SkillState::Blocked {
            blocked.push(status.id);
            continue;
        }
        if !force && status.state == SkillState::Current {
            skipped_current.push(status.id);
            continue;
        }
        let target = named_target(&status.id)?;
        match install(&target, force) {
            Ok(outcome) => applied.push((status.id, outcome)),
            Err(Error::Exists(_)) => skipped_current.push(status.id),
            Err(error) => return Err(error),
        }
    }
    Ok(BatchResult {
        applied,
        skipped_current,
        blocked,
        declined: false,
        none_detected: false,
    })
}

pub fn run_interactive_batch(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    interactive: bool,
) -> Result<BatchResult, Error> {
    let detected = detect()?;
    if detected.is_empty() {
        let _ = write!(writer, "{}", list_text());
        let _ = writeln!(
            writer,
            "\nNo agent skill roots detected.\nRun `tsk setup herdr` to register the Herdr plugin."
        );
        return Ok(BatchResult {
            applied: Vec::new(),
            skipped_current: Vec::new(),
            blocked: Vec::new(),
            declined: false,
            none_detected: true,
        });
    }

    let embedded = embedded_skill_version();
    let _ = writeln!(writer, "Detected agents:");
    let mut needs_work = Vec::new();
    let mut current = Vec::new();
    let mut blocked = Vec::new();
    for status in &detected {
        let detail = match status.state {
            SkillState::Missing => "not installed".to_string(),
            SkillState::Current => format!("current v{embedded}"),
            SkillState::Outdated => match &status.installed_version {
                Some(v) => format!("v{v} → v{embedded}"),
                None => format!("update to v{embedded}"),
            },
            SkillState::Blocked => "blocked (symlink)".to_string(),
        };
        let _ = writeln!(
            writer,
            "  {:<8} {}  ({detail})",
            status.id,
            status.skills_root.display()
        );
        match status.state {
            SkillState::Missing | SkillState::Outdated => needs_work.push(status.id.clone()),
            SkillState::Current => current.push(status.id.clone()),
            SkillState::Blocked => blocked.push(status.id.clone()),
        }
    }

    if needs_work.is_empty() {
        let _ = writeln!(
            writer,
            "All detected agent skills are current (v{embedded})."
        );
        return Ok(BatchResult {
            applied: Vec::new(),
            skipped_current: current,
            blocked,
            declined: false,
            none_detected: false,
        });
    }

    if !interactive {
        let _ = writeln!(
            writer,
            "Skipping agent skill setup (no TTY). Run:\n  tsk setup\nor:\n  tsk setup agents --yes"
        );
        return Ok(BatchResult {
            applied: Vec::new(),
            skipped_current: current,
            blocked,
            declined: true,
            none_detected: false,
        });
    }

    let list = needs_work.join(", ");
    let _ = write!(writer, "Install or update the tsk skill for {list}? [y/N] ");
    let _ = writer.flush();
    let mut line = String::new();
    if reader.read_line(&mut line).map_err(io_error)? == 0 {
        return Err(Error::Ended);
    }
    let answer = line.trim();
    if !matches!(answer, "y" | "Y" | "yes" | "YES") {
        let _ = writeln!(writer, "Skipped agent skill setup. Run later:\n  tsk setup");
        return Ok(BatchResult {
            applied: Vec::new(),
            skipped_current: current,
            blocked,
            declined: true,
            none_detected: false,
        });
    }

    let mut applied = Vec::new();
    for id in &needs_work {
        let target = named_target(id)?;
        let outcome = install(&target, false)?;
        applied.push((id.clone(), outcome));
    }
    Ok(BatchResult {
        applied,
        skipped_current: current,
        blocked,
        declined: false,
        none_detected: false,
    })
}

pub fn list_text() -> String {
    let home = env::var("HOME").ok().filter(|value| !value.is_empty());
    let display = |suffix: &str| match &home {
        Some(home) => format!("{home}/{suffix}/tsk-cli/SKILL.md"),
        None => format!("$HOME/{suffix}/tsk-cli/SKILL.md"),
    };
    format!(
        "{USAGE}\n\n\
         On a TTY, bare `tsk setup` detects agents and asks once to install or update.\n\
         herdr     register the plugin and shortcuts for this installed binary\n\
         agents    detect agents; --yes installs/updates without asking\n\
         claude    {}\n\
         pi        {}\n\
         omp       {}\n\
         cursor    {}\n\
         grok      {}\n\
         codex     {}\n\
         opencode  {}\n\
         --skill-dir <path>  write <path>/tsk-cli/SKILL.md\n\
         --detected-ids      print detected agent ids (for installers)\n",
        display(".claude/skills"),
        display(".pi/agent/skills"),
        omp_skill_display_path(),
        display(".cursor/skills"),
        display(".grok/skills"),
        display(".agents/skills"),
        display(".config/opencode/skills"),
    )
}

pub fn detection_json() -> Result<String, Error> {
    let agents = detect()?;
    let payload = serde_json::json!({
        "outcome": "detected",
        "embedded_skill_version": embedded_skill_version(),
        "agents": agents.iter().map(|agent| serde_json::json!({
            "id": agent.id,
            "present": agent.present,
            "skills_root": agent.skills_root.display().to_string(),
            "skill_path": agent.skill_path.display().to_string(),
            "installed_version": agent.installed_version,
            "state": match agent.state {
                SkillState::Missing => "missing",
                SkillState::Current => "current",
                SkillState::Outdated => "outdated",
                SkillState::Blocked => "blocked-symlink",
            },
        })).collect::<Vec<_>>(),
    });
    Ok(format!("{payload}\n"))
}

fn named_target(id: &str) -> Result<Target, Error> {
    match id {
        "claude" => Ok(Target::Claude),
        "pi" => Ok(Target::Pi),
        "omp" => Ok(Target::Omp),
        "cursor" => Ok(Target::Cursor),
        "grok" => Ok(Target::Grok),
        "codex" => Ok(Target::Codex),
        "opencode" => Ok(Target::OpenCode),
        _ => Err(usage()),
    }
}

fn agent_present(home: &Path, target: &Target, skills_root: &Path) -> bool {
    let marker = match target {
        Target::Claude => home.join(".claude"),
        Target::Pi => home.join(".pi"),
        Target::Omp => omp_config_root().unwrap_or_else(|_| home.join(".omp")),
        Target::Cursor => home.join(".cursor"),
        Target::Grok => home.join(".grok"),
        Target::Codex => home.join(".codex"),
        Target::OpenCode => home.join(".config/opencode"),
        Target::SkillDir(_) => return true,
    };
    let omp_agent_dir_present =
        matches!(target, Target::Omp) && skills_root.parent().is_some_and(real_dir);
    real_dir(&marker)
        || real_dir(skills_root)
        || omp_agent_dir_present
        || match target {
            Target::Claude => cli_on_path("claude"),
            Target::Omp => executable_cli_on_path("omp"),
            Target::Cursor => cli_on_path("cursor"),
            Target::Codex => cli_on_path("codex"),
            Target::OpenCode => cli_on_path("opencode"),
            Target::Pi | Target::Grok | Target::SkillDir(_) => false,
        }
}

fn real_dir(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(meta) => meta.is_dir() && !meta.file_type().is_symlink(),
        Err(_) => false,
    }
}

fn cli_on_path(name: &str) -> bool {
    path_contains(name, |candidate| {
        fs::symlink_metadata(candidate).is_ok_and(|meta| meta.is_file())
    })
}

fn executable_cli_on_path(name: &str) -> bool {
    path_contains(name, |candidate| {
        fs::metadata(candidate).is_ok_and(|meta| executable_file(&meta))
    })
}

fn path_contains(name: &str, predicate: impl Fn(&Path) -> bool) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path).any(|dir| predicate(&dir.join(name)))
}

fn executable_file(meta: &fs::Metadata) -> bool {
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

enum SkillStatusDetail {
    Missing,
    Blocked,
    Ready { version: Option<String> },
}

fn skill_status(root: &Path, dest: &Path) -> Result<SkillStatusDetail, Error> {
    if is_symlink(root)? || is_symlink(&root.join(SKILL_FOLDER))? || is_symlink(dest)? {
        return Ok(SkillStatusDetail::Blocked);
    }
    if !path_present(dest)? {
        return Ok(SkillStatusDetail::Missing);
    }
    let text = fs::read_to_string(dest).map_err(io_error)?;
    Ok(SkillStatusDetail::Ready {
        version: frontmatter_version(&text),
    })
}

fn is_symlink(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.file_type().is_symlink()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

fn push_agent(agents: &mut Vec<Target>, target: Target) -> Result<(), Error> {
    if agents
        .iter()
        .any(|existing| existing.name() == target.name())
    {
        return Err(usage());
    }
    agents.push(target);
    Ok(())
}

fn usage() -> Error {
    Error::Usage(USAGE.into())
}

fn path_present(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

fn home_dir() -> Result<PathBuf, Error> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(Error::Home)
}

/// Resolve OMP's active user agent directory. Named profiles are rooted under
/// `PI_CONFIG_DIR` and ignore `PI_CODING_AGENT_DIR`; the default profile honors
/// that agent-directory override, matching OMP's `getAgentDir()` contract.
fn omp_agent_dir() -> Result<PathBuf, Error> {
    if let Some(profile) = active_omp_profile()? {
        return Ok(omp_config_root()?
            .join("profiles")
            .join(profile)
            .join("agent"));
    }
    if let Some(override_dir) = env::var_os("PI_CODING_AGENT_DIR").filter(|value| !value.is_empty())
    {
        if !profile_derived_agent_override(&override_dir)? {
            let path = PathBuf::from(override_dir);
            let absolute = if path.is_absolute() {
                path
            } else {
                env::current_dir().map_err(io_error)?.join(path)
            };
            return Ok(normalize_absolute_path(&absolute));
        }
    }
    Ok(omp_config_root()?.join("agent"))
}

fn omp_config_root() -> Result<PathBuf, Error> {
    let config_dir = PathBuf::from(
        env::var_os("PI_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| ".omp".into()),
    );
    let relative: PathBuf = config_dir
        .components()
        .filter_map(|component| match component {
            Component::Prefix(_) | Component::RootDir => None,
            Component::CurDir => None,
            Component::ParentDir => Some("..".into()),
            Component::Normal(part) => Some(part.to_owned()),
        })
        .collect();
    Ok(normalize_absolute_path(&home_dir()?.join(relative)))
}

fn profile_derived_agent_override(override_dir: &std::ffi::OsStr) -> Result<bool, Error> {
    let Some(pi_profile) = env::var_os("PI_PROFILE") else {
        return Ok(false);
    };
    let Ok(Some(pi_profile)) = normalize_omp_profile(pi_profile) else {
        return Ok(false);
    };
    let profile_agent_dir = omp_config_root()?
        .join("profiles")
        .join(pi_profile)
        .join("agent");
    Ok(Path::new(override_dir) == profile_agent_dir)
}

fn active_omp_profile() -> Result<Option<String>, Error> {
    let raw = match env::var_os("OMP_PROFILE") {
        Some(value) => Some(value),
        None => env::var_os("PI_PROFILE"),
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    normalize_omp_profile(raw)
}

fn normalize_omp_profile(raw: std::ffi::OsString) -> Result<Option<String>, Error> {
    let profile = raw
        .into_string()
        .map_err(|value| Error::InvalidOmpProfile(value.to_string_lossy().into_owned()))?;
    let profile = profile.trim();
    if profile.is_empty() || profile == "default" {
        return Ok(None);
    }
    if !valid_omp_profile(profile) {
        return Err(Error::InvalidOmpProfile(profile.to_owned()));
    }
    Ok(Some(profile.to_owned()))
}

fn valid_omp_profile(profile: &str) -> bool {
    let bytes = profile.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 64
        || !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        || profile.ends_with('.')
        || !bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return false;
    }
    let base = profile.split('.').next().unwrap_or_default();
    !matches!(base, "con" | "prn" | "aux" | "nul")
        && !(base.len() == 4
            && (base.starts_with("com") || base.starts_with("lpt"))
            && base.as_bytes()[3].is_ascii_digit())
}

fn normalize_absolute_path(path: &Path) -> PathBuf {
    debug_assert!(path.is_absolute());
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn omp_skill_display_path() -> String {
    match omp_agent_dir() {
        Ok(path) => path
            .join("skills")
            .join(SKILL_FOLDER)
            .join(SKILL_FILE)
            .display()
            .to_string(),
        Err(Error::Home) => "$HOME/.omp/agent/skills/tsk-cli/SKILL.md".to_owned(),
        Err(error) => format!("<{}>", error),
    }
}

/// Write `contents` to a fresh sibling file and rename it over `dest`, so an interrupted
/// install leaves the agent's previous skill intact rather than a truncated one.
fn write_replace(folder: &Path, dest: &Path, contents: &str) -> Result<(), Error> {
    let staged = folder.join(format!(".{SKILL_FILE}.tmp.{}", std::process::id()));
    // `create_new` is O_CREAT|O_EXCL: it refuses an existing entry of any kind, so a
    // planted symlink at the staging name is an error rather than a write through it.
    // A stale regular file from a killed run is removed once (never a symlink), then retried.
    let open = || {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o644);
        }
        options.open(&staged)
    };
    let mut file = match open() {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let stale = fs::symlink_metadata(&staged).map_err(io_error)?;
            if !stale.file_type().is_file() {
                // Not ours: leave it in place and say so.
                return Err(Error::Symlink(staged));
            }
            fs::remove_file(&staged).map_err(io_error)?;
            open().map_err(io_error)?
        }
        Err(error) => return Err(io_error(error)),
    };
    // From here the staging file is ours; remove it on any failure.
    let result = (|| {
        io::Write::write_all(&mut file, contents.as_bytes()).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        fs::rename(&staged, dest).map_err(io_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn refuse_symlink(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::Symlink(path.to_path_buf())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn io_error(error: io::Error) -> Error {
    Error::Io(format!("could not write skill: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    struct OmpEnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl OmpEnvGuard {
        fn cleared() -> Self {
            let values = [
                "OMP_PROFILE",
                "PI_PROFILE",
                "PI_CONFIG_DIR",
                "PI_CODING_AGENT_DIR",
            ]
            .into_iter()
            .map(|key| {
                let value = env::var_os(key);
                env::remove_var(key);
                (key, value)
            })
            .collect();
            Self(values)
        }
    }

    impl Drop for OmpEnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
    }

    /// `install` only rewrites an installed skill when the frontmatter version differs, so
    /// the version must move between releases. It moves once per release (AGENTS.md): the
    /// first skill edit after a release bumps it one minor above the shipped version, every
    /// later edit before the next release keeps it and refreshes only the content hash.
    /// Development copies installed at the unreleased version need `tsk setup --force`.
    #[test]
    fn embedded_skill_declares_semver() {
        let version = embedded_skill_version();
        assert_eq!(version, "1.4.0");
        assert_eq!(frontmatter_version(SKILL_MD).as_deref(), Some("1.4.0"));
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in SKILL_MD.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(
            hash, 0x9a56_714b_82b3_83de,
            "skills/tsk-cli/SKILL.md changed: refresh this hash pin, and bump `version:` only if this is the first skill edit since the last release"
        );
    }

    #[test]
    fn embedded_skill_uses_json_for_agent_list_reads() {
        assert!(SKILL_MD.contains("**Read JSON, not presentation.**"));
        assert!(SKILL_MD.contains("Always add `--json` to `tsk list`."));
        let exit_zero = SKILL_MD
            .lines()
            .find(|line| line.starts_with("| 0 |"))
            .expect("exit-zero contract row");
        assert!(exit_zero.contains("verify") && exit_zero.contains("--json"));
        for line in SKILL_MD
            .lines()
            .filter(|line| line.starts_with("tsk list "))
        {
            assert!(
                line.contains("--json"),
                "agent-facing list example must use JSON: {line}"
            );
        }
    }

    #[test]
    fn normalize_strips_v_prefix() {
        assert_eq!(normalize_version("v1.2.3").as_deref(), Some("1.2.3"));
        assert!(normalize_version("1.2").is_none());
        assert!(normalize_version("1.2.3.4").is_none());
    }

    #[test]
    fn missing_version_parses_as_none() {
        let md = "---\nname: x\n---\n\nbody\n";
        assert_eq!(frontmatter_version(md), None);
    }

    #[test]
    fn interactive_batch_yes_installs_detected_agent() {
        let _lock = env_lock();
        let _omp_env = OmpEnvGuard::cleared();
        let root =
            std::env::temp_dir().join(format!("tsk-setup-agent-batch-yes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home/.cursor")).expect("cursor");
        fs::create_dir_all(root.join("empty-bin")).expect("bin");
        let previous_home = std::env::var_os("HOME");
        let previous_path = std::env::var_os("PATH");
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var("PATH", root.join("empty-bin"));
        let mut reader = Cursor::new(b"y\n".to_vec());
        let mut writer = Vec::new();
        let result = run_interactive_batch(&mut reader, &mut writer, true);
        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match previous_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        let result = result.expect("batch");
        assert!(!result.declined);
        assert_eq!(result.applied.len(), 1);
        assert_eq!(result.applied[0].0, "cursor");
        let dest = root.join("home/.cursor/skills/tsk-cli/SKILL.md");
        assert_eq!(fs::read_to_string(&dest).expect("written"), SKILL_MD);
        let transcript = String::from_utf8_lossy(&writer);
        assert!(
            transcript.contains("Install or update the tsk skill for cursor?"),
            "{transcript}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn interactive_batch_no_skips_write() {
        let _lock = env_lock();
        let _omp_env = OmpEnvGuard::cleared();
        let root =
            std::env::temp_dir().join(format!("tsk-setup-agent-batch-no-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home/.cursor")).expect("cursor");
        fs::create_dir_all(root.join("empty-bin")).expect("bin");
        let previous_home = std::env::var_os("HOME");
        let previous_path = std::env::var_os("PATH");
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var("PATH", root.join("empty-bin"));
        let mut reader = Cursor::new(b"n\n".to_vec());
        let mut writer = Vec::new();
        let result = run_interactive_batch(&mut reader, &mut writer, true);
        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match previous_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        let result = result.expect("batch");
        assert!(result.declined);
        assert!(result.applied.is_empty());
        assert!(!root.join("home/.cursor/skills/tsk-cli/SKILL.md").exists());
        let transcript = String::from_utf8_lossy(&writer);
        assert!(transcript.contains("tsk setup"), "{transcript}");
        let _ = fs::remove_dir_all(root);
    }
}
