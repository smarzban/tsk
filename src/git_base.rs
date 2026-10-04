//! Branch-only dispatch bases, resolved in the task repository, never the caller's checkout.
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBase {
    pub reference: String,
    pub commit: Option<String>,
    pub remote: Option<String>,
    pub warning: Option<String>,
}

/// Only exact local or remote branch names are accepted, not revisions or tags.
pub fn validate_branch(project: &Path, base: &str) -> Result<(), String> {
    branch_ref(project, base).map(|_| ())
}

fn git_output(project: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_string())
        .map_err(|error| format!("git returned invalid text: {error}"))
}

fn branch_ref(project: &Path, base: &str) -> Result<String, String> {
    if base.is_empty() || base.starts_with('-') || base == "HEAD" || base.ends_with("/HEAD") {
        return Err(format!("unknown base branch {base}"));
    }
    for namespace in ["refs/heads/", "refs/remotes/"] {
        let reference = format!("{namespace}{base}");
        if git_output(project, &["check-ref-format", &reference]).is_ok()
            && git_output(project, &["show-ref", "--verify", &reference]).is_ok()
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
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(project)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run git: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(status) if status.success() => return Ok(()),
            Some(_) => return Err("fetch failed (offline or unavailable remote)".into()),
            None if Instant::now() >= deadline => {
                #[cfg(unix)]
                // SAFETY: this is the process group we created for our git child. Killing
                // it also stops its SSH/credential children, not any caller process.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err("fetch timed out".into());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
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
    let _ = fetch_remote(project, "origin");
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
            (upstream, remote)
        } else {
            (None, remote_for_ref(project, &local))
        };
        let target = upstream.unwrap_or(local);
        selected_remote = remote;
        if let Some(remote) = selected_remote.as_deref() {
            if let Err(reason) = fetch_remote(project, remote) {
                warning = Some(format!("{reason}; using cached {remote} ref"));
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
        short = reference;
    }
    Ok(ResolvedBase {
        reference: short,
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
    if git_output(project, &["show-ref", "--verify", &exact]).is_ok() {
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
