//! Branch-only dispatch bases, resolved in the task repository, never the caller's checkout.
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBase {
    pub reference: String,
    /// Exact branch namespace, independent of later remote configuration changes.
    pub full_ref: Option<String>,
    pub commit: Option<String>,
    pub remote: Option<String>,
    pub warning: Option<String>,
}

/// Only exact local or remote branch names are accepted, not revisions or tags.
pub fn validate_branch(project: &Path, base: &str) -> Result<(), String> {
    branch_ref(project, base).map(|_| ())
}

/// CLI validation may refresh an explicitly named remote branch. Board saves use
/// `validate_branch` instead and never perform network I/O.
pub fn validate_branch_fresh(project: &Path, base: &str) -> Result<(), String> {
    let _ = refresh_explicit_remote(project, base);
    validate_branch(project, base)
}

fn refresh_explicit_remote(project: &Path, base: &str) -> Option<(String, Result<(), String>)> {
    if base.is_empty() || base.starts_with('-') || base == "HEAD" || base.ends_with("/HEAD") {
        return None;
    }
    // Exact local branches retain precedence even when their names look remote.
    if git_status(
        project,
        &["show-ref", "--verify", &format!("refs/heads/{base}")],
    )
    .is_ok_and(|status| status.success())
    {
        return None;
    }
    let remote = remote_for_ref(project, base)?;
    // The fetch window only covers refs already on disk: a branch named before tsk has
    // seen it (pushed seconds ago) always gets a real fetch.
    let result = if branch_ref(project, base).is_ok() {
        fetch_remote(project, &remote)
    } else {
        fetch_remote_now(project, &remote)
    };
    Some((remote, result))
}

const LOCAL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);
const NETWORK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A file rather than a pipe: a verbose Git process cannot block while we poll its
/// deadline, nor can a descendant holding stdout open stall a reader-thread join.
struct Capture {
    // Fields drop in declaration order: close the file before removing it on Windows.
    file: std::fs::File,
    _path: CapturePath,
}
struct CapturePath(std::path::PathBuf);
impl Drop for CapturePath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
impl Capture {
    fn new() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("tsk-git-{}", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(|error| error.to_string())?;
        Ok(Self {
            file,
            _path: CapturePath(path),
        })
    }
    fn read(&mut self) -> Result<Vec<u8>, String> {
        use std::io::{Read, Seek, SeekFrom};
        const LIMIT: u64 = 8 * 1024 * 1024;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        self.file
            .by_ref()
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > LIMIT {
            return Err("git output exceeded capture limit".into());
        }
        Ok(bytes)
    }
}
fn stop_git(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: our child was spawned into its own process group. This also stops
    // Git's SSH/credential children, never processes in the caller's group.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_git(
    project: &Path,
    args: &[&str],
    timeout: std::time::Duration,
    capture: bool,
) -> Result<std::process::Output, String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + timeout;
    let mut stdout = capture.then(Capture::new).transpose()?;
    let mut stderr = capture.then(Capture::new).transpose()?;
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(project)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null());
    for (stream, is_stdout) in [(&stdout, true), (&stderr, false)] {
        let io = match stream {
            Some(output) => {
                Stdio::from(output.file.try_clone().map_err(|error| error.to_string())?)
            }
            None => Stdio::null(),
        };
        if is_stdout {
            command.stdout(io);
        } else {
            command.stderr(io);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run git: {error}"))?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Err(error) => {
                stop_git(&mut child);
                return Err(format!("could not wait for git: {error}"));
            }
            Ok(None) if Instant::now() >= deadline => {
                stop_git(&mut child);
                return Err("git timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    Ok(std::process::Output {
        status,
        stdout: stdout
            .as_mut()
            .map(Capture::read)
            .transpose()?
            .unwrap_or_default(),
        stderr: stderr
            .as_mut()
            .map(Capture::read)
            .transpose()?
            .unwrap_or_default(),
    })
}

/// Bounded, noninteractive local query, retaining output and nonzero status.
pub fn git_process_output(project: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    git_process_output_timeout(project, args, LOCAL_TIMEOUT)
}

/// Captured Git work with a caller-selected finite deadline. Worktree operations
/// can use a longer deadline than metadata queries without reverting to unbounded I/O.
pub fn git_process_output_timeout(
    project: &Path,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    run_git(project, args, timeout, true)
}

/// Bounded, noninteractive local query with deadlock-free output capture.
pub fn git_output(project: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_process_output(project, args)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_string())
        .map_err(|error| format!("git returned invalid text: {error}"))
}

