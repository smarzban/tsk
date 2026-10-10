//! Branch-only dispatch bases, resolved in the task repository, never the caller's checkout.
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBase {
    pub reference: String,
    /// Exact branch namespace, independent of later remote configuration changes.
    pub full_ref: String,
    pub commit: Option<String>,
    pub remote: Option<String>,
    pub warning: Option<String>,
}

/// A task base names a branch in the task's own repository, so a desk task has none. `fresh`
/// lets CLI validation refresh an explicitly named remote branch first.
pub fn validate_task_base(
    scope: &crate::domain::TaskScope,
    base: &str,
    fresh: bool,
) -> Result<(), String> {
    let crate::domain::TaskScope::Project { path } = scope else {
        return Err("base requires a project task".into());
    };
    if fresh {
        validate_branch_fresh(Path::new(path), base)
    } else {
        validate_branch(Path::new(path), base)
    }
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
pub(crate) const NETWORK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Test suites run real Git on loaded machines, where a 250ms metadata query or a 5s local
/// fetch can overrun and silently flip a ref check. Debug builds let tests stretch the two
/// default deadlines above: unit tests always, integration suites through
/// `stretch_default_deadlines_for_tests`, and the debug binary they spawn through
/// `STRETCH_DEADLINES_ENV`. Release builds compile none of it: the hooks are no-ops and the
/// deadlines are the constants. Caller-chosen deadlines (`git_process_output_timeout`) are
/// never stretched.
#[doc(hidden)]
pub const STRETCH_DEADLINES_ENV: &str = "TSK_TEST_STRETCH_GIT_DEADLINES";

#[cfg(debug_assertions)]
mod test_deadlines {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread::LocalKey;
    use std::time::Duration;

    const SCALE: u32 = 20;
    static STRETCHED: AtomicU32 = AtomicU32::new(if cfg!(test) { SCALE } else { 1 });
    thread_local! {
        pub(super) static EXACT_LOCAL: Cell<bool> = const { Cell::new(false) };
        pub(super) static EXACT_FETCH: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) fn stretch() {
        STRETCHED.store(SCALE, Ordering::Relaxed);
    }

    pub(super) fn deadline(base: Duration, exact: &'static LocalKey<Cell<bool>>) -> Duration {
        if exact.with(Cell::get) {
            return base;
        }
        static FROM_ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let from_env =
            *FROM_ENV.get_or_init(|| std::env::var_os(super::STRETCH_DEADLINES_ENV).is_some());
        let scale = STRETCHED.load(Ordering::Relaxed);
        base * if from_env { SCALE } else { scale }
    }
}

/// Test hook: stretch the default Git deadlines for this whole process (debug builds only).
#[doc(hidden)]
pub fn stretch_default_deadlines_for_tests() {
    #[cfg(debug_assertions)]
    test_deadlines::stretch();
}

/// Test hook: a test asserting the shipped 250ms metadata deadline (or that some query does
/// not use it) keeps that value on its own thread, whatever parallel tests stretched.
#[doc(hidden)]
pub fn exact_local_deadline_on_this_thread() {
    #[cfg(debug_assertions)]
    test_deadlines::EXACT_LOCAL.with(|exact| exact.set(true));
}

/// Test hook: as `exact_local_deadline_on_this_thread`, for the shipped 5s fetch deadline.
#[doc(hidden)]
pub fn exact_fetch_deadline_on_this_thread() {
    #[cfg(debug_assertions)]
    test_deadlines::EXACT_FETCH.with(|exact| exact.set(true));
}

#[cfg(debug_assertions)]
fn local_deadline() -> std::time::Duration {
    test_deadlines::deadline(LOCAL_TIMEOUT, &test_deadlines::EXACT_LOCAL)
}

#[cfg(not(debug_assertions))]
fn local_deadline() -> std::time::Duration {
    LOCAL_TIMEOUT
}

#[cfg(debug_assertions)]
fn fetch_deadline() -> std::time::Duration {
    test_deadlines::deadline(NETWORK_TIMEOUT, &test_deadlines::EXACT_FETCH)
}

#[cfg(not(debug_assertions))]
fn fetch_deadline() -> std::time::Duration {
    NETWORK_TIMEOUT
}

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
fn stop_tree(child: &mut std::process::Child, tree: &ProcessTree) {
    #[cfg(unix)]
    // SAFETY: our child was spawned into its own process group. This also stops
    // Git's SSH/credential children, never processes in the caller's group.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    tree.kill();
    let _ = child.kill();
    let _ = child.wait();
}

/// Everything a spawned process starts, so a timeout stops the whole tree. On Unix the
/// process group does this; on Windows a Job Object holds the child from just after spawn
/// (a grandchild started in that instant escapes it). Closing the job's last handle kills
/// whatever is still in it, so a crashed tsk leaves no stuck Git behind either, unless the
/// tree is meant to outlive tsk.
struct ProcessTree {
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl ProcessTree {
    fn contain(_child: &std::process::Child, _outlive_tsk: bool) -> Self {
        Self {
            #[cfg(windows)]
            job: job::Job::contain(_child, !_outlive_tsk),
        }
    }

    fn kill(&self) {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.kill();
        }
    }

    /// The process exited on its own: let anything it detached on purpose (Git's background
    /// auto-gc) outlive the job instead of dying with its handle.
    fn release(self) {
        #[cfg(windows)]
        if let Some(job) = self.job {
            job.release();
        }
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub(super) struct Job(HANDLE);

    impl Job {
        pub(super) fn contain(child: &std::process::Child, kill_on_close: bool) -> Option<Self> {
            // SAFETY: plain Win32 calls on handles we own; the limit structure outlives the
            // call that reads it, and `Job` closes the job handle exactly once.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Self(handle);
                if kill_on_close && !job.limit(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE) {
                    return None;
                }
                if AssignProcessToJobObject(handle, child.as_raw_handle() as HANDLE) == 0 {
                    return None;
                }
                Some(job)
            }
        }

        fn limit(&self, flags: u32) -> bool {
            // SAFETY: see `contain`.
            unsafe {
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = flags;
                SetInformationJobObject(
                    self.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(limits).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) != 0
            }
        }

        pub(super) fn kill(&self) {
            // SAFETY: see `contain`.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }

        pub(super) fn release(self) {
            self.limit(0);
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateJobObjectW and is closed only here.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

fn run_git(
    project: &Path,
    args: &[&str],
    timeout: std::time::Duration,
    capture: bool,
) -> Result<std::process::Output, String> {
    run_git_env(project, args, &[], timeout, capture, None)
}

fn run_git_env(
    project: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: std::time::Duration,
    capture: bool,
    // Some: the Git tree outlives tsk, and the callback runs once it has started.
    outlive_tsk: Option<&mut dyn FnMut()>,
) -> Result<std::process::Output, String> {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(project)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .envs(envs.iter().copied());
    run_bounded(command, "git", timeout, capture, outlive_tsk)
}

/// Any other program under the same bound as Git: captured output, and at `timeout` its
/// whole process tree is killed and reaped. Errors name `program`, such as `herdr timed out`.
pub fn bounded_process_output(
    command: std::process::Command,
    program: &str,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    run_bounded(command, program, timeout, true, None)
}

fn run_bounded(
    mut command: std::process::Command,
    program: &str,
    timeout: std::time::Duration,
    capture: bool,
    outlive_tsk: Option<&mut dyn FnMut()>,
) -> Result<std::process::Output, String> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + timeout;
    let mut stdout = capture.then(Capture::new).transpose()?;
    let mut stderr = capture.then(Capture::new).transpose()?;
    command.stdin(Stdio::null());
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
        .map_err(|error| format!("could not run {program}: {error}"))?;
    let tree = ProcessTree::contain(&child, outlive_tsk.is_some());
    if let Some(started) = outlive_tsk {
        started();
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Err(error) => {
                stop_tree(&mut child, &tree);
                return Err(format!("could not wait for {program}: {error}"));
            }
            Ok(None) if Instant::now() >= deadline => {
                stop_tree(&mut child, &tree);
                return Err(format!("{program} timed out"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    tree.release();
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
    run_git(project, args, local_deadline(), true)
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

/// Captured Git work that must never stop halfway because tsk exits: it still stops at
/// `timeout` while tsk waits, but if tsk exits first (a board quit outliving its bound) the
/// Git tree finishes on its own instead of dying with tsk's job handle. `started` runs once
/// Git is running.
pub fn git_process_output_outliving_tsk(
    project: &Path,
    args: &[&str],
    timeout: std::time::Duration,
    started: &mut dyn FnMut(),
) -> Result<std::process::Output, String> {
    run_git_env(project, args, &[], timeout, true, Some(started))
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
    run_git(project, args, local_deadline(), false).map(|output| output.status)
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
    bounded_git_env(project, args, &[])
}

fn bounded_git_env(project: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<(), String> {
    let output =
        run_git_env(project, args, envs, fetch_deadline(), false, None).map_err(|reason| {
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
    /// The branches were fetched but `refs/remotes/<remote>/HEAD` could not be refreshed
    /// (only the pre-2.48 `remote set-head` fallback can fail separately).
    FetchedStaleHead,
    /// Inside the fetch window, or joined a fetch that finished while this caller waited.
    Reused,
}

/// What a caller needs fresh before it may skip the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FetchNeed {
    /// Remote-tracking branches inside the window.
    Refs,
    /// Branches and the remote default (`<remote>/HEAD`): default-base resolution.
    RefsAndHead,
    /// Fetch now; only joining a fetch already running counts.
    Now,
}

#[derive(Debug, Default)]
struct FetchSlot {
    last_success: Option<std::time::SystemTime>,
    /// Last success that also refreshed the remote default.
    last_head: Option<std::time::SystemTime>,
    /// Ok(true) when that attempt refreshed the remote default too.
    last_attempt: Option<(std::time::Instant, Result<bool, String>)>,
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

/// The remote default's own stamp: written only when that fetch refreshed `<remote>/HEAD`.
fn head_stamp_name(key: &FetchKey) -> String {
    format!("{}\nHEAD", stamp_name(key))
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

fn stamp_fresh(dir: &Path, name: &str, now: std::time::SystemTime) -> bool {
    read_stamps(dir).get(name).is_some_and(|secs| {
        within_window(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(*secs),
            now,
        )
    })
}

/// Best effort: a lost stamp only costs the next process one fetch.
fn write_stamp(dir: &Path, names: &[String], now: std::time::SystemTime) {
    let mut stamps = read_stamps(dir);
    stamps.retain(|_, secs| {
        within_window(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(*secs),
            now,
        )
    });
    for name in names {
        stamps.insert(name.clone(), unix_secs(now));
    }
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
    need: FetchNeed,
    fetch: impl FnOnce() -> Result<bool, String>,
) -> Result<FetchOutcome, String> {
    let arrived = std::time::Instant::now();
    let slot = fetch_slot(key);
    let mut slot = slot.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some((finished, result)) = &slot.last_attempt {
        if *finished >= arrived {
            match result {
                Err(reason) => return Err(reason.clone()),
                // A joined fetch that left the default stale does not serve a caller that
                // needs the default: it fetches again below.
                Ok(head) if *head || need != FetchNeed::RefsAndHead => {
                    return Ok(FetchOutcome::Reused)
                }
                Ok(_) => {}
            }
        }
    }
    let fresh = match need {
        FetchNeed::Now => false,
        FetchNeed::Refs => {
            slot.last_success
                .is_some_and(|stamp| within_window(stamp, now()))
                || stamps.is_some_and(|dir| stamp_fresh(dir, &stamp_name(key), now()))
        }
        FetchNeed::RefsAndHead => {
            slot.last_head
                .is_some_and(|stamp| within_window(stamp, now()))
                || stamps.is_some_and(|dir| stamp_fresh(dir, &head_stamp_name(key), now()))
        }
    };
    if fresh {
        return Ok(FetchOutcome::Reused);
    }
    let result = fetch();
    if let Ok(head) = result {
        let stamp = now();
        slot.last_success = Some(stamp);
        let mut names = vec![stamp_name(key)];
        if head {
            slot.last_head = Some(stamp);
            names.push(head_stamp_name(key));
        }
        if let Some(dir) = stamps {
            write_stamp(dir, &names, stamp);
        }
    }
    slot.last_attempt = Some((std::time::Instant::now(), result.clone()));
    result.map(|head| {
        if head {
            FetchOutcome::Fetched
        } else {
            FetchOutcome::FetchedStaleHead
        }
    })
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
        || stamp_dir().is_some_and(|dir| stamp_fresh(&dir, &stamp_name(&key), now))
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
    gated_fetch_remote(project, remote, FetchNeed::Refs)
}

/// Fetch even inside the window (still joining a fetch already running), for a ref the
/// caller needs that the cached refs do not have.
fn fetch_remote_now(project: &Path, remote: &str) -> Result<(), String> {
    gated_fetch_remote(project, remote, FetchNeed::Now).map(|_| ())
}

/// Default-base resolution: the window counts only when its fetch also refreshed
/// `<remote>/HEAD`.
fn fetch_remote_with_head(project: &Path, remote: &str) -> Result<FetchOutcome, String> {
    gated_fetch_remote(project, remote, FetchNeed::RefsAndHead)
}

fn gated_fetch_remote(
    project: &Path,
    remote: &str,
    need: FetchNeed,
) -> Result<FetchOutcome, String> {
    if remote.is_empty() || remote.starts_with('-') || remote == "." {
        return Err("invalid remote".into());
    }
    let key = fetch_key(project, remote);
    gated_fetch(
        &key,
        stamp_dir().as_deref(),
        std::time::SystemTime::now,
        need,
        || fetch_with_remote_head(project, remote),
    )
}

/// One fetch that also refreshes `refs/remotes/<remote>/HEAD`, so a fetch that fills the
/// window normally leaves the remote default as fresh as the branches. Returns whether the
/// default was refreshed: only then may default-base resolution reuse the window.
///
/// The setting goes through Git's environment config so the argv stays a plain `fetch`.
fn fetch_with_remote_head(project: &Path, remote: &str) -> Result<bool, String> {
    let fetch = [
        "fetch",
        "--quiet",
        "--no-tags",
        "--no-recurse-submodules",
        remote,
    ];
    let inherited = std::env::var_os("GIT_CONFIG_COUNT");
    let follow = follows_remote_head(project)
        .then(|| follow_head_env(inherited.as_deref(), remote))
        .flatten();
    if let Some(envs) = follow {
        let envs: Vec<(&str, &str)> = envs
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        bounded_git_env(project, &fetch, &envs)?;
        return Ok(true);
    }
    // Git before 2.48 (or a malformed inherited count we will not rewrite): ask the remote
    // separately. Its failure keeps the cached default and is reported, never certified.
    bounded_git(project, &fetch)?;
    Ok(bounded_git(project, &["remote", "set-head", remote, "--auto"]).is_ok())
}

/// `remote.<remote>.followRemoteHEAD=always` appended after any `GIT_CONFIG_*` entries the
/// caller's environment already carries (an absent or empty count is zero). A malformed
/// count is left alone: rewriting it would change how Git reads the caller's own entries
/// (Git itself rejects it), so the caller falls back to `remote set-head`.
fn follow_head_env(
    inherited: Option<&std::ffi::OsStr>,
    remote: &str,
) -> Option<[(String, String); 3]> {
    let index = match inherited.map(|count| count.to_str()) {
        None => 0,
        Some(Some("")) => 0,
        Some(Some(count)) => count.parse::<usize>().ok()?,
        Some(None) => return None,
    };
    Some([
        ("GIT_CONFIG_COUNT".into(), (index + 1).to_string()),
        (
            format!("GIT_CONFIG_KEY_{index}"),
            format!("remote.{remote}.followRemoteHEAD"),
        ),
        (format!("GIT_CONFIG_VALUE_{index}"), "always".into()),
    ])
}

#[cfg(test)]
thread_local! {
    /// Tests force either remote-HEAD path regardless of the installed Git.
    static FOLLOWS_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

fn follows_remote_head(project: &Path) -> bool {
    #[cfg(test)]
    if let Some(forced) = FOLLOWS_OVERRIDE.with(std::cell::Cell::get) {
        return forced;
    }
    git_follows_remote_head(project)
}

fn git_follows_remote_head(project: &Path) -> bool {
    static FOLLOWS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FOLLOWS.get_or_init(|| {
        git_output(project, &["--version"])
            .ok()
            .and_then(|text| {
                let version = text.strip_prefix("git version ")?;
                let mut parts = version.split('.');
                let major: u32 = parts.next()?.parse().ok()?;
                let minor: u32 = parts.next()?.parse().ok()?;
                Some((major, minor) >= (2, 48))
            })
            .unwrap_or(false)
    })
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

/// The configured remote a remote-tracking name belongs to, shortest prefix first.
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
        // Refresh origin/HEAD with the branches: an existing local symbolic ref can be stale
        // after the hosting service changes its default. The window is reused only when its
        // fetch refreshed the default too.
        match fetch_remote_with_head(project, "origin") {
            Err(reason) => {
                warning = Some(format!("{reason}; using cached origin default ref"));
            }
            Ok(FetchOutcome::FetchedStaleHead) => {
                warning = Some("could not refresh origin/HEAD; using cached default ref".into());
            }
            Ok(FetchOutcome::Fetched | FetchOutcome::Reused) => {}
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
        full_ref: reference,
        commit: Some(commit),
        remote: selected_remote,
        warning,
    })
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

    /// The child half of [`a_git_meant_to_outlive_tsk_survives_tsk_exiting_and_others_do_not`]:
    /// a test process that runs one long Git and is killed while it waits.
    #[cfg(windows)]
    #[test]
    #[ignore = "helper process, run by its parent test"]
    fn outliving_git_helper() {
        let (Some(dir), Some(alias)) = (
            std::env::var_os("TSK_OUTLIVE_DIR"),
            std::env::var("TSK_OUTLIVE_ALIAS").ok(),
        ) else {
            return;
        };
        let outlive = std::env::var("TSK_OUTLIVE").as_deref() == Ok("1");
        let mut started = || {};
        let _ = run_git_env(
            Path::new(&dir),
            &["-c", &alias, "hang"],
            &[],
            std::time::Duration::from_secs(120),
            true,
            outlive.then_some(&mut started as &mut dyn FnMut()),
        );
    }

    /// A board quit that outlives its bound ends tsk while a Windows worktree removal runs.
    /// Ordinary Git dies with tsk; a removal's Git must finish, never stop mid-delete.
    #[cfg(windows)]
    #[test]
    fn a_git_meant_to_outlive_tsk_survives_tsk_exiting_and_others_do_not() {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        };
        for outlive in [false, true] {
            let dir = std::env::temp_dir().join(format!(
                "tsk-git-outlive-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).expect("mkdir");
            let pid_file = dir.join("pid.txt");
            let alias = format!(
                "alias.hang=!powershell -NoProfile -Command '$PID | Out-File -Encoding ascii \
                 \"{}\"; Start-Sleep 60'",
                pid_file.to_string_lossy().replace('\\', "/")
            );
            let mut helper = std::process::Command::new(std::env::current_exe().expect("exe"))
                .args([
                    "--exact",
                    "git_base::tests::outliving_git_helper",
                    "--ignored",
                    "--test-threads=1",
                ])
                .env("TSK_OUTLIVE_DIR", &dir)
                .env("TSK_OUTLIVE_ALIAS", &alias)
                .env("TSK_OUTLIVE", if outlive { "1" } else { "0" })
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("helper");
            let started = std::time::Instant::now();
            let pid: u32 = loop {
                if let Some(pid) = std::fs::read_to_string(&pid_file)
                    .ok()
                    .and_then(|text| text.trim().parse().ok())
                {
                    break pid;
                }
                assert!(
                    started.elapsed() < std::time::Duration::from_secs(60),
                    "no grandchild"
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            };
            // tsk exits mid-Git, as when a board quit outlives its bound.
            helper.kill().expect("kill helper");
            helper.wait().expect("reap helper");
            // SAFETY: plain Win32 calls on a handle opened and closed here.
            let exited = unsafe {
                let handle = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid);
                if handle.is_null() {
                    true
                } else {
                    let exited = WaitForSingleObject(handle, 5_000) == WAIT_OBJECT_0;
                    TerminateProcess(handle, 1);
                    CloseHandle(handle);
                    exited
                }
            };
            assert_eq!(exited, !outlive, "outlive {outlive}: grandchild {pid}");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// A timed-out Git takes its whole process tree down, as the Unix process group does:
    /// here an alias whose shell starts a long-lived `sleep` grandchild.
    #[cfg(windows)]
    #[test]
    fn a_timed_out_git_kills_its_grandchildren_on_windows() {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        let dir = std::env::temp_dir().join(format!(
            "tsk-git-tree-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let pid_file = dir.join("pid.txt");
        let pid_path = pid_file.to_string_lossy().replace('\\', "/");
        // Git for Windows' shell starts a native `sleep` in milliseconds (PowerShell's start-up
        // could outlast the deadline on a slow runner). Its Windows pid is recorded only once
        // the background child has exec'd `sleep`; before that it is a short-lived fork stub,
        // so a child that never shows as `sleep` publishes nothing and the test fails.
        let alias = format!(
            "alias.hang=!sleep 120 & p=$!; i=0; \
             until grep -q sleep /proc/$p/exename 2>/dev/null || [ $i -ge 400 ]; \
             do i=$((i+1)); sleep 0.05; done; \
             grep -q sleep /proc/$p/exename 2>/dev/null || {{ kill $p; exit 3; }}; \
             cat /proc/$p/winpid > \"{pid_path}.tmp\" && mv \"{pid_path}.tmp\" \"{pid_path}\"; \
             wait"
        );
        let deadline = std::time::Duration::from_secs(30);
        let git = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                git_process_output_timeout(&dir, &["-c", &alias, "hang"], deadline).map(|_| ())
            })
        };
        let started = std::time::Instant::now();
        let pid: u32 = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                break pid;
            }
            assert!(started.elapsed() < deadline, "the sleeper never started");
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        // SAFETY: plain Win32 calls; the handle is held across the deadline, so the pid cannot
        // be reused, and closed once below.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(
            !handle.is_null(),
            "recorded pid {pid} is not a running process"
        );
        // Negative control: before the deadline the recorded process is the live, long-lived
        // sleeper, so a wrong (already exited) pid fails here instead of passing vacuously.
        // SAFETY: a zero-timeout wait on the handle opened above.
        assert_eq!(
            unsafe { WaitForSingleObject(handle, 0) },
            WAIT_TIMEOUT,
            "recorded pid {pid} exited before the deadline"
        );
        assert_eq!(
            git.join().expect("git thread"),
            Err("git timed out".to_string())
        );
        // SAFETY: as above.
        let exited = unsafe {
            let waited = WaitForSingleObject(handle, 5_000);
            CloseHandle(handle);
            waited == WAIT_OBJECT_0
        };
        assert!(exited, "grandchild {pid} outlived the timed-out git");
        let _ = std::fs::remove_dir_all(dir);
    }
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
            Ok(true)
        };
        assert_eq!(
            gated_fetch(&key, None, || start, FetchNeed::Refs, fetch),
            Ok(FetchOutcome::Fetched)
        );
        let later = start + Duration::from_secs(59);
        assert_eq!(
            gated_fetch(&key, None, || later, FetchNeed::Refs, fetch),
            Ok(FetchOutcome::Reused)
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let expired = start + FETCH_WINDOW;
        assert_eq!(
            gated_fetch(&key, None, || expired, FetchNeed::Refs, fetch),
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
            Ok(true)
        };
        gated_fetch(&key, None, || now, FetchNeed::Refs, fetch).unwrap();
        assert_eq!(
            gated_fetch(&key, None, || now, FetchNeed::Now, fetch),
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
        assert!(gated_fetch(&key, None, || now, FetchNeed::Refs, fail).is_err());
        assert!(gated_fetch(&key, None, || now, FetchNeed::Refs, fail).is_err());
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
                gated_fetch(&key, None, SystemTime::now, FetchNeed::Refs, || {
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
                gated_fetch(&key, None, SystemTime::now, FetchNeed::Refs, || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(true)
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
        assert!(!stamp_fresh(&dir, &stamp_name(&key), now));
        write_stamp(&dir, &[stamp_name(&key)], now);
        assert!(stamp_fresh(
            &dir,
            &stamp_name(&key),
            now + Duration::from_secs(30)
        ));
        assert!(!stamp_fresh(&dir, &stamp_name(&key), now + FETCH_WINDOW));
        // A fresh in-memory slot for the same key (a new process) reuses the stamp.
        let other = key.clone();
        FETCHES.lock().unwrap().remove(&other);
        let count = AtomicUsize::new(0);
        let fetch = || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        };
        assert_eq!(
            gated_fetch(&other, Some(&dir), || now, FetchNeed::Refs, fetch),
            Ok(FetchOutcome::Reused)
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fetch_that_left_the_default_stale_never_serves_default_resolution() {
        let key = key("stale-head");
        let now = SystemTime::now();
        let count = AtomicUsize::new(0);
        let refs_only = || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(false)
        };
        assert_eq!(
            gated_fetch(&key, None, || now, FetchNeed::Refs, refs_only),
            Ok(FetchOutcome::FetchedStaleHead)
        );
        assert_eq!(
            gated_fetch(&key, None, || now, FetchNeed::Refs, refs_only),
            Ok(FetchOutcome::Reused),
            "branches are fresh"
        );
        assert_eq!(
            gated_fetch(&key, None, || now, FetchNeed::RefsAndHead, refs_only),
            Ok(FetchOutcome::FetchedStaleHead),
            "the default is not"
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
        let with_head = || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        };
        gated_fetch(&key, None, || now, FetchNeed::RefsAndHead, with_head).unwrap();
        assert_eq!(
            gated_fetch(&key, None, || now, FetchNeed::RefsAndHead, with_head),
            Ok(FetchOutcome::Reused)
        );
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_stale_default_is_not_stamped_for_other_processes() {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tsk-head-stamps-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let key = key("head-stamps");
        let now = SystemTime::now();
        gated_fetch(&key, Some(&dir), || now, FetchNeed::Refs, || Ok(false)).unwrap();
        FETCHES.lock().unwrap().remove(&key);
        let count = AtomicUsize::new(0);
        let fetch = || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        };
        assert_eq!(
            gated_fetch(&key, Some(&dir), || now, FetchNeed::Refs, fetch),
            Ok(FetchOutcome::Reused)
        );
        assert_eq!(
            gated_fetch(&key, Some(&dir), || now, FetchNeed::RefsAndHead, fetch),
            Ok(FetchOutcome::Fetched)
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn follow_head_config_appends_after_inherited_entries() {
        use std::ffi::OsStr;
        let entry = |inherited: Option<&OsStr>| follow_head_env(inherited, "origin");
        let expect = |index: usize| {
            Some([
                ("GIT_CONFIG_COUNT".to_string(), (index + 1).to_string()),
                (
                    format!("GIT_CONFIG_KEY_{index}"),
                    "remote.origin.followRemoteHEAD".to_string(),
                ),
                (format!("GIT_CONFIG_VALUE_{index}"), "always".to_string()),
            ])
        };
        assert_eq!(entry(None), expect(0));
        assert_eq!(entry(Some(OsStr::new(""))), expect(0));
        assert_eq!(entry(Some(OsStr::new("2"))), expect(2));
        // Malformed: leave the caller's environment alone and use `remote set-head`.
        assert_eq!(entry(Some(OsStr::new("two"))), None);
        assert_eq!(entry(Some(OsStr::new("-1"))), None);
    }

    struct Repos {
        root: std::path::PathBuf,
        local: std::path::PathBuf,
    }

    impl Drop for Repos {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// A clone whose remote default then moves to `trunk`, unseen by the clone.
    fn moved_default() -> Repos {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "tsk-head-repos-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let (remote, local) = (root.join("remote"), root.join("local"));
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "-q", "-b", "main"]);
        git(&remote, &["config", "user.email", "test@example.com"]);
        git(&remote, &["config", "user.name", "test"]);
        git(&remote, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        git(
            &root,
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                local.to_str().unwrap(),
            ],
        );
        git(&remote, &["branch", "trunk"]);
        git(&remote, &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
        Repos { root, local }
    }

    /// Route the clone's upload-pack through a wrapper that counts calls and fails every
    /// call after the first `ok_calls`.
    #[cfg(unix)]
    fn counted_upload_pack(repos: &Repos, ok_calls: usize) -> std::path::PathBuf {
        let counter = repos.root.join("upload-count");
        let wrapper = repos.root.join("upload-pack");
        crate::test_stub::write_stub(
            &wrapper,
            format!(
                "#!/bin/sh\nprintf 'x\\n' >> '{counter}'\nif [ $(wc -l < '{counter}') -gt {ok_calls} ]; then exit 1; fi\nexec git-upload-pack \"$@\"\n",
                counter = counter.display()
            ),
            0o700,
        );
        git(
            &repos.local,
            &[
                "config",
                "remote.origin.uploadpack",
                wrapper.to_str().unwrap(),
            ],
        );
        counter
    }

    #[cfg(unix)]
    fn calls(counter: &Path) -> usize {
        std::fs::read_to_string(counter)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    fn origin_head(repos: &Repos) -> String {
        git(&repos.local, &["symbolic-ref", "refs/remotes/origin/HEAD"])
    }

    #[test]
    fn the_legacy_path_refreshes_the_remote_default_with_set_head() {
        FOLLOWS_OVERRIDE.with(|forced| forced.set(Some(false)));
        let repos = moved_default();
        fetch_remote(&repos.local, "origin").unwrap();
        FOLLOWS_OVERRIDE.with(|forced| forced.set(None));
        assert_eq!(origin_head(&repos), "refs/remotes/origin/trunk");
        let base = resolve(&repos.local, None).unwrap();
        assert_eq!(base.reference, "origin/trunk");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_legacy_set_head_is_reported_and_never_certifies_the_default() {
        FOLLOWS_OVERRIDE.with(|forced| forced.set(Some(false)));
        let repos = moved_default();
        // The fetch succeeds; set-head (the second upload-pack call) and later calls fail.
        let counter = counted_upload_pack(&repos, 1);
        fetch_remote(&repos.local, "origin").unwrap();
        assert_eq!(calls(&counter), 2);
        assert_eq!(origin_head(&repos), "refs/remotes/origin/main");
        let base = resolve(&repos.local, None).unwrap();
        FOLLOWS_OVERRIDE.with(|forced| forced.set(None));
        assert_eq!(
            calls(&counter),
            3,
            "default resolution fetches again instead of reusing a stale default"
        );
        assert!(base
            .warning
            .is_some_and(|warning| warning.contains("cached")));
    }

    #[cfg(unix)]
    #[test]
    fn the_follow_path_refreshes_the_remote_default_inside_the_fetch() {
        if !git_follows_remote_head(Path::new(".")) {
            return; // needs Git 2.48+; the legacy path has its own tests
        }
        FOLLOWS_OVERRIDE.with(|forced| forced.set(Some(true)));
        let repos = moved_default();
        let counter = counted_upload_pack(&repos, usize::MAX >> 1);
        let outcome = fetch_remote_outcome(&repos.local, "origin");
        FOLLOWS_OVERRIDE.with(|forced| forced.set(None));
        assert_eq!(outcome, Ok(FetchOutcome::Fetched));
        assert_eq!(calls(&counter), 1, "no separate set-head round trip");
        assert_eq!(origin_head(&repos), "refs/remotes/origin/trunk");
    }
}
