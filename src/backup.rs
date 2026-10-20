//! Backups of the task store to git repositories, and the versions a restore picks from.
//!
//! A backup is the live `tsk.json` byte for byte, so it carries its own `format_version`,
//! plus `config.toml`. Targets are the `[[backup]]` entries of `config.toml`. Saves and full
//! board opens never wait on a backup: when a target is due they start a detached
//! `tsk backup run` child and return. Each target's last success and last error live in
//! `backup-status.json`.
//!
//! Layout, shared with the cross-machine sync that builds on it: a target holds
//! `machines/<machine-id>/` with `tsk.json`, `config.toml` (agent `env` values removed), and
//! `machine.toml` (host name, tsk version), one commit per backup. The repository root is left
//! free on purpose: the later sync (T253) puts its shared file there.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, TableLike};

use crate::fsperm;

/// Each target's last success, attempt, and error.
const STATUS_FILE: &str = "backup-status.json";
/// Held by the one `backup run` working at a time.
const RUN_LOCK_FILE: &str = "backup.lock";
/// Held by the one `backup run` waiting for the run lock; a third finds it taken and exits,
/// since the waiter reads the store only once it runs.
const PENDING_LOCK_FILE: &str = "backup.pending.lock";
/// A background run records its trigger here before it looks for the waiter's slot, so the
/// waiter that runs instead of it also backs up what that trigger made due.
const TRIGGER_FLAG_PREFIX: &str = "backup.trigger-";
/// A detached probe writes what it found here (`backup.probe-<nonce>`) for the command waiting.
const PROBE_FILE_PREFIX: &str = "backup.probe-";
/// How long `tsk backup add` and `setup` wait for the detached probe.
const PROBE_WAIT: Duration = Duration::from_secs(5);
/// This machine's id, generated once.
const MACHINE_ID_FILE: &str = "machine-id";
/// Clones of git targets, one directory per target.
const REPOS_DIR: &str = "backup-repos";
pub(crate) const STATUS_TEMP_PREFIX: &str = ".backup-status.json.tmp.";

/// The board suggests a first backup target once, when the store holds this many tasks.
pub const NUDGE_AFTER_TASKS: usize = 10;
/// A git target is listed with this many versions per machine unless `--all` asks for more.
pub const GIT_VERSIONS_SHOWN: usize = 20;
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
/// A failed daily backup is retried no sooner than this.
const RETRY_AFTER_SECS: u64 = 10 * 60;
const DAY_SECS: u64 = 24 * 60 * 60;

/// Whether a state-directory entry belongs to backups and holds owner-only content.
pub(crate) fn is_private_state_name(name: &str) -> bool {
    name == STATUS_FILE
        || name == RUN_LOCK_FILE
        || name == PENDING_LOCK_FILE
        || name == MACHINE_ID_FILE
        || name.starts_with(TRIGGER_FLAG_PREFIX)
        || name.starts_with(PROBE_FILE_PREFIX)
        || name.starts_with(STATUS_TEMP_PREFIX)
}

/// Where a target writes. Git is the only kind; `kind` stays in the entry's grammar so
/// another can be added without changing existing files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Git,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Self::Git => "git",
        }
    }
}

/// How often a target is backed up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Every {
    Save,
    Daily,
}

impl Every {
    pub const ALL: [Every; 2] = [Every::Save, Every::Daily];

    pub fn word(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Daily => "daily",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|every| every.word() == word)
    }

    /// How often, for people: `every save` or `daily`.
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Save => "every save",
            Self::Daily => "daily",
        }
    }
}

/// What started a background run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Save,
    Open,
}

impl Trigger {
    pub fn word(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Open => "open",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "save" => Some(Self::Save),
            "open" => Some(Self::Open),
            _ => None,
        }
    }
}

/// One `[[backup]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: Kind,
    /// The repository URL or path, as written in the entry.
    pub location: String,
    pub every: Every,
}

impl Target {
    /// A target backed up on every save: the commit is local and quick, and the push runs in
    /// the background child, coalesced so a burst of saves pushes at most twice.
    pub fn new(location: String) -> Self {
        Self {
            kind: Kind::Git,
            location,
            every: Every::Save,
        }
    }

    /// Identity in `backup-status.json`.
    fn key(&self) -> String {
        format!("{}:{}", self.kind.word(), self.location)
    }

    /// Short stable id, naming a git target's clone.
    fn id(&self) -> String {
        format!("{:016x}", fnv1a(self.key().as_bytes()))
    }
}

/// Read the `[[backup]]` entries of `<state_dir>/config.toml`. A missing file or table is no
/// targets. Only this table is validated, so a broken profile never blocks backups.
pub fn load_targets(state_dir: &Path) -> Result<Vec<Target>, String> {
    match crate::agents::read_config(state_dir) {
        Ok(Some(content)) => parse_targets(&content),
        Ok(None) => Ok(Vec::new()),
        Err(error) => Err(format!("could not read config.toml: {error}")),
    }
}

pub fn parse_targets(content: &str) -> Result<Vec<Target>, String> {
    let document: DocumentMut = content
        .parse()
        .map_err(|error| format!("config.toml is malformed: {error}"))?;
    let Some(item) = document.get("backup") else {
        return Ok(Vec::new());
    };
    let entries =
        backup_entries(item).ok_or("config.toml: backup must be a list of [[backup]] tables")?;
    let mut targets = Vec::new();
    for (index, table) in entries.into_iter().enumerate() {
        let table =
            table.ok_or_else(|| format!("config.toml: backup {} must be a table", index + 1))?;
        targets.push(parse_target(index, table)?);
    }
    Ok(targets)
}

/// The entries of `backup` in both shapes the loader takes: `[[backup]]` tables, or an
/// inline `backup = [{ … }]` list (`None` for an element that is not a table). `None` when
/// `backup` is neither. The loader and [`sanitized_config`] both read through it, so a shape
/// one accepts is never missed by the other.
fn backup_entries(item: &toml_edit::Item) -> Option<Vec<Option<&dyn TableLike>>> {
    if let Some(tables) = item.as_array_of_tables() {
        return Some(
            tables
                .iter()
                .map(|table| Some(table as &dyn TableLike))
                .collect(),
        );
    }
    let array = item.as_array()?;
    Some(
        array
            .iter()
            .map(|value| value.as_inline_table().map(|table| table as &dyn TableLike))
            .collect(),
    )
}

/// [`backup_entries`], writable, skipping elements that are not tables.
fn backup_entries_mut(item: &mut toml_edit::Item) -> Vec<&mut dyn TableLike> {
    if item.is_array_of_tables() {
        return item
            .as_array_of_tables_mut()
            .into_iter()
            .flat_map(|tables| tables.iter_mut())
            .map(|table| table as &mut dyn TableLike)
            .collect();
    }
    match item.as_array_mut() {
        Some(array) => array
            .iter_mut()
            .filter_map(|value| value.as_inline_table_mut())
            .map(|table| table as &mut dyn TableLike)
            .collect(),
        None => Vec::new(),
    }
}

fn parse_target(index: usize, table: &dyn TableLike) -> Result<Target, String> {
    let invalid = |message: String| format!("config.toml: [[backup]] {}: {message}", index + 1);
    let string = |key: &str| -> Result<Option<String>, String> {
        match table.get(key) {
            None => Ok(None),
            Some(item) => item
                .as_str()
                .map(|value| Some(value.to_string()))
                .ok_or_else(|| invalid(format!("{key} must be a string"))),
        }
    };
    match string("kind")?.as_deref() {
        None | Some("git") => {}
        Some(other) => return Err(invalid(format!("kind {other:?} is not git"))),
    }
    if let Some(key) = table
        .iter()
        .map(|(key, _)| key)
        .find(|key| !["kind", "url", "every"].contains(key))
    {
        return Err(invalid(format!("unknown key {key:?}")));
    }
    let location = string("url")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid("url is required".into()))?;
    let mut target = Target::new(location);
    if let Some(word) = string("every")? {
        target.every = Every::parse(&word)
            .ok_or_else(|| invalid(format!("every {word:?} is not save or daily")))?;
    }
    Ok(target)
}

/// What `tsk backup add` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    Added,
    /// The target existed; its frequency changed.
    Updated,
    /// The target existed with this frequency already.
    Unchanged,
}

/// Add a `[[backup]]` entry to `config.toml`, creating the file if needed and keeping every
/// other line of it. An existing entry for the same target only has its `every` updated.
pub fn add_target(
    state_dir: &Path,
    location: &str,
    every: Option<Every>,
) -> Result<(Target, AddOutcome), String> {
    // Under the run lock, like a remove: two edits of config.toml never interleave, so
    // neither drops the other's target.
    with_run_lock(state_dir, true, || {
        Ok(add_target_locked(state_dir, location, every))
    })
    .map_err(|error| error.to_string())
    .and_then(|added| added)
}

fn add_target_locked(
    state_dir: &Path,
    location: &str,
    every: Option<Every>,
) -> Result<(Target, AddOutcome), String> {
    let existing = load_targets(state_dir)?;
    let content = crate::agents::read_config(state_dir)
        .map_err(|error| format!("could not read config.toml: {error}"))?
        .unwrap_or_default();
    let mut document: DocumentMut = content
        .parse()
        .map_err(|error| format!("config.toml is malformed: {error}"))?;
    if let Some(index) = existing
        .iter()
        .position(|target| target.location == location)
    {
        let mut target = existing[index].clone();
        let Some(every) = every.filter(|every| *every != target.every) else {
            return Ok((target, AddOutcome::Unchanged));
        };
        target.every = every;
        let entry = document
            .get_mut("backup")
            .and_then(|item| item.as_array_of_tables_mut())
            .and_then(|tables| tables.get_mut(index))
            .ok_or("config.toml: write `every` by hand for an inline backup list")?;
        entry["every"] = toml_edit::value(every.word());
        write_document(state_dir, &document)?;
        return Ok((target, AddOutcome::Updated));
    }
    let mut target = Target::new(location.to_string());
    if let Some(every) = every {
        target.every = every;
    }
    let mut table = toml_edit::Table::new();
    table["url"] = toml_edit::value(location);
    table["every"] = toml_edit::value(target.every.word());
    match document.get_mut("backup") {
        None => {
            let mut tables = toml_edit::ArrayOfTables::new();
            tables.push(table);
            document.insert("backup", toml_edit::Item::ArrayOfTables(tables));
        }
        Some(item) => item
            .as_array_of_tables_mut()
            .ok_or("config.toml: add the target by hand to the inline backup list")?
            .push(table),
    }
    write_document(state_dir, &document)?;
    Ok((target, AddOutcome::Added))
}

/// Remove the `[[backup]]` entry `selector` names: its URL or path as written or as
/// `tsk backup list` shows it, or its number there. Its status is forgotten; backups
/// already pushed stay.
pub fn remove_target(state_dir: &Path, selector: &str) -> Result<Target, String> {
    // Under the run lock: a run reads its targets and saves its status under it, so it never
    // pushes to this target afterwards or writes its status back.
    with_run_lock(state_dir, true, || {
        Ok(remove_target_locked(state_dir, selector))
    })
    .map_err(|error| error.to_string())
    .and_then(|removed| removed)
}