/// Bounded local status query, retaining nonzero status for ancestry/ref checks.
pub fn git_status(project: &Path, args: &[&str]) -> Result<std::process::ExitStatus, String> {
    run_git(project, args, LOCAL_TIMEOUT, false).map(|output| output.status)
}

fn branch_ref(project: &Path, base: &str) -> Result<String, String> {
    if base.is_empty() || base.starts_with('-') || base == "HEAD" || base.ends_with("/HEAD") {
        return Err(format!("unknown base branch {base}"));
    }
    for namespace in ["refs/heads/", "refs/remotes/"] {
        let reference = format!("{namespace}{base}");
        if git_status(project, &["check-ref-format", &reference])
            .is_ok_and(|status| status.success())
            && git_status(project, &["show-ref", "--verify", &reference])
                .is_ok_and(|status| status.success())
        {
            return Ok(reference);
        }
    }
    Err(format!("unknown base branch {base}"))
}

/// Local metadata only, safe to call from a background footer cache without a fetch.
pub fn default_branch_name(project: &Path) -> Option<String> {
    default_ref(project).ok().and_then(|reference| {
        reference
            .strip_prefix("refs/remotes/origin/")
            .map(str::to_string)
    })
}

fn default_ref(project: &Path) -> Result<String, String> {
    git_output(
        project,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    )
    .map_err(|_| {
        "repo has no origin default branch; set origin/HEAD or choose an explicit base".into()
    })
}

/// The network path is bounded and cannot prompt in an unattended dispatch or picker.
fn bounded_git(project: &Path, args: &[&str]) -> Result<(), String> {
    let output = run_git(project, args, NETWORK_TIMEOUT, false).map_err(|reason| {
        if reason == "git timed out" {
            "fetch timed out".into()
        } else {
            reason
        }
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err("fetch failed (offline or unavailable remote)".into())
    }
}

/// A remote tsk fetched successfully this recently is not fetched again: dispatch, the
/// branch picker and cleanup share one window, so back-to-back surfaces pay one round trip.
pub const FETCH_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// Picker row while a background fetch fails: the list stays on the cached refs.
pub const OFFLINE_BRANCHES: &str = "offline, showing cached branches";

const FETCH_STAMPS_FILE: &str = "fetch-stamps.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchOutcome {
    Fetched,
    /// Inside the fetch window, or joined a fetch that finished while this caller waited.
    Reused,
}

#[derive(Debug, Default)]
struct FetchSlot {
    last_success: Option<std::time::SystemTime>,
    last_attempt: Option<(std::time::Instant, Result<(), String>)>,
}

type FetchKey = (std::path::PathBuf, String);

/// One slot per repository and remote. Holding the slot's lock for the whole fetch is what
/// serialises callers: a second surface waits for the running fetch and takes its result.
static FETCHES: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::HashMap<FetchKey, std::sync::Arc<std::sync::Mutex<FetchSlot>>>,
    >,
> = std::sync::LazyLock::new(Default::default);

/// State dir for the cross-process stamp file. Only the binary's entry points set it, so
/// in-process tests never write beside a real store.
static FETCH_STAMP_DIR: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// Share the fetch window with later `tsk` processes through `fetch-stamps.json` in `dir`.
pub fn remember_fetches_in(dir: &Path) {
    if let Ok(mut slot) = FETCH_STAMP_DIR.lock() {
        *slot = Some(dir.to_path_buf());
    }
}

fn fetch_key(project: &Path, remote: &str) -> FetchKey {
    (
        project
            .canonicalize()
            .unwrap_or_else(|_| project.to_path_buf()),
        remote.to_string(),
    )
}

fn fetch_slot(key: &FetchKey) -> std::sync::Arc<std::sync::Mutex<FetchSlot>> {
    let mut slots = FETCHES.lock().unwrap_or_else(|poison| poison.into_inner());
    std::sync::Arc::clone(slots.entry(key.clone()).or_default())
}

fn within_window(stamp: std::time::SystemTime, now: std::time::SystemTime) -> bool {
    // A stamp from the future (clock moved back) is not trusted.
    now.duration_since(stamp)
        .is_ok_and(|age| age < FETCH_WINDOW)
}

fn stamp_name((project, remote): &FetchKey) -> String {
    format!("{}\n{remote}", project.display())
}

