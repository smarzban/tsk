//! Headless `backup` and `restore`.

use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::SystemTime;

use super::{presenter, CliOutput};
use crate::backup::{self, AddOutcome, Every, Probe, RunMode, RunOutcome, Trigger, Version};
use crate::store::{default_state_dir, RestoreError, TaskStore};
use crate::ui::terminal_text;

/// Parsed `backup` input.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BackupAction {
    Now,
    Add {
        location: String,
        every: Option<Every>,
    },
    List,
    /// Create or reuse a private GitHub repository with `gh`, then add it.
    Setup {
        repo: Option<String>,
    },
    Remove {
        target: String,
    },
    /// Internal: the detached child a save or board open starts.
    Run {
        trigger: Trigger,
    },
    /// Internal: the detached check `add` starts after its first backup.
    Probe {
        spec: String,
    },
}

struct BackupInput {
    action: BackupAction,
    state_dir: Option<PathBuf>,
    help: bool,
}

/// Split `--state-dir` and `--help` out of `args[2..]`, leaving the rest in order.
fn common_flags(args: &[String]) -> Result<(Vec<String>, Option<PathBuf>, bool), String> {
    let mut rest = Vec::new();
    let mut state_dir = None;
    let mut help = false;
    let mut index = 2;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--help" => help = true,
            "--state-dir" => {
                index += 1;
                match args.get(index) {
                    Some(value) if !value.starts_with('-') => {
                        state_dir = Some(PathBuf::from(value))
                    }
                    _ => return Err("missing value for --state-dir".into()),
                }
            }
            flag if flag.starts_with("--state-dir=") => {
                state_dir = Some(PathBuf::from(&flag["--state-dir=".len()..]));
            }
            _ => rest.push(arg.clone()),
        }
        index += 1;
    }
    Ok((rest, state_dir, help))
}

fn parse_backup(args: &[String]) -> Result<BackupInput, String> {
    let (rest, state_dir, help) = common_flags(args)?;
    if help {
        return Ok(BackupInput {
            action: BackupAction::List,
            state_dir,
            help,
        });
    }
    let mut words = rest.iter().map(String::as_str);
    let action = match words.next() {
        None | Some("now") => BackupAction::Now,
        Some("list") => BackupAction::List,
        Some("setup") => {
            let repo = match (words.next(), words.next()) {
                (None, _) => None,
                (Some("--repo"), Some(name)) if !name.starts_with('-') => Some(name.to_string()),
                (Some(flag), _) if flag.starts_with("--repo=") => {
                    Some(flag["--repo=".len()..].to_string())
                }
                (Some("--repo"), _) => return Err("missing value for --repo".into()),
                (Some(other), _) => return Err(format!("unexpected backup argument {other}")),
            };
            if repo.as_deref().is_some_and(|name| !valid_repo_name(name)) {
                return Err("--repo takes name, owner/name, or host/owner/name".into());
            }
            BackupAction::Setup { repo }
        }
        Some("remove") => BackupAction::Remove {
            target: words
                .next()
                .filter(|word| !word.starts_with('-'))
                .ok_or("target is required")?
                .to_string(),
        },
        Some("add") => {
            let mut location = None;
            let mut every = None;
            while let Some(word) = words.next() {
                match word {
                    "--every" => {
                        let value = words.next().ok_or("missing value for --every")?;
                        every = Some(
                            Every::parse(value)
                                .ok_or_else(|| format!("--every {value} is not save or daily"))?,
                        );
                    }
                    flag if flag.starts_with("--every=") => {
                        let value = &flag["--every=".len()..];
                        every = Some(
                            Every::parse(value)
                                .ok_or_else(|| format!("--every {value} is not save or daily"))?,
                        );
                    }
                    flag if flag.starts_with('-') => {
                        return Err(format!("unknown backup argument {flag}"))
                    }
                    value if location.is_none() => location = Some(value.to_string()),
                    value => return Err(format!("unexpected backup argument {value}")),
                }
            }
            BackupAction::Add {
                location: location.ok_or("git URL or repository path is required")?,
                every,
            }
        }
        Some("run") => match (words.next(), words.next()) {
            (Some("--on"), Some(word)) => BackupAction::Run {
                trigger: Trigger::parse(word).ok_or("run takes --on save or --on open")?,
            },
            (Some("--probe"), Some(spec)) => BackupAction::Probe {
                spec: spec.to_string(),
            },
            _ => return Err("run takes --on save or --on open".into()),
        },
        Some(other) => return Err(format!("unknown backup action {other}")),
    };
    if let Some(extra) = words.next() {
        return Err(format!("unexpected backup argument {extra}"));
    }
    Ok(BackupInput {
        action,
        state_dir,
        help,
    })
}