fn remove_target_locked(state_dir: &Path, selector: &str) -> Result<Target, String> {
    let targets = load_targets(state_dir)?;
    let absolute = absolute_path(selector);
    let exact = targets.iter().position(|target| {
        target.location == selector
            || absolute
                .as_deref()
                .is_some_and(|path| target.location == path)
    });
    let index = match selector
        .parse::<usize>()
        .ok()
        .filter(|number| (1..=targets.len()).contains(number))
        .map(|number| number - 1)
        .or(exact)
    {
        Some(index) => index,
        // As `tsk backup list` shows it: two targets that differ only in credentials look
        // alike there, and removing either one silently would be a guess.
        None => {
            let shown: Vec<usize> = (0..targets.len())
                .filter(|&index| shown_location(&targets[index].location) == selector)
                .collect();
            match shown.as_slice() {
                [index] => *index,
                [] => return Err(format!("no backup target {}", shown_location(selector))),
                _ => {
                    return Err(format!(
                        "{} names {} targets; remove one by its number in tsk backup list, or its full URL",
                        shown_location(selector),
                        shown.len()
                    ))
                }
            }
        }
    };
    let content = crate::agents::read_config(state_dir)
        .map_err(|error| format!("could not read config.toml: {error}"))?
        .unwrap_or_default();
    let mut document: DocumentMut = content
        .parse()
        .map_err(|error| format!("config.toml is malformed: {error}"))?;
    let tables = document
        .get_mut("backup")
        .and_then(|item| item.as_array_of_tables_mut())
        .ok_or("config.toml: remove the target by hand from the inline backup list")?;
    tables.remove(index);
    if tables.is_empty() {
        document.remove("backup");
    }
    write_document(state_dir, &document)?;
    let removed = targets[index].clone();
    let mut status = load_status(state_dir);
    status.targets.remove(&removed.key());
    let _ = save_status(state_dir, &status);
    Ok(removed)
}

fn write_document(state_dir: &Path, document: &DocumentMut) -> Result<(), String> {
    crate::agents::write_config(state_dir, &document.to_string())
        .map_err(|error| format!("could not write config.toml: {error}"))
}

/// `path` made absolute against the working directory, when it names an existing directory
/// (a local repository), so the target works from any directory.
pub fn absolute_path(path: &str) -> Option<String> {
    let path = Path::new(path);
    if !path.is_dir() {
        return None;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let absolute = fs::canonicalize(&absolute).unwrap_or(absolute);
    absolute.to_str().map(plain_path)
}

/// A Windows verbatim path (`\\?\C:\…`, `\\?\UNC\server\share\…`), as `canonicalize`
/// returns, in its ordinary spelling (`C:\…`, `\\server\share\…`), which git and people
/// can use. Any other path, including a verbatim one with no drive or UNC form, is unchanged.
pub fn plain_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        let drive = bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes.len() == 2 || bytes[2] == b'\\');
        if drive {
            return rest.to_string();
        }
    }
    path.to_string()
}

/// One target's backup history, as `backup-status.json` keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetStatus {
    /// Unix seconds of the last backup that reached the target, or found it up to date.
    #[serde(default)]
    pub last_success: Option<u64>,
    #[serde(default)]
    pub last_attempt: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// FNV-1a of the bytes the last success wrote.
    #[serde(default)]
    pub last_hash: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StatusFile {
    #[serde(default)]
    targets: BTreeMap<String, TargetStatus>,
    /// The board already suggested adding a first target.
    #[serde(default)]
    nudged: bool,
}