fn read_stamps(dir: &Path) -> std::collections::BTreeMap<String, u64> {
    std::fs::read_to_string(dir.join(FETCH_STAMPS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn unix_secs(time: std::time::SystemTime) -> u64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|age| age.as_secs())
        .unwrap_or(0)
}

fn stamp_fresh(dir: &Path, key: &FetchKey, now: std::time::SystemTime) -> bool {
    read_stamps(dir).get(&stamp_name(key)).is_some_and(|secs| {
        within_window(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(*secs),
            now,
        )
    })
}

/// Best effort: a lost stamp only costs the next process one fetch.
fn write_stamp(dir: &Path, key: &FetchKey, now: std::time::SystemTime) {
    let mut stamps = read_stamps(dir);
    stamps.retain(|_, secs| {
        within_window(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(*secs),
            now,
        )
    });
    stamps.insert(stamp_name(key), unix_secs(now));
    let Ok(text) = serde_json::to_string(&stamps) else {
        return;
    };
    if crate::fsperm::ensure_private_dir(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".{FETCH_STAMPS_FILE}.{}", uuid::Uuid::new_v4()));
    let written = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = crate::fsperm::create_private_file(&tmp)?;
        file.write_all(text.as_bytes())?;
        drop(file);
        crate::fsperm::replace_file(&tmp, &dir.join(FETCH_STAMPS_FILE))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The fetch window's decision, with the network call and clock injected for tests.
fn gated_fetch(
    key: &FetchKey,
    stamps: Option<&Path>,
    now: impl Fn() -> std::time::SystemTime,
    window: bool,
    fetch: impl FnOnce() -> Result<(), String>,
) -> Result<FetchOutcome, String> {
    let arrived = std::time::Instant::now();
    let slot = fetch_slot(key);
    let mut slot = slot.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some((finished, result)) = &slot.last_attempt {
        if *finished >= arrived {
            return result.clone().map(|()| FetchOutcome::Reused);
        }
    }
    let fresh = window
        && (slot
            .last_success
            .is_some_and(|stamp| within_window(stamp, now()))
            || stamps.is_some_and(|dir| stamp_fresh(dir, key, now())));
    if fresh {
        return Ok(FetchOutcome::Reused);
    }
    let result = fetch();
    if result.is_ok() {
        let stamp = now();
        slot.last_success = Some(stamp);
        if let Some(dir) = stamps {
            write_stamp(dir, key, stamp);
        }
    }
    slot.last_attempt = Some((std::time::Instant::now(), result.clone()));
    result.map(|()| FetchOutcome::Fetched)
}

/// Whether `remote` needs no fetch now: it was fetched inside the window, or the repository
/// has no such remote to fetch. A fetch still running counts as not fresh.
pub fn fetch_is_fresh(project: &Path, remote: &str) -> bool {
    if !git_output(project, &["remote"])
        .is_ok_and(|remotes| remotes.lines().any(|name| name == remote))
    {
        return true;
    }
    let key = fetch_key(project, remote);
    let now = std::time::SystemTime::now();
    let slot = fetch_slot(&key);
    let Ok(slot) = slot.try_lock() else {
        return false;
    };
    slot.last_success
        .is_some_and(|stamp| within_window(stamp, now))
        || stamp_dir().is_some_and(|dir| stamp_fresh(&dir, &key, now))
}

fn stamp_dir() -> Option<std::path::PathBuf> {
    FETCH_STAMP_DIR.lock().ok().and_then(|dir| dir.clone())
}

pub fn fetch_remote(project: &Path, remote: &str) -> Result<(), String> {
    fetch_remote_outcome(project, remote).map(|_| ())
}

/// Bounded fetch behind the shared fetch window. Failures are never remembered: the next
/// caller tries the network again, unless it was already waiting on that failed attempt.
pub fn fetch_remote_outcome(project: &Path, remote: &str) -> Result<FetchOutcome, String> {
    gated_fetch_remote(project, remote, true)
}

/// Fetch even inside the window (still joining a fetch already running), for a ref the
/// caller needs that the cached refs do not have.
fn fetch_remote_now(project: &Path, remote: &str) -> Result<(), String> {
    gated_fetch_remote(project, remote, false).map(|_| ())
}

fn gated_fetch_remote(project: &Path, remote: &str, window: bool) -> Result<FetchOutcome, String> {
    if remote.is_empty() || remote.starts_with('-') || remote == "." {
        return Err("invalid remote".into());
    }
    let key = fetch_key(project, remote);
    gated_fetch(
        &key,
        stamp_dir().as_deref(),
        std::time::SystemTime::now,
        window,
        || {
            bounded_git(
                project,
                &[
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--no-recurse-submodules",
                    remote,
                ],
            )
        },
    )
}

pub fn list_branches(project: &Path) -> Result<Vec<String>, String> {
    list_branches_with_warning(project).map(|(branches, _)| branches)
}

/// Refresh picker branches, retaining an explicit cached-ref fallback warning.
pub fn list_branches_with_warning(project: &Path) -> Result<(Vec<String>, Option<String>), String> {
    let warning = fetch_remote(project, "origin")
        .err()
        .map(|_| OFFLINE_BRANCHES.to_string());
    list_cached_branches(project).map(|branches| (branches, warning))
}

/// Local and `origin/*` branches from the refs already on disk. Never touches the network.
pub fn list_cached_branches(project: &Path) -> Result<Vec<String>, String> {
    let text = git_output(
        project,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes/origin",
        ],
    )?;
    let mut branches = std::collections::BTreeSet::new();
    for reference in text.lines() {
        let name = reference
            .strip_prefix("refs/heads/")
            .or_else(|| reference.strip_prefix("refs/remotes/"));
        if let Some(name) = name.filter(|name| !name.ends_with("/HEAD")) {
            branches.insert(name.to_string());
        }
    }
    Ok(branches.into_iter().collect())
}