fn output(stdout: String, stderr: String, code: u8) -> CliOutput {
    CliOutput {
        stdout,
        stderr,
        code,
    }
}

fn refused(verb: &str, message: &str) -> CliOutput {
    output(
        String::new(),
        format!("tsk {verb}: {}\n", terminal_text(message)),
        1,
    )
}

/// `tsk backup …`.
pub fn run_backup(args: Vec<String>) -> CliOutput {
    let input = match parse_backup(&args) {
        Ok(input) => input,
        Err(reason) => return presenter::backup_usage(&reason),
    };
    if input.help {
        return presenter::backup_help();
    }
    let state_dir = input.state_dir.unwrap_or_else(default_state_dir);
    match input.action {
        BackupAction::Run { trigger } => {
            // The detached child: no one reads its output; failures land in backup-status.json.
            let _ = backup::run(&state_dir, RunMode::Due(trigger), SystemTime::now());
            output(String::new(), String::new(), 0)
        }
        BackupAction::Probe { spec } => {
            let _ = backup::run_probe(&state_dir, &spec);
            output(String::new(), String::new(), 0)
        }
        BackupAction::Now => backup_now(&state_dir),
        BackupAction::List => backup_list(&state_dir),
        BackupAction::Setup { repo } => backup_setup(&state_dir, repo.as_deref()),
        BackupAction::Add { location, every } => backup_add(&state_dir, &location, every),
        BackupAction::Remove { target } => match backup::remove_target(&state_dir, &target) {
            Ok(removed) => output(
                format!(
                    "removed {} target {}; copies already made stay where they are\n",
                    removed.kind.word(),
                    terminal_text(&removed.location)
                ),
                String::new(),
                0,
            ),
            Err(error) => refused("backup", &error),
        },
    }
}

/// `name` or `owner/name`, in the characters GitHub allows.
fn valid_repo_name(name: &str) -> bool {
    let part = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let parts: Vec<&str> = name.split('/').collect();
    // `name`, `owner/name`, or `host/owner/name`; a host may hold dots and a port.
    match parts.as_slice() {
        [repo] | [_, repo] if !part(repo) => false,
        [repo] => part(repo),
        [owner, repo] => part(owner) && part(repo),
        [host, owner, repo] => {
            !host.is_empty()
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':'))
                && part(owner)
                && part(repo)
        }
        _ => false,
    }
}

/// The GitHub CLI `tsk backup setup` runs: `TSK_GH` when set (tests point it at a fake), else
/// `gh` on PATH.
fn gh_program() -> std::path::PathBuf {
    std::env::var_os("TSK_GH")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "gh".into())
}

/// Run gh, on `host` when one is named (gh reads `GH_HOST`; without it, the user's own
/// `GH_HOST` or gh's default host applies).
fn gh_on(host: Option<&str>, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = std::process::Command::new(gh_program());
    if let Some(host) = host {
        command.env("GH_HOST", host);
    }
    command
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1");
    crate::git_base::bounded_process_output(command, "gh", std::time::Duration::from_secs(60))
}

/// How to get gh on this platform.
fn gh_install_hint() -> &'static str {
    if cfg!(windows) {
        "install it with `winget install GitHub.cli` (or from https://cli.github.com)"
    } else {
        "install it from https://cli.github.com"
    }
}

/// Default repository name `tsk backup setup` creates.
const SETUP_REPO: &str = "tsk-backup";

