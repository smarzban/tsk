//! Headless `backup` and `restore`.

use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::SystemTime;

use super::{presenter, CliOutput};
use crate::backup::{self, AddOutcome, Every, Kind, RunMode, RunOutcome, Trigger, Version};
use crate::store::{default_state_dir, RestoreError, TaskStore};
use crate::ui::terminal_text;

/// Parsed `backup` input.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BackupAction {
    Now,
    Add {
        location: String,
        every: Option<Every>,
        kind: Option<Kind>,
    },
    List,
    Remove {
        target: String,
    },
    /// Internal: the detached child a save or board open starts.
    Run {
        trigger: Trigger,
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
            let mut kind = None;
            while let Some(word) = words.next() {
                match word {
                    "--every" => {
                        let value = words.next().ok_or("missing value for --every")?;
                        every = Some(Every::parse(value).ok_or_else(|| {
                            format!("--every {value} is not save, open, daily, or twice-daily")
                        })?);
                    }
                    "--git" | "--folder" => {
                        let chosen = if word == "--git" {
                            Kind::Git
                        } else {
                            Kind::Folder
                        };
                        if kind
                            .replace(chosen)
                            .is_some_and(|previous| previous != chosen)
                        {
                            return Err("--folder and --git conflict".into());
                        }
                    }
                    flag if flag.starts_with("--every=") => {
                        let value = &flag["--every=".len()..];
                        every = Some(Every::parse(value).ok_or_else(|| {
                            format!("--every {value} is not save, open, daily, or twice-daily")
                        })?);
                    }
                    flag if flag.starts_with('-') => {
                        return Err(format!("unknown backup argument {flag}"))
                    }
                    value if location.is_none() => location = Some(value.to_string()),
                    value => return Err(format!("unexpected backup argument {value}")),
                }
            }
            BackupAction::Add {
                location: location.ok_or("folder or git URL is required")?,
                every,
                kind,
            }
        }
        Some("run") => {
            let trigger = match (words.next(), words.next()) {
                (Some("--on"), Some(word)) => Trigger::parse(word),
                _ => None,
            }
            .ok_or("run takes --on save or --on open")?;
            BackupAction::Run { trigger }
        }
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
        BackupAction::Now => backup_now(&state_dir),
        BackupAction::List => backup_list(&state_dir),
        BackupAction::Add {
            location,
            every,
            kind,
        } => backup_add(&state_dir, &location, every, kind),
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

fn backup_add(
    state_dir: &std::path::Path,
    location: &str,
    every: Option<Every>,
    kind: Option<Kind>,
) -> CliOutput {
    let kind = kind.unwrap_or_else(|| {
        if backup::looks_like_git(location) {
            Kind::Git
        } else {
            Kind::Folder
        }
    });
    let location = match kind {
        Kind::Folder => match backup::absolute_path(location) {
            Some(path) => path,
            None => return refused("backup", &format!("folder {location} not found")),
        },
        // A local repository path is stored absolute so it works from any directory.
        Kind::Git => backup::absolute_path(location).unwrap_or_else(|| location.to_string()),
    };
    match backup::add_target(state_dir, kind, &location, every) {
        Ok((target, outcome)) => {
            let verb = match outcome {
                AddOutcome::Added => "added",
                AddOutcome::Updated => "updated",
                AddOutcome::Unchanged => "already have",
            };
            let mut stdout = format!(
                "{verb} {} target {} (every {})\n",
                target.kind.word(),
                terminal_text(&target.location),
                target.every.word()
            );
            if outcome == AddOutcome::Added {
                stdout.push_str("Backups contain your task notes: keep the target private.\n");
                stdout.push_str("Run `tsk backup` to back up now.\n");
            }
            output(stdout, String::new(), 0)
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
            "no backup targets; add one with tsk backup add <folder|git-url>",
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
            "no backup targets; add one with tsk backup add <folder|git-url>\n".into(),
            String::new(),
            0,
        );
    }
    let mut stdout = String::new();
    for (index, (target, status)) in listed.iter().enumerate() {
        stdout.push_str(&format!(
            "{}. {} {} (every {})\n",
            index + 1,
            target.kind.word(),
            terminal_text(&target.location),
            target.every.word()
        ));
        let last = status
            .last_success
            .map(backup::human_time)
            .unwrap_or_else(|| "never".into());
        stdout.push_str(&format!("   last backup: {last}\n"));
        if target.kind == Kind::Folder {
            stdout.push_str(&format!(
                "   keeps: the newest {} and one a day for {} days\n",
                target.keep_last, target.keep_days
            ));
        }
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
                "no backup targets; add the one holding your backups with tsk backup add <folder|git-url>",
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