/// Infer a legacy/direct remote name. Explicitly resolved upstreams keep actual provenance.
pub fn remote_for_ref(project: &Path, reference: &str) -> Option<String> {
    let short = reference.strip_prefix("refs/remotes/").unwrap_or(reference);
    git_output(project, &["remote"])
        .ok()?
        .lines()
        .filter(|remote| short.starts_with(&format!("{remote}/")))
        .min_by_key(|remote| remote.len())
        .map(str::to_string)
}

pub fn resolve(project: &Path, explicit: Option<&str>) -> Result<ResolvedBase, String> {
    let mut warning = None;
    let selected_remote;
    let reference = if let Some(base) = explicit {
        let refreshed = refresh_explicit_remote(project, base);
        if let Some((remote, Err(reason))) = &refreshed {
            warning = Some(format!("{reason}; using cached {remote} ref"));
        }
        let local = branch_ref(project, base)?;
        let (upstream, remote) = if local.starts_with("refs/heads/") {
            let upstream = git_output(project, &["for-each-ref", "--format=%(upstream)", &local])
                .ok()
                .filter(|value| !value.is_empty());
            let remote = git_output(
                project,
                &["for-each-ref", "--format=%(upstream:remotename)", &local],
            )
            .ok()
            .filter(|value| !value.is_empty() && value != ".");
            match upstream.filter(|reference| reference.starts_with("refs/remotes/")) {
                Some(upstream) if remote.is_some() => (Some(upstream), remote),
                _ => (None, None),
            }
        } else {
            (None, remote_for_ref(project, &local))
        };
        let target = upstream.unwrap_or(local);
        selected_remote = remote;
        if let Some(remote) = selected_remote.as_deref() {
            if refreshed
                .as_ref()
                .is_none_or(|(fetched, _)| fetched != remote)
            {
                if let Err(reason) = fetch_remote(project, remote) {
                    warning = Some(format!("{reason}; using cached {remote} ref"));
                }
            }
        }
        target
    } else {
        selected_remote = Some("origin".into());
        match fetch_remote_outcome(project, "origin") {
            Err(reason) => {
                warning = Some(format!("{reason}; using cached origin default ref"));
            }
            // Refresh origin/HEAD too: an existing local symbolic ref can be stale after
            // the hosting service changes its default. Failure preserves the cached value.
            // Inside the fetch window the cached default is as fresh as the cached refs.
            Ok(FetchOutcome::Fetched) => {
                if bounded_git(project, &["remote", "set-head", "origin", "--auto"]).is_err() {
                    warning =
                        Some("could not refresh origin/HEAD; using cached default ref".into());
                }
            }
            Ok(FetchOutcome::Reused) => {}
        }
        default_ref(project)?
    };
    let commit = git_output(
        project,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )?;
    let mut short = reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(&reference)
        .to_string();
    // Preserve an exact local namespace when its name could be mistaken for a remote
    // ref during cleanup (a local branch named origin/main is legal).
    if reference.starts_with("refs/heads/") && remote_for_ref(project, &short).is_some() {
        short = reference.clone();
    }
    Ok(ResolvedBase {
        reference: short,
        full_ref: Some(reference),
        commit: Some(commit),
        remote: selected_remote,
        warning,
    })
}

