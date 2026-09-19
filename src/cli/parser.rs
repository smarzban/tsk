//! Argument parsing for the `add` command.

use std::path::PathBuf;

use uuid::Uuid;

use super::steps::StepsAction;
use crate::domain::{normalize_thread, thread_refusal_message, HumanStatus};

/// A direct task operand, either the internal UUID or its human task number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAddress {
    Id(Uuid),
    Number(u64),
}

impl TaskAddress {
    /// A notice row is board-only, so no CLI address reaches it, not even its UUID.
    pub fn matches(self, task: &crate::domain::Task) -> bool {
        !task.is_notice()
            && match self {
                Self::Id(id) => task.id == id,
                Self::Number(number) => task.number == Some(number),
            }
    }

    pub fn display(self) -> String {
        match self {
            Self::Number(number) => format!("T{number}"),
            Self::Id(id) => id.to_string(),
        }
    }
}

/// Parse a task UUID, bare-decimal human number, or the displayed `T<number>` form.
pub fn parse_task_address(value: &str) -> Result<TaskAddress, String> {
    let number = value
        .strip_prefix('T')
        .or_else(|| value.strip_prefix('t'))
        .unwrap_or(value);
    if !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) {
        return number
            .parse::<u64>()
            .map(TaskAddress::Number)
            .map_err(|_| format!("invalid task id {value}"));
    }
    Uuid::parse_str(value)
        .map(TaskAddress::Id)
        .map_err(|_| format!("invalid task id {value}"))
}

/// Parsed `dispatch` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagDispatch {
    pub task: Option<TaskAddress>,
    pub again: bool,
    pub state_dir: Option<PathBuf>,
    pub help: bool,
}

pub fn parse_flag_dispatch(args: &[String]) -> Result<FlagDispatch, String> {
    if args.get(1).map(String::as_str) != Some("dispatch") {
        return Err("expected dispatch command".into());
    }
    let mut parsed = FlagDispatch {
        task: None,
        again: false,
        state_dir: None,
        help: false,
    };
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        let value = |name: &str| match args.get(index + 1) {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        match flag {
            "--again" => {
                parsed.again = true;
                index += 1;
            }
            "--help" => {
                parsed.help = true;
                index += 1;
            }
            flag if flag.starts_with("--state-dir=") => {
                parsed.state_dir = Some(PathBuf::from(&flag["--state-dir=".len()..]));
                index += 1;
            }
            "--state-dir" => {
                parsed.state_dir = Some(PathBuf::from(value(flag)?));
                index += 2;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown dispatch argument {flag}"))
            }
            value => {
                if parsed.task.is_some() {
                    return Err(format!("unexpected dispatch argument {value}"));
                }
                parsed.task = Some(parse_task_address(value)?);
                index += 1;
            }
        }
    }
    Ok(parsed)
}

/// Parsed `trash` input. Positionals are the action (`restore`) and the task address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagTrash {
    pub action: Option<TrashAction>,
    pub state_dir: Option<PathBuf>,
    pub help: bool,
}

/// The parsed `trash` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashAction {
    Restore { target: TaskAddress },
}

/// Parse `tsk trash` arguments, including argv0 and the `trash` subcommand.
/// One project archive action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectAction {
    Archive { name: String },
    Unarchive { name: String },
}

/// Parsed `tsk project archive|unarchive <name>` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagProject {
    pub action: Option<ProjectAction>,
    pub state_dir: Option<PathBuf>,
    pub help: bool,
}

/// Parse `tsk project <action> <name>` arguments, including argv0. Mirrors the
/// two-positional shape of `parse_flag_trash`.
pub fn parse_flag_project(args: &[String]) -> Result<FlagProject, String> {
    if args.get(1).map(String::as_str) != Some("project") {
        return Err("expected project command".into());
    }

    let mut parsed = FlagProject {
        action: None,
        state_dir: None,
        help: false,
    };
    let mut positionals: Vec<&str> = Vec::new();
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        let value = |name: &str| match args.get(index + 1) {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        match flag {
            "--help" => {
                parsed.help = true;
                index += 1;
            }
            flag if flag.starts_with("--state-dir=") => {
                parsed.state_dir = Some(PathBuf::from(flag["--state-dir=".len()..].to_owned()));
                index += 1;
            }
            "--state-dir" => {
                parsed.state_dir = Some(PathBuf::from(value(flag)?));
                index += 2;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown project argument {flag}"))
            }
            positional => {
                if positionals.len() == 2 {
                    return Err(format!("unexpected project argument {positional}"));
                }
                positionals.push(positional);
                index += 1;
            }
        }
    }

    if parsed.help {
        return Ok(parsed);
    }
    match positionals.as_slice() {
        [] => {}
        ["archive", name] => {
            parsed.action = Some(ProjectAction::Archive {
                name: (*name).to_string(),
            });
        }
        ["unarchive", name] => {
            parsed.action = Some(ProjectAction::Unarchive {
                name: (*name).to_string(),
            });
        }
        [action, _] => return Err(format!("unknown project action {action}")),
        [action] => {
            return Err(match *action {
                "archive" | "unarchive" => "project name is required".into(),
                other => format!("unknown project action {other}"),
            })
        }
        _ => unreachable!("positionals are capped at two"),
    }
    Ok(parsed)
}