/// `tsk backup setup [--repo <[host/]owner/name|name>]`: make sure a private GitHub repository exists
/// (creating it when missing, refusing a public one), then add it like `tsk backup add`.
fn backup_setup(state_dir: &std::path::Path, repo: Option<&str>) -> CliOutput {
    let other_hosts =
        "For another git host, create a private repository and run `tsk backup add <git-url>`.";
    if !crate::dispatch::pr::gh_available(&gh_program()) {
        return refused(
            "backup",
            &format!(
                "setup needs the GitHub CLI (gh): {}, run `gh auth login`, then `tsk backup setup` again.\n{other_hosts}",
                gh_install_hint()
            ),
        );
    }
    // `[host/]owner/name` or `name`: a named host is used for every gh call.
    let parts: Vec<&str> = repo
        .map(|repo| repo.split('/').collect())
        .unwrap_or_default();
    let (host, owner, name) = match parts.as_slice() {
        [host, owner, name] => (Some(*host), Some(*owner), *name),
        [owner, name] => (None, Some(*owner), *name),
        [name] => (None, None, *name),
        _ => (None, None, SETUP_REPO),
    };
    let gh = |args: &[&str]| gh_on(host, args);
    let gh_error = |output: &std::process::Output| crate::dispatch::pr::gh_message(&output.stderr);
    match gh(&["auth", "status"]) {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            return refused(
                "backup",
                &format!(
                    "gh is not logged in ({}): run `gh auth login`, then `tsk backup setup` again.\n{other_hosts}",
                    gh_error(&output)
                ),
            )
        }
        Err(error) => return refused("backup", &error),
    }
    let owner = match owner {
        Some(owner) => owner.to_string(),
        None => match gh(&["api", "user", "--jq", ".login"]) {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            }
            Ok(output) => {
                return refused(
                    "backup",
                    &format!("could not read your GitHub user: {}", gh_error(&output)),
                )
            }
            Err(error) => return refused("backup", &error),
        },
    };
    let full = format!("{owner}/{name}");
    let shown = match host {
        Some(host) => format!("{host}/{full}"),
        None => full.clone(),
    };
    let view = |full: &str| -> Result<Result<serde_json::Value, String>, String> {
        let output = gh(&["repo", "view", full, "--json", "visibility,sshUrl,url"])?;
        if !output.status.success() {
            return Ok(Err(gh_error(&output)));
        }
        serde_json::from_slice(&output.stdout)
            .map(Ok)
            .map_err(|error| format!("gh repo view printed something unexpected: {error}"))
    };
    let (repository, created) = match view(&full) {
        Ok(Ok(repository)) => (repository, false),
        Ok(Err(missing)) => {
            match gh(&["repo", "create", &full, "--private"]) {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    return refused(
                        "backup",
                        &format!(
                            "could not create {shown}: {} (gh repo view said: {missing})",
                            gh_error(&output)
                        ),
                    )
                }
                Err(error) => return refused("backup", &error),
            }
            match view(&full) {
                Ok(Ok(repository)) => (repository, true),
                Ok(Err(error)) => {
                    return refused(
                        "backup",
                        &format!("created {shown}, but gh cannot read it: {error}"),
                    )
                }
                Err(error) => return refused("backup", &error),
            }
        }
        Err(error) => return refused("backup", &error),
    };
    let visibility = repository
        .get("visibility")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_ascii_uppercase();
    let internal_note = match visibility.as_str() {
        "PRIVATE" => None,
        "INTERNAL" => Some(format!(
            "{shown} is internal: everyone in its enterprise can read it, so treat it like a team repository.\n"
        )),
        _ => {
            return refused(
                "backup",
                &format!("{shown} is not private: backups hold your task notes. Make it private, or pick another name with `tsk backup setup --repo <name>`."),
            )
        }
    };
    let ssh = gh(&["config", "get", "git_protocol"])
        .ok()
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "ssh");
    let https = repository
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(|url| format!("{}.git", url.trim_end_matches(".git")));
    let ssh_url = repository
        .get("sshUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let Some(url) = (if ssh { ssh_url.clone() } else { https.clone() }) else {
        return refused("backup", "gh did not report the repository's clone URL");
    };
    let configured = backup::load_targets(state_dir).unwrap_or_default();
    if configured.iter().any(|target| {
        Some(&target.location) == ssh_url.as_ref()
            || Some(&target.location) == https.as_ref()
            || backup::absolute_path(&url).as_ref() == Some(&target.location)
    }) {
        return output(
            format!("already backing up to {shown} ({})\n", terminal_text(&url)),
            String::new(),
            0,
        );
    }
    // Reuse only a repository tsk could have made: empty, or holding backups already.
    if !created {
        match backup::repo_contents(state_dir, &url) {
            Ok(backup::RepoContents::Other) => {
                return refused(
                    "backup",
                    &format!("{shown} has other content; use `tsk backup setup --repo <name>` for a repository of its own"),
                )
            }
            Ok(_) => {}
            Err(error) => {
                return refused("backup", &format!("could not read {shown}: {error}"))
            }
        }
    }
    let mut result = backup_add(state_dir, &url, None);
    let lead = if created {
        format!("created private repository {shown}\n")
    } else {
        format!("using private repository {shown}\n")
    };
    result.stdout = lead + internal_note.as_deref().unwrap_or("") + &result.stdout;
    if result.code == 0 {
        result.stdout.push_str(
            "Each save pushes tsk.json and config.toml; values under [agent.<name>.env] are never pushed.\n",
        );
    }
    if result.code == 0 {
        // A new machine: point at the versions other machines left, never restore by itself.
        let this = backup::machine_id(state_dir).unwrap_or_default();
        let empty = std::fs::read(state_dir.join("tsk.json"))
            .ok()
            .and_then(|bytes| backup::count_tasks(&bytes))
            .is_none_or(|count| count == 0);
        if empty {
            if let Ok((versions, _)) = backup::list_versions(state_dir, false) {
                let mut others: Vec<&str> = versions
                    .iter()
                    .filter(|version| version.machine_id != this)
                    .map(|version| version.machine.as_str())
                    .collect();
                others.sort_unstable();
                others.dedup();
                if !others.is_empty() {
                    result.stdout.push_str(&format!(
                        "{shown} already holds backups from {}: run `tsk restore` to bring one back.\n",
                        terminal_text(&others.join(", "))
                    ));
                }
            }
        }
    }
    result
}