/// Resolve a recorded name without Git's tag/branch ambiguity rules. New ambiguous
/// local names are recorded fully qualified; legacy names conservatively use branches.
pub fn recorded_branch_ref(project: &Path, reference: &str) -> Result<String, String> {
    let exact = if reference.starts_with("refs/heads/") || reference.starts_with("refs/remotes/") {
        reference.to_string()
    } else if remote_for_ref(project, reference).is_some() {
        format!("refs/remotes/{reference}")
    } else {
        format!("refs/heads/{reference}")
    };
    if git_status(project, &["show-ref", "--verify", &exact]).is_ok_and(|status| status.success()) {
        return Ok(exact);
    }
    // Pre-base v6 dispatches could record a detached HEAD commit. Accept only an
    // exact object id here, never a revision expression or tag, and never on input.
    if matches!(reference.len(), 40 | 64) && reference.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        let commit = git_output(
            project,
            &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
        )?;
        if commit.eq_ignore_ascii_case(reference) {
            return Ok(commit);
        }
    }
    Err(format!("recorded base {reference} is unavailable"))
}

/// Template value: the short branch name, using the actual resolved remote.
pub fn short_name_for_remote(reference: &str, remote: Option<&str>) -> String {
    if let Some(local) = reference.strip_prefix("refs/heads/") {
        return local.to_string();
    }
    let reference = reference.strip_prefix("refs/remotes/").unwrap_or(reference);
    remote
        .and_then(|remote| reference.strip_prefix(&format!("{remote}/")))
        .unwrap_or(reference)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    fn key(name: &str) -> FetchKey {
        (
            std::path::PathBuf::from(format!("/fetch-window/{name}/{}", uuid::Uuid::new_v4())),
            "origin".into(),
        )
    }

    #[test]
    fn fetch_window_skips_a_second_fetch_inside_sixty_seconds_and_fetches_after() {
        let key = key("window");
        let start = SystemTime::now();
        let count = AtomicUsize::new(0);
        let fetch = || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        assert_eq!(
            gated_fetch(&key, None, || start, true, fetch),
            Ok(FetchOutcome::Fetched)
        );
        let later = start + Duration::from_secs(59);
        assert_eq!(
            gated_fetch(&key, None, || later, true, fetch),
            Ok(FetchOutcome::Reused)
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let expired = start + FETCH_WINDOW;
        assert_eq!(
            gated_fetch(&key, None, || expired, true, fetch),
            Ok(FetchOutcome::Fetched)
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_ref_missing_from_disk_fetches_inside_the_window() {
        let key = key("forced");
        let now = SystemTime::now();
        let count = AtomicUsize::new(0);
        let fetch = || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        gated_fetch(&key, None, || now, true, fetch).unwrap();
        assert_eq!(
            gated_fetch(&key, None, || now, false, fetch),
            Ok(FetchOutcome::Fetched)
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn fetch_window_never_remembers_a_failure() {
        let key = key("failure");
        let now = SystemTime::now();
        let count = AtomicUsize::new(0);
        let fail = || {
            count.fetch_add(1, Ordering::SeqCst);
            Err("fetch failed".to_string())
        };
        assert!(gated_fetch(&key, None, || now, true, fail).is_err());
        assert!(gated_fetch(&key, None, || now, true, fail).is_err());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_caller_arriving_during_a_fetch_waits_and_takes_its_result() {
        let key = key("join");
        let count = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let first = {
            let key = key.clone();
            let count = Arc::clone(&count);
            std::thread::spawn(move || {
                gated_fetch(&key, None, SystemTime::now, true, || {
                    count.fetch_add(1, Ordering::SeqCst);
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Err("offline".to_string())
                })
            })
        };
        started_rx.recv().unwrap();
        let second = {
            let key = key.clone();
            let count = Arc::clone(&count);
            std::thread::spawn(move || {
                gated_fetch(&key, None, SystemTime::now, true, || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        // The second caller is blocked behind the first fetch, not running its own.
        std::thread::sleep(Duration::from_millis(50));
        assert!(!second.is_finished());
        release_tx.send(()).unwrap();
        assert_eq!(first.join().unwrap(), Err("offline".into()));
        assert_eq!(second.join().unwrap(), Err("offline".into()));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fetch_stamps_share_the_window_across_processes() {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tsk-fetch-stamps-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let key = key("stamps");
        let now = SystemTime::now();
        assert!(!stamp_fresh(&dir, &key, now));
        write_stamp(&dir, &key, now);
        assert!(stamp_fresh(&dir, &key, now + Duration::from_secs(30)));
        assert!(!stamp_fresh(&dir, &key, now + FETCH_WINDOW));
        // A fresh in-memory slot for the same key (a new process) reuses the stamp.
        let other = key.clone();
        FETCHES.lock().unwrap().remove(&other);
        let count = AtomicUsize::new(0);
        let fetch = || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        assert_eq!(
            gated_fetch(&other, Some(&dir), || now, true, fetch),
            Ok(FetchOutcome::Reused)
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
