//! Backups of the task store to folders and git repositories, and the versions a restore
//! picks from.
//!
//! A backup is the live `tsk.json` byte for byte, so it carries its own `format_version`.
//! Targets are the `[[backup]]` entries of `config.toml`. Saves and full board opens never
//! wait on a backup: when a target is due they start a detached `tsk backup run` child and
//! return. Each target's last success and last error live in `backup-status.json`.
//!
//! Layout, shared with the cross-machine sync that builds on it: a folder target holds
//! `<folder>/tsk-backups/<machine>/tsk-<UTC time>.json`; a git target holds
//! `machines/<machine>/tsk.json`, one commit per backup, with the repository root left free.

use std::collections::{BTreeMap, BTreeSet};
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
/// This machine's id, generated once.
const MACHINE_ID_FILE: &str = "machine-id";
/// Clones of git targets, one directory per target.
const REPOS_DIR: &str = "backup-repos";
/// Folder targets keep their copies under this directory.
const FOLDER_ROOT: &str = "tsk-backups";
pub(crate) const STATUS_TEMP_PREFIX: &str = ".backup-status.json.tmp.";

pub const DEFAULT_KEEP_LAST: usize = 10;
pub const DEFAULT_KEEP_DAYS: usize = 7;
/// A git target is listed with this many versions per machine unless `--all` asks for more.
pub const GIT_VERSIONS_SHOWN: usize = 20;
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
/// A failed daily or twice-daily backup is retried no sooner than this.
const RETRY_AFTER_SECS: u64 = 10 * 60;
const DAY_SECS: u64 = 24 * 60 * 60;

/// Whether a state-directory entry belongs to backups and holds owner-only content.
pub(crate) fn is_private_state_name(name: &str) -> bool {
    name == STATUS_FILE
        || name == RUN_LOCK_FILE
        || name == PENDING_LOCK_FILE
        || name == MACHINE_ID_FILE
        || name.starts_with(STATUS_TEMP_PREFIX)
}

/// Where a target writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Git,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Git => "git",
        }
    }

    fn location_key(self) -> &'static str {
        match self {
            Self::Folder => "path",
            Self::Git => "url",
        }
    }

    /// The frequency a target of this kind gets when its entry names none.
    pub fn default_every(self) -> Every {
        match self {
            Self::Folder => Every::Save,
            Self::Git => Every::Daily,
        }
    }
}

/// How often a target is backed up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Every {
    Save,
    Open,
    Daily,
    TwiceDaily,
}

impl Every {
    pub const ALL: [Every; 4] = [Every::Save, Every::Open, Every::Daily, Every::TwiceDaily];

    pub fn word(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Open => "open",
            Self::Daily => "daily",
            Self::TwiceDaily => "twice-daily",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|every| every.word() == word)
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
    /// The folder path or the git URL, as written in the entry.
    pub location: String,
    pub every: Every,
    pub keep_last: usize,
    pub keep_days: usize,
}