fn load_status(state_dir: &Path) -> StatusFile {
    fs::read(state_dir.join(STATUS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_status(state_dir: &Path, status: &StatusFile) -> io::Result<()> {
    let data = serde_json::to_vec_pretty(status).map_err(io::Error::other)?;
    let tmp = state_dir.join(format!(
        "{STATUS_TEMP_PREFIX}{}.{}",
        std::process::id(),
        now_nanos()
    ));
    let result = (|| -> io::Result<()> {
        let mut file = fsperm::create_private_file(&tmp)?;
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        fsperm::replace_file(&tmp, &state_dir.join(STATUS_FILE))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Each target paired with its recorded status, for `tsk backup list`.
pub fn statuses(state_dir: &Path) -> Result<Vec<(Target, TargetStatus)>, String> {
    let targets = load_targets(state_dir)?;
    let status = load_status(state_dir);
    Ok(targets
        .into_iter()
        .map(|target| {
            let record = status
                .targets
                .get(&target.key())
                .cloned()
                .unwrap_or_default();
            (target, record)
        })
        .collect())
}

/// The first failing target's error, for the status row of every full board open until
/// that target succeeds. Reads only: `backup-status.json` is replaced whole, so this never
/// waits for a run.
pub fn failure_message(state_dir: &Path) -> Option<String> {
    // Most boards have no targets: answer without reading config.toml.
    if !state_dir.join(STATUS_FILE).exists() {
        return None;
    }
    let targets = load_targets(state_dir).ok()?;
    let status = load_status(state_dir);
    targets.iter().find_map(|target| {
        let error = status.targets.get(&target.key())?.last_error.as_deref()?;
        Some(format!(
            "backup failed: {} (tsk backup list)",
            redacted(first_line(error))
        ))
    })
}

/// The one-time suggestion to add a first target: once the store holds
/// [`NUDGE_AFTER_TASKS`] tasks and no target is configured, the first call says so and records
/// it; every later call, and any call while a target exists or `config.toml` is broken, is
/// `None`. `None` too while a run holds the lock.
pub fn take_backup_nudge(state_dir: &Path, tasks: usize) -> Option<String> {
    if tasks < NUDGE_AFTER_TASKS
        || !matches!(load_targets(state_dir), Ok(targets) if targets.is_empty())
    {
        return None;
    }
    if load_status(state_dir).nudged {
        return None;
    }
    with_run_lock(state_dir, false, || {
        let mut status = load_status(state_dir);
        if status.nudged {
            return Ok(None);
        }
        status.nudged = true;
        save_status(state_dir, &status)?;
        Ok(Some("No backup yet: run tsk backup setup".to_string()))
    })
    .ok()
    .flatten()
}

/// Whether `target` should back up now. A `save` target follows saves; a `daily` one is due
/// on a save or a full board open once 24 hours have passed since its last success, and a
/// failed attempt is retried after ten minutes.
pub fn is_due(every: Every, trigger: Trigger, status: &TargetStatus, now: u64) -> bool {
    let interval = match every {
        Every::Save => return trigger == Trigger::Save,
        Every::Daily => DAY_SECS,
    };
    let stale = status
        .last_success
        .is_none_or(|last| now.saturating_sub(last) >= interval);
    let retrying_too_soon = status.last_error.is_some()
        && status
            .last_attempt
            .is_some_and(|attempt| now.saturating_sub(attempt) < RETRY_AFTER_SECS);
    stale && !retrying_too_soon
}

static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// Let saves and board opens start background backups. Only the binary turns this on, so
/// in-process callers (tests, the library) never spawn a `tsk` child.
pub fn enable_background_backups() {
    BACKGROUND.store(true, Ordering::SeqCst);
}

/// After a durable save: start a detached `tsk backup run --on save` when a target is due.
/// Never fails and never waits.
pub(crate) fn after_save(state_dir: &Path) {
    start_background(state_dir, Trigger::Save);
}

/// On a full board open: start a detached `tsk backup run --on open` when a target is due.
pub fn after_open(state_dir: &Path) {
    start_background(state_dir, Trigger::Open);
}

fn start_background(state_dir: &Path, trigger: Trigger) {
    if !BACKGROUND.load(Ordering::SeqCst) {
        return;
    }
    if any_due(state_dir, trigger, unix_secs(SystemTime::now()))
        && !waiter_will_cover(state_dir, trigger)
    {
        let _ = spawn_run(state_dir, trigger);
    }
}

/// Record `trigger` for the run waiting behind the current one, and say whether such a
/// waiter exists. It reads the flags only after it lets go of its slot, so while the slot is
/// held this flag is still ahead of it, and a burst of saves starts one child, not one each.
fn waiter_will_cover(state_dir: &Path, trigger: Trigger) -> bool {
    if fs::write(trigger_flag(state_dir, trigger), b"").is_err() {
        return false;
    }
    let Ok(pending) = open_lock(state_dir, PENDING_LOCK_FILE) else {
        return false;
    };
    matches!(pending.try_lock(), Err(fs::TryLockError::WouldBlock))
}

/// Whether any configured target is due for `trigger`. A configuration error is never due.
pub fn any_due(state_dir: &Path, trigger: Trigger, now: u64) -> bool {
    let Ok(targets) = load_targets(state_dir) else {
        return false;
    };
    if targets.is_empty() {
        return false;
    }
    let status = load_status(state_dir);
    targets.iter().any(|target| {
        let record = status
            .targets
            .get(&target.key())
            .cloned()
            .unwrap_or_default();
        is_due(target.every, trigger, &record, now)
    })
}

/// Start `tsk backup run --on <trigger>` detached: no stdio, its own process group, no
/// console window. A thread reaps it so a long-lived board leaves no zombies.
fn spawn_run(state_dir: &Path, trigger: Trigger) -> io::Result<()> {
    spawn_detached(state_dir, &["--on", trigger.word()])
}

/// Start `tsk backup run <args>` the way the board does: detached, without stdio.
fn spawn_detached(state_dir: &Path, args: &[&str]) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .args(["backup", "run"])
        .args(args)
        .arg("--state-dir")
        .arg(state_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// What a detached probe of a target found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// A detached `tsk backup run`, as the board starts one, reached the target.
    Reached,
    /// It could not; the string is git's reason.
    Failed(String),
    /// No answer within [`PROBE_WAIT`] (or it could not be started).
    NoAnswer,
}

/// Check that a background run can reach the target at `index` of `[[backup]]`: start a
/// detached `tsk backup run --probe` (no terminal, no stdin, its own process group, exactly
/// as a save starts one) and wait up to [`PROBE_WAIT`] for what it found. A foreground push
/// can succeed where this fails, for example when git needs a passphrase prompt.
pub fn probe_detached(state_dir: &Path, index: usize) -> Probe {
    sweep_late_probes(state_dir);
    let nonce = format!("{}-{}", std::process::id(), now_nanos());
    let file = state_dir.join(format!("{PROBE_FILE_PREFIX}{nonce}"));
    if spawn_detached(state_dir, &["--probe", &format!("{index}:{nonce}")]).is_err() {
        return Probe::NoAnswer;
    }
    let deadline = std::time::Instant::now() + PROBE_WAIT;
    while std::time::Instant::now() < deadline {
        if let Ok(found) = fs::read_to_string(&file) {
            if let Some(line) = found.strip_suffix('\n') {
                let _ = fs::remove_file(&file);
                return match line.strip_prefix("error: ") {
                    Some(error) => Probe::Failed(error.to_string()),
                    None => Probe::Reached,
                };
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    // A late answer is left to the next probe's sweep.
    Probe::NoAnswer
}

/// Remove probe answers no one waited for (older than a minute).
fn sweep_late_probes(state_dir: &Path) {
    let Ok(entries) = fs::read_dir(state_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let late = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(60));
        if late
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(PROBE_FILE_PREFIX))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The detached half of [`probe_detached`]: `git ls-remote` the target and write the result
/// for the waiting command. Reads nothing else and touches no clone or lock.
pub fn run_probe(state_dir: &Path, spec: &str) -> Result<(), String> {
    let (index, nonce) = spec.split_once(':').ok_or("bad probe")?;
    if nonce.is_empty() || !nonce.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Err("bad probe".into());
    }
    let index: usize = index.parse().map_err(|_| "bad probe")?;
    let target = load_targets(state_dir)?
        .into_iter()
        .nth(index)
        .ok_or("no such target")?;
    let repos = repos_dir(state_dir);
    fsperm::ensure_private_dir(&repos).map_err(|error| error.to_string())?;
    let mut command = git_command(&repos);
    command.args(["ls-remote", "--heads", "--", &target.location]);
    let found = match crate::git_base::bounded_process_output(command, "git", PROBE_WAIT) {
        Ok(output) if output.status.success() => "reached\n".to_string(),
        Ok(output) => format!(
            "error: {}\n",
            target_error(&target, git_failure("ls-remote", &output))
        ),
        Err(error) => format!(
            "error: {}\n",
            target_error(&target, error.replace('\n', " / "))
        ),
    };
    // Written beside its name and renamed, so the waiter never reads half a line.
    let file = state_dir.join(format!("{PROBE_FILE_PREFIX}{nonce}"));
    let tmp = state_dir.join(format!("{PROBE_FILE_PREFIX}{nonce}.tmp"));
    let mut handle = fsperm::create_private_file(&tmp).map_err(|e| e.to_string())?;
    handle
        .write_all(found.as_bytes())
        .map_err(|e| e.to_string())?;
    drop(handle);
    fsperm::replace_file(&tmp, &file).map_err(|e| e.to_string())
}

/// Which targets a run backs up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Every target, now (`tsk backup now`).
    Now,
    /// Only due targets whose copy is out of date (the background run).
    Due(Trigger),
    /// The one target at this index of `[[backup]]`, now (`tsk backup add` tests it).
    One(usize),
}

/// One target and what a run did for it.
pub type TargetRun = (Target, Result<RunOutcome, String>);

/// Every listed version, and each target that could not be read with why.
pub type Listing = (Vec<Version>, Vec<(Target, String)>);

/// What a run did for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// A copy was written; the string says where.
    Written(String),
    /// The target already holds this store.
    UpToDate,
    NotDue,
    /// There is no `tsk.json` yet. A git target was still reached (cloned or fetched).
    NoStore,
}

/// Back the live store up to the targets `mode` selects. Runs one at a time per state
/// directory. A background run first records its trigger, then takes the single waiter's
/// slot; a run that finds the slot taken exits, and the waiter, once it holds the run lock,
/// backs up for every trigger recorded until then, its own included.
pub fn run(state_dir: &Path, mode: RunMode, now: SystemTime) -> Result<Vec<TargetRun>, String> {
    let select = |targets: Vec<Target>| -> Vec<Target> {
        match mode {
            RunMode::One(index) => targets.into_iter().skip(index).take(1).collect(),
            RunMode::Now | RunMode::Due(_) => targets,
        }
    };
    // A first look without the lock only to skip the common no-target case; the targets
    // backed up are read again once the run lock is held.
    if select(load_targets(state_dir)?).is_empty() {
        return Ok(Vec::new());
    }
    fsperm::ensure_private_dir(state_dir).map_err(|error| error.to_string())?;
    let _pending = if let RunMode::Due(trigger) = mode {
        // Recorded before the slot check: a run that loses the slot leaves its trigger to the
        // waiter, which reads the flags only after it releases the slot.
        fs::write(trigger_flag(state_dir, trigger), b"").map_err(|e| e.to_string())?;
        let pending = open_lock(state_dir, PENDING_LOCK_FILE).map_err(|e| e.to_string())?;
        match pending.try_lock() {
            Ok(()) => Some(LockGuard(pending)),
            Err(fs::TryLockError::WouldBlock) => return Ok(Vec::new()),
            Err(fs::TryLockError::Error(error)) => return Err(error.to_string()),
        }
    } else {
        None
    };
    let run_lock = open_lock(state_dir, RUN_LOCK_FILE).map_err(|e| e.to_string())?;
    run_lock.lock().map_err(|e| e.to_string())?;
    let _run = LockGuard(run_lock);
    drop(_pending);
    // Read under the lock: `tsk backup remove` rewrites config.toml under it, so a run that
    // waited never pushes to a target removed meanwhile.
    let targets = select(load_targets(state_dir)?);
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let triggers: Vec<Trigger> = match mode {
        RunMode::Now | RunMode::One(_) => Vec::new(),
        RunMode::Due(own) => {
            let mut triggers = vec![own];
            for trigger in [Trigger::Save, Trigger::Open] {
                if fs::remove_file(trigger_flag(state_dir, trigger)).is_ok()
                    && !triggers.contains(&trigger)
                {
                    triggers.push(trigger);
                }
            }
            triggers
        }
    };

    let bytes = match fs::read(state_dir.join("tsk.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Nothing to copy, but a git target is still reached, so a bad URL or missing
            // access shows now rather than at the first backup.
            if let RunMode::Due(_) = mode {
                return Ok(targets
                    .into_iter()
                    .map(|target| (target, Ok(RunOutcome::NoStore)))
                    .collect());
            }
            // Recorded like a backup's result, so `tsk backup list` and the board show a
            // target that cannot be reached.
            let mut status = load_status(state_dir);
            let now_secs = unix_secs(now);
            let results: Vec<TargetRun> = targets
                .into_iter()
                .map(|target| {
                    let reached = sync_clone(state_dir, &target)
                        .map(|_| RunOutcome::NoStore)
                        .map_err(|error| target_error(&target, error));
                    let record = status.targets.entry(target.key()).or_default();
                    record.last_attempt = Some(now_secs);
                    record.last_error = reached.as_ref().err().cloned();
                    (target, reached)
                })
                .collect();
            save_status(state_dir, &status)
                .map_err(|error| format!("could not record backup status: {error}"))?;
            return Ok(results);
        }
        Err(error) => return Err(format!("could not read tsk.json: {error}")),
    };
    // A damaged store is never copied over good backups.
    let invalid = crate::store::check_document(&bytes)
        .err()
        .map(|error| format!("store does not parse, not backed up: {error}"));
    let config = match fs::read_to_string(state_dir.join("config.toml")) {
        Ok(config) => sanitized_config(&config),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("could not read config.toml: {error}")),
    };
    let machine = Machine {
        id: machine_id(state_dir).map_err(|error| error.to_string())?,
        hostname: hostname(),
    };
    let copy = Payload {
        store: &bytes,
        config: config.as_deref().map(str::as_bytes),
        machine_toml: format!(
            "# Written by tsk backup: which machine these files come from.\nhostname = {:?}\ntsk_version = {:?}\n",
            machine.hostname,
            env!("CARGO_PKG_VERSION")
        ),
        summary: Summary {
            tasks: count_tasks(&bytes),
            format: crate::store::format_version(&bytes),
        },
    };
    let mut hashed = bytes.clone();
    hashed.extend_from_slice(b"\0config.toml\0");
    hashed.extend_from_slice(config.as_deref().unwrap_or_default().as_bytes());
    let hash = format!("{:016x}", fnv1a(&hashed));
    let mut status = load_status(state_dir);
    let now_secs = unix_secs(now);
    let mut results = Vec::new();
    for target in targets {
        let record = status.targets.entry(target.key()).or_default();
        if let RunMode::Due(_) = mode {
            let due = triggers
                .iter()
                .any(|trigger| is_due(target.every, *trigger, record, now_secs));
            if !due {
                results.push((target, Ok(RunOutcome::NotDue)));
                continue;
            }
        }
        if let Some(error) = &invalid {
            record.last_attempt = Some(now_secs);
            record.last_error = Some(error.clone());
            results.push((target, Err(error.clone())));
            continue;
        }
        if let RunMode::Due(_) = mode {
            if record.last_hash.as_deref() == Some(hash.as_str()) && record.last_error.is_none() {
                record.last_success = Some(now_secs);
                results.push((target, Ok(RunOutcome::UpToDate)));
                continue;
            }
        }
        record.last_attempt = Some(now_secs);
        let outcome = match target.kind {
            Kind::Git => backup_to_git(state_dir, &target, &machine, &copy, now),
        }
        .map_err(|error| target_error(&target, error));
        if outcome.is_ok() && !matches!(mode, RunMode::Due(_)) {
            tidy_clone(state_dir, &target);
        }
        match &outcome {
            Ok(_) => {
                record.last_success = Some(now_secs);
                record.last_error = None;
                record.last_hash = Some(hash.clone());
            }
            Err(error) => {
                record.last_error = Some(error.clone());
            }
        }
        results.push((target, outcome));
    }
    save_status(state_dir, &status)
        .map_err(|error| format!("could not record backup status: {error}"))?;
    Ok(results)
}

fn trigger_flag(state_dir: &Path, trigger: Trigger) -> PathBuf {
    state_dir.join(format!("{TRIGGER_FLAG_PREFIX}{}", trigger.word()))
}

/// What one backup writes: the store, `config.toml` beside it when there is one (already
/// stripped of secrets), `machine.toml`, and what its commit message records.
struct Payload<'a> {
    store: &'a [u8],
    config: Option<&'a [u8]>,
    machine_toml: String,
    summary: Summary,
}

/// What a commit message records about its backup, so `tsk restore` lists versions from one
/// `git log` without reading each file.
struct Summary {
    tasks: Option<usize>,
    format: Option<u32>,
}

/// This machine as a backup names it: the directory key and the name people read.
struct Machine {
    id: String,
    hostname: String,
}

/// Value every `[agent.<name>.env]` entry takes in a backed-up `config.toml`.
pub const ENV_PLACEHOLDER: &str = "<not backed up>";

/// `config.toml` as a backup pushes it: every value of every `[agent.<name>.env]` table
/// replaced by [`ENV_PLACEHOLDER`] (the keys stay, so a restore shows what to fill in), under a
/// comment saying so. A file that does not parse is not pushed at all, since its secrets
/// cannot be found.
pub fn sanitized_config(content: &str) -> Option<String> {
    let mut document: DocumentMut = content.parse().ok()?;
    if let Some(agents) = document
        .get_mut("agent")
        .and_then(|item| item.as_table_like_mut())
    {
        for (_, profile) in agents.iter_mut() {
            let Some(env) = profile
                .as_table_like_mut()
                .and_then(|profile| profile.get_mut("env"))
                .and_then(|env| env.as_table_like_mut())
            else {
                continue;
            };
            for (_, value) in env.iter_mut() {
                let decor = value.as_value().map(|v| v.decor().clone());
                *value = toml_edit::value(ENV_PLACEHOLDER);
                if let (Some(decor), Some(v)) = (decor, value.as_value_mut()) {
                    *v.decor_mut() = decor;
                }
            }
        }
    }
    // A token written into a target's URL (`https://user:token@host/…`) is removed, in
    // either shape the loader takes.
    if let Some(backups) = document.get_mut("backup") {
        for table in backup_entries_mut(backups) {
            let stripped = table
                .get("url")
                .and_then(|url| url.as_str())
                .and_then(without_credentials);
            if let (Some(stripped), Some(url)) = (stripped, table.get_mut("url")) {
                let decor = url.as_value().map(|v| v.decor().clone());
                *url = toml_edit::value(stripped);
                if let (Some(decor), Some(v)) = (decor, url.as_value_mut()) {
                    *v.decor_mut() = decor;
                }
            }
        }
    }
    Some(format!(
        "# Backed up by tsk. Values under [agent.<name>.env] and credentials in backup URLs\n# are never pushed: env values read \"{ENV_PLACEHOLDER}\" here. Fill them in by\n# hand after copying a profile over.\n{document}"
    ))
}

/// `url` without the credentials of its `scheme://userinfo@` part, or `None` when it has
/// none. A user name alone stays for ssh (`ssh://git@host/…`), where it is not a secret;
/// over http(s) a bare user name is often a token, so it goes too. scp-style `git@host:path`
/// carries no password and is left as is.
pub fn without_credentials(url: &str) -> Option<String> {
    let (scheme, kept, host) = credentials(url)?;
    let kept = kept.map(|user| format!("{user}@")).unwrap_or_default();
    Some(format!("{scheme}://{kept}{host}"))
}

/// `url` as tsk prints it: the credentials [`without_credentials`] removes read `***`
/// (`https://***@host/…`, `ssh://git:***@host/…`). Anything else is shown as written.
pub fn shown_location(url: &str) -> String {
    match credentials(url) {
        Some((scheme, Some(user), host)) => format!("{scheme}://{user}:***@{host}"),
        Some((scheme, None, host)) => format!("{scheme}://***@{host}"),
        None => url.to_string(),
    }
}

/// `text` with every URL in it shown as [`shown_location`] shows it: git's messages quote
/// the URL they failed on, credentials included. Only a URL's authority (up to the first
/// `/`, `?`, `#`, or space) is rewritten, so quotes and other characters a password may
/// hold never cut a credential short.
pub fn redacted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        // The scheme's characters are ASCII, so `start` is always a char boundary.
        let start = rest[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            .last()
            .map_or(at, |(index, _)| index);
        let authority = at + 3;
        let end = rest[authority..]
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .map_or(rest.len(), |index| authority + index);
        out.push_str(&rest[..start]);
        out.push_str(&shown_location(&rest[start..end]));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// `error`, from a git call on `target`, as tsk records and prints it: [`redacted`], and
/// with the target's credentials gone even where git quotes them outside a URL
/// (`unable to look up me:token@host`).
fn target_error(target: &Target, error: String) -> String {
    let error = redacted(&error);
    let userinfo = credentials(&target.location).and_then(|_| {
        let rest = target.location.split_once("://")?.1;
        let authority = rest.split(['/', '?', '#']).next()?;
        Some(authority.rsplit_once('@')?.0).filter(|userinfo| !userinfo.is_empty())
    });
    match userinfo {
        Some(userinfo) => error.replace(&format!("{userinfo}@"), "***@"),
        None => error,
    }
}

/// A `url` whose `scheme://userinfo@` part holds a secret, split into its scheme, the user
/// name a redaction keeps, and the rest from the host on. `None` when it holds no secret.
fn credentials(url: &str) -> Option<(&str, Option<&str>, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (userinfo, _) = rest[..authority_end].rsplit_once('@')?;
    let host = &rest[userinfo.len() + 1..];
    let http = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    let kept = match userinfo.split_once(':') {
        None if !http => return None,
        Some((user, _)) if !http && !user.is_empty() => Some(user),
        _ => None,
    };
    Some((scheme, kept, host))
}

/// Pushes a backup tries before recording the rejection as the target's error. Each retry
/// fetches and rebuilds the commit on the new remote head: a machine writes only its own
/// directory, so the rebuild is the conflict-free rebase.
const PUSH_ATTEMPTS: usize = 3;

fn backup_to_git(
    state_dir: &Path,
    target: &Target,
    machine: &Machine,
    copy: &Payload<'_>,
    now: SystemTime,
) -> Result<RunOutcome, String> {
    let clone = sync_clone(state_dir, target)?;
    let base = format!("machines/{}", machine.id);
    let relative = format!("{base}/tsk.json");
    let config_relative = format!("{base}/config.toml");
    let machine_relative = format!("{base}/machine.toml");
    let mut last_error = String::new();
    for attempt in 0..PUSH_ATTEMPTS {
        if attempt > 0 {
            git_ok(&clone.dir, &["fetch", "--quiet", "--prune", "origin"])?;
        }
        let remote = format!("refs/remotes/origin/{}", clone.branch);
        let has_remote = git(&clone.dir, &["rev-parse", "--verify", "--quiet", &remote])?
            .status
            .success();
        if has_remote {
            git_ok(
                &clone.dir,
                &["checkout", "--quiet", "-f", "-B", &clone.branch, &remote],
            )?;
        } else {
            git_ok(
                &clone.dir,
                &[
                    "symbolic-ref",
                    "HEAD",
                    &format!("refs/heads/{}", clone.branch),
                ],
            )?;
        }
        write_in_clone(&clone.dir, &relative, copy.store)?;
        git_ok(&clone.dir, &["add", "--", &relative])?;
        if let Some(config) = copy.config {
            write_in_clone(&clone.dir, &config_relative, config)?;
            git_ok(&clone.dir, &["add", "--", &config_relative])?;
        }
        write_in_clone(&clone.dir, &machine_relative, copy.machine_toml.as_bytes())?;
        git_ok(&clone.dir, &["add", "--", &machine_relative])?;
        let staged = git(
            &clone.dir,
            &[
                "diff",
                "--cached",
                "--quiet",
                "--",
                &relative,
                &config_relative,
                &machine_relative,
            ],
        )?;
        if !staged.status.success() {
            let message = commit_message(machine, &copy.summary, now);
            git_ok(
                &clone.dir,
                &[
                    "-c",
                    &format!("user.email=tsk@{}", machine.hostname),
                    "commit",
                    "--quiet",
                    "--no-verify",
                    "-m",
                    &message,
                ],
            )?;
        }
        let ahead = if has_remote {
            git_ok(
                &clone.dir,
                &["rev-list", "--count", &format!("{remote}..HEAD")],
            )?
            .trim()
                != "0"
        } else {
            git(&clone.dir, &["rev-parse", "--verify", "--quiet", "HEAD"])?
                .status
                .success()
        };
        if !ahead {
            return Ok(RunOutcome::UpToDate);
        }
        let push = git(
            &clone.dir,
            &[
                "push",
                "--quiet",
                "--no-verify",
                "origin",
                &format!("HEAD:refs/heads/{}", clone.branch),
            ],
        )?;
        if push.status.success() {
            return Ok(RunOutcome::Written(format!(
                "{} {relative}",
                shown_location(&target.location)
            )));
        }
        last_error = git_failure("push", &push);
        // Only a push another machine raced is worth another try; anything else (access,
        // a hook on the server) fails the same way again.
        let stderr = String::from_utf8_lossy(&push.stderr);
        let raced = [
            "non-fast-forward",
            "fetch first",
            "cannot lock ref",
            "reference already exists",
        ]
        .iter()
        .any(|sign| stderr.contains(sign));
        if !raced {
            return Err(last_error);
        }
    }
    Err(format!(
        "push rejected {PUSH_ATTEMPTS} times (another machine kept pushing): {last_error}"
    ))
}

/// Let git repack the target's clone if it wants to (`gc --auto` with git's own threshold).
/// Only foreground runs (`tsk backup`, `add`, `setup`) call this, where the user is already
/// waiting; a failure only leaves loose objects for the next one.
fn tidy_clone(state_dir: &Path, target: &Target) {
    let dir = repos_dir(state_dir).join(target.id());
    if dir.is_dir() {
        let _ = git(&dir, &["-c", "gc.auto=6700", "gc", "--auto", "--quiet"]);
    }
}

/// A backup commit's message: a readable subject, then `key: value` lines `tsk restore` reads.
fn commit_message(machine: &Machine, summary: &Summary, now: SystemTime) -> String {
    let mut message = format!(
        "tsk backup from {} at {}\n\nmachine: {}\nhostname: {}\ntsk: {}\n",
        machine.hostname,
        human_time(unix_secs(now)),
        machine.id,
        machine.hostname,
        env!("CARGO_PKG_VERSION")
    );
    if let Some(tasks) = summary.tasks {
        message.push_str(&format!("tasks: {tasks}\n"));
    }
    if let Some(format) = summary.format {
        message.push_str(&format!("format: {format}\n"));
    }
    message
}

/// Write `bytes` at `relative` inside `clone` without following a link: every component
/// below the clone must be a real directory (or, for the last, absent or a regular file),
/// and the file is written beside its name and renamed over it, which replaces a link itself
/// rather than its target. Checkouts already run with `core.symlinks=false`; this holds even
/// for a link some other git put in the work tree.
fn write_in_clone(clone: &Path, relative: &str, bytes: &[u8]) -> Result<(), String> {
    let components: Vec<&str> = relative.split('/').collect();
    let mut path = clone.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        path.push(component);
        let last = index + 1 == components.len();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "refusing to write through a link at {relative} in the backup repository"
                ))
            }
            Ok(metadata) if !last && !metadata.is_dir() => {
                return Err(format!(
                    "{relative} in the backup repository is not a directory"
                ))
            }
            Ok(metadata) if last && !metadata.is_file() => {
                return Err(format!("{relative} in the backup repository is not a file"))
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !last {
                    fs::create_dir(&path).map_err(|error| error.to_string())?;
                }
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    let tmp = path.with_file_name(format!(".tsk-backup.tmp.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        drop(file);
        fs::rename(&tmp, &path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&tmp);
        return Err(error.to_string());
    }
    Ok(())
}

/// A git target's local clone, fetched, and the branch backups go to.
struct GitClone {
    dir: PathBuf,
    branch: String,
}

/// Clone the target under `backup-repos/<id>` the first time, fetch it after. The branch is
/// the remote's default, or `main` for an empty repository.
/// Where target clones live, absolute: git runs with `-C` in one directory and is handed
/// paths for another, so a relative state directory would resolve twice.
fn repos_dir(state_dir: &Path) -> PathBuf {
    let repos = state_dir.join(REPOS_DIR);
    std::path::absolute(&repos).unwrap_or(repos)
}

fn sync_clone(state_dir: &Path, target: &Target) -> Result<GitClone, String> {
    let repos = repos_dir(state_dir);
    fsperm::ensure_private_dir(&repos).map_err(|error| error.to_string())?;
    let dir = repos.join(target.id());
    if dir.join(".git").is_dir() {
        git_ok(
            &dir,
            &["remote", "set-url", "origin", "--", &target.location],
        )?;
        git_ok(&dir, &["fetch", "--quiet", "--prune", "origin"])?;
    } else {
        let _ = fs::remove_dir_all(&dir);
        let mut command = git_command(&repos);
        command
            .args([
                "clone",
                "--quiet",
                "--no-checkout",
                "--config",
                "core.symlinks=false",
                "--",
                &target.location,
            ])
            .arg(&dir);
        let output = crate::git_base::bounded_process_output(command, "git", GIT_TIMEOUT)?;
        if !output.status.success() {
            let _ = fs::remove_dir_all(&dir);
            return Err(git_failure("clone", &output));
        }
    }
    let branch = git(
        &dir,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .filter(|output| output.status.success())
    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    .and_then(|name| name.strip_prefix("origin/").map(str::to_string))
    .or_else(|| {
        ["main", "master"].into_iter().find_map(|name| {
            git(
                &dir,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/remotes/origin/{name}"),
                ],
            )
            .ok()
            .filter(|output| output.status.success())
            .map(|_| name.to_string())
        })
    })
    // An empty remote: `git clone` set the clone's HEAD to the remote's unborn default branch
    // (`master` from a Git for Windows `init --bare`, `main` on GitHub), so the first push
    // lands where the remote's HEAD already points.
    .or_else(|| {
        git(&dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|name| !name.is_empty())
    })
    .unwrap_or_else(|| "main".to_string());
    Ok(GitClone { dir, branch })
}

/// Every git call a backup makes. It runs in a detached child with no terminal, so it must
/// never prompt and never depend on the user's global git setup: a fixed identity, no signing,
/// no hooks (an empty hooks directory), no interactive credential helper, and ssh in batch
/// mode unless the user chose their own `GIT_SSH_COMMAND`. A backup remote is not trusted to
/// plant links in the clone either: links are checked out as plain files.
fn git_command(dir: &Path) -> Command {
    // `dir` is the repos directory or a clone inside it; the empty hooks directory lives there.
    let repos = if dir.file_name().is_some_and(|name| name == REPOS_DIR) {
        dir.to_path_buf()
    } else {
        dir.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dir.to_path_buf())
    };
    let hooks = repos.join(".no-hooks");
    let _ = fs::create_dir_all(&hooks);
    let mut command = Command::new("git");
    command.arg("-C").arg(dir);
    for setting in [
        "core.symlinks=false".to_string(),
        "user.name=tsk".to_string(),
        "user.email=tsk@localhost".to_string(),
        "commit.gpgsign=false".to_string(),
        "tag.gpgsign=false".to_string(),
        format!("core.hooksPath={}", hooks.display()),
        "credential.interactive=never".to_string(),
        // A background run never stops to repack: `gc --auto` can take seconds on a clone
        // that has collected thousands of backups. Foreground runs tidy instead (see
        // `tidy_clone`).
        "gc.auto=0".to_string(),
        "maintenance.auto=false".to_string(),
    ] {
        command.arg("-c").arg(setting);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    command
}

fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = git_command(dir);
    command.args(args);
    crate::git_base::bounded_process_output(command, "git", GIT_TIMEOUT)
}

fn git_ok(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = git(dir, args)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(git_failure(args.first().copied().unwrap_or("git"), &output))
    }
}

fn git_failure(verb: &str, output: &std::process::Output) -> String {
    // Git's whole message (its last few lines when long): one line alone can be a fragment.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let detail = if lines.is_empty() {
        "no error output".to_string()
    } else {
        lines[lines.len().saturating_sub(4)..].join(" / ")
    };
    format!("git {verb} failed: {detail}")
}

/// One restorable copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// Short id `tsk restore <id>` takes.
    pub id: String,
    /// Unix seconds the copy was made.
    pub at: u64,
    /// The host name the backup's commit records (the machine id's start when it has none).
    pub machine: String,
    /// The machine id: the directory under `machines/` the version came from.
    pub machine_id: String,
    /// The target's URL or path, as [`shown_location`] shows it.
    pub target: String,
    /// Tasks in the copy (notices and deleted tasks excluded); `None` when unreadable.
    pub tasks: Option<usize>,
    /// Orders versions made in the same second: the commit's place in history (newer is
    /// higher).
    order: u32,
    source: Source,
}