fn backup_add(state_dir: &std::path::Path, location: &str, every: Option<Every>) -> CliOutput {
    // A local repository path is stored absolute so it works from any directory.
    let location = backup::absolute_path(location).unwrap_or_else(|| location.to_string());
    match backup::add_target(state_dir, &location, every) {
        Ok((target, outcome)) => {
            let verb = match outcome {
                AddOutcome::Added => "added",
                AddOutcome::Updated => "updated",
                AddOutcome::Unchanged => "already have",
            };
            let mut stdout = format!(
                "{verb} target {} ({})\n",
                terminal_text(&target.location),
                target.every.phrase()
            );
            if outcome == AddOutcome::Added {
                stdout.push_str("Backups contain your task notes: keep the target private.\n");
            }
            // Test the target now with a first backup, so a wrong URL or missing access
            // shows here rather than at the first restore.
            let index = backup::load_targets(state_dir)
                .ok()
                .and_then(|targets| targets.iter().position(|t| t.location == target.location))
                .unwrap_or(0);
            let result = backup::run(state_dir, RunMode::One(index), SystemTime::now()).and_then(
                |mut results| {
                    results
                        .pop()
                        .map(|(_, result)| result)
                        .unwrap_or(Err("the target is no longer in config.toml".into()))
                },
            );
            match result {
                Ok(outcome) => {
                    stdout.push_str(&match outcome {
                        RunOutcome::Written(place) => {
                            format!("backed up to {}\n", terminal_text(&place))
                        }
                        RunOutcome::UpToDate => "the target is up to date\n".into(),
                        RunOutcome::NoStore => {
                            "reached the repository; nothing to back up yet\n".into()
                        }
                        RunOutcome::NotDue => String::new(),
                    });
                    // Backups after this one run detached from the board; check that such a
                    // run reaches the target too.
                    let stderr = match backup::probe_detached(state_dir, index) {
                        Probe::Failed(error) => format!(
                            "tsk backup: this backup worked, but a background backup (as a save starts one) could not reach {}: {}\nBackground backups have no terminal: git must reach the target without a prompt (an ssh-agent or credential helper the board's shell can see).\n",
                            terminal_text(&target.location),
                            terminal_text(&error),
                        ),
                        Probe::Reached | Probe::NoAnswer => String::new(),
                    };
                    output(stdout, stderr, 0)
                }
                Err(error) => output(
                    stdout,
                    format!(
                        "tsk backup: the first backup to {} failed: {}\nThe target is saved. Fix the URL or your git access, then run `tsk backup`.\n",
                        terminal_text(&target.location),
                        terminal_text(&error),
                    ),
                    1,
                ),
            }
        }
        Err(error) => refused("backup", &error),
    }
}

