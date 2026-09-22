//! Text output for headless command results.

use std::collections::BTreeMap;

use super::CliOutput;
use crate::cli::add::{AddError, FlagAddResult};
use crate::cli::archive::{ArchiveCliError, ArchiveResult, ProjectResult};
use crate::cli::edit::{EditError, EditResult};
use crate::cli::list::{ListError, ListResult, ListRow, ListView};
use crate::cli::parser::TaskAddress;
use crate::cli::status::{StatusError, StatusResult};
use crate::cli::steps::{StepLine, StepsError, StepsResult};
use crate::cli::trash::{TrashCliError, TrashRestoreResult};
use crate::dispatch::{
    BranchCleanup, CleanupError, CleanupResult, DispatchError, DispatchResult, WorktreeCleanup,
};
use crate::domain::HumanStatus;
use crate::ui::terminal_text;

fn human_reason(reason: &str) -> String {
    terminal_text(reason)
}

struct HelpDoc {
    usage: Vec<String>,
    purpose: String,
    groups: Vec<HelpGroup>,
    examples: Vec<String>,
    refusals: Vec<String>,
    exit: String,
}

struct HelpGroup {
    heading: String,
    options: Vec<(String, String)>,
}

impl HelpDoc {
    fn render(self) -> String {
        const WIDTH: usize = 80;
        let mut output = String::new();
        for (index, usage) in self.usage.iter().enumerate() {
            let prefix = if index == 0 { "usage: " } else { "       " };
            append_usage_wrapped(&mut output, prefix, "       ", usage, WIDTH);
        }
        output.push('\n');
        append_help_wrapped(&mut output, "", "", &self.purpose, WIDTH);
        output.push('\n');

        for group in self.groups.iter().filter(|group| !group.options.is_empty()) {
            output.push_str(&group.heading);
            output.push('\n');
            let width = group
                .options
                .iter()
                .map(|(flag, _)| flag.len())
                .max()
                .unwrap_or(0);
            for (flag, text) in &group.options {
                let prefix = format!("  {flag:width$}  ");
                let continuation = " ".repeat(prefix.len());
                append_help_wrapped(&mut output, &prefix, &continuation, text, WIDTH);
            }
            output.push('\n');
        }

        if !self.examples.is_empty() {
            output.push_str("Examples:\n");
            for example in &self.examples {
                append_help_wrapped(&mut output, "  ", "  ", example, WIDTH);
            }
        }
        if !self.refusals.is_empty() {
            output.push_str("\nRefusals (exit 1):\n");
            for refusal in &self.refusals {
                append_help_wrapped(&mut output, "  ", "  ", refusal, WIDTH);
            }
        }
        if !self.exit.is_empty() {
            output.push_str("\nExit:\n");
            append_help_wrapped(&mut output, "  ", "  ", &self.exit, WIDTH);
        }
        output
    }
}

fn help(doc: HelpDoc) -> CliOutput {
    CliOutput {
        stdout: doc.render(),
        stderr: String::new(),
        code: 0,
    }
}

fn group(heading: &str, options: &[(&str, &str)]) -> HelpGroup {
    HelpGroup {
        heading: heading.into(),
        options: options
            .iter()
            .map(|(flag, text)| ((*flag).into(), (*text).into()))
            .collect(),
    }
}

fn exit_line(success: &str, refusal: Option<&str>, store_io: bool) -> String {
    let mut clauses = vec![format!("0 {success}")];
    if let Some(refusal) = refusal {
        clauses.push(format!("1 {refusal}"));
    }
    clauses.push("2 usage, nothing persisted".into());
    if store_io {
        clauses.push("3 store I/O, verify with list".into());
    }
    clauses.join(" · ")
}

pub fn top_level_help() -> String {
    concat!(
        "usage: tsk [capture] | <command> [args] | help [<command>] | --help | --version\n\n",
        "Tasks\n",
        "  add      create one task or apply a JSON plan\n",
        "  list     inspect tasks\n",
        "  status   set a task's human status\n",
        "  dispatch hand a task to its assigned agent\n",
        "  clean    remove a dispatched worktree safely\n",
        "  edit     update a task's title or notes\n",
        "  steps    add, toggle, rename, or remove one step on a task\n\n",
        "Board\n",
        "  archive    keep a task off the working views\n",
        "  unarchive  put an archived task back\n",
        "  project    archive or unarchive a project\n",
        "  trash      restore a trashed task\n\n",
        "Setup\n",
        "  setup    register herdr, or install the agent skill\n",
        "  update   install the latest stable release\n",
        "  guide    print the agent workflow skill\n\n",
        "Statuses\n",
        "  open     captured, not yet picked (inbox)\n",
        "  ready    picked, up next (on deck)\n",
        "  started  in motion\n",
        "  blocked  waiting on something\n",
        "  review   done by the agent, waiting on you\n",
        "  done     closed\n\n",
        "Run `tsk help <command>` or `tsk <command> --help` for one command.\n",
        "Agents: run `tsk guide`, or read https://gettsk.sh/docs/agents.md\n"
    )
    .into()
}