/// Where a version lives: a file at a commit of a target's clone.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Source {
    dir: PathBuf,
    commit: String,
    path: String,
    /// The target the clone belongs to, so a failed read is reported as [`target_error`].
    target: Target,
}

/// Every version on every target, newest first, plus a failure per target that could not be
/// read. A git target lists [`GIT_VERSIONS_SHOWN`] versions per machine unless `all`.
pub fn list_versions(state_dir: &Path, all: bool) -> Result<Listing, String> {
    let targets = load_targets(state_dir)?;
    let mut versions = Vec::new();
    let mut failures = Vec::new();
    for target in targets {
        // The clone is shared with backup runs: never fetch it under one.
        let listed = with_run_lock(state_dir, true, || {
            Ok(git_versions(state_dir, &target, all))
        })
        .map_err(|error| error.to_string())
        .and_then(|listed| listed);
        match listed {
            Ok(found) => versions.extend(found),
            Err(error) => {
                let error = target_error(&target, error);
                failures.push((target, error));
            }
        }
    }
    versions.sort_by(|a, b| {
        (b.at, b.order)
            .cmp(&(a.at, a.order))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok((versions, failures))
}

fn git_versions(state_dir: &Path, target: &Target, all: bool) -> Result<Vec<Version>, String> {
    let clone = sync_clone(state_dir, target)?;
    let remote = format!("refs/remotes/origin/{}", clone.branch);
    if !git(&clone.dir, &["rev-parse", "--verify", "--quiet", &remote])?
        .status
        .success()
    {
        return Ok(Vec::new());
    }
    // One `git log`: each commit's message carries its host name and task count.
    let log = git_ok(
        &clone.dir,
        &[
            "log",
            "--format=%x01%H %ct%x02%B%x03",
            "--name-only",
            &remote,
            "--",
            "machines",
        ],
    )?;
    let mut per_machine: BTreeMap<String, usize> = BTreeMap::new();
    let mut versions = Vec::new();
    // `git log` lists newest first: the first record gets the highest order.
    for (position, record) in log
        .split('\u{1}')
        .filter(|record| !record.trim().is_empty())
        .enumerate()
    {
        let order = u32::MAX - u32::try_from(position).unwrap_or(u32::MAX);
        let Some((head, rest)) = record.split_once('\u{2}') else {
            continue;
        };
        let Some((message, names)) = rest.split_once('\u{3}') else {
            continue;
        };
        let Some((commit, time)) = head.trim().split_once(' ') else {
            continue;
        };
        let Ok(at) = time.trim().parse::<u64>() else {
            continue;
        };
        let field = |key: &str| {
            message.lines().find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix(':'))
                    .map(|value| value.trim().to_string())
            })
        };
        let hostname = field("hostname");
        let tasks = field("tasks").and_then(|count| count.parse().ok());
        for path in names.lines().map(str::trim) {
            let Some(machine) = path
                .strip_prefix("machines/")
                .and_then(|rest| rest.strip_suffix("/tsk.json"))
                .filter(|machine| is_machine_id(machine))
            else {
                continue;
            };
            let shown = per_machine.entry(machine.to_string()).or_default();
            if !all && *shown >= GIT_VERSIONS_SHOWN {
                continue;
            }
            *shown += 1;
            let source = Source {
                dir: clone.dir.clone(),
                commit: commit.to_string(),
                path: path.to_string(),
                target: target.clone(),
            };
            versions.push(Version {
                id: version_id(target, machine, commit),
                at,
                machine: hostname
                    .clone()
                    .unwrap_or_else(|| machine.chars().take(8).collect()),
                machine_id: machine.to_string(),
                target: shown_location(&target.location),
                tasks,
                order,
                source,
            });
        }
    }
    Ok(versions)
}