impl Target {
    pub fn new(kind: Kind, location: String) -> Self {
        Self {
            kind,
            location,
            every: kind.default_every(),
            keep_last: DEFAULT_KEEP_LAST,
            keep_days: DEFAULT_KEEP_DAYS,
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
    let mut targets = Vec::new();
    if let Some(tables) = item.as_array_of_tables() {
        for (index, table) in tables.iter().enumerate() {
            targets.push(parse_target(index, table)?);
        }
    } else if let Some(array) = item.as_array() {
        for (index, value) in array.iter().enumerate() {
            let table = value
                .as_inline_table()
                .ok_or_else(|| format!("config.toml: backup {} must be a table", index + 1))?;
            targets.push(parse_target(index, table)?);
        }
    } else {
        return Err("config.toml: backup must be a list of [[backup]] tables".into());
    }
    Ok(targets)
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
    let kind = match string("kind")?.as_deref() {
        Some("folder") => Kind::Folder,
        Some("git") => Kind::Git,
        Some(other) => return Err(invalid(format!("kind {other:?} is not folder or git"))),
        None => return Err(invalid("kind is required (folder or git)".into())),
    };
    let allowed: &[&str] = match kind {
        Kind::Folder => &["kind", "path", "every", "keep_last", "keep_days"],
        Kind::Git => &["kind", "url", "every"],
    };
    if let Some(key) = table
        .iter()
        .map(|(key, _)| key)
        .find(|key| !allowed.contains(key))
    {
        return Err(invalid(format!(
            "unknown key {key:?} for a {} target",
            kind.word()
        )));
    }
    let location = string(kind.location_key())?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(format!("{} is required", kind.location_key())))?;
    let mut target = Target::new(kind, location);
    if let Some(word) = string("every")? {
        target.every = Every::parse(&word).ok_or_else(|| {
            invalid(format!(
                "every {word:?} is not save, open, daily, or twice-daily"
            ))
        })?;
    }
    let count = |key: &str, minimum: i64| -> Result<Option<usize>, String> {
        match table.get(key) {
            None => Ok(None),
            Some(item) => item
                .as_integer()
                .filter(|value| *value >= minimum)
                .and_then(|value| usize::try_from(value).ok())
                .map(Some)
                .ok_or_else(|| {
                    invalid(format!(
                        "{key} must be a whole number of at least {minimum}"
                    ))
                }),
        }
    };
    if let Some(keep_last) = count("keep_last", 1)? {
        target.keep_last = keep_last;
    }
    if let Some(keep_days) = count("keep_days", 0)? {
        target.keep_days = keep_days;
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
    kind: Kind,
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
        .position(|target| target.kind == kind && target.location == location)
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
    let mut target = Target::new(kind, location.to_string());
    if let Some(every) = every {
        target.every = every;
    }
    let mut table = toml_edit::Table::new();
    table["kind"] = toml_edit::value(kind.word());
    table[kind.location_key()] = toml_edit::value(location);
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

/// Remove the `[[backup]]` entry `selector` names: its folder path or URL as `tsk backup list`
/// shows it, or its number there. Its status is forgotten; copies already written stay.
pub fn remove_target(state_dir: &Path, selector: &str) -> Result<Target, String> {
    let targets = load_targets(state_dir)?;
    let index = selector
        .parse::<usize>()
        .ok()
        .filter(|number| (1..=targets.len()).contains(number))
        .map(|number| number - 1)
        .or_else(|| {
            let absolute = absolute_path(selector);
            targets.iter().position(|target| {
                target.location == selector
                    || absolute
                        .as_deref()
                        .is_some_and(|path| target.location == path)
            })
        })
        .ok_or_else(|| format!("no backup target {selector}"))?;
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
    let _ = with_run_lock(state_dir, true, || {
        let mut status = load_status(state_dir);
        status.targets.remove(&removed.key());
        save_status(state_dir, &status)
    });
    Ok(removed)
}

fn write_document(state_dir: &Path, document: &DocumentMut) -> Result<(), String> {
    crate::agents::write_config(state_dir, &document.to_string())
        .map_err(|error| format!("could not write config.toml: {error}"))
}

/// `path` made absolute against the working directory, when it names an existing directory.
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
    absolute.to_str().map(str::to_string)
}

/// Whether `location` reads as a git remote rather than a folder: a URL, scp-style
/// `user@host:path`, a path ending in `.git`, or a directory holding a bare repository.
pub fn looks_like_git(location: &str) -> bool {
    if location.contains("://") || location.ends_with(".git") || location.ends_with(".git/") {
        return true;
    }
    if let Some((user_host, _)) = location.split_once(':') {
        if user_host.contains('@') && !user_host.contains('/') {
            return true;
        }
    }
    let path = Path::new(location);
    path.join("HEAD").is_file() && path.join("objects").is_dir() && path.join("refs").is_dir()
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
    /// The board already showed `last_error` once.
    #[serde(default)]
    pub error_shown: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StatusFile {
    #[serde(default)]
    targets: BTreeMap<String, TargetStatus>,
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

/// The first backup failure the board has not shown yet, marked shown. `None` while a run
/// holds the lock: the board never waits for one.
pub fn take_unshown_error(state_dir: &Path) -> Option<String> {
    // Most boards have no targets: answer without creating lock files.
    if !state_dir.join(STATUS_FILE).exists() {
        return None;
    }
    with_run_lock(state_dir, false, || {
        let targets = load_targets(state_dir).unwrap_or_default();
        let mut status = load_status(state_dir);
        let mut message = None;
        for target in &targets {
            if let Some(record) = status.targets.get_mut(&target.key()) {
                if let (Some(error), false) = (&record.last_error, record.error_shown) {
                    record.error_shown = true;
                    message.get_or_insert_with(|| {
                        format!("backup failed: {} (tsk backup list)", first_line(error))
                    });
                }
            }
        }
        if message.is_some() {
            save_status(state_dir, &status)?;
        }
        Ok(message)
    })
    .ok()
    .flatten()
}

/// Whether `target` should back up now. `save` and `open` targets follow their trigger;
/// `daily` and `twice-daily` are due on either trigger once 24 or 12 hours have passed since
/// their last success, and a failed attempt is retried after ten minutes.
pub fn is_due(every: Every, trigger: Trigger, status: &TargetStatus, now: u64) -> bool {
    let interval = match every {
        Every::Save => return trigger == Trigger::Save,
        Every::Open => return trigger == Trigger::Open,
        Every::Daily => DAY_SECS,
        Every::TwiceDaily => DAY_SECS / 2,
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

/// The copies a folder target keeps: the `keep_last` newest, plus the newest of each of the
/// `keep_days` UTC days ending today. `stamps` are Unix seconds.
pub fn retention_keep(
    stamps: &[u64],
    now: u64,
    keep_last: usize,
    keep_days: usize,
) -> BTreeSet<u64> {
    let mut newest_first = stamps.to_vec();
    newest_first.sort_unstable_by(|a, b| b.cmp(a));
    newest_first.dedup();
    let mut keep: BTreeSet<u64> = newest_first.iter().take(keep_last).copied().collect();
    let today = now / DAY_SECS;
    for back in 0..keep_days as u64 {
        let Some(day) = today.checked_sub(back) else {
            break;
        };
        if let Some(stamp) = newest_first.iter().find(|stamp| **stamp / DAY_SECS == day) {
            keep.insert(*stamp);
        }
    }
    keep
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
    if any_due(state_dir, trigger, unix_secs(SystemTime::now())) {
        let _ = spawn_run(state_dir, trigger);
    }
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
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .args(["backup", "run", "--on", trigger.word(), "--state-dir"])
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

/// Which targets a run backs up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Every target, now (`tsk backup now`).
    Now,
    /// Only due targets whose copy is out of date (the background run).
    Due(Trigger),
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
    /// There is no `tsk.json` yet.
    NoStore,
}

/// Back the live store up to the targets `mode` selects. Runs one at a time per state
/// directory; a background run that finds another already waiting leaves the work to it.
pub fn run(state_dir: &Path, mode: RunMode, now: SystemTime) -> Result<Vec<TargetRun>, String> {
    let targets = load_targets(state_dir)?;
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    fsperm::ensure_private_dir(state_dir).map_err(|error| error.to_string())?;
    let _pending = if matches!(mode, RunMode::Due(_)) {
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

    let bytes = match fs::read(state_dir.join("tsk.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(targets
                .into_iter()
                .map(|target| (target, Ok(RunOutcome::NoStore)))
                .collect())
        }
        Err(error) => return Err(format!("could not read tsk.json: {error}")),
    };
    let hash = format!("{:016x}", fnv1a(&bytes));
    let machine = machine_name(state_dir).map_err(|error| error.to_string())?;
    let mut status = load_status(state_dir);
    let now_secs = unix_secs(now);
    let mut results = Vec::new();
    for target in targets {
        let record = status.targets.entry(target.key()).or_default();
        if let RunMode::Due(trigger) = mode {
            if !is_due(target.every, trigger, record, now_secs) {
                results.push((target, Ok(RunOutcome::NotDue)));
                continue;
            }
            if record.last_hash.as_deref() == Some(hash.as_str()) && record.last_error.is_none() {
                record.last_success = Some(now_secs);
                results.push((target, Ok(RunOutcome::UpToDate)));
                continue;
            }
        }
        record.last_attempt = Some(now_secs);
        let outcome = match target.kind {
            Kind::Folder => backup_to_folder(&target, &machine, &bytes, now),
            Kind::Git => backup_to_git(state_dir, &target, &machine, &bytes, now),
        };
        match &outcome {
            Ok(_) => {
                record.last_success = Some(now_secs);
                record.last_error = None;
                record.last_hash = Some(hash.clone());
                record.error_shown = false;
            }
            Err(error) => {
                record.last_error = Some(error.clone());
                record.error_shown = false;
            }
        }
        results.push((target, outcome));
    }
    save_status(state_dir, &status)
        .map_err(|error| format!("could not record backup status: {error}"))?;
    Ok(results)
}

fn backup_to_folder(
    target: &Target,
    machine: &str,
    bytes: &[u8],
    now: SystemTime,
) -> Result<RunOutcome, String> {
    let root = Path::new(&target.location);
    if !root.is_dir() {
        return Err(format!("folder {} not found", target.location));
    }
    let dir = root.join(FOLDER_ROOT).join(machine);
    fs::create_dir_all(&dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    let name = format!("tsk-{}.json", utc_stamp(now));
    let path = dir.join(&name);
    // Written whole beside the target, then renamed, so a sync client never uploads half a copy.
    let tmp = dir.join(format!(
        ".tsk-backup.tmp.{}.{}",
        std::process::id(),
        now_nanos()
    ));
    let result = (|| -> io::Result<()> {
        let mut file = fsperm::create_private_file(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&tmp);
        return Err(format!("could not write {}: {error}", path.display()));
    }
    prune_folder(&dir, unix_secs(now), target.keep_last, target.keep_days);
    Ok(RunOutcome::Written(path.display().to_string()))
}

/// Delete the copies [`retention_keep`] does not keep. Other files are left alone.
fn prune_folder(dir: &Path, now: u64, keep_last: usize, keep_days: usize) {
    let copies = folder_copies(dir);
    let stamps: Vec<u64> = copies.iter().map(|(stamp, _)| *stamp).collect();
    let keep = retention_keep(&stamps, now, keep_last, keep_days);
    for (stamp, path) in copies {
        if !keep.contains(&stamp) {
            let _ = fs::remove_file(path);
        }
    }
}

/// `tsk-<stamp>.json` copies in one machine directory with their times.
fn folder_copies(dir: &Path) -> Vec<(u64, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let stamp = name
                .to_str()?
                .strip_prefix("tsk-")?
                .strip_suffix(".json")
                .and_then(parse_utc_stamp)?;
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_file())
                .map(|_| (stamp, entry.path()))
        })
        .collect()
}

fn backup_to_git(
    state_dir: &Path,
    target: &Target,
    machine: &str,
    bytes: &[u8],
    now: SystemTime,
) -> Result<RunOutcome, String> {
    let clone = sync_clone(state_dir, target)?;
    let relative = format!("machines/{machine}/tsk.json");
    let mut last_error = String::new();
    for attempt in 0..2 {
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
        let file = clone.dir.join(&relative);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&file, bytes).map_err(|error| error.to_string())?;
        git_ok(&clone.dir, &["add", "--", &relative])?;
        let staged = git(
            &clone.dir,
            &["diff", "--cached", "--quiet", "--", &relative],
        )?;
        if !staged.status.success() {
            let message = format!(
                "tsk backup from {machine} at {}",
                human_time(unix_secs(now))
            );
            git_ok(
                &clone.dir,
                &[
                    "-c",
                    &format!("user.name=tsk ({machine})"),
                    "-c",
                    "user.email=tsk@localhost",
                    "-c",
                    "commit.gpgsign=false",
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
                target.location
            )));
        }
        last_error = git_failure("push", &push);
    }
    Err(last_error)
}

/// A git target's local clone, fetched, and the branch backups go to.
struct GitClone {
    dir: PathBuf,
    branch: String,
}

/// Clone the target under `backup-repos/<id>` the first time, fetch it after. The branch is
/// the remote's default, or `main` for an empty repository.
fn sync_clone(state_dir: &Path, target: &Target) -> Result<GitClone, String> {
    let repos = state_dir.join(REPOS_DIR);
    fsperm::ensure_private_dir(&repos).map_err(|error| error.to_string())?;
    let dir = repos.join(target.id());
    if dir.join(".git").is_dir() {
        git_ok(&dir, &["remote", "set-url", "origin", &target.location])?;
        git_ok(&dir, &["fetch", "--quiet", "--prune", "origin"])?;
    } else {
        let _ = fs::remove_dir_all(&dir);
        let mut command = git_command(&repos);
        command
            .args(["clone", "--quiet", "--no-checkout", &target.location])
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
    .unwrap_or_else(|| "main".to_string());
    Ok(GitClone { dir, branch })
}

fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("no error output");
    format!("git {verb} failed: {detail}")
}

/// One restorable copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// Short id `tsk restore <id>` takes.
    pub id: String,
    /// Unix seconds the copy was made.
    pub at: u64,
    pub machine: String,
    /// The target's folder path or URL.
    pub target: String,
    /// Tasks in the copy (notices and deleted tasks excluded); `None` when unreadable.
    pub tasks: Option<usize>,
    source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    File(PathBuf),
    Git {
        dir: PathBuf,
        commit: String,
        path: String,
    },
}

/// Every version on every target, newest first, plus a failure per target that could not be
/// read. A git target lists [`GIT_VERSIONS_SHOWN`] versions per machine unless `all`.
pub fn list_versions(state_dir: &Path, all: bool) -> Result<Listing, String> {
    let targets = load_targets(state_dir)?;
    let mut versions = Vec::new();
    let mut failures = Vec::new();
    for target in targets {
        let listed = match target.kind {
            Kind::Folder => folder_versions(&target),
            Kind::Git => git_versions(state_dir, &target, all),
        };
        match listed {
            Ok(found) => versions.extend(found),
            Err(error) => failures.push((target, error)),
        }
    }
    versions.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
    Ok((versions, failures))
}

fn folder_versions(target: &Target) -> Result<Vec<Version>, String> {
    let root = Path::new(&target.location);
    if !root.is_dir() {
        return Err(format!("folder {} not found", target.location));
    }
    let Ok(machines) = fs::read_dir(root.join(FOLDER_ROOT)) else {
        return Ok(Vec::new());
    };
    let mut versions = Vec::new();
    for machine in machines.flatten() {
        if !machine.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let machine_name = machine.file_name().to_string_lossy().into_owned();
        for (at, path) in folder_copies(&machine.path()) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            versions.push(Version {
                id: version_id(target, &machine_name, name.as_deref().unwrap_or_default()),
                at,
                machine: machine_name.clone(),
                target: target.location.clone(),
                tasks: fs::read(&path).ok().and_then(|bytes| count_tasks(&bytes)),
                source: Source::File(path),
            });
        }
    }
    Ok(versions)
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
    let log = git_ok(
        &clone.dir,
        &[
            "log",
            "--format=%x01%H %ct",
            "--name-only",
            &remote,
            "--",
            "machines",
        ],
    )?;
    let mut per_machine: BTreeMap<String, usize> = BTreeMap::new();
    let mut versions = Vec::new();
    for record in log
        .split('\u{1}')
        .filter(|record| !record.trim().is_empty())
    {
        let mut lines = record.lines();
        let Some((commit, time)) = lines.next().and_then(|line| line.split_once(' ')) else {
            continue;
        };
        let Ok(at) = time.trim().parse::<u64>() else {
            continue;
        };
        for path in lines.map(str::trim) {
            let Some(machine) = path
                .strip_prefix("machines/")
                .and_then(|rest| rest.strip_suffix("/tsk.json"))
                .filter(|machine| !machine.contains('/'))
            else {
                continue;
            };
            let shown = per_machine.entry(machine.to_string()).or_default();
            if !all && *shown >= GIT_VERSIONS_SHOWN {
                continue;
            }
            *shown += 1;
            let source = Source::Git {
                dir: clone.dir.clone(),
                commit: commit.to_string(),
                path: path.to_string(),
            };
            versions.push(Version {
                id: version_id(target, machine, commit),
                at,
                machine: machine.to_string(),
                target: target.location.clone(),
                tasks: read_source(&source)
                    .ok()
                    .and_then(|bytes| count_tasks(&bytes)),
                source,
            });
        }
    }
    Ok(versions)
}

/// The bytes of one listed version.
pub fn read_version(version: &Version) -> Result<Vec<u8>, String> {
    read_source(&version.source)
}

fn read_source(source: &Source) -> Result<Vec<u8>, String> {
    match source {
        Source::File(path) => {
            fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))
        }
        Source::Git { dir, commit, path } => {
            let output = git(dir, &["show", &format!("{commit}:{path}")])?;
            if output.status.success() {
                Ok(output.stdout)
            } else {
                Err(git_failure("show", &output))
            }
        }
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

/// `<host>-<first 8 of the machine id>`, the directory a machine's backups go under.
pub fn machine_name(state_dir: &Path) -> io::Result<String> {
    let id = machine_id(state_dir)?;
    Ok(format!("{}-{}", hostname(), &id[..8]))
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
    (id.len() >= 8 && id.chars().all(|c| c.is_ascii_alphanumeric())).then_some(id)
}

/// The host name's first label, lowercased, as letters, digits, and hyphens.
fn hostname() -> String {
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

/// Days since the Unix epoch from a civil date, the inverse of [`civil_from_days`].
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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

/// Unix seconds from a [`utc_stamp`].
pub fn parse_utc_stamp(stamp: &str) -> Option<u64> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 16 || bytes[8] != b'T' || bytes[15] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(4..6)?, number(6..8)?);
    let (hour, minute, second) = (number(9..11)?, number(11..13)?, number(13..15)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
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

    #[test]
    fn stamps_round_trip_and_sort_by_time() {
        for secs in [
            0,
            1_792_135_428,
            951_782_400, /* 2000-02-29 */
            4_102_444_799,
        ] {
            let stamp = utc_stamp(at(secs));
            assert_eq!(parse_utc_stamp(&stamp), Some(secs), "{stamp}");
        }
        assert_eq!(utc_stamp(at(951_782_400)), "20000229T000000Z");
        assert_eq!(human_time(1_792_135_428), "2026-10-16 07:23 UTC");
        assert!(utc_stamp(at(100)) < utc_stamp(at(1_000_000)));
        assert_eq!(parse_utc_stamp("20261301T000000Z"), None);
        assert_eq!(parse_utc_stamp("garbage"), None);
    }

    #[test]
    fn retention_keeps_the_newest_ten_plus_one_per_day_for_seven_days() {
        let now = 100 * DAY + 12 * HOUR;
        // Twelve copies an hour apart today, then two a day for the ten days before.
        let mut stamps: Vec<u64> = (0..12).map(|hour| now - hour * HOUR).collect();
        for day in 1..=10 {
            stamps.push(now - day * DAY);
            stamps.push(now - day * DAY - HOUR);
        }
        let keep = retention_keep(&stamps, now, 10, 7);
        let newest_ten: Vec<u64> = (0..10).map(|hour| now - hour * HOUR).collect();
        for stamp in &newest_ten {
            assert!(keep.contains(stamp));
        }
        // Today's newest is already among the ten; days 1..=6 each keep their newest only.
        for day in 1..=6 {
            assert!(keep.contains(&(now - day * DAY)), "day {day}");
            assert!(
                !keep.contains(&(now - day * DAY - HOUR)),
                "day {day} older copy"
            );
        }
        for day in 7..=10 {
            assert!(
                !keep.contains(&(now - day * DAY)),
                "day {day} is past the window"
            );
        }
        assert_eq!(keep.len(), 10 + 6);
    }

    #[test]
    fn retention_honours_custom_numbers() {
        let now = 50 * DAY;
        let stamps: Vec<u64> = (0..20).map(|n| now - n * DAY).collect();
        let keep = retention_keep(&stamps, now, 2, 3);
        assert_eq!(
            keep,
            [now, now - DAY, now - 2 * DAY]
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
        let keep = retention_keep(&stamps, now, 5, 0);
        assert_eq!(keep.len(), 5);
        assert!(keep.contains(&now) && keep.contains(&(now - 4 * DAY)));
    }

    #[test]
    fn due_follows_each_frequency() {
        let never = TargetStatus::default();
        let now = 10 * DAY;
        assert!(is_due(Every::Save, Trigger::Save, &never, now));
        assert!(!is_due(Every::Save, Trigger::Open, &never, now));
        assert!(is_due(Every::Open, Trigger::Open, &never, now));
        assert!(!is_due(Every::Open, Trigger::Save, &never, now));
        for trigger in [Trigger::Save, Trigger::Open] {
            assert!(
                is_due(Every::Daily, trigger, &never, now),
                "never backed up"
            );
            assert!(is_due(Every::TwiceDaily, trigger, &never, now));
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
        assert!(!is_due(
            Every::TwiceDaily,
            Trigger::Open,
            &recent(11 * HOUR),
            now
        ));
        assert!(is_due(
            Every::TwiceDaily,
            Trigger::Open,
            &recent(12 * HOUR),
            now
        ));
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
kind = "folder"
path = "/backups"

[[backup]]
kind = "folder"
path = "/other"
every = "daily"
keep_last = 3
keep_days = 0

[[backup]]
kind = "git"
url = "git@example.com:me/tsk.git"
"#,
        )
        .expect("parses");
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].every, Every::Save);
        assert_eq!((targets[0].keep_last, targets[0].keep_days), (10, 7));
        assert_eq!(targets[1].every, Every::Daily);
        assert_eq!((targets[1].keep_last, targets[1].keep_days), (3, 0));
        assert_eq!(
            (targets[2].kind, targets[2].every),
            (Kind::Git, Every::Daily)
        );
        let inline =
            parse_targets(r#"backup = [{ kind = "folder", path = "/x", every = "open" }]"#)
                .expect("inline list parses");
        assert_eq!(inline[0].every, Every::Open);
        for (bad, why) in [
            ("[[backup]]\npath = \"/x\"", "kind is required"),
            (
                "[[backup]]\nkind = \"s3\"\npath = \"/x\"",
                "not folder or git",
            ),
            ("[[backup]]\nkind = \"folder\"", "path is required"),
            (
                "[[backup]]\nkind = \"git\"\nurl = \"u\"\nkeep_last = 2",
                "unknown key \"keep_last\"",
            ),
            (
                "[[backup]]\nkind = \"folder\"\npath = \"/x\"\nevery = \"hourly\"",
                "not save, open",
            ),
            (
                "[[backup]]\nkind = \"folder\"\npath = \"/x\"\nkeep_last = 0",
                "at least 1",
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
        assert_eq!(
            (targets[0].kind, targets[0].every),
            (Kind::Folder, Every::Save)
        );
    }

    #[test]
    fn other_config_readers_ignore_backup_tables() {
        let content =
            "[[backup]]\nkind = \"folder\"\npath = \"/x\"\n\n[agent.a]\ncommand = [\"a\"]\n";
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
        let (target, outcome) =
            add_target(&dir.0, Kind::Folder, "/backups", None).expect("add folder");
        assert_eq!((outcome, target.every), (AddOutcome::Added, Every::Save));
        let (_, outcome) = add_target(&dir.0, Kind::Git, "/r.git", Some(Every::Open)).expect("git");
        assert_eq!(outcome, AddOutcome::Added);
        let (_, again) = add_target(&dir.0, Kind::Folder, "/backups", None).expect("again");
        assert_eq!(again, AddOutcome::Unchanged);
        let (updated, outcome) =
            add_target(&dir.0, Kind::Folder, "/backups", Some(Every::Daily)).expect("update");
        assert_eq!(
            (outcome, updated.every),
            (AddOutcome::Updated, Every::Daily)
        );
        let content = fs::read_to_string(dir.0.join("config.toml")).expect("read");
        assert!(content.starts_with(original), "{content}");
        let targets = load_targets(&dir.0).expect("load");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].every, Every::Daily);

        assert_eq!(
            remove_target(&dir.0, "2").expect("by number").location,
            "/r.git"
        );
        assert_eq!(
            remove_target(&dir.0, "/backups").expect("by path").location,
            "/backups"
        );
        assert!(remove_target(&dir.0, "/backups").is_err());
        assert_eq!(
            fs::read_to_string(dir.0.join("config.toml")).expect("read"),
            original,
            "the last remove drops the empty table"
        );
    }

    #[test]
    fn add_creates_config_toml_in_a_fresh_state_dir() {
        let dir = TempDir::new("fresh");
        add_target(&dir.0, Kind::Git, "file:///r.git", None).expect("add");
        assert_eq!(load_targets(&dir.0).expect("load")[0].every, Every::Daily);
    }

    #[test]
    fn git_locations_are_recognized() {
        for git in [
            "https://github.com/me/tsk-backup",
            "file:///tmp/r",
            "git@github.com:me/tsk-backup.git",
            "me@host:backups",
            "/tmp/r.git",
        ] {
            assert!(looks_like_git(git), "{git}");
        }
        for folder in [
            "/Users/me/Library/Mobile Documents",
            "C:\\Users\\me\\Dropbox",
            "backups",
        ] {
            assert!(!looks_like_git(folder), "{folder}");
        }
    }

    #[test]
    fn machine_id_is_generated_once() {
        let dir = TempDir::new("machine");
        let first = machine_id(&dir.0).expect("id");
        assert_eq!(machine_id(&dir.0).expect("again"), first);
        let name = machine_name(&dir.0).expect("name");
        assert!(name.ends_with(&first[..8]), "{name}");
        assert!(name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }

    #[test]
    fn counts_tasks_without_notices_or_deleted() {
        let document = br#"{"format_version":13,"tasks":[{"title":"a"},{"title":"n","notice":{"kind":"x"}},{"title":"d","soft_deleted":true},{"title":"b","notice":null}]}"#;
        assert_eq!(count_tasks(document), Some(2));
        assert_eq!(count_tasks(b"nope"), None);
    }

    fn write_store(dir: &Path, body: &str) {
        fs::write(dir.join("tsk.json"), body).expect("store");
    }

    #[test]
    fn folder_run_writes_prunes_and_skips_unchanged_bytes() {
        let state = TempDir::new("run-state");
        let folder = TempDir::new("run-folder");
        fs::write(
            state.0.join("config.toml"),
            format!(
                "[[backup]]\nkind = \"folder\"\npath = {:?}\nkeep_last = 2\nkeep_days = 0\n",
                folder.0.display().to_string()
            ),
        )
        .expect("config");
        let now = 400 * DAY;
        assert_eq!(
            run(&state.0, RunMode::Due(Trigger::Save), at(now)).expect("run")[0].1,
            Ok(RunOutcome::NoStore)
        );
        for (index, body) in ["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"].iter().enumerate() {
            write_store(&state.0, body);
            let results = run(
                &state.0,
                RunMode::Due(Trigger::Save),
                at(now + index as u64),
            )
            .expect("run");
            assert!(
                matches!(results[0].1, Ok(RunOutcome::Written(_))),
                "{results:?}"
            );
        }
        let results = run(&state.0, RunMode::Due(Trigger::Save), at(now + 10)).expect("run");
        assert_eq!(
            results[0].1,
            Ok(RunOutcome::UpToDate),
            "same bytes are not copied twice"
        );
        let results = run(&state.0, RunMode::Due(Trigger::Open), at(now + 11)).expect("run");
        assert_eq!(results[0].1, Ok(RunOutcome::NotDue));
        let machine = machine_name(&state.0).expect("machine");
        let copies = folder_copies(&folder.0.join(FOLDER_ROOT).join(&machine));
        assert_eq!(copies.len(), 2, "keep_last = 2 pruned the oldest");
        let (versions, failures) = list_versions(&state.0, false).expect("versions");
        assert!(failures.is_empty());
        assert_eq!(versions.len(), 2);
        assert_eq!(read_version(&versions[0]).expect("read"), b"{\"a\":3}");
        assert_eq!(versions[0].machine, machine);
    }

    #[test]
    fn a_failing_target_records_its_error_once_for_the_board() {
        let state = TempDir::new("fail-state");
        fs::write(
            state.0.join("config.toml"),
            "[[backup]]\nkind = \"folder\"\npath = \"/nonexistent/tsk-backup-target\"\n",
        )
        .expect("config");
        write_store(&state.0, "{}");
        let results = run(&state.0, RunMode::Now, at(DAY)).expect("run");
        assert!(results[0]
            .1
            .as_ref()
            .is_err_and(|e| e.contains("not found")));
        let listed = statuses(&state.0).expect("statuses");
        assert!(listed[0].1.last_error.is_some());
        let message = take_unshown_error(&state.0).expect("first open shows it");
        assert!(
            message.starts_with("backup failed: folder /nonexistent/tsk-backup-target"),
            "{message}"
        );
        assert_eq!(take_unshown_error(&state.0), None, "shown once");
    }
}
