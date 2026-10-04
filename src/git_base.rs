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
    let result = fetch_remote(project, &remote);
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

pub fn fetch_remote(project: &Path, remote: &str) -> Result<(), String> {
    if remote.is_empty() || remote.starts_with('-') || remote == "." {
        return Err("invalid remote".into());
    }
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
}

pub fn list_branches(project: &Path) -> Result<Vec<String>, String> {
    list_branches_with_warning(project).map(|(branches, _)| branches)
}

/// Refresh picker branches, retaining an explicit cached-ref fallback warning.
pub fn list_branches_with_warning(project: &Path) -> Result<(Vec<String>, Option<String>), String> {
    let warning = fetch_remote(project, "origin")
        .err()
        .map(|reason| format!("{reason}; using cached origin refs"));
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
    Ok((branches.into_iter().collect(), warning))
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
        if let Err(reason) = fetch_remote(project, "origin") {
            warning = Some(format!("{reason}; using cached origin default ref"));
        } else {
            // Refresh origin/HEAD too: an existing local symbolic ref can be stale after
            // the hosting service changes its default. Failure preserves the cached value.
            if bounded_git(project, &["remote", "set-head", "origin", "--auto"]).is_err() {
                warning = Some("could not refresh origin/HEAD; using cached default ref".into());
            }
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