/// The git command that prints the `config.toml` backed up with `version`, for people.
/// `None` when the version has none.
pub fn version_config(version: &Version) -> Option<String> {
    let Source {
        dir, commit, path, ..
    } = &version.source;
    let config = format!("{}/config.toml", path.rsplit_once('/')?.0);
    let spec = format!("{commit}:{config}");
    git(dir, &["cat-file", "-e", &spec])
        .ok()
        .filter(|output| output.status.success())
        .map(|_| format!("git -C \"{}\" show {spec}", dir.display()))
}

/// What a repository holds, for `tsk backup setup` deciding whether to reuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoContents {
    Empty,
    /// It has `machines/`: backups from tsk.
    Backups,
    /// Anything else: someone else's repository.
    Other,
}

/// Clone (or fetch) `location` the way a backup would and say what its default branch holds.
pub fn repo_contents(state_dir: &Path, location: &str) -> Result<RepoContents, String> {
    let target = Target::new(location.to_string());
    with_run_lock(state_dir, true, || {
        Ok((|| {
            let clone = sync_clone(state_dir, &target)?;
            let remote = format!("refs/remotes/origin/{}", clone.branch);
            if !git(&clone.dir, &["rev-parse", "--verify", "--quiet", &remote])?
                .status
                .success()
            {
                return Ok(RepoContents::Empty);
            }
            let names = git_ok(&clone.dir, &["ls-tree", "--name-only", &remote])?;
            let names: Vec<&str> = names
                .lines()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .collect();
            Ok(if names.is_empty() {
                RepoContents::Empty
            } else if names.contains(&"machines") {
                RepoContents::Backups
            } else {
                RepoContents::Other
            })
        })())
    })
    .map_err(|error| error.to_string())
    .and_then(|contents| contents)
    .map_err(|error| target_error(&target, error))
}

/// The bytes of one listed version.
pub fn read_version(version: &Version) -> Result<Vec<u8>, String> {
    read_source(&version.source).map_err(|error| target_error(&version.source.target, error))
}

fn read_source(source: &Source) -> Result<Vec<u8>, String> {
    let output = git(
        &source.dir,
        &["show", &format!("{}:{}", source.commit, source.path)],
    )?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(git_failure("show", &output))
    }
}

fn version_id(target: &Target, machine: &str, key: &str) -> String {
    let seed = format!("{}\0{machine}\0{key}", target.key());
    format!("{:016x}", fnv1a(seed.as_bytes()))[..8].to_string()
}

/// Tasks in a store document, leaving out notices and soft-deleted tasks.
pub fn count_tasks(bytes: &[u8]) -> Option<usize> {
    let document: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let tasks = document.get("tasks")?.as_array()?;
    Some(
        tasks
            .iter()
            .filter(|task| task.get("notice").is_none_or(serde_json::Value::is_null))
            .filter(|task| task.get("soft_deleted") != Some(&serde_json::Value::Bool(true)))
            .count(),
    )
}

/// This state directory's machine id, generated once and kept in `machine-id` (never in
/// `tsk.json`, so a restored store does not carry another machine's id).
pub fn machine_id(state_dir: &Path) -> io::Result<String> {
    let path = state_dir.join(MACHINE_ID_FILE);
    if let Some(id) = read_machine_id(&path) {
        return Ok(id);
    }
    fsperm::ensure_private_dir(state_dir)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(format!("{id}\n").as_bytes())?;
            file.sync_all()?;
            Ok(id)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => read_machine_id(&path)
            .ok_or_else(|| io::Error::other("machine-id is being written; try again")),
        Err(error) => Err(error),
    }
}

fn read_machine_id(path: &Path) -> Option<String> {
    let id = fs::read_to_string(path).ok()?.trim().to_string();
    is_machine_id(&id).then_some(id)
}

/// The shape of a machine id, and so of every `machines/<id>/` directory tsk lists: anything
/// else in a shared repository is skipped, since its name ends up in a printed command.
fn is_machine_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The host name's first label, lowercased, as letters, digits, and hyphens.
pub fn hostname() -> String {
    let raw = raw_hostname().unwrap_or_default();
    let label = raw.split('.').next().unwrap_or_default().to_lowercase();
    let cleaned: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(32)
        .collect();
    let cleaned = cleaned.trim_matches('-').to_string();
    if cleaned.is_empty() {
        "machine".to_string()
    } else {
        cleaned
    }
}

#[cfg(unix)]
fn raw_hostname() -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its full length, which is what we pass.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result != 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8(buffer[..end].to_vec()).ok()
}

#[cfg(not(unix))]
fn raw_hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

fn open_lock(state_dir: &Path, name: &str) -> io::Result<File> {
    let path = state_dir.join(name);
    let file = fsperm::open_lock_file(&path)?;
    fsperm::tighten_file(&path);
    Ok(file)
}

struct LockGuard(File);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Run `body` under the run lock: waiting for it when `wait`, else failing when it is held.
fn with_run_lock<T>(
    state_dir: &Path,
    wait: bool,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    // A new machine's first `tsk backup setup` may come before any store exists.
    fsperm::ensure_private_dir(state_dir)?;
    let file = open_lock(state_dir, RUN_LOCK_FILE)?;
    if wait {
        file.lock()?;
    } else {
        file.try_lock().map_err(io::Error::from)?;
    }
    let _guard = LockGuard(file);
    body()
}

/// 64-bit FNV-1a: change detection and short ids, not security.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    hash
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn split_secs(secs: u64) -> ((i64, i64, i64), (u64, u64, u64)) {
    let date = civil_from_days((secs / DAY_SECS) as i64);
    let rest = secs % DAY_SECS;
    (date, (rest / 3_600, rest % 3_600 / 60, rest % 60))
}