pub fn help_usage(reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk help: {}\nusage: tsk help [<command>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

pub fn add_help() -> CliOutput {
    help(HelpDoc {
        usage: vec![
            "tsk add -t <title> [-n <notes>] [-p <project> | --desk] [--thread <name>] [--assignee <name> | --unassign] [--json] [--state-dir <dir>]".into(),
            "tsk add [--file <path|->] [--state-dir <dir>]".into(),
        ],
        purpose: "Create one task or apply a JSON plan.".into(),
        groups: vec![
            group("Scope", &[("-p, --project <project>", "create in a project"), ("--desk", "create on your desk")]),
            group("Output", &[("--json", "print one result object for a flag add")]),
            group("Values", &[("-t, --title <title>", "required task title"), ("-n, --notes <notes>", "optional notes"), ("--thread <name>", "optional normalized thread"), ("--assignee <name>", "assign a defined agent profile"), ("--unassign", "leave the task unassigned"), ("--file <path|->", "read a JSON plan from a file or stdin"), ("--state-dir <dir>", "use another board store"), ("--flag=<value>", "use equals syntax for dash-leading title, notes, project, state-dir, or file values")]),
        ],
        examples: vec!["tsk add -t \"Draft release notes\"".into(), "tsk add -t \"Buy milk\" --desk".into(), "tsk add -t \"Fix widget\" --project widget --thread release-2026".into(), "tsk add --file plan.json".into(), "cat plan.json | tsk add".into()],
        refusals: vec![
            "empty-title".into(),
            "invalid-title".into(),
            "invalid-thread (JSON plan)".into(),
            "invalid-item (JSON plan)".into(),
            "unknown-project".into(),
            "unknown-agent".into(),
            "project-archived".into(),
        ],
        exit: exit_line("every item was created or already existed", Some("one or more items refused, retry failed only"), true),
    })
}

pub fn list_help(_terminal_width: Option<usize>) -> CliOutput {
    help(HelpDoc {
        usage: vec!["tsk list [<task>] [-p <project> | --desk | --all] [--thread <name>] [--assignee <name>] [--open | --ready | --done | --deleted | --archived] [--json] [--state-dir <dir>]".into()],
        purpose: "Inspect tasks in the selected scope, or one task anywhere in the live store.".into(),
        groups: vec![
            group("Scope", &[("-p, --project <project>", "select a project"), ("--desk", "select your desk"), ("--all", "select every scope")]),
            group("Filters", &[("<task>", "a task number or UUID, not combined with filters"), ("--thread <name>", "filter within the selected scope"), ("--assignee <name>", "filter by exact assignee"), ("--open, --ready", "show inbox or picked on-deck tasks"), ("--done, --archived", "show done or archived tasks"), ("--deleted", "show soft-deleted and trashed tasks")]),
            group("Output", &[("--json", "print machine-readable task rows")]),
            group("Values", &[("--state-dir <dir>", "use another board store"), ("--project=<scope>", "use equals syntax for a dash-leading project or state-dir value")]),
        ],
        examples: vec!["tsk list".into(), "tsk list T12".into(), "tsk list --all --json".into(), "tsk list --archived --all".into()],
        refusals: Vec::new(),
        exit: exit_line("tasks listed", None, true),
    })
}

pub fn added(result: FlagAddResult, json: bool) -> CliOutput {
    let stdout = match result {
        FlagAddResult::Created {
            id,
            number,
            title,
            project,
            assignee,
        } if json => format!(
            "{}\n",
            serde_json::json!({
                "outcome": "created",
                "id": id,
                "number": number,
                "title": title,
                "project": project,
                "assignee": assignee,
            })
        ),
        FlagAddResult::Existing {
            id,
            number,
            title,
            project,
            assignee,
        } if json => format!(
            "{}\n",
            serde_json::json!({
                "outcome": "existing",
                "id": id,
                "number": number,
                "title": title,
                "project": project,
                "assignee": assignee,
            })
        ),
        FlagAddResult::Created { title, .. } => format!("added {}\n", terminal_text(&title)),
        FlagAddResult::Existing { .. } => "task already exists\n".into(),
    };
    CliOutput {
        stdout,
        stderr: String::new(),
        code: 0,
    }
}

pub fn plan(result: crate::cli::add::PlanResult) -> CliOutput {
    let code = if result.has_failures() { 1 } else { 0 };
    CliOutput {
        stdout: format!(
            "{}\n",
            serde_json::to_string(&result).expect("plan result is serializable")
        ),
        stderr: String::new(),
        code,
    }
}

pub fn usage(reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk add: {}\nusage: tsk add -t <title> [-n <notes>] [-p <project> | --desk] [--thread <name>] [--json] [--state-dir <dir>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

pub fn list(result: ListResult, json: bool, terminal_width: Option<usize>) -> CliOutput {
    let stdout = if json {
        list_json(&result)
    } else {
        list_human(&result, terminal_width)
    };
    CliOutput {
        stdout,
        stderr: String::new(),
        code: 0,
    }
}

/// Filtered listings keep the compact row schema. A directly addressed task
/// carries its complete readable content in a stable field order.
fn list_json(result: &ListResult) -> String {
    if let Some(direct) = result.direct.as_ref() {
        let row = result
            .rows
            .first()
            .expect("direct task details imply one task row");
        #[derive(serde::Serialize)]
        struct DirectRow<'a> {
            id: uuid::Uuid,
            number: u64,
            project: &'a Option<String>,
            status: HumanStatus,
            title: &'a str,
            notes: &'a Option<String>,
            steps: &'a [StepLine],
            assignee: &'a Option<String>,
            thread: &'a Option<String>,
        }
        let direct_row = DirectRow {
            id: row.id,
            number: row.number,
            project: &row.project,
            status: row.status,
            title: &row.title,
            notes: &direct.notes,
            steps: &direct.steps,
            assignee: &row.assignee,
            thread: &row.thread,
        };
        return format!(
            "{}\n",
            serde_json::to_string(&[direct_row]).expect("direct list row is serializable")
        );
    }

    let value = serde_json::to_value(&result.rows).expect("list rows are serializable");
    format!("{value}\n")
}

fn list_human(result: &ListResult, terminal_width: Option<usize>) -> String {
    let output_width = terminal_width.unwrap_or(usize::MAX);
    let groups: &[(Option<HumanStatus>, &str)] = match result.view {
        ListView::Open => &[
            (Some(HumanStatus::Started), "STARTED"),
            (Some(HumanStatus::Ready), "READY"),
            (Some(HumanStatus::Open), "OPEN"),
            (Some(HumanStatus::Blocked), "BLOCKED"),
            (Some(HumanStatus::Review), "REVIEW"),
        ],
        ListView::Done => &[(None, "DONE")],
        ListView::Deleted => &[(None, "DELETED")],
        ListView::Archived => &[(None, "ARCHIVED")],
    };
    let mut output = String::new();
    let labels = result.include_scope.then(|| scope_labels(&result.rows));
    for (status, heading) in groups {
        let rows = result
            .rows
            .iter()
            .filter(|row| status.is_none_or(|status| row.status == status))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        append_wrapped(&mut output, "", "", heading, output_width);
        if result.include_scope {
            append_scope_groups(
                &mut output,
                rows,
                labels.as_ref().expect("scope labels"),
                output_width,
            );
        } else {
            append_rows(
                &mut output,
                &rows,
                " ",
                output_width,
                result.direct.is_none(),
            );
            if let Some(direct) = result.direct.as_ref() {
                append_direct_details(
                    &mut output,
                    direct.notes.as_deref(),
                    &direct.steps,
                    rows[0].assignee.as_deref(),
                    rows[0].thread.as_deref(),
                    " ",
                    output_width,
                );
            }
        }
    }
    output
}

fn append_scope_groups(
    output: &mut String,
    rows: Vec<&ListRow>,
    labels: &BTreeMap<Option<String>, String>,
    output_width: usize,
) {
    let mut scopes = Vec::<(Option<&str>, Vec<&ListRow>)>::new();
    for row in rows {
        let scope = row.project.as_deref();
        match scopes
            .iter_mut()
            .find(|(group_scope, _)| *group_scope == scope)
        {
            Some((_, rows)) => rows.push(row),
            None => scopes.push((scope, vec![row])),
        }
    }
    for (scope, rows) in scopes {
        let label = terminal_text(
            labels
                .get(&scope.map(str::to_owned))
                .expect("label for displayed scope"),
        );
        append_wrapped(output, "  ", "  ", &label, output_width);
        append_rows(output, &rows, "    ", output_width, true);
    }
}

fn append_rows(
    output: &mut String,
    rows: &[&ListRow],
    indent: &str,
    output_width: usize,
    include_thread: bool,
) {
    for row in rows {
        let mut content = terminal_text(&row.title);
        if include_thread {
            if let Some(assignee) = row.assignee.as_deref() {
                content.push_str(" @");
                content.push_str(&terminal_text(assignee));
            }
            if let Some(thread) = row.thread.as_deref() {
                content.push_str(" #");
                content.push_str(&terminal_text(thread));
            }
        }
        if let Some(mark) = row.archived {
            content.push_str(" · ");
            content.push_str(mark);
        }
        let first_prefix = format!("{indent}- {} ", row.number);
        let continuation_prefix = " ".repeat(first_prefix.len());
        append_wrapped(
            output,
            &first_prefix,
            &continuation_prefix,
            &content,
            output_width,
        );
    }
}

/// Direct detail is an ordered set of present blocks. Blank rows separate only
/// adjacent blocks that exist: notes, steps, then thread.
fn append_direct_details(
    output: &mut String,
    notes: Option<&str>,
    steps: &[StepLine],
    assignee: Option<&str>,
    thread: Option<&str>,
    indent: &str,
    output_width: usize,
) {
    let mut has_prior = false;
    let detail_prefix = format!("{indent}  ");
    if let Some(notes) = notes {
        append_wrapped(output, &detail_prefix, &detail_prefix, notes, output_width);
        has_prior = true;
    }
    if !steps.is_empty() {
        if has_prior {
            output.push('\n');
        }
        append_step_lines(output, steps, indent, output_width);
        has_prior = true;
    }
    if let Some(assignee) = assignee {
        if has_prior {
            output.push('\n');
        }
        let assignee = terminal_text(&format!("@{assignee}"));
        append_wrapped(
            output,
            &detail_prefix,
            &detail_prefix,
            &assignee,
            output_width,
        );
        has_prior = true;
    }
    if let Some(thread) = thread {
        if has_prior {
            output.push('\n');
        }
        let thread = terminal_text(&format!("#{thread}"));
        append_wrapped(
            output,
            &detail_prefix,
            &detail_prefix,
            &thread,
            output_width,
        );
    }
}

/// One line per step: state and text, with continuation rows aligned under text.
fn append_step_lines(output: &mut String, steps: &[StepLine], indent: &str, output_width: usize) {
    for step in steps {
        let first_prefix = format!("{indent}  [{}] ", if step.done { 'x' } else { ' ' });
        let continuation_prefix = " ".repeat(first_prefix.len());
        append_wrapped(
            output,
            &first_prefix,
            &continuation_prefix,
            &terminal_text(&step.text),
            output_width,
        );
    }
}

/// Wrap list help and errors while preserving each logical line's leading
/// indentation. Redirected output is returned byte-for-byte.
fn wrap_list_document(text: &str, terminal_width: Option<usize>) -> String {
    let Some(output_width) = terminal_width else {
        return text.to_owned();
    };
    let lines = crate::ui::split_line_breaks(text).collect::<Vec<_>>();
    let has_trailing_break = text.ends_with('\n') || text.ends_with('\r');
    let mut output = String::new();
    for (index, line) in lines.iter().enumerate() {
        if has_trailing_break && index + 1 == lines.len() && line.is_empty() {
            continue;
        }
        let indent_len = line.bytes().take_while(|byte| *byte == b' ').count();
        let (prefix, content) = line.split_at(indent_len);
        append_wrapped(&mut output, prefix, prefix, content, output_width);
    }
    if !has_trailing_break {
        output.pop();
    }
    output
}

/// Append usage within one terminal width, keeping syntax groups intact.
fn append_usage_wrapped(
    output: &mut String,
    first_prefix: &str,
    continuation_prefix: &str,
    usage: &str,
    output_width: usize,
) {
    let tokens = usage_tokens(usage);
    let mut prefix = first_prefix;
    let mut line = String::new();
    for token in tokens {
        if line.is_empty() {
            line = token;
        } else if prefix.len() + line.len() + 1 + token.len() > output_width {
            output.push_str(prefix);
            output.push_str(line.trim_end());
            output.push('\n');
            prefix = continuation_prefix;
            line = token;
        } else {
            line.push(' ');
            line.push_str(&token);
        }
    }
    if !line.is_empty() {
        output.push_str(prefix);
        output.push_str(line.trim_end());
        output.push('\n');
    }
}

fn usage_tokens(usage: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut bracket_depth = 0;
    let mut angle_depth = 0;
    for character in usage.chars() {
        match character {
            '[' => {
                bracket_depth += 1;
                token.push(character);
            }
            ']' => {
                bracket_depth -= 1;
                token.push(character);
            }
            '<' => {
                angle_depth += 1;
                token.push(character);
            }
            '>' => {
                angle_depth -= 1;
                token.push(character);
            }
            character if character.is_whitespace() && bracket_depth == 0 && angle_depth == 0 => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            _ => token.push(character),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }

    let mut grouped = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let mut group = tokens[index].clone();
        while index + 2 < tokens.len() && tokens[index + 1] == "|" {
            group.push_str(" | ");
            group.push_str(&tokens[index + 2]);
            index += 2;
        }
        grouped.push(group);
        index += 1;
    }
    grouped
}

fn append_help_wrapped(
    output: &mut String,
    first_prefix: &str,
    continuation_prefix: &str,
    text: &str,
    output_width: usize,
) {
    let mut wrapped = String::new();
    append_wrapped(
        &mut wrapped,
        first_prefix,
        continuation_prefix,
        text,
        output_width,
    );
    for line in wrapped.lines() {
        output.push_str(line.trim_end());
        output.push('\n');
    }
}

/// Append text within one terminal width. Prefixes are ASCII CLI chrome, so
/// their byte lengths are also their display widths.
fn append_wrapped(
    output: &mut String,
    first_prefix: &str,
    continuation_prefix: &str,
    text: &str,
    output_width: usize,
) {
    let content_width = output_width.saturating_sub(first_prefix.len()).max(1);
    for (index, row) in crate::ui::edit::wrap_text(text, content_width)
        .into_iter()
        .enumerate()
    {
        output.push_str(if index == 0 {
            first_prefix
        } else {
            continuation_prefix
        });
        output.push_str(&row.text);
        output.push('\n');
    }
}

fn scope_labels(rows: &[ListRow]) -> BTreeMap<Option<String>, String> {
    let mut entries = Vec::new();
    let mut empty_projects = 0;
    for row in rows {
        if entries
            .iter()
            .any(|entry: &ScopeLabel| entry.scope == row.project)
        {
            continue;
        }
        let empty_number = if row
            .project
            .as_deref()
            .is_some_and(|path| path.trim().is_empty())
        {
            empty_projects += 1;
            empty_projects
        } else {
            0
        };
        entries.push(ScopeLabel::new(row.project.clone(), empty_number));
    }

    loop {
        let mut labels = BTreeMap::<String, Vec<usize>>::new();
        for (index, entry) in entries.iter().enumerate() {
            labels
                .entry(terminal_text(&entry.label()))
                .or_default()
                .push(index);
        }
        let duplicate_groups = labels
            .values()
            .filter(|indexes| indexes.len() > 1)
            .cloned()
            .collect::<Vec<_>>();
        if duplicate_groups.is_empty() {
            break;
        }

        let mut changed = false;
        for indexes in &duplicate_groups {
            for &index in indexes {
                changed |= entries[index].widen();
            }
        }
        if !changed {
            break;
        }
    }

    entries
        .into_iter()
        .map(|entry| {
            let label = entry.label();
            (entry.scope, label)
        })
        .collect()
}

struct ScopeLabel {
    scope: Option<String>,
    segments: Vec<String>,
    depth: usize,
    prefixed: bool,
    raw: bool,
    empty_number: usize,
}

impl ScopeLabel {
    fn new(scope: Option<String>, empty_number: usize) -> Self {
        let segments = scope
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(path_segments)
            .unwrap_or_default();
        Self {
            scope,
            depth: 1,
            segments,
            prefixed: false,
            raw: false,
            empty_number,
        }
    }

    fn label(&self) -> String {
        let mut label = match self.scope.as_deref() {
            None => "desk".into(),
            Some(path) if path.trim().is_empty() => {
                format!("project: <empty project {}>", self.empty_number)
            }
            Some(path) if self.raw => format!(
                "project: {}",
                serde_json::to_string(path).expect("scope path is serializable")
            ),
            Some(path) if self.segments.is_empty() => path.into(),
            Some(_) => {
                let start = self.segments.len().saturating_sub(self.depth);
                self.segments[start..].join("/")
            }
        };
        if self.prefixed && !self.raw {
            label = format!("project: {label}");
        }
        label
    }

    fn widen(&mut self) -> bool {
        if self.scope.is_none()
            || self
                .scope
                .as_deref()
                .is_some_and(|path| path.trim().is_empty())
        {
            return false;
        }
        if self.depth < self.segments.len() {
            self.depth += 1;
            true
        } else if !self.prefixed {
            self.prefixed = true;
            true
        } else if !self.raw {
            self.raw = true;
            true
        } else {
            false
        }
    }
}

fn path_segments(path: &str) -> Vec<String> {
    path.split(|character| character == '/' || (cfg!(windows) && character == '\\'))
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn steps_help() -> CliOutput {
    help(HelpDoc {
        usage: vec![
            "tsk steps <task> add <text> [--state-dir <dir>]".into(),
            "tsk steps <task> toggle <step-short-id> [--state-dir <dir>]".into(),
            "tsk steps <task> rename <step-short-id> <text> [--state-dir <dir>]".into(),
            "tsk steps <task> remove <step-short-id> [--state-dir <dir>]".into(),
        ],
        purpose: "Add, toggle, rename, or remove one step on a task.".into(),
        groups: vec![group(
            "Values",
            &[
                ("<task>", "a task number or UUID"),
                (
                    "<step-short-id>",
                    "an unambiguous prefix from tsk list <task> --json",
                ),
                ("--state-dir <dir>", "use another board store"),
            ],
        )],
        examples: vec![
            "tsk steps T12 add \"Write the failing test\"".into(),
            "tsk steps T12 toggle a3".into(),
            "tsk steps T12 rename a3 \"Write the failing test first\"".into(),
        ],
        refusals: vec![
            "empty-step-text".into(),
            "invalid-step-text".into(),
            "unknown-task".into(),
            "soft-deleted-task".into(),
            "unknown-step".into(),
            "ambiguous-step".into(),
        ],
        exit: exit_line(
            "step created, toggled, renamed, or removed",
            Some("step refusal, verify with list before retrying"),
            true,
        ),
    })
}

pub fn steps(result: StepsResult) -> CliOutput {
    let stdout = match result {
        StepsResult::Added { short_id, text } => {
            format!("added {short_id} {}\n", terminal_text(&text))
        }
        StepsResult::Toggled {
            short_id,
            text,
            done,
        } => format!(
            "toggled {short_id} [{}] {}\n",
            if done { "x" } else { " " },
            terminal_text(&text)
        ),
        StepsResult::Renamed { short_id, text } => {
            format!("renamed {short_id} {}\n", terminal_text(&text))
        }
        StepsResult::Removed { short_id, text } => {
            format!("removed {short_id} {}\n", terminal_text(&text))
        }
    };
    CliOutput {
        stdout,
        stderr: String::new(),
        code: 0,
    }
}

pub fn steps_usage(reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk steps: {}\nusage: tsk steps <task> add <text> | toggle <step-short-id> | rename <step-short-id> <text> | remove <step-short-id> [--state-dir <dir>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

/// Human tail for a task-verb refusal, painted after its stable code as
/// `code: message` so scripts branch on the code and people read the message.
fn task_refusal_message(code: &str, task: TaskAddress) -> String {
    let display = task.display();
    let message = match code {
        "unknown-task" => format!("{display} is not on the board"),
        "soft-deleted-task" => format!("{display} is deleted"),
        "empty-title" => "the title is empty".to_string(),
        "invalid-title" => "the title contains control characters".to_string(),
        "empty-step-text" => "the step text is empty".to_string(),
        "invalid-step-text" => "the step text contains control characters".to_string(),
        "unknown-step" => format!("no step on {display} matches that id"),
        "ambiguous-step" => format!("more than one step on {display} matches that id"),
        _ => return code.to_string(),
    };
    format!("{code}: {message}")
}

pub fn steps_rejected(error: StepsError, task: TaskAddress) -> CliOutput {
    let (detail, code) = match error {
        StepsError::Store(detail) => (detail, 3),
        other => (task_refusal_message(other.code(), task), 1),
    };
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk steps: {detail}\n"),
        code,
    }
}

fn status_name(status: HumanStatus) -> &'static str {
    match status {
        HumanStatus::Open => "open",
        HumanStatus::Ready => "ready",
        HumanStatus::Started => "started",
        HumanStatus::Blocked => "blocked",
        HumanStatus::Review => "review",
        HumanStatus::Done => "done",
    }
}

pub fn dispatch_help() -> CliOutput {
    help(HelpDoc {
        usage: vec!["tsk dispatch <task> [--again] [--state-dir <dir>]".into()],
        purpose: "Create a project worktree and launch the task's assigned agent in Herdr.".into(),
        groups: vec![group(
            "Options",
            &[
                (
                    "--again",
                    "relaunch, recreating a cleaned worktree when needed",
                ),
                ("--state-dir <dir>", "use another board store"),
            ],
        )],
        examples: vec!["tsk dispatch T12".into(), "tsk dispatch 12 --again".into()],
        refusals: vec![
            "unknown-task".into(),
            "soft-deleted-task".into(),
            "no-assignee".into(),
            "not-in-herdr".into(),
            "needs-git-project".into(),
            "done-task".into(),
            "archived-task".into(),
            "already-dispatched".into(),
            "unknown-agent".into(),
            "agent-config".into(),
            "herdr-failed".into(),
        ],
        exit: exit_line(
            "task dispatched",
            Some("dispatch refused, nothing persisted"),
            true,
        ),
    })
}

pub fn clean_help() -> CliOutput {
    help(HelpDoc {
        usage: vec!["tsk clean <task> [--json] [--state-dir <dir>]".into()],
        purpose: "Remove a clean dispatched worktree, keeping an unmerged branch.".into(),
        groups: vec![
            group(
                "Options",
                &[
                    ("--json", "print one machine-readable cleanup result"),
                    ("--state-dir <dir>", "use another board store"),
                ],
            ),
            group(
                "Store failures",
                &[("store-error (exit 3)", "verify the task with tsk list")],
            ),
        ],
        examples: vec!["tsk clean T12".into(), "tsk clean 12 --json".into()],
        refusals: vec![
            "not-dispatched".into(),
            "already-cleaned".into(),
            "dirty-worktree".into(),
            "worktree-mismatch".into(),
            "herdr-failed".into(),
        ],
        exit: exit_line(
            "dispatch cleaned, or its worktree was already missing",
            Some("cleanup refused, task and dispatch record unchanged"),
            true,
        ),
    })
}

pub fn cleaned(result: CleanupResult, json: bool) -> CliOutput {
    let worktree = match result.worktree {
        WorktreeCleanup::Removed => "removed",
        WorktreeCleanup::Missing => "missing",
    };
    let branch = match result.branch {
        BranchCleanup::Removed => "removed",
        BranchCleanup::Kept => "kept",
    };
    let workspace = if result.workspace_removed {
        "removed"
    } else {
        "kept"
    };
    let stdout = if json {
        format!(
            "{}\n",
            serde_json::json!({
                "number": result.number,
                "title": result.title,
                "worktree": {"path": result.worktree_path, "outcome": worktree},
                "branch": {"name": result.branch_name, "outcome": branch},
                "workspace": {"id": result.workspace_id, "outcome": workspace},
            })
        )
    } else {
        format!(
            "cleaned T{}: worktree {} ({}), branch {} ({}), workspace {} ({})\n",
            result.number,
            terminal_text(&result.worktree_path),
            worktree,
            terminal_text(&result.branch_name),
            branch,
            terminal_text(&result.workspace_id),
            workspace,
        )
    };
    CliOutput {
        stdout,
        stderr: String::new(),
        code: 0,
    }
}

pub fn clean_usage(reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk clean: {}\nusage: tsk clean <task> [--json] [--state-dir <dir>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

pub fn clean_rejected(error: CleanupError, task: TaskAddress) -> CliOutput {
    let exit = if matches!(error, CleanupError::Store(_)) {
        3
    } else {
        1
    };
    let refusal = error.code();
    let detail = match error {
        CleanupError::UnknownTask => format!("{} is not on the board", task.display()),
        other => other.to_string(),
    };
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk clean: {refusal}: {}\n", human_reason(&detail)),
        code: exit,
    }
}

pub fn dispatched(result: DispatchResult) -> CliOutput {
    CliOutput {
        stdout: format!(
            "dispatched T{} to @{} in {}\n",
            result.number,
            terminal_text(&result.assignee),
            terminal_text(&result.record.worktree)
        ),
        stderr: String::new(),
        code: 0,
    }
}

pub fn dispatch_usage(reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk dispatch: {}\nusage: tsk dispatch <task> [--again] [--state-dir <dir>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

pub fn dispatch_rejected(error: DispatchError, task: TaskAddress) -> CliOutput {
    let exit = if matches!(error, DispatchError::Store(_)) {
        3
    } else {
        1
    };
    let refusal = error.code();
    let detail = match error {
        DispatchError::UnknownTask => format!("{} is not on the board", task.display()),
        other => other.to_string(),
    };
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk dispatch: {refusal}: {}\n", human_reason(&detail)),
        code: exit,
    }
}

pub fn status_help() -> CliOutput {
    help(HelpDoc {
        usage: vec!["tsk status <task> <status> [--clean] [--state-dir <dir>]".into()],
        purpose: "Set a task's human status, optionally cleaning its dispatch after done persists."
            .into(),
        groups: vec![group(
            "Values",
            &[
                ("<task>", "a task number or UUID"),
                (
                    "<status>",
                    "open, ready, started (or start), blocked, review, or done",
                ),
                ("--clean", "after setting done, safely clean its dispatch"),
                ("--state-dir <dir>", "use another board store"),
            ],
        )],
        examples: vec![
            "tsk status T12 ready".into(),
            "tsk status T12 review".into(),
        ],
        refusals: vec!["unknown-task".into(), "soft-deleted-task".into()],
        exit: exit_line(
            "status set, or it already had the value",
            Some("status refusal, verify with list before retrying"),
            true,
        ),
    })
}

pub fn status(result: StatusResult) -> CliOutput {
    CliOutput {
        stdout: format!(
            "status T{} {} {}\n",
            result.number,
            status_name(result.status),
            terminal_text(&result.title)
        ),
        stderr: String::new(),
        code: 0,
    }
}

pub fn status_usage(reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk status: {}\nusage: tsk status <task> <status> [--state-dir <dir>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

pub fn status_rejected(error: StatusError, task: TaskAddress) -> CliOutput {
    let (detail, code) = match error {
        StatusError::Store(detail) => (detail, 3),
        other => (task_refusal_message(other.code(), task), 1),
    };
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk status: {detail}\n"),
        code,
    }
}

pub fn edit_help() -> CliOutput {
    help(HelpDoc {
        usage: vec![
            "tsk edit <task> [--title <title>] [--notes <notes>] [--assignee <name> | --unassign] [--state-dir <dir>]".into(),
        ],
        purpose: "Update a task's title, notes, or assignee without changing its scope or thread.".into(),
        groups: vec![group(
            "Values",
            &[
                ("<task>", "a task number or UUID"),
                ("--title <title>", "replace the title"),
                ("--notes <notes>", "replace notes, or clear them when blank"),
                ("--assignee <name>", "assign a defined agent profile"),
                ("--unassign", "clear the assignee"),
                ("--state-dir <dir>", "use another board store"),
                (
                    "--flag=<value>",
                    "use --title=<value>, --notes=<value>, or --state-dir=<dir> for dash-leading values",
                ),
            ],
        )],
        examples: vec![
            "tsk edit T12 --title \"Fix timeout on slow connections\"".into(),
            "tsk edit T12 --notes \"Reproduced with a delayed response\"".into(),
        ],
        refusals: vec![
            "unknown-task".into(),
            "soft-deleted-task".into(),
            "empty-title".into(),
            "invalid-title".into(),
            "unknown-agent".into(),
        ],
        exit: exit_line(
            "fields written, or already had the values",
            Some("edit refusal, verify with list before retrying"),
            true,
        ),
    })
}

pub fn edited(result: EditResult) -> CliOutput {
    CliOutput {
        stdout: format!(
            "edited T{} {}\n",
            result.number,
            terminal_text(&result.title)
        ),
        stderr: String::new(),
        code: 0,
    }
}

pub fn edit_usage(reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk edit: {}\nusage: tsk edit <task> [--title <title>] [--notes <notes>] [--assignee <name> | --unassign] [--state-dir <dir>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

pub fn edit_rejected(error: EditError, task: TaskAddress) -> CliOutput {
    let (detail, code) = match error {
        EditError::AgentConfig(detail) => (detail, 2),
        EditError::Store(detail) => (detail, 3),
        other => (task_refusal_message(other.code(), task), 1),
    };
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk edit: {detail}\n"),
        code,
    }
}

pub fn trash_help() -> CliOutput {
    help(HelpDoc {
        usage: vec!["tsk trash restore <task> [--state-dir <dir>]".into()],
        purpose: "Restore one trashed task to the board.".into(),
        groups: vec![group(
            "Values",
            &[
                ("<task>", "a task number or UUID from tsk list --deleted"),
                ("--state-dir <dir>", "use another board store"),
            ],
        )],
        examples: vec![
            "tsk list --deleted --all".into(),
            "tsk trash restore T12".into(),
        ],
        refusals: vec![
            "no matching trash line".into(),
            "task is already live".into(),
        ],
        exit: exit_line(
            "task restored",
            Some("no matching trash line, or task already live"),
            true,
        ),
    })
}

pub fn trash_usage(reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk trash: {}\nusage: tsk trash restore <task> [--state-dir <dir>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

pub fn trash_restored(result: TrashRestoreResult) -> CliOutput {
    CliOutput {
        stdout: format!(
            "restored {} {}\n",
            result.identifier,
            terminal_text(&result.title)
        ),
        stderr: String::new(),
        code: 0,
    }
}

pub fn trash_rejected(error: TrashCliError) -> CliOutput {
    let (detail, code) = match error {
        TrashCliError::Store(detail) => (detail, 3),
        TrashCliError::NotInTrash(detail) => (detail, 1),
    };
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk trash: {detail}\n"),
        code,
    }
}

pub fn archive_help(verb: &str) -> CliOutput {
    let antiverb = if verb == "archive" {
        "unarchive"
    } else {
        "archive"
    };
    let verb_title = if verb == "archive" {
        "Archive"
    } else {
        "Unarchive"
    };
    let success = if verb == "archive" {
        "task archived, or already was"
    } else {
        "task unarchived, or already was"
    };
    help(HelpDoc {
        usage: vec![format!("tsk {verb} <task> [--state-dir <dir>]")],
        purpose: format!("{verb_title} one task while keeping its human status."),
        groups: vec![group(
            "Values",
            &[
                ("<task>", "a task number or UUID"),
                ("--state-dir <dir>", "use another board store"),
            ],
        )],
        examples: vec![format!("tsk {verb} T12"), format!("tsk {antiverb} T12")],
        refusals: vec!["unknown-task".into(), "soft-deleted-task".into()],
        exit: exit_line(success, Some("unknown or deleted task"), true),
    })
}

pub fn archive_usage(verb: &str, reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk {verb}: {}\nusage: tsk {verb} <task> [--state-dir <dir>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

pub fn project_help() -> CliOutput {
    help(HelpDoc {
        usage: vec![
            "tsk project archive <name> [--state-dir <dir>]".into(),
            "tsk project unarchive <name> [--state-dir <dir>]".into(),
        ],
        purpose: "Archive or unarchive a whole project while preserving task statuses.".into(),
        groups: vec![
            group(
                "Scope",
                &[("<name>", "a project basename or verbatim path")],
            ),
            group(
                "Values",
                &[("--state-dir <dir>", "use another board store")],
            ),
        ],
        examples: vec![
            "tsk project archive atlas".into(),
            "tsk project unarchive atlas".into(),
        ],
        refusals: vec!["no project with that name has tasks".into()],
        exit: exit_line(
            "record written, or already had the value",
            Some("no project with that name has tasks"),
            true,
        ),
    })
}

pub fn project_usage(reason: &str) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!(
            "tsk project: {}\nusage: tsk project archive <name> | tsk project unarchive <name> [--state-dir <dir>]\n",
            human_reason(reason)
        ),
        code: 2,
    }
}

pub fn project_archived(result: ProjectResult, verb: &str) -> CliOutput {
    let past = if verb == "archive" {
        "archived"
    } else {
        "unarchived"
    };
    CliOutput {
        stdout: format!("{past} project {}\n", terminal_text(&result.name)),
        stderr: String::new(),
        code: 0,
    }
}

pub fn archived(result: ArchiveResult, verb: &str) -> CliOutput {
    // The row reads in the past tense: `archived T7 title` / `unarchived T7 title`.
    let past = if verb == "archive" {
        "archived"
    } else {
        "unarchived"
    };
    CliOutput {
        stdout: format!(
            "{past} T{} {}\n",
            result.number,
            terminal_text(&result.title)
        ),
        stderr: String::new(),
        code: 0,
    }
}

pub fn archive_rejected(error: ArchiveCliError, verb: &str) -> CliOutput {
    // Task-verb refusals name the invoked verb (`tsk archive:` / `tsk unarchive:`);
    // project refusals keep their own prefix.
    let error_code = error.code();
    let (verb, detail, code) = match error {
        ArchiveCliError::Store(detail) => (verb.to_string(), detail, 3),
        ArchiveCliError::UnknownTask(detail) | ArchiveCliError::SoftDeleted(detail) => {
            (verb.to_string(), format!("{error_code}: {detail}"), 1)
        }
        ArchiveCliError::UnknownProject(detail) => {
            ("project".to_string(), terminal_text(&detail), 1)
        }
    };
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk {verb}: {detail}\n"),
        code,
    }
}

pub fn list_usage(reason: &str, terminal_width: Option<usize>) -> CliOutput {
    let stderr = format!(
        "tsk list: {}\nusage: tsk list [<task>] [-p <project> | --desk | --all] [--thread <name>] [--assignee <name>] [--open | --ready | --done | --deleted | --archived] [--json] [--state-dir <dir>]\n",
        human_reason(reason)
    );
    CliOutput {
        stdout: String::new(),
        stderr: wrap_list_document(&stderr, terminal_width),
        code: 2,
    }
}

pub fn list_rejected(error: ListError, terminal_width: Option<usize>) -> CliOutput {
    match error {
        ListError::Store(detail) => CliOutput {
            stdout: String::new(),
            stderr: wrap_list_document(
                &format!("tsk list: {}\n", human_reason(&detail)),
                terminal_width,
            ),
            code: 3,
        },
        // A well-formed address that addresses no task: the invocation is wrong, not the store.
        ListError::UnknownTask => list_usage("unknown task", terminal_width),
    }
}

pub fn rejected(error: AddError) -> CliOutput {
    let (detail, code) = match &error {
        AddError::UnknownProject(detail) => {
            (format!("unknown-project: {}", terminal_text(detail)), 1)
        }
        AddError::ProjectArchived(name) => {
            let name = terminal_text(name);
            (
                format!(
                    "project-archived: project {name} is archived. Use --desk, -p <other project>, or tsk project unarchive {name}"
                ),
                1,
            )
        }
        AddError::UnknownAgent(detail) => (format!("unknown-agent: {}", terminal_text(detail)), 1),
        AddError::AgentConfig(detail) => (detail.clone(), 2),
        AddError::Store(detail) => (detail.clone(), 3),
        other => (other.code().into(), 1),
    };
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk add: {detail}\n"),
        code,
    }
}

pub fn setup_help() -> CliOutput {
    help(HelpDoc {
        usage: vec!["tsk setup [herdr | agents | claude | pi | omp | cursor | grok | codex | opencode | --skill-dir <path>] [--yes] [--force] [--json]".into(), "tsk setup --detected-ids | --skill-states".into(), "tsk setup herdr --check".into()],
        purpose: "Register Herdr, or install the bundled agent workflow skill.".into(),
        groups: vec![group("Output", &[("--json", "print machine-readable agent detection or install output"), ("--detected-ids", "print space-separated detected agent ids"), ("--skill-states", "print one tab-separated line per detected agent: id, state, installed version, path (for installers)"), ("herdr --check", "print bound when both plugin commands are already in the Herdr config, otherwise unbound")]), group("Values", &[("herdr", "register plugin assets and keyboard shortcuts"), ("agents --yes", "install or update every detected agent skill; add --force to rewrite matching versions"), ("<agent>, --skill-dir <path>", "install one named agent skill"), ("--force", "overwrite a matching skill version")])],
        examples: vec!["tsk setup herdr".into(), "tsk setup agents --yes".into(), "tsk setup pi".into()],
        refusals: vec!["skill-exists".into(), "setup failure".into(), "blocked skill root".into()],
        exit: exit_line("setup completed or help listed", Some("setup refusal or failure"), false),
    })
}

pub fn update_help() -> CliOutput {
    help(HelpDoc {
        usage: vec!["tsk update".into()],
        purpose: "Install the latest stable release for an installer-managed copy.".into(),
        groups: Vec::new(),
        examples: vec!["tsk update".into()],
        refusals: vec!["the update could not be installed".into()],
        exit: exit_line(
            "latest stable release installed or staged, or Homebrew guidance printed",
            Some("update failed"),
            false,
        ),
    })
}

pub fn guide_help() -> CliOutput {
    help(HelpDoc {
        usage: vec!["tsk guide".into()],
        purpose: "Print the bundled agent workflow skill.".into(),
        groups: Vec::new(),
        examples: vec!["tsk guide".into()],
        refusals: Vec::new(),
        exit: exit_line("agent workflow printed", None, false),
    })
}

pub fn setup_agent_listed(json: bool) -> CliOutput {
    if json {
        return match crate::setup_agent::detection_json() {
            Ok(text) => CliOutput {
                stdout: text,
                stderr: String::new(),
                code: 0,
            },
            Err(error) => setup_error(&error.to_string(), 1),
        };
    }
    CliOutput {
        stdout: crate::setup_agent::list_text(),
        stderr: String::new(),
        code: 0,
    }
}

pub fn setup_agent_detected_json(text: String) -> CliOutput {
    CliOutput {
        stdout: text,
        stderr: String::new(),
        code: 0,
    }
}

/// Plain probe output for installers (`--skill-states`, `herdr --check`).
pub fn setup_probe(text: String) -> CliOutput {
    CliOutput {
        stdout: text,
        stderr: String::new(),
        code: 0,
    }
}

pub fn setup_agent_detected_ids(ids: Vec<String>) -> CliOutput {
    CliOutput {
        stdout: if ids.is_empty() {
            String::new()
        } else {
            format!("{}\n", ids.join(" "))
        },
        stderr: String::new(),
        code: 0,
    }
}

pub fn setup_agent_written(
    target: &crate::setup_agent::Target,
    outcome: &crate::setup_agent::InstallOutcome,
    json: bool,
) -> CliOutput {
    let path = outcome.path();
    if json {
        return setup_agent_json(outcome.kind(), Some(target.name()), Some(path), 0);
    }
    CliOutput {
        stdout: format!("{}\n", terminal_text(&path.display().to_string())),
        stderr: String::new(),
        code: 0,
    }
}

pub fn setup_agent_exists(
    target: &crate::setup_agent::Target,
    path: &std::path::Path,
    json: bool,
) -> CliOutput {
    if json {
        return setup_agent_json("exists", Some(target.name()), Some(path), 1);
    }
    CliOutput {
        stdout: String::new(),
        stderr: "tsk setup: skill-exists\n".into(),
        code: 1,
    }
}

pub fn setup_agent_batch(result: crate::setup_agent::BatchResult, json: bool) -> CliOutput {
    if json {
        let payload = serde_json::json!({
            "outcome": if result.declined {
                "declined"
            } else if result.none_detected {
                "none-detected"
            } else if result.applied.is_empty() {
                "current"
            } else {
                "batch"
            },
            "applied": result.applied.iter().map(|(id, outcome)| serde_json::json!({
                "id": id,
                "kind": outcome.kind(),
                "path": outcome.path().display().to_string(),
            })).collect::<Vec<_>>(),
            "skipped_current": result.skipped_current,
            "blocked": result.blocked,
            "declined": result.declined,
            "none_detected": result.none_detected,
        });
        return CliOutput {
            stdout: format!("{payload}\n"),
            stderr: batch_blocked_error(&result.blocked),
            code: u8::from(!result.blocked.is_empty()),
        };
    }
    // One summary row per agent on stdout, for the interactive and scripted paths alike.
    let mut stdout = String::new();
    for (id, outcome) in &result.applied {
        stdout.push_str(&format!("    {id:<8} {}\n", outcome.path().display()));
    }
    for id in &result.skipped_current {
        stdout.push_str(&format!("    {id:<8} current\n"));
    }
    for id in &result.blocked {
        stdout.push_str(&format!("    {id:<8} blocked (symlink)\n"));
    }
    if result.none_detected && stdout.is_empty() {
        stdout.push_str("No agent skill roots detected.\n");
    }
    CliOutput {
        stdout,
        stderr: batch_blocked_error(&result.blocked),
        code: u8::from(!result.blocked.is_empty()),
    }
}

fn batch_blocked_error(blocked: &[String]) -> String {
    if blocked.is_empty() {
        String::new()
    } else {
        format!(
            "tsk setup: blocked agent skill roots: {}\n",
            blocked.join(", ")
        )
    }
}

fn setup_agent_json(
    outcome: &str,
    target: Option<&str>,
    path: Option<&std::path::Path>,
    code: u8,
) -> CliOutput {
    CliOutput {
        stdout: format!(
            "{}\n",
            serde_json::json!({
                "outcome": outcome,
                "target": target,
                "path": path.map(|value| value.display().to_string()),
            })
        ),
        stderr: String::new(),
        code,
    }
}
pub fn setup_error(reason: &str, code: u8) -> CliOutput {
    // Agent-skill reasons can carry user text (a `--skill-dir` path, for one), so every
    // control character is escaped, newlines included. The fixed usage text is the one
    // reason that legitimately spans two lines.
    let rendered = if reason == crate::setup_agent::USAGE {
        reason.to_string()
    } else {
        terminal_text(reason)
    };
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk setup: {rendered}\n"),
        code,
    }
}
/// `tsk setup herdr` failures. Those reasons are built from fixed text, paths tsk chose,
/// and Herdr's own output, which `setup::herdr` already escapes per line so its
/// diagnostics keep their line breaks; nothing in them is typed by the user.
pub fn setup_herdr_error(reason: &str, code: u8) -> CliOutput {
    let rendered = reason
        .split('\n')
        .map(terminal_text)
        .collect::<Vec<_>>()
        .join("\n");
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk setup: {rendered}\n"),
        code,
    }
}
pub fn setup(result: crate::setup::SetupResult) -> CliOutput {
    let mut stdout = String::new();
    if let Some(backup) = result.backup {
        stdout.push_str(&format!(
            "    Config backup:\n        {}\n",
            terminal_text(&backup.display().to_string())
        ));
    }
    // The content-addressed plugin root is not something users act on; it stays in
    // error messages, where `plugin unlink` or a manual look needs it.
    let shortcuts: Vec<String> = result
        .shortcuts
        .iter()
        .map(|(keys, label)| format!("{} {label}", terminal_text(keys)))
        .collect();
    if shortcuts.is_empty() {
        stdout.push_str("    Shortcuts:      none bound\n");
    } else {
        stdout.push_str(&format!("    Shortcuts:      {}\n", shortcuts.join(", ")));
    }
    if result.declined_conflicts {
        stdout.push_str("Declined conflicts were left unchanged.\n");
    }
    stdout.push_str(
        "\nReload Herdr (herdr server reload-config) or restart it to apply the shortcuts.\n",
    );
    CliOutput {
        stdout,
        stderr: String::new(),
        code: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[cfg(windows)]
    #[test]
    fn path_labels_split_windows_and_unix_separators() {
        assert_eq!(path_segments(r"C:\work\tsk"), ["C:", "work", "tsk"]);
        assert_eq!(path_segments("/work/tsk"), ["work", "tsk"]);
    }

    #[cfg(unix)]
    #[test]
    fn path_labels_keep_a_literal_unix_backslash() {
        assert_eq!(path_segments(r"/work/tsk\name"), ["work", r"tsk\name"]);
    }

    #[test]
    fn help_doc_wraps_hangs_aligns_per_group_and_omits_empty_sections() {
        let output = HelpDoc {
            usage: vec!["tsk example <very-long-operand> [--another-option]".into()],
            purpose: "A purpose that is deliberately long enough to wrap beneath the usage line while remaining easy to scan in the reference output.".into(),
            groups: vec![
                group("Scope", &[("-p <project>", "select a project")]),
                group("Filters", &[("-x", "short option"), ("--long-filter", "a long option whose explanation wraps with a hanging indent beneath the text column")]),
                group("Empty", &[]),
            ],
            examples: Vec::new(),
            refusals: Vec::new(),
            exit: "0 completed · 2 usage, nothing persisted".into(),
        }
        .render();

        assert!(output.lines().all(|line| line.len() <= 80));
        assert!(output.lines().all(|line| line == line.trim_end()));
        assert!(output.find("Scope").unwrap() < output.find("Filters").unwrap());
        assert!(!output.contains("Empty\n"));
        assert!(!output.contains("Examples:"));
        assert!(!output.contains("Refusals (exit 1):"));
        let lines = output.lines().collect::<Vec<_>>();
        let long_index = lines
            .iter()
            .position(|line| line.contains("--long-filter"))
            .unwrap();
        let short = lines.iter().find(|line| line.contains("-x")).unwrap();
        let long = lines[long_index];
        let text_column = long.find("a long option").unwrap();
        assert_eq!(short.find("short option"), Some(text_column));
        assert!(lines[long_index + 1].starts_with(&" ".repeat(text_column)));

        let mut usage = String::new();
        append_usage_wrapped(
            &mut usage,
            "usage: ",
            "       ",
            "tsk setup [herdr | agents | claude | pi | omp | cursor | grok | codex | opencode | --skill-dir <path>] [--yes]",
            80,
        );
        assert_eq!(
            usage,
            "usage: tsk setup\n       [herdr | agents | claude | pi | omp | cursor | grok | codex | opencode | --skill-dir <path>]\n       [--yes]\n"
        );
        assert!(usage.lines().all(|line| line == line.trim_end()));

        let mut alternatives = String::new();
        append_usage_wrapped(
            &mut alternatives,
            "usage: ",
            "       ",
            "tsk pick alpha | beta | gamma [--long argument]",
            28,
        );
        assert_eq!(
            alternatives,
            "usage: tsk pick\n       alpha | beta | gamma\n       [--long argument]\n"
        );
    }

    #[test]
    fn clean_output_names_every_resource_in_human_and_json_forms() {
        let result = CleanupResult {
            number: 12,
            title: "finished".into(),
            worktree_path: "/tmp/task-12".into(),
            branch_name: "tsk/t12-finished".into(),
            workspace_id: "w12".into(),
            worktree: WorktreeCleanup::Removed,
            branch: BranchCleanup::Kept,
            workspace_removed: true,
        };
        let human = cleaned(result.clone(), false);
        assert_eq!(
            human.stdout,
            "cleaned T12: worktree /tmp/task-12 (removed), branch tsk/t12-finished (kept), workspace w12 (removed)\n"
        );
        let json = cleaned(result, true);
        let value: serde_json::Value = serde_json::from_str(json.stdout.trim()).unwrap();
        assert_eq!(value["number"], 12);
        assert_eq!(value["worktree"]["outcome"], "removed");
        assert_eq!(value["branch"]["outcome"], "kept");
        assert_eq!(value["workspace"]["outcome"], "removed");
    }

    #[test]
    fn setup_usage_error_keeps_its_two_lines() {
        let output = setup_error(crate::setup_agent::USAGE, 2);
        assert_eq!(
            output.stderr,
            format!("tsk setup: {}\n", crate::setup_agent::USAGE)
        );
        assert!(!output.stderr.contains("\\u{000a}"));
    }

    #[test]
    fn herdr_setup_errors_keep_herdr_line_breaks_and_escape_the_rest() {
        let output = setup_herdr_error(
            "herdr config check failed: bad\u{1b}]0;evil\n  herdr config reset-keys  ...",
            1,
        );
        assert_eq!(
            output.stderr,
            "tsk setup: herdr config check failed: bad\\u{001b}]0;evil\n  herdr config reset-keys  ...\n"
        );
    }

    #[test]
    fn user_text_in_setup_errors_cannot_forge_a_line_break() {
        let output = setup_error("skill dir /tmp/a\nspoof is a symlink\u{1b}]0;x", 1);
        assert_eq!(
            output.stderr,
            "tsk setup: skill dir /tmp/a\\u{000a}spoof is a symlink\\u{001b}]0;x\n"
        );
    }

    #[test]
    fn setup_prints_indented_label_rows_and_one_reload_line() {
        let output = setup(crate::setup::SetupResult {
            binary: PathBuf::from("/home/box/.local/bin/tsk"),
            root: PathBuf::from("/home/box/.config/herdr/tsk-plugins/c578550bfb36dea8"),
            backup: Some(PathBuf::from(
                "/home/box/.config/herdr/config.toml.tsk-backup-20260912-143022",
            )),
            declined_conflicts: false,
            shortcuts: vec![
                ("prefix+t".to_string(), "board"),
                ("prefix+a".to_string(), "quick capture"),
            ],
        });
        assert_eq!(
            output.stdout,
            "    Config backup:\n        /home/box/.config/herdr/config.toml.tsk-backup-20260912-143022\n    Shortcuts:      prefix+t board, prefix+a quick capture\n\nReload Herdr (herdr server reload-config) or restart it to apply the shortcuts.\n"
        );
        assert_eq!(output.code, 0);
    }

    #[test]
    fn setup_omits_absent_backup_and_names_declined_conflicts_on_their_own_line() {
        let output = setup(crate::setup::SetupResult {
            binary: PathBuf::from("/home/box/.local/bin/tsk"),
            root: PathBuf::from("/home/box/.config/herdr/tsk-plugins/c578550bfb36dea8"),
            backup: None,
            declined_conflicts: true,
            shortcuts: vec![
                ("prefix+t".to_string(), "board"),
                ("prefix+a".to_string(), "quick capture"),
            ],
        });
        assert!(!output.stdout.contains("Config backup:"));
        assert!(output
            .stdout
            .contains(
                "    Shortcuts:      prefix+t board, prefix+a quick capture\nDeclined conflicts were left unchanged.\n"
            ));
    }

    #[test]
    fn setup_reports_the_keys_the_commands_are_actually_on() {
        let output = setup(crate::setup::SetupResult {
            binary: PathBuf::from("/home/box/.local/bin/tsk"),
            root: PathBuf::from("/home/box/.config/herdr/tsk-plugins/c578550bfb36dea8"),
            backup: None,
            declined_conflicts: false,
            shortcuts: vec![
                ("prefix+b / prefix+t".to_string(), "board"),
                ("prefix+a".to_string(), "quick capture"),
            ],
        });
        assert!(output
            .stdout
            .contains("    Shortcuts:      prefix+b / prefix+t board, prefix+a quick capture\n"));
    }

    #[test]
    fn setup_with_nothing_bound_says_so_instead_of_dropping_the_row() {
        let output = setup(crate::setup::SetupResult {
            binary: PathBuf::from("/home/box/.local/bin/tsk"),
            root: PathBuf::from("/home/box/.config/herdr/tsk-plugins/c578550bfb36dea8"),
            backup: None,
            declined_conflicts: true,
            shortcuts: Vec::new(),
        });
        assert!(output
            .stdout
            .contains("    Shortcuts:      none bound\nDeclined conflicts were left unchanged.\n"));
    }

    #[test]
    fn setup_agent_written_escapes_the_printed_path() {
        let outcome = crate::setup_agent::InstallOutcome::Written(PathBuf::from(
            "/home/box/.claude/skills/tsk-cli\u{001b}]0;x\u{0007}/SKILL.md",
        ));
        let output = setup_agent_written(&crate::setup_agent::Target::Claude, &outcome, false);
        assert!(!output.stdout.contains('\u{001b}'), "{:?}", output.stdout);
        assert!(!output.stdout.contains('\u{0007}'), "{:?}", output.stdout);
        assert!(output.stdout.ends_with("/SKILL.md\n"));
    }

    #[test]
    fn setup_agent_batch_prints_padded_rows_for_each_outcome() {
        let result = crate::setup_agent::BatchResult {
            applied: vec![(
                "pi".into(),
                crate::setup_agent::InstallOutcome::Written(PathBuf::from(
                    "/home/box/.pi/agent/skills/tsk-cli/SKILL.md",
                )),
            )],
            skipped_current: vec!["cursor".into()],
            blocked: vec!["grok".into()],
            declined: false,
            none_detected: false,
        };
        let output = setup_agent_batch(result, false);
        assert_eq!(
            output.stdout,
            "    pi       /home/box/.pi/agent/skills/tsk-cli/SKILL.md\n    cursor   current\n    grok     blocked (symlink)\n"
        );
    }
}