fn backup_now(state_dir: &std::path::Path) -> CliOutput {
    let results = match backup::run(state_dir, RunMode::Now, SystemTime::now()) {
        Ok(results) => results,
        Err(error) => return refused("backup", &error),
    };
    if results.is_empty() {
        return refused(
            "backup",
            "no backup targets; add one with tsk backup add <git-url>",
        );
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    for (target, result) in results {
        let location = terminal_text(&target.location);
        match result {
            Ok(RunOutcome::Written(place)) => {
                stdout.push_str(&format!("backed up to {}\n", terminal_text(&place)))
            }
            Ok(RunOutcome::UpToDate) => stdout.push_str(&format!("{location} is up to date\n")),
            Ok(RunOutcome::NoStore) => {
                stdout.push_str(&format!("nothing to back up to {location}: no tasks yet\n"))
            }
            Ok(RunOutcome::NotDue) => {}
            Err(error) => stderr.push_str(&format!(
                "tsk backup: {location}: {}\n",
                terminal_text(&error)
            )),
        }
    }
    let code = if stderr.is_empty() { 0 } else { 1 };
    output(stdout, stderr, code)
}

fn backup_list(state_dir: &std::path::Path) -> CliOutput {
    let listed = match backup::statuses(state_dir) {
        Ok(listed) => listed,
        Err(error) => return refused("backup", &error),
    };
    if listed.is_empty() {
        return output(
            "no backup targets; add one with tsk backup add <git-url>\n".into(),
            String::new(),
            0,
        );
    }
    let mut stdout = String::new();
    for (index, (target, status)) in listed.iter().enumerate() {
        stdout.push_str(&format!(
            "{}. {} ({})\n",
            index + 1,
            terminal_text(&target.location),
            target.every.phrase()
        ));
        let last = status
            .last_success
            .map(backup::human_time)
            .unwrap_or_else(|| "never".into());
        stdout.push_str(&format!("   last backup: {last}\n"));
        if let Some(error) = &status.last_error {
            let when = status
                .last_attempt
                .map(backup::human_time)
                .unwrap_or_default();
            stdout.push_str(&format!(
                "   last error ({when}): {}\n",
                terminal_text(error)
            ));
        }
    }
    output(stdout, String::new(), 0)
}

/// `tsk restore [<version>] [--all]`.
pub fn run_restore<R: Read>(args: Vec<String>, stdin: &mut R, stdin_is_tty: bool) -> CliOutput {
    let (rest, state_dir, help) = match common_flags(&args) {
        Ok(parsed) => parsed,
        Err(reason) => return presenter::restore_usage(&reason),
    };
    if help {
        return presenter::restore_help();
    }
    let mut all = false;
    let mut wanted = None;
    for word in &rest {
        match word.as_str() {
            "--all" => all = true,
            flag if flag.starts_with('-') => {
                return presenter::restore_usage(&format!("unknown restore argument {flag}"))
            }
            id if wanted.is_none() => wanted = Some(id.to_string()),
            extra => {
                return presenter::restore_usage(&format!("unexpected restore argument {extra}"))
            }
        }
    }
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    match backup::load_targets(&state_dir) {
        Ok(targets) if targets.is_empty() => {
            return refused(
                "restore",
                "no backup targets; add the one holding your backups with tsk backup add <git-url>",
            )
        }
        Ok(_) => {}
        Err(error) => return refused("restore", &error),
    }
    let (versions, failures) = match backup::list_versions(&state_dir, all) {
        Ok(listed) => listed,
        Err(error) => return refused("restore", &error),
    };
    let mut stderr = String::new();
    for (target, error) in &failures {
        stderr.push_str(&format!(
            "tsk restore: could not read {}: {}\n",
            terminal_text(&target.location),
            terminal_text(error)
        ));
    }
    if let Some(id) = wanted {
        let Some(version) = versions.iter().find(|version| version.id == id) else {
            stderr.push_str(&format!(
                "tsk restore: unknown version {}; run tsk restore to list them\n",
                terminal_text(&id)
            ));
            if looks_like_task(&id) {
                stderr.push_str(&format!(
                    "did you mean `tsk trash restore {}`?\n",
                    terminal_text(&id)
                ));
            }
            return output(String::new(), stderr, 1);
        };
        let mut result = restore_version(&state_dir, version);
        result.stderr = stderr + &result.stderr;
        return result;
    }
    if versions.is_empty() {
        stderr.push_str("tsk restore: no backed-up versions found\n");
        return output(String::new(), stderr, 1);
    }
    let table = version_table(&versions);
    let interactive = stdin_is_tty && std::io::stderr().is_terminal();
    if !interactive {
        return output(
            format!("{table}Restore one with tsk restore <id>.\n"),
            stderr,
            0,
        );
    }
    let mut terminal = std::io::stderr();
    let _ = write!(
        terminal,
        "{stderr}{table}Restore which? Number or id, Enter to cancel: "
    );
    let _ = terminal.flush();
    let mut answer = String::new();
    if BufReader::new(stdin).read_line(&mut answer).is_err() {
        return output(
            String::new(),
            "tsk restore: could not read the answer\n".into(),
            1,
        );
    }
    let answer = answer.trim();
    if answer.is_empty() {
        return output("nothing restored\n".into(), String::new(), 0);
    }
    let picked = answer
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| versions.get(index))
        .or_else(|| versions.iter().find(|version| version.id == answer));
    match picked {
        Some(version) => restore_version(&state_dir, version),
        None => refused("restore", &format!("unknown version {answer}")),
    }
}