/// `YYYYMMDDTHHMMSSZ`, UTC: sorts by time and is safe in any file name.
pub fn utc_stamp(time: SystemTime) -> String {
    let ((year, month, day), (hour, minute, second)) = split_secs(unix_secs(time));
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// `YYYY-MM-DD HH:MM UTC` for people.
pub fn human_time(secs: u64) -> String {
    let ((year, month, day), (hour, minute, _)) = split_secs(secs);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tsk-backup-{label}-{}-{}-{}",
                std::process::id(),
                now_nanos(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const DAY: u64 = DAY_SECS;
    const HOUR: u64 = 3_600;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// An empty bare repository under `root`.
    fn bare_repo(root: &Path) -> PathBuf {
        let remote = root.join("remote.git");
        fs::create_dir_all(&remote).expect("remote");
        // Not `main`: the first push must follow the remote's own default branch, as on Git
        // for Windows, whose `init` still defaults to `master`.
        git_in(
            &remote,
            &["init", "--quiet", "--bare", "--initial-branch=trunk"],
        );
        remote
    }

    /// A state dir under `root` whose one target is `remote`, plus `extra` entry keys.
    fn state_with_target(root: &Path, remote: &Path, extra: &str) -> PathBuf {
        let state = root.join("state");
        fs::create_dir_all(&state).expect("state");
        fs::write(
            state.join("config.toml"),
            format!(
                "[[backup]]\nurl = {:?}\n{extra}",
                remote.display().to_string()
            ),
        )
        .expect("config");
        state
    }

    /// Commits on every branch of `remote`.
    fn commits(remote: &Path) -> usize {
        git_in(remote, &["rev-list", "--count", "--all"])
            .trim()
            .parse()
            .unwrap_or(0)
    }

    /// Write a valid store holding one task titled `title`; returns its bytes.
    fn write_store(dir: &Path, title: &str) -> Vec<u8> {
        let mut state = crate::domain::DomainState::new();
        state
            .create(
                title,
                None,
                crate::domain::TaskScope::Global,
                crate::domain::ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
        let bytes = serde_json::to_vec_pretty(&state).expect("json");
        fs::write(dir.join("tsk.json"), &bytes).expect("store");
        bytes
    }

    #[test]
    fn stamps_and_human_times_read_in_utc() {
        assert_eq!(utc_stamp(at(951_782_400)), "20000229T000000Z");
        assert_eq!(utc_stamp(at(1_792_135_428)), "20261016T072348Z");
        assert_eq!(human_time(1_792_135_428), "2026-10-16 07:23 UTC");
        assert!(utc_stamp(at(100)) < utc_stamp(at(1_000_000)));
    }

    #[test]
    fn due_follows_each_frequency() {
        let never = TargetStatus::default();
        let now = 10 * DAY;
        assert!(is_due(Every::Save, Trigger::Save, &never, now));
        assert!(!is_due(Every::Save, Trigger::Open, &never, now));
        for trigger in [Trigger::Save, Trigger::Open] {
            assert!(
                is_due(Every::Daily, trigger, &never, now),
                "never backed up"
            );
        }
        let recent = |ago: u64| TargetStatus {
            last_success: Some(now - ago),
            last_attempt: Some(now - ago),
            ..TargetStatus::default()
        };
        assert!(!is_due(
            Every::Daily,
            Trigger::Save,
            &recent(23 * HOUR),
            now
        ));
        assert!(is_due(Every::Daily, Trigger::Save, &recent(24 * HOUR), now));
        assert!(is_due(Every::Daily, Trigger::Open, &recent(24 * HOUR), now));
        let failed = |ago: u64| TargetStatus {
            last_success: Some(now - 2 * DAY),
            last_attempt: Some(now - ago),
            last_error: Some("offline".into()),
            ..TargetStatus::default()
        };
        assert!(
            !is_due(Every::Daily, Trigger::Save, &failed(60), now),
            "retry backs off"
        );
        assert!(is_due(Every::Daily, Trigger::Save, &failed(11 * 60), now));
    }

    #[test]
    fn targets_parse_with_defaults_and_refuse_bad_entries() {
        let targets = parse_targets(
            r#"
[agent.claude]
command = ["claude"]

[[backup]]
url = "git@github.com:me/tsk-backup.git"

[[backup]]
kind = "git"
url = "/srv/tsk.git"
every = "daily"
"#,
        )
        .expect("parses");
        assert_eq!(targets.len(), 2);
        assert_eq!(
            (targets[0].kind, targets[0].every),
            (Kind::Git, Every::Save)
        );
        assert_eq!(
            (targets[1].kind, targets[1].every),
            (Kind::Git, Every::Daily)
        );
        let inline = parse_targets(r#"backup = [{ url = "/x.git", every = "daily" }]"#)
            .expect("inline list parses");
        assert_eq!(inline[0].every, Every::Daily);
        for gone in ["open", "twice-daily"] {
            let error = parse_targets(&format!("[[backup]]\nurl = \"/x\"\nevery = \"{gone}\""))
                .expect_err(gone);
            assert!(error.contains("not save or daily"), "{error}");
        }
        for (bad, why) in [
            ("[[backup]]\nevery = \"save\"", "url is required"),
            (
                "[[backup]]\nkind = \"folder\"\nurl = \"/x\"",
                "kind \"folder\" is not git",
            ),
            (
                "[[backup]]\nurl = \"/x\"\npath = \"/y\"",
                "unknown key \"path\"",
            ),
            (
                "[[backup]]\nurl = \"/x\"\nkeep_last = 2",
                "unknown key \"keep_last\"",
            ),
            (
                "[[backup]]\nurl = \"/x\"\nevery = \"hourly\"",
                "not save or daily",
            ),
            ("backup = 3", "list of [[backup]]"),
        ] {
            let error = parse_targets(bad).expect_err(bad);
            assert!(error.contains(why), "{bad}: {error}");
        }
    }

    #[test]
    fn starter_backup_example_parses_once_uncommented() {
        let starter = crate::agents::STARTER_CONFIG;
        assert!(parse_targets(starter).expect("starter parses").is_empty());
        let mut in_block = false;
        let uncommented = starter
            .lines()
            .map(|line| {
                in_block = line == "# [[backup]]" || (in_block && !line.is_empty());
                if in_block {
                    line.strip_prefix("# ").unwrap_or(line)
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let targets = parse_targets(&uncommented).expect("example parses");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].every, Every::Save);
        assert!(targets[0].location.contains("github.com"));
    }

    #[test]
    fn other_config_readers_ignore_backup_tables() {
        let content = "[[backup]]\nurl = \"/x.git\"\n\n[agent.a]\ncommand = [\"a\"]\n";
        let dir = TempDir::new("readers");
        fs::write(dir.0.join("config.toml"), content).expect("write");
        assert!(crate::agents::AgentProfiles::load(&dir.0)
            .expect("profiles load")
            .get("a")
            .is_some());
        crate::agents::BoardDefaults::load(&dir.0).expect("board defaults load");
        crate::agents::MergeMethod::load(&dir.0).expect("merge method loads");
    }

    #[test]
    fn add_and_remove_keep_the_rest_of_config_toml() {
        let dir = TempDir::new("config");
        let original = "# my notes\n[agent.a]\ncommand = [\"a\"] # keep me\n";
        fs::write(dir.0.join("config.toml"), original).expect("write");
        let (target, outcome) = add_target(&dir.0, "git@h:me/one.git", None).expect("add");
        assert_eq!((outcome, target.every), (AddOutcome::Added, Every::Save));
        let (_, outcome) = add_target(&dir.0, "/r.git", Some(Every::Daily)).expect("second");
        assert_eq!(outcome, AddOutcome::Added);
        let (_, again) = add_target(&dir.0, "git@h:me/one.git", None).expect("again");
        assert_eq!(again, AddOutcome::Unchanged);
        let (updated, outcome) =
            add_target(&dir.0, "git@h:me/one.git", Some(Every::Daily)).expect("update");
        assert_eq!(
            (outcome, updated.every),
            (AddOutcome::Updated, Every::Daily)
        );
        let content = fs::read_to_string(dir.0.join("config.toml")).expect("read");
        assert!(content.starts_with(original), "{content}");
        assert!(
            !content.contains("kind"),
            "a git entry needs no kind: {content}"
        );
        let targets = load_targets(&dir.0).expect("load");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].every, Every::Daily);

        assert_eq!(
            remove_target(&dir.0, "2").expect("by number").location,
            "/r.git"
        );
        assert_eq!(
            remove_target(&dir.0, "git@h:me/one.git")
                .expect("by url")
                .location,
            "git@h:me/one.git"
        );
        assert!(remove_target(&dir.0, "git@h:me/one.git").is_err());
        assert_eq!(
            fs::read_to_string(dir.0.join("config.toml")).expect("read"),
            original,
            "the last remove drops the empty table"
        );
    }

    #[test]
    fn add_creates_config_toml_in_a_fresh_state_dir() {
        let dir = TempDir::new("fresh");
        add_target(&dir.0, "file:///r.git", None).expect("add");
        assert_eq!(load_targets(&dir.0).expect("load")[0].every, Every::Save);
    }

    #[test]
    fn machine_id_is_generated_once() {
        let dir = TempDir::new("machine");
        let first = machine_id(&dir.0).expect("id");
        assert_eq!(machine_id(&dir.0).expect("again"), first);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()), "{first}");
        let host = hostname();
        assert!(host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }

    #[test]
    fn counts_tasks_without_notices_or_deleted() {
        let document = br#"{"format_version":13,"tasks":[{"title":"a"},{"title":"n","notice":{"kind":"x"}},{"title":"d","soft_deleted":true},{"title":"b","notice":null}]}"#;
        assert_eq!(count_tasks(document), Some(2));
        assert_eq!(count_tasks(b"nope"), None);
    }

    #[test]
    fn a_run_follows_its_trigger_and_skips_an_unchanged_store() {
        let root = TempDir::new("run");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "");
        let now = 400 * DAY;
        assert_eq!(
            run(&state, RunMode::Due(Trigger::Save), at(now)).expect("run")[0].1,
            Ok(RunOutcome::NoStore)
        );
        let mut last = Vec::new();
        for (index, title) in ["a1", "a2"].iter().enumerate() {
            last = write_store(&state, title);
            let results =
                run(&state, RunMode::Due(Trigger::Save), at(now + index as u64)).expect("run");
            assert!(
                matches!(results[0].1, Ok(RunOutcome::Written(_))),
                "{results:?}"
            );
        }
        assert_eq!(commits(&remote), 2, "one commit per backup");
        let results = run(&state, RunMode::Due(Trigger::Save), at(now + 10)).expect("run");
        assert_eq!(
            results[0].1,
            Ok(RunOutcome::UpToDate),
            "same bytes are not pushed twice"
        );
        let results = run(&state, RunMode::Due(Trigger::Open), at(now + 11)).expect("run");
        assert_eq!(results[0].1, Ok(RunOutcome::NotDue));
        let machine = machine_id(&state).expect("machine");
        let (versions, failures) = list_versions(&state, false).expect("versions");
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(versions.len(), 2);
        assert_eq!(read_version(&versions[0]).expect("read"), last);
        // Keyed by machine id, listed by host name and task count from the commit message.
        assert_eq!(versions[0].machine_id, machine);
        assert_eq!(versions[0].machine, hostname());
        assert_eq!(versions[0].tasks, Some(1));
        let machine_toml = git_in(
            &remote,
            &["show", &format!("HEAD:machines/{machine}/machine.toml")],
        );
        assert!(
            machine_toml.contains(&format!("hostname = {:?}", hostname())),
            "{machine_toml}"
        );
        let message = git_in(&remote, &["log", "-1", "--format=%B"]);
        for line in [
            format!("machine: {machine}"),
            "tasks: 1".to_string(),
            format!("format: {}", crate::domain::STORE_FORMAT_VERSION),
            format!("tsk: {}", env!("CARGO_PKG_VERSION")),
        ] {
            assert!(message.contains(&line), "{line} in {message}");
        }
    }

    #[test]
    fn versions_made_in_one_second_list_newest_first() {
        let root = TempDir::new("same-second");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "");
        let mut written = Vec::new();
        for n in 0..3 {
            written.push(write_store(&state, &format!("n{n}")));
            run(&state, RunMode::Now, at(DAY)).expect("run");
        }
        let (versions, _) = list_versions(&state, false).expect("versions");
        let bodies: Vec<Vec<u8>> = versions
            .iter()
            .map(|version| read_version(version).expect("read"))
            .collect();
        written.reverse();
        assert_eq!(bodies, written);
    }

    #[test]
    fn a_failing_target_shows_on_every_board_open_until_it_succeeds() {
        let root = TempDir::new("fail");
        let missing = root.0.join("no-such-repo.git");
        let state = state_with_target(&root.0, &missing, "");
        write_store(&state, "x");
        let results = run(&state, RunMode::Now, at(DAY)).expect("run");
        assert!(results[0]
            .1
            .as_ref()
            .is_err_and(|e| e.contains("git clone failed")));
        let listed = statuses(&state).expect("statuses");
        assert!(listed[0].1.last_error.is_some());
        for open in 0..3 {
            let message = failure_message(&state).expect("every open shows it");
            assert!(
                message.starts_with("backup failed: git clone failed"),
                "open {open}: {message}"
            );
        }
        fs::create_dir_all(&missing).expect("repo");
        git_in(&missing, &["init", "--quiet", "--bare"]);
        let results = run(&state, RunMode::Now, at(DAY + 1)).expect("run");
        assert!(results[0].1.is_ok(), "{results:?}");
        assert_eq!(failure_message(&state), None, "gone once it succeeds");
    }

    #[test]
    fn a_run_that_loses_the_waiter_slot_leaves_its_trigger_to_the_waiter() {
        let root = TempDir::new("union");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "every = \"save\"\n");
        write_store(&state, "a1");
        // A waiter holds the slot when a save's run arrives: that run exits at once.
        let slot = open_lock(&state, PENDING_LOCK_FILE).expect("slot");
        slot.try_lock().expect("take the slot");
        assert!(run(&state, RunMode::Due(Trigger::Save), at(DAY))
            .expect("save run")
            .is_empty());
        // Unlock, as `LockGuard` does: closing alone leaves the lock with any child a parallel
        // test is spawning at that moment, which shares the open file until it execs.
        slot.unlock().expect("release the slot");
        // The waiter was started by a board open; it still backs up for the save.
        let results = run(&state, RunMode::Due(Trigger::Open), at(DAY + 1)).expect("waiter");
        assert!(
            matches!(results[0].1, Ok(RunOutcome::Written(_))),
            "{results:?}"
        );
        write_store(&state, "a2");
        let results = run(&state, RunMode::Due(Trigger::Open), at(DAY + 2)).expect("next");
        assert_eq!(results[0].1, Ok(RunOutcome::NotDue), "the flag is used up");
    }

    #[test]
    fn a_waiting_run_never_pushes_to_a_target_removed_meanwhile() {
        let root = TempDir::new("removed");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "");
        write_store(&state, "a1");
        // A run is pushing (it holds the run lock) when a save starts the waiter.
        let running = open_lock(&state, RUN_LOCK_FILE).expect("lock");
        running.lock().expect("hold the run lock");
        let waiter = {
            let state = state.clone();
            std::thread::spawn(move || run(&state, RunMode::Due(Trigger::Save), at(DAY)))
        };
        // It records its trigger just before it queues on the run lock.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !trigger_flag(&state, Trigger::Save).exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the waiter never queued"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(300));
        // Meanwhile the user removes the target (as `tsk backup remove` writes it).
        fs::write(state.join("config.toml"), "").expect("config");
        running.unlock().expect("release");
        let results = waiter.join().expect("waiter").expect("run");
        assert!(results.is_empty(), "{results:?}");
        assert_eq!(commits(&remote), 0);
        assert!(
            load_status(&state).targets.is_empty(),
            "no status resurrected"
        );
    }

    #[test]
    fn remove_waits_for_a_running_backup() {
        let root = TempDir::new("remove-lock");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "");
        let running = open_lock(&state, RUN_LOCK_FILE).expect("lock");
        running.lock().expect("hold the run lock");
        let remover = {
            let state = state.clone();
            std::thread::spawn(move || remove_target(&state, "1"))
        };
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            load_targets(&state).expect("targets").len(),
            1,
            "config.toml untouched while a run holds the lock"
        );
        running.unlock().expect("release");
        remover.join().expect("remover").expect("removed");
        assert!(load_targets(&state).expect("targets").is_empty());
    }

    #[test]
    fn add_waits_for_a_running_backup() {
        let root = TempDir::new("add-lock");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "");
        let running = open_lock(&state, RUN_LOCK_FILE).expect("lock");
        running.lock().expect("hold the run lock");
        let adder = {
            let state = state.clone();
            std::thread::spawn(move || add_target(&state, "/srv/other.git", None))
        };
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            load_targets(&state).expect("targets").len(),
            1,
            "config.toml untouched while a run holds the lock"
        );
        running.unlock().expect("release");
        adder.join().expect("adder").expect("added");
        assert_eq!(load_targets(&state).expect("targets").len(), 2);
    }

    #[test]
    fn only_machine_id_directories_are_listed() {
        let root = TempDir::new("hostile-machine");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "");
        write_store(&state, "mine");
        run(&state, RunMode::Now, at(DAY)).expect("run");
        // Another writer to a shared repository adds a directory whose name is a command.
        let work = root.0.join("work");
        git_in(
            &root.0,
            &["clone", "--quiet", remote.to_str().expect("utf-8"), "work"],
        );
        let hostile = work.join("machines").join("x$(touch pwned)");
        fs::create_dir_all(&hostile).expect("dir");
        fs::write(hostile.join("tsk.json"), "{}").expect("file");
        git_in(&work, &["add", "-A"]);
        git_in(
            &work,
            &["commit", "--quiet", "-m", "tsk backup\n\nhostname: evil"],
        );
        git_in(&work, &["push", "--quiet", "origin", "HEAD"]);
        let (versions, failures) = list_versions(&state, true).expect("list");
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(versions.len(), 1, "{versions:?}");
        assert!(versions.iter().all(|v| is_machine_id(&v.machine_id)));
    }

    #[test]
    fn credentials_in_backup_urls_are_never_pushed() {
        let pushed = sanitized_config(
            "[[backup]]\nurl = \"https://me:ghp_s3cret@github.com/me/b.git\" # mine\n\n[[backup]]\nurl = \"ssh://git:pw@host/x.git\"\n\n[[backup]]\nurl = \"git@github.com:me/b.git\"\n",
        )
        .expect("parses");
        assert!(
            !pushed.contains("s3cret") && !pushed.contains(":pw@"),
            "{pushed}"
        );
        assert!(
            pushed.contains("url = \"https://github.com/me/b.git\" # mine"),
            "{pushed}"
        );
        assert!(pushed.contains("ssh://git@host/x.git"), "{pushed}");
        assert!(pushed.contains("git@github.com:me/b.git"), "{pushed}");
        assert_eq!(
            without_credentials("https://TOKEN@github.com/a.git").as_deref(),
            Some("https://github.com/a.git")
        );
        assert_eq!(without_credentials("ssh://git@host/a.git"), None);
        assert_eq!(without_credentials("https://github.com/a@b.git"), None);
    }

    #[test]
    fn credentials_in_an_inline_backup_list_are_never_pushed() {
        let config = "backup = [{ url = \"https://me:ghp_s3cret@github.com/me/b.git\" }, { url = \"ssh://git:pw@host/x.git\", every = \"daily\" }]\n";
        assert_eq!(parse_targets(config).expect("parses").len(), 2);
        let pushed = sanitized_config(config).expect("parses");
        assert!(
            !pushed.contains("s3cret") && !pushed.contains(":pw@"),
            "{pushed}"
        );
        assert!(pushed.contains("https://github.com/me/b.git"), "{pushed}");
        assert!(pushed.contains("ssh://git@host/x.git"), "{pushed}");
    }

    #[test]
    fn a_password_holding_any_sub_delimiter_is_hidden_whole() {
        for c in "!$&'()*+,;=".chars() {
            let url = format!("https://me:sec{c}ret@host/x.git");
            assert_eq!(shown_location(&url), "https://***@host/x.git", "{c}");
            assert_eq!(
                redacted(&format!("fatal: '{url}' failed")),
                "fatal: 'https://***@host/x.git' failed",
                "{c}"
            );
        }
    }

    #[test]
    fn redacting_never_splits_a_character() {
        assert_eq!(redacted("remote: é://x"), "remote: é://x");
        assert_eq!(redacted("é://me:pw@h/x"), "é://me:***@h/x");
        assert_eq!(redacted("日本https://t@h/x"), "日本https://***@h/x");
        assert_eq!(
            redacted("https://t@hé/x 😀://a:b@c"),
            "https://***@hé/x 😀://a:***@c"
        );
        let pieces = [
            "://", "@", ":", "é", "日", "'", "/", " ", "a", "https", "-", "+", "😀", "?", "#",
        ];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..5_000 {
            let mut text = String::new();
            for _ in 0..12 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push_str(pieces[(seed % pieces.len() as u64) as usize]);
            }
            let once = redacted(&text);
            if !text.contains("://") {
                assert_eq!(once, text);
            }
        }
    }

    #[test]
    fn a_shown_location_naming_two_targets_removes_neither() {
        let root = TempDir::new("remove-ambiguous");
        let state = root.0.join("state");
        fs::create_dir_all(&state).expect("state");
        fs::write(
            state.join("config.toml"),
            "[[backup]]\nurl = \"https://a:one@h/x.git\"\n\n[[backup]]\nurl = \"https://b:two@h/x.git\"\n",
        )
        .expect("config");
        let error = remove_target(&state, "https://***@h/x.git").expect_err("ambiguous");
        assert!(error.contains("names 2 targets"), "{error}");
        assert!(
            !error.contains(":one@") && !error.contains(":two@"),
            "{error}"
        );
        assert_eq!(load_targets(&state).expect("targets").len(), 2);
        let removed = remove_target(&state, "https://b:two@h/x.git").expect("full URL");
        assert_eq!(removed.location, "https://b:two@h/x.git");
        // One left: its shown form names it.
        remove_target(&state, "https://***@h/x.git").expect("shown form");
        assert!(load_targets(&state).expect("targets").is_empty());
    }

    #[test]
    fn a_stored_error_never_holds_a_token() {
        // git quotes the first inside a URL, the second outside one.
        for (url, shown) in [
            (
                "file:///nope/https://me:S3CRET@h/x.git",
                "https://***@h/x.git",
            ),
            ("git://me:S3CRET@127.0.0.1:9/x.git", "***@127.0.0.1"),
        ] {
            let root = TempDir::new("stored-error");
            let state = state_with_target(&root.0, Path::new(url), "");
            write_store(&state, "a1");
            let results = run(&state, RunMode::Now, at(DAY)).expect("run");
            let returned = results[0].1.clone().expect_err("unreachable");
            let stored = load_status(&state)
                .targets
                .values()
                .find_map(|record| record.last_error.clone())
                .expect("an error is recorded");
            for error in [returned, stored, failure_message(&state).expect("shown")] {
                assert!(!error.contains("S3CRET"), "{error}");
                assert!(error.contains(shown), "{error}");
            }
        }
    }

    #[test]
    fn a_url_that_looks_like_an_option_is_only_a_url_for_an_existing_clone() {
        let root = TempDir::new("dash-set-url");
        let marker = root.0.join("ran");
        let url = format!("--upload-pack=touch {}", marker.display());
        let state = state_with_target(&root.0, Path::new(&url), "");
        write_store(&state, "a1");
        // A clone for this target already exists, so the run goes through `set-url`.
        let dir = repos_dir(&state).join(Target::new(url.clone()).id());
        fs::create_dir_all(&dir).expect("clone dir");
        git_in(&dir, &["init", "--quiet"]);
        git_in(&dir, &["remote", "add", "origin", "/nowhere.git"]);
        let results = run(&state, RunMode::Now, at(DAY)).expect("run");
        let error = results[0].1.clone().expect_err("no such repository");
        // set-url stored it as the URL, and fetch refused it as a path: no option was taken.
        assert_eq!(git_in(&dir, &["remote", "get-url", "origin"]).trim(), url);
        assert!(!error.contains("unknown option"), "{error}");
        assert!(error.contains("'--upload-pack=touch"), "{error}");
        assert!(!marker.exists());
    }

    #[test]
    fn a_url_that_looks_like_an_option_is_only_a_repository() {
        let root = TempDir::new("dash-url");
        let marker = root.0.join("ran");
        let url = format!("--upload-pack=touch {}", marker.display());
        let state = state_with_target(&root.0, Path::new(&url), "");
        write_store(&state, "a1");
        let results = run(&state, RunMode::Now, at(DAY)).expect("run");
        let error = results[0].1.clone().expect_err("no such repository");
        assert!(error.contains("'--upload-pack=touch"), "{error}");
        assert!(!marker.exists());
    }

    #[test]
    fn shown_locations_and_messages_hide_credentials() {
        assert_eq!(
            shown_location("https://me:ghp_s3cret@github.com/me/b.git"),
            "https://***@github.com/me/b.git"
        );
        assert_eq!(
            shown_location("https://TOKEN@github.com/a.git"),
            "https://***@github.com/a.git"
        );
        assert_eq!(
            shown_location("ssh://git:pw@host/x.git"),
            "ssh://git:***@host/x.git"
        );
        for plain in [
            "ssh://git@host/a.git",
            "git@github.com:me/b.git",
            "/srv/tsk.git",
            "https://github.com/a@b.git",
        ] {
            assert_eq!(shown_location(plain), plain);
        }
        assert_eq!(
            redacted("fatal: unable to access 'https://me:s3cret@example.invalid/b.git/': Could not resolve host"),
            "fatal: unable to access 'https://***@example.invalid/b.git/': Could not resolve host"
        );
        assert_eq!(
            redacted("ssh://git:pw@a/x and https://t@b/y"),
            "ssh://git:***@a/x and https://***@b/y"
        );
        assert_eq!(redacted("no url here"), "no url here");
    }

    #[test]
    fn a_store_that_does_not_parse_is_never_pushed() {
        let root = TempDir::new("damaged");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "");
        fs::write(
            state.join("tsk.json"),
            b"{\"format_version\":13,\"tasks\":7}",
        )
        .expect("store");
        let results = run(&state, RunMode::Now, at(DAY)).expect("run");
        let error = results[0].1.clone().expect_err("refused");
        assert!(
            error.starts_with("store does not parse, not backed up"),
            "{error}"
        );
        assert_eq!(commits(&remote), 0);
        assert_eq!(
            statuses(&state).expect("status")[0].1.last_error,
            Some(error)
        );
    }

    #[test]
    fn a_backup_carries_config_toml() {
        let root = TempDir::new("git-config");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "");
        let config = fs::read_to_string(state.join("config.toml")).expect("config");
        let config = sanitized_config(&config).expect("sanitized");
        write_store(&state, "with config");
        let results = run(&state, RunMode::Now, at(DAY)).expect("run");
        assert!(
            matches!(results[0].1, Ok(RunOutcome::Written(_))),
            "{results:?}"
        );
        let machine = machine_id(&state).expect("machine");
        let shown = git_in(
            &remote,
            &["show", &format!("HEAD:machines/{machine}/config.toml")],
        );
        assert_eq!(shown, config);
        let (versions, _) = list_versions(&state, false).expect("versions");
        assert!(version_config(&versions[0])
            .is_some_and(|hint| hint.contains(&format!("machines/{machine}/config.toml"))));
    }

    #[test]
    fn the_first_backup_nudge_shows_once_past_ten_tasks_and_never_with_a_target() {
        let state = TempDir::new("nudge");
        assert_eq!(take_backup_nudge(&state.0, NUDGE_AFTER_TASKS - 1), None);
        let message = take_backup_nudge(&state.0, NUDGE_AFTER_TASKS).expect("shown");
        assert!(message.contains("tsk backup setup"), "{message}");
        assert_eq!(take_backup_nudge(&state.0, 50), None, "only once ever");

        let root = TempDir::new("nudge-configured");
        let configured = state_with_target(&root.0, &root.0.join("r.git"), "");
        assert_eq!(take_backup_nudge(&configured, 50), None);
    }

    /// A backup remote whose `machines/<machine>/tsk.json`, or that machine's directory, is a
    /// link out of the clone must never get the store written through it.
    #[cfg(unix)]
    #[test]
    fn a_hostile_remote_cannot_redirect_a_git_backup_through_a_link() {
        for planted_dir in [false, true] {
            let root = TempDir::new("hostile");
            let remote = bare_repo(&root.0);
            let state = state_with_target(&root.0, &remote, "");
            let machine = machine_id(&state).expect("machine");
            let victim_dir = root.0.join("victim");
            fs::create_dir_all(&victim_dir).expect("victim dir");
            let victim = victim_dir.join("tsk.json");
            fs::write(&victim, b"precious").expect("victim");

            let work = root.0.join("work");
            fs::create_dir_all(work.join("machines")).expect("work");
            git_in(&work, &["init", "--quiet"]);
            let (link, link_target) = if planted_dir {
                (work.join("machines").join(&machine), victim_dir.clone())
            } else {
                fs::create_dir_all(work.join("machines").join(&machine)).expect("machine dir");
                (
                    work.join("machines").join(&machine).join("tsk.json"),
                    victim.clone(),
                )
            };
            std::os::unix::fs::symlink(&link_target, &link).expect("plant link");
            git_in(&work, &["add", "-A"]);
            git_in(&work, &["commit", "--quiet", "-m", "plant"]);
            git_in(
                &work,
                &[
                    "push",
                    "--quiet",
                    remote.to_str().expect("utf-8"),
                    "HEAD:refs/heads/main",
                ],
            );
            git_in(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);

            write_store(&state, "store");
            let _ = run(&state, RunMode::Now, at(DAY)).expect("run");
            assert_eq!(
                fs::read(&victim).expect("victim"),
                b"precious",
                "planted_dir={planted_dir}"
            );
            let stray: Vec<_> = fs::read_dir(&victim_dir)
                .expect("victim dir")
                .flatten()
                .map(|entry| entry.file_name())
                .collect();
            assert_eq!(
                stray.len(),
                1,
                "nothing written beside the victim: {stray:?}"
            );
        }
    }

    #[test]
    fn verbatim_windows_paths_read_as_plain_paths() {
        assert_eq!(
            plain_path(r"\\?\C:\Users\me\remote.git"),
            r"C:\Users\me\remote.git"
        );
        assert_eq!(plain_path(r"\\?\D:"), r"D:");
        assert_eq!(
            plain_path(r"\\?\UNC\server\share\tsk"),
            r"\\server\share\tsk"
        );
        for unchanged in [
            r"\\?\Volume{0a1b}\tsk",
            r"\\?\C:relative",
            r"C:\Users\me",
            r"\\server\share",
            "/Users/me/repo.git",
        ] {
            assert_eq!(plain_path(unchanged), unchanged);
        }
    }

    #[test]
    fn a_git_failure_keeps_the_whole_message() {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: Vec::new(),
            stderr: b"fatal: destination path 'x' already exists\n  and the repository exists.\n\n"
                .to_vec(),
        };
        assert_eq!(
            git_failure("clone", &output),
            "git clone failed: fatal: destination path 'x' already exists / and the repository exists."
        );
    }

    #[test]
    fn env_values_never_leave_the_machine() {
        let root = TempDir::new("secrets");
        let remote = bare_repo(&root.0);
        let state = state_with_target(
            &root.0,
            &remote,
            "\n[agent.claude]\ncommand = [\"claude\"]\n[agent.claude.env]\nAPI_TOKEN = \"s3cret-token\" # mine\n\n[agent.pi]\ncommand = [\"pi\"]\nenv = { OTHER = \"inline-s3cret\" }\n",
        );
        write_store(&state, "x");
        let results = run(&state, RunMode::Now, at(DAY)).expect("run");
        assert!(
            matches!(results[0].1, Ok(RunOutcome::Written(_))),
            "{results:?}"
        );
        let machine = machine_id(&state).expect("machine");
        let pushed = git_in(
            &remote,
            &["show", &format!("HEAD:machines/{machine}/config.toml")],
        );
        assert!(!pushed.contains("s3cret"), "{pushed}");
        assert!(
            pushed.contains("API_TOKEN") && pushed.contains("OTHER"),
            "{pushed}"
        );
        assert!(pushed.contains(ENV_PLACEHOLDER), "{pushed}");
        assert!(pushed.starts_with("# Backed up by tsk."), "{pushed}");
        let parsed = crate::agents::AgentProfiles::parse_for_tests(&pushed);
        assert!(parsed, "the pushed file still parses as profiles");
        assert_eq!(
            sanitized_config("not = [toml"),
            None,
            "a broken file is not pushed"
        );
    }

    #[test]
    fn two_machines_pushing_at_once_both_land() {
        let root = TempDir::new("race");
        let remote = bare_repo(&root.0);
        let states: Vec<PathBuf> = (0..2)
            .map(|n| {
                let dir = root.0.join(format!("machine{n}"));
                fs::create_dir_all(&dir).expect("dir");
                state_with_target(&dir, &remote, "")
            })
            .collect();
        for round in 0..3 {
            let handles: Vec<_> = states
                .iter()
                .cloned()
                .map(|state| {
                    std::thread::spawn(move || {
                        write_store(&state, &format!("round {round}"));
                        run(&state, RunMode::Now, at(DAY + round)).expect("run")
                    })
                })
                .collect();
            for handle in handles {
                let results = handle.join().expect("thread");
                assert!(
                    matches!(results[0].1, Ok(RunOutcome::Written(_))),
                    "{results:?}"
                );
            }
        }
        assert_eq!(
            commits(&remote),
            6,
            "every backup from both machines landed"
        );
    }

    #[test]
    fn a_background_run_never_repacks_the_clone() {
        let root = TempDir::new("no-gc");
        let remote = bare_repo(&root.0);
        let state = state_with_target(&root.0, &remote, "");
        write_store(&state, "first");
        run(&state, RunMode::Now, at(DAY)).expect("run");
        let clone = fs::read_dir(state.join(REPOS_DIR))
            .expect("repos")
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.join(".git").is_dir())
            .expect("a clone");
        // Enough loose objects that git's own fetch or commit would start `gc --auto`.
        let blobs = root.0.join("blobs");
        fs::create_dir_all(&blobs).expect("blobs");
        let mut paths = Vec::new();
        for n in 0..2000 {
            let path = blobs.join(n.to_string());
            fs::write(&path, format!("loose {n}")).expect("blob");
            paths.push(path.display().to_string());
        }
        // Through stdin: 2000 paths overflow a Windows command line.
        let mut hash = Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["hash-object", "-w", "--stdin-paths"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("git hash-object");
        hash.stdin
            .take()
            .expect("stdin")
            .write_all(format!("{}\n", paths.join("\n")).as_bytes())
            .expect("paths");
        assert!(hash.wait().expect("hash-object").success());
        git_in(&clone, &["config", "gc.auto", "10"]);
        git_in(&clone, &["config", "gc.autoDetach", "false"]);
        let packs = || {
            git_in(&clone, &["count-objects", "-v"])
                .lines()
                .find_map(|line| line.strip_prefix("packs: ").map(str::to_string))
                .expect("packs line")
        };
        let before = packs();
        write_store(&state, "second");
        let results = run(&state, RunMode::Due(Trigger::Save), at(2 * DAY)).expect("run");
        assert!(
            matches!(results[0].1, Ok(RunOutcome::Written(_))),
            "{results:?}"
        );
        assert_eq!(packs(), before, "no repack in a background run");
    }
}