/// Parsed `archive` / `unarchive` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagArchive {
    pub task: Option<TaskAddress>,
    pub state_dir: Option<PathBuf>,
    pub help: bool,
}

/// Parse `tsk archive <task>` / `tsk unarchive <task>` arguments, including argv0.
pub fn parse_flag_archive(args: &[String], verb: &str) -> Result<FlagArchive, String> {
    if args.get(1).map(String::as_str) != Some(verb) {
        return Err(format!("expected {verb} command"));
    }

    let mut parsed = FlagArchive {
        task: None,
        state_dir: None,
        help: false,
    };
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        let value = |name: &str| match args.get(index + 1) {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        match flag {
            "--help" => {
                parsed.help = true;
                index += 1;
            }
            flag if flag.starts_with("--state-dir=") => {
                parsed.state_dir = Some(PathBuf::from(flag["--state-dir=".len()..].to_owned()));
                index += 1;
            }
            "--state-dir" => {
                parsed.state_dir = Some(PathBuf::from(value(flag)?));
                index += 2;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown {verb} argument {flag}")),
            flag => {
                if parsed.task.is_some() {
                    return Err(format!("unknown {verb} argument {flag}"));
                }
                parsed.task = Some(parse_task_address(flag)?);
                index += 1;
            }
        }
    }
    Ok(parsed)
}

pub fn parse_flag_trash(args: &[String]) -> Result<FlagTrash, String> {
    if args.get(1).map(String::as_str) != Some("trash") {
        return Err("expected trash command".into());
    }

    let mut parsed = FlagTrash {
        action: None,
        state_dir: None,
        help: false,
    };
    let mut positionals: Vec<&str> = Vec::new();
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        let value = |name: &str| match args.get(index + 1) {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        match flag {
            "--help" => {
                parsed.help = true;
                index += 1;
            }
            flag if flag.starts_with("--state-dir=") => {
                parsed.state_dir = Some(PathBuf::from(flag["--state-dir=".len()..].to_owned()));
                index += 1;
            }
            "--state-dir" => {
                parsed.state_dir = Some(PathBuf::from(value(flag)?));
                index += 2;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown trash argument {flag}")),
            positional => {
                if positionals.len() == 2 {
                    return Err(format!("unexpected trash argument {positional}"));
                }
                positionals.push(positional);
                index += 1;
            }
        }
    }

    if parsed.help {
        return Ok(parsed);
    }
    match positionals.as_slice() {
        [] => {}
        ["restore", address] => {
            parsed.action = Some(TrashAction::Restore {
                target: parse_task_address(address)?,
            });
        }
        [action, _] => return Err(format!("unknown trash action {action}")),
        [action] => {
            return Err(match *action {
                "restore" => "task id is required".into(),
                other => format!("unknown trash action {other}"),
            })
        }
        _ => unreachable!("positionals are capped at two"),
    }
    Ok(parsed)
}

/// Parsed `status` input. Positionals are the task address then the status name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagStatus {
    pub task: Option<TaskAddress>,
    pub status: Option<HumanStatus>,
    pub state_dir: Option<PathBuf>,
    pub help: bool,
}

/// Parse `tsk status <task> <status>` arguments, including argv0.
pub fn parse_flag_status(args: &[String]) -> Result<FlagStatus, String> {
    if args.get(1).map(String::as_str) != Some("status") {
        return Err("expected status command".into());
    }

    let mut parsed = FlagStatus {
        task: None,
        status: None,
        state_dir: None,
        help: false,
    };
    let mut positionals: Vec<&str> = Vec::new();
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        let value = |name: &str| match args.get(index + 1) {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        match flag {
            "--help" => {
                parsed.help = true;
                index += 1;
            }
            flag if flag.starts_with("--state-dir=") => {
                parsed.state_dir = Some(PathBuf::from(flag["--state-dir=".len()..].to_owned()));
                index += 1;
            }
            "--state-dir" => {
                parsed.state_dir = Some(PathBuf::from(value(flag)?));
                index += 2;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown status argument {flag}")),
            positional => {
                if positionals.len() == 2 {
                    return Err(format!("unexpected status argument {positional}"));
                }
                positionals.push(positional);
                index += 1;
            }
        }
    }

    if parsed.help {
        return Ok(parsed);
    }
    match positionals.as_slice() {
        [] => {}
        [task] => {
            parsed.task = Some(parse_task_address(task)?);
        }
        [task, status] => {
            parsed.task = Some(parse_task_address(task)?);
            parsed.status = Some(parse_human_status(status)?);
        }
        _ => unreachable!("positionals are capped at two"),
    }
    Ok(parsed)
}

fn parse_human_status(value: &str) -> Result<HumanStatus, String> {
    match value {
        "open" => Ok(HumanStatus::Open),
        "ready" => Ok(HumanStatus::Ready),
        "start" | "started" => Ok(HumanStatus::Started),
        "blocked" => Ok(HumanStatus::Blocked),
        "review" => Ok(HumanStatus::Review),
        "done" => Ok(HumanStatus::Done),
        other => Err(format!("unknown status {other}")),
    }
}

/// Parsed `edit` input. One positional task address, then title/notes flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagEdit {
    pub task: Option<TaskAddress>,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub assignee: Option<String>,
    pub unassign: bool,
    pub state_dir: Option<PathBuf>,
    pub help: bool,
}

/// Parse `tsk edit <task> [--title <title>] [--notes <notes>]` arguments, including argv0.
pub fn parse_flag_edit(args: &[String]) -> Result<FlagEdit, String> {
    if args.get(1).map(String::as_str) != Some("edit") {
        return Err("expected edit command".into());
    }

    let mut parsed = FlagEdit {
        task: None,
        title: None,
        notes: None,
        assignee: None,
        unassign: false,
        state_dir: None,
        help: false,
    };
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        let value = |name: &str| match args.get(index + 1) {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        match flag {
            flag if flag.starts_with("--title=") => {
                parsed.title = Some(flag["--title=".len()..].to_owned());
                index += 1;
            }
            flag if flag.starts_with("--notes=") => {
                parsed.notes = Some(flag["--notes=".len()..].to_owned());
                index += 1;
            }
            "-t" | "--title" => {
                parsed.title = Some(value(flag)?);
                index += 2;
            }
            "-n" | "--notes" => {
                parsed.notes = Some(value(flag)?);
                index += 2;
            }
            flag if flag.starts_with("--assignee=") => {
                parsed.assignee = Some(normalize_thread(&flag["--assignee=".len()..]).map_err(
                    |error| format!("invalid agent name · {}", thread_refusal_message(error)),
                )?);
                index += 1;
            }
            "--assignee" => {
                parsed.assignee = Some(normalize_thread(&value(flag)?).map_err(|error| {
                    format!("invalid agent name · {}", thread_refusal_message(error))
                })?);
                index += 2;
            }
            "--unassign" => {
                parsed.unassign = true;
                index += 1;
            }
            "--help" => {
                parsed.help = true;
                index += 1;
            }
            flag if flag.starts_with("--state-dir=") => {
                parsed.state_dir = Some(PathBuf::from(flag["--state-dir=".len()..].to_owned()));
                index += 1;
            }
            "--state-dir" => {
                parsed.state_dir = Some(PathBuf::from(value(flag)?));
                index += 2;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown edit argument {flag}")),
            flag => {
                if parsed.task.is_some() {
                    return Err(format!("unknown edit argument {flag}"));
                }
                parsed.task = Some(parse_task_address(flag)?);
                index += 1;
            }
        }
    }
    if parsed.unassign && parsed.assignee.is_some() {
        return Err("--unassign cannot be used with --assignee".into());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::{
        parse_flag_edit, parse_flag_status, parse_flag_steps, parse_flag_trash, parse_task_address,
        FlagEdit, FlagStatus, TaskAddress, TrashAction,
    };
    use crate::cli::steps::StepsAction;
    use crate::domain::HumanStatus;

    #[test]
    fn task_addresses_accept_the_displayed_identifier_case_insensitively() {
        assert_eq!(parse_task_address("T30"), Ok(TaskAddress::Number(30)));
        assert_eq!(parse_task_address("t30"), Ok(TaskAddress::Number(30)));
        assert_eq!(parse_task_address("30"), Ok(TaskAddress::Number(30)));
        assert_eq!(TaskAddress::Number(30).display(), "T30");
        assert!(parse_task_address("T-30").is_err());
    }

    #[test]
    fn trash_parse_accepts_restore_with_number_and_flags() {
        let parsed = parse_flag_trash(&[
            "tsk".into(),
            "trash".into(),
            "restore".into(),
            "T7".into(),
            "--state-dir".into(),
            "/tmp/dir".into(),
        ])
        .expect("parse");
        assert_eq!(
            parsed.action,
            Some(TrashAction::Restore {
                target: TaskAddress::Number(7)
            })
        );
        assert_eq!(
            parsed.state_dir.as_deref(),
            Some(std::path::Path::new("/tmp/dir"))
        );

        let parsed =
            parse_flag_trash(&["tsk".into(), "trash".into(), "--help".into()]).expect("parse help");
        assert!(parsed.help);

        assert!(parse_flag_trash(&["tsk".into(), "trash".into(), "restore".into()]).is_err());
        assert!(
            parse_flag_trash(&["tsk".into(), "trash".into(), "bogus".into(), "T1".into()]).is_err()
        );
        assert!(parse_flag_trash(&[
            "tsk".into(),
            "trash".into(),
            "restore".into(),
            "T1".into(),
            "extra".into()
        ])
        .is_err());
    }

    #[test]
    fn status_parse_accepts_task_and_status() {
        let parsed = parse_flag_status(&[
            "tsk".into(),
            "status".into(),
            "T4".into(),
            "blocked".into(),
            "--state-dir".into(),
            "/tmp/dir".into(),
        ])
        .expect("parse");
        assert_eq!(
            parsed,
            FlagStatus {
                task: Some(TaskAddress::Number(4)),
                status: Some(HumanStatus::Blocked),
                state_dir: Some(std::path::PathBuf::from("/tmp/dir")),
                help: false,
            }
        );
        assert!(
            parse_flag_status(&["tsk".into(), "status".into(), "T4".into(), "nope".into()])
                .is_err()
        );
        assert!(parse_flag_status(&[
            "tsk".into(),
            "status".into(),
            "T4".into(),
            "blocked".into(),
            "extra".into()
        ])
        .is_err());
        let parsed =
            parse_flag_status(&["tsk".into(), "status".into(), "T4".into(), "start".into()])
                .expect("start alias");
        assert_eq!(parsed.status, Some(HumanStatus::Started));
        let parsed =
            parse_flag_status(&["tsk".into(), "status".into(), "T4".into(), "open".into()])
                .expect("open status");
        assert_eq!(parsed.status, Some(HumanStatus::Open));
    }

    #[test]
    fn edit_parse_accepts_equals_forms_for_dash_leading_values() {
        let parsed = parse_flag_edit(&[
            "tsk".into(),
            "edit".into(),
            "12".into(),
            "--title=-fix parser".into(),
            "--notes=-5 degrees".into(),
        ])
        .expect("parse");
        assert_eq!(
            parsed,
            FlagEdit {
                task: Some(TaskAddress::Number(12)),
                title: Some("-fix parser".into()),
                notes: Some("-5 degrees".into()),
                assignee: None,
                unassign: false,
                state_dir: None,
                help: false,
            }
        );
    }

    #[test]
    fn steps_parse_accepts_rename_and_remove() {
        let parsed = parse_flag_steps(&[
            "tsk".into(),
            "steps".into(),
            "T2".into(),
            "rename".into(),
            "ab".into(),
            "new text".into(),
        ])
        .expect("parse rename");
        assert_eq!(parsed.task, Some(TaskAddress::Number(2)));
        assert_eq!(
            parsed.action,
            Some(StepsAction::Rename {
                short_id: "ab".into(),
                text: "new text".into(),
            })
        );

        let parsed = parse_flag_steps(&[
            "tsk".into(),
            "steps".into(),
            "T2".into(),
            "remove".into(),
            "ab".into(),
        ])
        .expect("parse remove");
        assert_eq!(
            parsed.action,
            Some(StepsAction::Remove {
                short_id: "ab".into(),
            })
        );
        assert!(parse_flag_steps(&[
            "tsk".into(),
            "steps".into(),
            "T2".into(),
            "rename".into(),
            "ab".into()
        ])
        .is_err());
        assert!(parse_flag_steps(&[
            "tsk".into(),
            "steps".into(),
            "T2".into(),
            "add".into(),
            "one".into(),
            "two".into()
        ])
        .is_err());
    }
}

/// Parsed add input. A plan source is selected by `file` or piped stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagAdd {
    pub title: Option<String>,
    pub notes: Option<String>,
    pub project: Option<String>,
    /// Normalized at the argv boundary so add only receives valid thread names.
    pub thread: Option<String>,
    /// Normalized agent name, exact profile validation happens at execution.
    pub assignee: Option<String>,
    pub unassign: bool,
    pub global: bool,
    pub json: bool,
    pub state_dir: Option<PathBuf>,
    pub file: Option<PathBuf>,
    pub has_item_flags: bool,
    pub help: bool,
}

/// Parse `tsk add` arguments, including argv0 and the `add` subcommand.
pub fn parse_flag_add(args: &[String]) -> Result<FlagAdd, String> {
    if args.get(1).map(String::as_str) != Some("add") {
        return Err("expected add command".into());
    }

    let mut parsed = FlagAdd {
        title: None,
        notes: None,
        project: None,
        thread: None,
        assignee: None,
        unassign: false,
        global: false,
        json: false,
        state_dir: None,
        file: None,
        has_item_flags: false,
        help: false,
    };
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        let value = |name: &str| match args.get(index + 1) {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        let file_value = |name: &str| match args.get(index + 1) {
            Some(value) if value == "-" || !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        match flag {
            flag if flag.starts_with("--title=") => {
                parsed.title = Some(flag["--title=".len()..].to_owned());
                parsed.has_item_flags = true;
                index += 1;
            }
            flag if flag.starts_with("--notes=") => {
                parsed.notes = Some(flag["--notes=".len()..].to_owned());
                parsed.has_item_flags = true;
                index += 1;
            }
            flag if flag.starts_with("--project=") => {
                parsed.project = Some(flag["--project=".len()..].to_owned());
                parsed.has_item_flags = true;
                index += 1;
            }
            flag if flag.starts_with("--thread=") => {
                parsed.thread = Some(normalize_thread(&flag["--thread=".len()..]).map_err(
                    |error| format!("invalid thread name · {}", thread_refusal_message(error)),
                )?);
                parsed.has_item_flags = true;
                index += 1;
            }
            "-t" | "--title" => {
                parsed.title = Some(value(flag)?);
                parsed.has_item_flags = true;
                index += 2;
            }
            "-n" | "--notes" => {
                parsed.notes = Some(value(flag)?);
                parsed.has_item_flags = true;
                index += 2;
            }
            "-p" | "--project" => {
                parsed.project = Some(value(flag)?);
                parsed.has_item_flags = true;
                index += 2;
            }
            "--thread" => {
                parsed.thread = Some(normalize_thread(&value(flag)?).map_err(|error| {
                    format!("invalid thread name · {}", thread_refusal_message(error))
                })?);
                parsed.has_item_flags = true;
                index += 2;
            }
            flag if flag.starts_with("--assignee=") => {
                parsed.assignee = Some(normalize_thread(&flag["--assignee=".len()..]).map_err(
                    |error| format!("invalid agent name · {}", thread_refusal_message(error)),
                )?);
                parsed.has_item_flags = true;
                index += 1;
            }
            "--assignee" => {
                parsed.assignee = Some(normalize_thread(&value(flag)?).map_err(|error| {
                    format!("invalid agent name · {}", thread_refusal_message(error))
                })?);
                parsed.has_item_flags = true;
                index += 2;
            }
            "--unassign" => {
                parsed.unassign = true;
                parsed.has_item_flags = true;
                index += 1;
            }
            "--desk" => {
                parsed.global = true;
                parsed.has_item_flags = true;
                index += 1;
            }
            "--json" => {
                parsed.json = true;
                index += 1;
            }
            "--help" => {
                parsed.help = true;
                index += 1;
            }
            flag if flag.starts_with("--state-dir=") => {
                parsed.state_dir = Some(PathBuf::from(flag["--state-dir=".len()..].to_owned()));
                index += 1;
            }
            flag if flag.starts_with("--file=") => {
                parsed.file = Some(PathBuf::from(flag["--file=".len()..].to_owned()));
                index += 1;
            }
            "--state-dir" => {
                parsed.state_dir = Some(PathBuf::from(value(flag)?));
                index += 2;
            }
            "--file" => {
                parsed.file = Some(PathBuf::from(file_value(flag)?));
                index += 2;
            }
            _ => return Err(format!("unknown add argument {flag}")),
        }
    }

    if parsed.global && parsed.project.is_some() {
        return Err("--desk cannot be used with --project".into());
    }
    if parsed.unassign && parsed.assignee.is_some() {
        return Err("--unassign cannot be used with --assignee".into());
    }
    if parsed.has_item_flags && parsed.file.is_some() {
        return Err("item flags cannot be used with --file".into());
    }
    Ok(parsed)
}

/// Parsed `steps` input. Flags come first; the positionals are task id, action,
/// and the action's operand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagSteps {
    pub task: Option<TaskAddress>,
    pub action: Option<StepsAction>,
    pub state_dir: Option<PathBuf>,
    pub help: bool,
}

/// Parse `tsk steps` arguments, including argv0 and the `steps` subcommand.
///
/// Flags may appear anywhere; the positionals in order are task id, action, and
/// the action's operand.
pub fn parse_flag_steps(args: &[String]) -> Result<FlagSteps, String> {
    if args.get(1).map(String::as_str) != Some("steps") {
        return Err("expected steps command".into());
    }

    let mut parsed = FlagSteps {
        task: None,
        action: None,
        state_dir: None,
        help: false,
    };
    let mut positionals: Vec<&str> = Vec::new();
    let mut index = 2;
    while let Some(flag) = args.get(index).map(String::as_str) {
        let value = |name: &str| match args.get(index + 1) {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("missing value for {name}")),
        };
        match flag {
            "--help" => {
                parsed.help = true;
                index += 1;
            }
            flag if flag.starts_with("--state-dir=") => {
                parsed.state_dir = Some(PathBuf::from(flag["--state-dir=".len()..].to_owned()));
                index += 1;
            }
            "--state-dir" => {
                parsed.state_dir = Some(PathBuf::from(value(flag)?));
                index += 2;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown steps argument {flag}")),
            positional => {
                if positionals.len() == 4 {
                    return Err(format!("unexpected steps argument {positional}"));
                }
                positionals.push(positional);
                index += 1;
            }
        }
    }

    if !parsed.help {
        if positionals.len() < 2 {
            return Err(if positionals.is_empty() {
                "task id is required".into()
            } else {
                "steps action is required".into()
            });
        }
        let extra = |index: usize| {
            positionals
                .get(index)
                .copied()
                .map(|positional| format!("unexpected steps argument {positional}"))
        };
        let task = parse_task_address(positionals[0])?;
        parsed.action = Some(match positionals[1] {
            "add" => {
                if let Some(reason) = extra(3) {
                    return Err(reason);
                }
                StepsAction::Add {
                    text: positionals
                        .get(2)
                        .copied()
                        .map(str::to_owned)
                        .ok_or_else(|| "step text is required".to_owned())?,
                }
            }
            "toggle" => {
                if let Some(reason) = extra(3) {
                    return Err(reason);
                }
                StepsAction::Toggle {
                    short_id: positionals
                        .get(2)
                        .copied()
                        .map(str::to_owned)
                        .ok_or_else(|| "step short id is required".to_owned())?,
                }
            }
            "remove" => {
                if let Some(reason) = extra(3) {
                    return Err(reason);
                }
                StepsAction::Remove {
                    short_id: positionals
                        .get(2)
                        .copied()
                        .map(str::to_owned)
                        .ok_or_else(|| "step short id is required".to_owned())?,
                }
            }
            "rename" => {
                let short_id = positionals
                    .get(2)
                    .copied()
                    .map(str::to_owned)
                    .ok_or_else(|| "step short id is required".to_owned())?;
                let text = positionals
                    .get(3)
                    .copied()
                    .map(str::to_owned)
                    .ok_or_else(|| "step text is required".to_owned())?;
                StepsAction::Rename { short_id, text }
            }
            other => return Err(format!("unknown steps action {other}")),
        });
        parsed.task = Some(task);
    }
    Ok(parsed)
}