/// `T12`, `t12`, or `12`: a task address, never a version id (eight hex characters).
fn looks_like_task(word: &str) -> bool {
    let digits = word
        .strip_prefix('T')
        .or_else(|| word.strip_prefix('t'))
        .unwrap_or(word);
    !digits.is_empty() && digits.len() < 8 && digits.bytes().all(|b| b.is_ascii_digit())
}

fn version_table(versions: &[Version]) -> String {
    let rows: Vec<[String; 6]> = versions
        .iter()
        .enumerate()
        .map(|(index, version)| {
            [
                format!("{}", index + 1),
                version.id.clone(),
                backup::human_time(version.at),
                terminal_text(&version.machine),
                version
                    .tasks
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "?".into()),
                terminal_text(&version.target),
            ]
        })
        .collect();
    let header = ["#", "id", "time", "machine", "tasks", "target"].map(str::to_string);
    let mut widths = [0usize; 6];
    for row in std::iter::once(&header).chain(rows.iter()) {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut table = String::new();
    for row in std::iter::once(&header).chain(rows.iter()) {
        let line = row
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:width$}"))
            .collect::<Vec<_>>()
            .join("  ");
        table.push_str(line.trim_end());
        table.push('\n');
    }
    table
}

fn restore_version(state_dir: &std::path::Path, version: &Version) -> CliOutput {
    let bytes = match backup::read_version(version) {
        Ok(bytes) => bytes,
        Err(error) => return refused("restore", &error),
    };
    match TaskStore::new(state_dir).restore_document(&bytes, SystemTime::now()) {
        Ok(kept) => {
            let tasks = version
                .tasks
                .map(|count| format!("{count} tasks"))
                .unwrap_or_else(|| "the store".into());
            let mut stdout = format!(
                "restored {} ({tasks} from {}, {})\n",
                version.id,
                terminal_text(&version.machine),
                backup::human_time(version.at)
            );
            if let Some(kept) = kept {
                stdout.push_str(&format!(
                    "the store it replaced is saved as {}\n",
                    terminal_text(&kept.display().to_string())
                ));
            }
            // Restore never touches config.toml: say where the backed-up one is.
            if let Some(config) = backup::version_config(version) {
                stdout.push_str(&format!(
                    "config.toml was not changed; the one backed up with this version is at {}\n",
                    terminal_text(&config)
                ));
            }
            output(stdout, String::new(), 0)
        }
        Err(RestoreError::Store(error)) => output(
            String::new(),
            format!("tsk restore: {}\n", terminal_text(&error.to_string())),
            3,
        ),
        Err(error) => refused("restore", &error.to_string()),
    }
}
