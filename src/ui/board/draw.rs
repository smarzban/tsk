//! Queue chrome, overlays, verb bar, and frame drawing hooks.

use std::collections::BTreeSet;
use std::path::Path;

use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::domain::{HumanStatus, TaskScope};
use crate::ui::capture::CaptureField;
use crate::ui::edit::{
    escaped_line_window, wrap_text, wrapped_draft_rows, wrapped_edit_rows, EditBuffer,
};
use crate::ui::input::help_card_lines_for_query;
use crate::ui::mouse::BoardPopup;
use crate::ui::render::{
    self, BoardSurface, FormDropdown, NavChipKind, NavChipPaint, NavPaint, PaletteCommandRow,
    QueueFrameModel, QueueOverlay, VerbEntry,
};
use crate::ui::tier;
use crate::ui::{present_line, terminal_text};

use super::chrome::{notice_framed, row_width, BULK_DELETE_NOTICE_UNDO, DELETE_NOTICE_UNDO};
use super::commands::CommandSurface;
use super::model::{
    project_option_label, project_scope_option_label, BoardForm, BoardInputMode, BoardLocation,
    BoardModel, CleanupPrompt, CleanupRow, CleanupRowState, CleanupRun, DispatchPrompt, PickerTab,
    ProjectScopeOption, ProjectsView, RelaunchPrompt, StartAnywayPrompt,
};
use crate::ui::render::{CleanupCardLine, CleanupFooter, CleanupTitle};

fn home_dir() -> Option<String> {
    std::env::var("HOME").ok().filter(|home| !home.is_empty())
}

/// Show a path under the home directory as `~/…`.
pub(crate) fn tilde_path(path: &str, home: Option<&str>) -> String {
    let Some(home) = home.map(|home| home.trim_end_matches('/')) else {
        return path.to_string();
    };
    match path.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') && !home.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn identifiers_list<'a>(identifiers: impl IntoIterator<Item = &'a String>) -> String {
    identifiers
        .into_iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The branch half of what `y` will do for one row.
fn cleanup_branch_action(row: &CleanupRow) -> &'static str {
    if row.checking() {
        "delete branch if merged"
    } else if row.branch_deletable() {
        "delete branch"
    } else {
        "keep branch"
    }
}

/// The cleanup card's semantic body. Both cards say what `y` will do, never what happened.
pub(crate) fn cleanup_overlay<'a>(prompt: &CleanupPrompt, home: Option<&str>) -> QueueOverlay<'a> {
    let footer = match (prompt.can_clean_any(), prompt.bulk.is_some()) {
        (false, _) => CleanupFooter::Dirty,
        (true, false) => CleanupFooter::Single,
        (true, true) => CleanupFooter::Bulk,
    };
    let (title, lines) = match (&prompt.bulk, prompt.rows.first()) {
        (None, Some(row)) => single_cleanup_lines(row, home),
        (Some(bulk), _) => {
            let total =
                prompt.rows.len() + bulk.refused.len() + bulk.missing.len() + bulk.plain.len();
            let dispatched = prompt.rows.len() + bulk.refused.len();
            let cleanable = prompt.rows.iter().filter(|row| row.cleanable()).count();
            let title = if cleanable == 0 {
                CleanupTitle {
                    full: format!("Done {total} tasks · can't clean up"),
                    short: format!("Done {total} · can't clean up"),
                    bare: format!("Done {total} tasks"),
                    question: "Can't clean up any worktree.".into(),
                }
            } else {
                CleanupTitle {
                    full: format!("Done {total} tasks · clean up {cleanable} of {dispatched}?"),
                    short: format!("Done {total} · clean {cleanable} of {dispatched}?"),
                    bare: format!("Done {total} tasks"),
                    question: format!("Clean up {cleanable} of {dispatched} worktrees?"),
                }
            };
            let mut lines = Vec::new();
            for row in &prompt.rows {
                let (verdict, actions) = if row.dirty {
                    (
                        "uncommitted changes".to_string(),
                        "keep everything".to_string(),
                    )
                } else {
                    let verdict = if row.no_recorded_base() {
                        "no recorded base".to_string()
                    } else if row.checking() {
                        "checking…".to_string()
                    } else if row.unreachable_remote.is_some() {
                        "not confirmed (offline)".to_string()
                    } else if !row.base_available {
                        format!("base {} unavailable", row.base)
                    } else if row.branch_merged {
                        "merged ✓".to_string()
                    } else {
                        format!("not merged into {}", row.base)
                    };
                    let mut actions = vec![cleanup_branch_action(row), "remove worktree"];
                    if row.workspace_exists {
                        actions.push("close pane");
                    }
                    (verdict, actions.join(" · "))
                };
                lines.push(CleanupCardLine::Field {
                    label: format!("T{}", row.number),
                    value: verdict,
                });
                lines.push(CleanupCardLine::Field {
                    label: String::new(),
                    value: actions,
                });
                if let Some(warning) = &row.warning {
                    lines.push(CleanupCardLine::Field {
                        label: String::new(),
                        value: warning.clone(),
                    });
                }
            }
            for (identifier, reason) in &bulk.refused {
                lines.push(CleanupCardLine::Field {
                    label: identifier.clone(),
                    value: format!("can't inspect: {reason}"),
                });
                lines.push(CleanupCardLine::Field {
                    label: String::new(),
                    value: "keep everything".into(),
                });
            }
            bulk_trailer_lines(bulk, &mut lines);
            (title, lines)
        }
        (None, None) => (
            CleanupTitle {
                full: String::new(),
                short: String::new(),
                bare: String::new(),
                question: String::new(),
            },
            Vec::new(),
        ),
    };
    QueueOverlay::CleanupConfirm {
        title,
        lines,
        footer,
        scroll: prompt.scroll,
    }
}

/// A confirmed card: what happened to each row so far, as it lands.
pub(crate) fn cleanup_run_overlay<'a>(
    prompt: &CleanupPrompt,
    run: &CleanupRun,
) -> QueueOverlay<'a> {
    let finished = run.finished();
    let (landed, total) = run.progress();
    let title = match (run.bulk, run.rows.first()) {
        (None, Some(row)) => {
            let (full, short) = if finished {
                (
                    format!("Done T{} · cleanup finished", row.number),
                    format!("T{} · finished", row.number),
                )
            } else {
                (
                    format!("Done T{} · cleaning up", row.number),
                    format!("T{} · cleaning up", row.number),
                )
            };
            CleanupTitle {
                full,
                short,
                bare: format!("Done T{}", row.number),
                question: if finished {
                    "Cleanup finished."
                } else {
                    "Cleaning up…"
                }
                .into(),
            }
        }
        (bulk, _) => {
            let done = bulk.map_or(run.rows.len(), |(done, _)| done);
            let current = (landed + 1).min(total.max(1));
            let (progress, question) = if finished {
                (
                    "cleanup finished".to_string(),
                    "Cleanup finished.".to_string(),
                )
            } else {
                (
                    format!("cleaning {current} of {total}"),
                    format!("Cleaning {current} of {total}…"),
                )
            };
            CleanupTitle {
                full: format!("Done {done} tasks · {progress}"),
                short: format!("Done {done} · {progress}"),
                bare: format!("Done {done} tasks"),
                question,
            }
        }
    };
    let mut lines = Vec::new();
    for row in &run.rows {
        let (status, detail) = match &row.state {
            CleanupRowState::Checking => ("checking merge…".to_string(), None),
            CleanupRowState::Queued => ("waiting".to_string(), None),
            CleanupRowState::Removing => ("removing worktree…".to_string(), None),
            CleanupRowState::Cleaned { branch_kept: None } => {
                ("✓ cleaned · branch deleted".to_string(), None)
            }
            CleanupRowState::Cleaned {
                branch_kept: Some((_, full)),
            } => ("✓ cleaned · branch kept".to_string(), Some(full.clone())),
            CleanupRowState::Kept { full, .. } => (format!("kept: {full}"), None),
        };
        lines.push(CleanupCardLine::Field {
            label: format!("T{}", row.number),
            value: status,
        });
        if let Some(detail) = detail {
            lines.push(CleanupCardLine::Field {
                label: String::new(),
                value: detail,
            });
        }
    }
    for (identifier, reason) in &run.refused {
        lines.push(CleanupCardLine::Field {
            label: identifier.clone(),
            value: format!("kept: can't inspect: {reason}"),
        });
    }
    if let Some(bulk) = &prompt.bulk {
        bulk_trailer_lines(bulk, &mut lines);
    }
    QueueOverlay::CleanupConfirm {
        title,
        lines,
        footer: if finished {
            CleanupFooter::Finished
        } else {
            CleanupFooter::Running
        },
        scroll: prompt.scroll,
    }
}

/// The bulk card's closing lines for targets that needed no cleanup.
fn bulk_trailer_lines(bulk: &super::BulkCleanup, lines: &mut Vec<CleanupCardLine>) {
    if !bulk.missing.is_empty() {
        let noun = if bulk.missing.len() == 1 {
            "worktree"
        } else {
            "worktrees"
        };
        lines.push(CleanupCardLine::Text(format!(
            "+ {} {noun} already gone, marked cleaned",
            identifiers_list(bulk.missing.iter().map(|(_, identifier, _)| identifier))
        )));
    }
    if !bulk.plain.is_empty() {
        let verb = if bulk.plain.len() == 1 { "has" } else { "have" };
        lines.push(CleanupCardLine::Text(format!(
            "+ {} {verb} no dispatch, just marked done",
            identifiers_list(&bulk.plain)
        )));
    }
}

/// The block card over the cursor task or a marked set.
fn block_card_overlay<'a>(
    model: &BoardModel,
    card: &crate::ui::board::BlockCard,
) -> QueueOverlay<'a> {
    use crate::ui::board::{BlockCardField, OnKind};
    let identifier = |id: &uuid::Uuid| {
        model
            .tasks
            .iter()
            .find(|task| task.id == *id)
            .and_then(|task| task.board_identifier())
    };
    let review = card.kind() == crate::domain::BlockKind::Review;
    let noun = if review { "review" } else { "block" };
    let title = match (card.is_edit(), card.targets()) {
        (true, [id]) => format!("edit {noun} {}", identifier(id).unwrap_or_default()),
        (false, [id]) => format!("{noun} {}", identifier(id).unwrap_or_default()),
        (_, targets) => format!("{noun} {} tasks", targets.len()),
    };
    let (focus, cursor) = match card.field() {
        BlockCardField::Why => (0, card.why().cursor()),
        BlockCardField::On if review => (3, card.on_text().cursor()),
        BlockCardField::On => (1, card.on_text().cursor()),
        BlockCardField::Needs => (2, card.needs().cursor()),
        BlockCardField::Done => (0, card.done().cursor()),
        BlockCardField::Check => (1, card.checks().cursor()),
        BlockCardField::Next => (2, card.next().cursor()),
    };
    QueueOverlay::BlockCard(render::BlockCardPaint {
        title: title.trim().to_string(),
        review,
        why: terminal_text(card.why().value()),
        needs: terminal_text(card.needs().value()),
        done: terminal_text(card.done().value()),
        checks: card
            .checks()
            .value()
            .split('\n')
            .map(terminal_text)
            .collect::<Vec<_>>()
            .join("\n"),
        next: terminal_text(card.next().value()),
        on_kind: match card.on_kind() {
            OnKind::You => "you",
            OnKind::Task => "task",
            OnKind::Agent => "agent",
            OnKind::Other => "other",
        },
        on_text: terminal_text(card.on_text().value()),
        focus,
        cursor,
        refusal: card.refusal().map(str::to_string),
        edit: card.is_edit(),
    })
}

/// The bulk dispatch card: one row per task `y` launches (assignee and base), then the skipped
/// tasks with the single-task refusal. `default_branch` names a repository's remote default.
pub(crate) fn dispatch_overlay<'a>(
    prompt: &DispatchPrompt,
    default_branch: impl Fn(&Path) -> String,
) -> QueueOverlay<'a> {
    if let Some(relaunch) = &prompt.relaunch {
        return relaunch_overlay(relaunch, prompt.scroll);
    }
    if let Some(start) = &prompt.start_anyway {
        return start_anyway_overlay(start, prompt.scroll);
    }
    let count = prompt.launch.len();
    let noun = if count == 1 { "task" } else { "tasks" };
    let title = if count == 0 {
        let starts = prompt.start_only.len();
        let noun = if starts == 1 { "task" } else { "tasks" };
        CleanupTitle {
            full: format!("Start {starts} {noun}?"),
            short: format!("Start {starts}?"),
            bare: "Start".into(),
            question: format!("Start {starts} {noun}?"),
        }
    } else {
        CleanupTitle {
            full: format!("Dispatch {count} {noun}?"),
            short: format!("Dispatch {count}?"),
            bare: "Dispatch".into(),
            question: format!("Dispatch {count} {noun}?"),
        }
    };
    let mut lines = Vec::new();
    for eligible in &prompt.launch {
        let base = match eligible.base() {
            Some(base) => format!("from {base}"),
            None => match default_branch(eligible.project()) {
                default if default == "default" => "from default".to_string(),
                default => format!("from default ({default})"),
            },
        };
        let checking = if prompt.checking() {
            "  checking…"
        } else {
            ""
        };
        lines.push(CleanupCardLine::Field {
            label: format!("T{}", eligible.number),
            value: format!("@{}  {base}{checking}", eligible.assignee),
        });
    }
    if !prompt.start_only.is_empty() {
        lines.push(CleanupCardLine::Text("start only".into()));
        for (identifier, id) in &prompt.start_only {
            let value =
                if let Some((_, reason)) = prompt.no_launch.iter().find(|(row, _)| row == id) {
                    format!("started · no launch: {reason}")
                } else if prompt.corrections.contains(id) {
                    "status correction, no launch".to_string()
                } else {
                    "started, no launch".to_string()
                };
            lines.push(CleanupCardLine::Field {
                label: identifier.clone(),
                value,
            });
        }
    }
    if !prompt.skipped.is_empty() {
        lines.push(CleanupCardLine::Text("not started".into()));
        for (identifier, reason) in &prompt.skipped {
            lines.push(CleanupCardLine::Field {
                label: identifier.clone(),
                value: reason.clone(),
            });
        }
    }
    QueueOverlay::CleanupConfirm {
        title,
        lines,
        footer: CleanupFooter::Dispatch(count),
        scroll: prompt.scroll,
    }
}

/// The start-anyway card: `T203 runs after T202 (started).` per waiting target.
fn start_anyway_overlay<'a>(prompt: &StartAnywayPrompt, scroll: usize) -> QueueOverlay<'a> {
    let title = CleanupTitle {
        full: "Start anyway?".into(),
        short: "Start anyway?".into(),
        bare: "Start".into(),
        question: "Start anyway?".into(),
    };
    let lines = prompt
        .waiting
        .iter()
        .map(|line| CleanupCardLine::Text(format!("{line}.")))
        .collect();
    QueueOverlay::CleanupConfirm {
        title,
        lines,
        footer: CleanupFooter::StartAnyway,
        scroll,
    }
}

/// The relaunch card: one cursor task whose dispatched agent is gone.
fn relaunch_overlay<'a>(prompt: &RelaunchPrompt, scroll: usize) -> QueueOverlay<'a> {
    let number = prompt.number;
    let assignee = &prompt.assignee;
    let title = CleanupTitle {
        full: format!("T{number} · relaunch @{assignee}?"),
        short: format!("Relaunch @{assignee}?"),
        bare: format!("T{number}"),
        question: format!("Relaunch @{assignee}?"),
    };
    let lines = vec![
        CleanupCardLine::Text(format!("@{assignee} is no longer running.")),
        CleanupCardLine::Field {
            label: "worktree".into(),
            value: prompt.worktree.clone(),
        },
    ];
    QueueOverlay::CleanupConfirm {
        title,
        lines,
        footer: CleanupFooter::Relaunch,
        scroll,
    }
}

fn single_cleanup_lines(
    row: &CleanupRow,
    home: Option<&str>,
) -> (CleanupTitle, Vec<CleanupCardLine>) {
    let number = row.number;
    let base = &row.base;
    let title = if row.dirty {
        CleanupTitle {
            full: format!("Done T{number} · can't clean up"),
            short: format!("T{number} · can't clean up"),
            bare: format!("Done T{number}"),
            question: "Can't clean up.".into(),
        }
    } else {
        CleanupTitle {
            full: format!("Done T{number} · clean up?"),
            short: format!("T{number} · clean up?"),
            bare: format!("Done T{number}"),
            question: "Clean up?".into(),
        }
    };
    let headline = if row.dirty {
        "Worktree has uncommitted changes, so it stays.".to_string()
    } else if row.no_recorded_base() {
        "No recorded base, so the branch stays.".to_string()
    } else if row.checking() {
        format!("Checking merge into {base}…")
    } else if row.unreachable_remote.is_some() {
        format!("Merge into {base} not confirmed (offline), so the branch stays.")
    } else if !row.base_available {
        format!("Base {base} is unavailable, so the branch stays.")
    } else if row.branch_merged {
        format!("Merged into {base} ✓")
    } else {
        format!("Not merged into {base} (squash-merged? delete it by hand)")
    };
    let mut lines = vec![CleanupCardLine::Text(headline)];
    if let Some(warning) = &row.warning {
        lines.push(CleanupCardLine::Text(warning.clone()));
    }
    lines.push(CleanupCardLine::Blank);
    let field = |label: &str, value: String| CleanupCardLine::Field {
        label: label.to_string(),
        value,
    };
    let worktree = tilde_path(&row.worktree, home);
    if row.dirty {
        lines.push(field("keep branch", row.branch.clone()));
        lines.push(field("keep worktree", worktree));
        lines.push(field(
            "agent pane",
            if row.workspace_exists {
                "stays open"
            } else {
                "already closed"
            }
            .into(),
        ));
    } else {
        lines.push(field(cleanup_branch_action(row), row.branch.clone()));
        lines.push(field("remove worktree", worktree));
        if row.workspace_exists {
            lines.push(field("close", "agent pane".into()));
        } else {
            lines.push(field("agent pane", "already closed".into()));
        }
    }
    (title, lines)
}

/// Verb bar for the base board list.
///
/// The bar is a prompt, not a keymap: the few things you are most likely to do next from
/// where the cursor is, then the way out, then `? help`. Everything else lives in `?` and
/// `:`. Every surface reads the same shape (primary · status · create · out · help) so the
/// compact budget clips help first and never an action.
pub fn board_verb_items(model: &BoardModel) -> Vec<VerbEntry<'static>> {
    const HELP: VerbEntry<'static> = VerbEntry {
        key: "?",
        label: "help",
    };
    const ADD: VerbEntry<'static> = VerbEntry {
        key: "+",
        label: "add",
    };
    const OPEN: VerbEntry<'static> = VerbEntry {
        key: "enter",
        label: "open",
    };

    if model.input_mode() == BoardInputMode::Search {
        return vec![
            VerbEntry {
                key: "enter",
                label: "pin",
            },
            VerbEntry {
                key: "esc",
                label: "clear",
            },
        ];
    }

    // The inline step editor: Enter saves this step (and opens the next row on an add),
    // Shift+Enter saves the whole task session.
    if model.input_mode() == BoardInputMode::EditStep {
        return vec![
            VerbEntry {
                key: "enter",
                label: "step",
            },
            VerbEntry {
                key: "shift+enter",
                label: "save",
            },
            VerbEntry {
                key: "esc",
                label: "cancel",
            },
        ];
    }

    if model.input_mode() == BoardInputMode::EditReply && model.reply_is_feedback() {
        return vec![
            VerbEntry {
                key: "shift+enter",
                label: "save",
            },
            VerbEntry {
                key: "ctrl+s",
                label: "send back",
            },
            VerbEntry {
                key: "ctrl+d",
                label: "approve",
            },
            VerbEntry {
                key: "esc",
                label: "cancel",
            },
        ];
    }

    if model.input_mode() == BoardInputMode::EditReply {
        return vec![
            VerbEntry {
                key: "shift+enter",
                label: "save",
            },
            VerbEntry {
                key: "ctrl+s",
                label: "save + unblock",
            },
            VerbEntry {
                key: "esc",
                label: "cancel",
            },
        ];
    }

    // The read-only archived focus offers only what works there.
    if model.focus_is_archived() {
        return vec![
            VerbEntry {
                key: "u",
                label: "unarchive",
            },
            OPEN,
            VerbEntry {
                key: "esc",
                label: "close",
            },
            HELP,
        ];
    }

    // The task page's view mode: its own legend, true for the bound task.
    let page_task = if model.input_mode() == BoardInputMode::TaskPage {
        model
            .form
            .as_ref()
            .filter(|form| form.is_task())
            .and_then(|form| form.task_id())
            .and_then(|id| model.tasks.iter().find(|t| t.id == id))
    } else {
        None
    };
    if let Some(task) = page_task {
        return task_page_verb_items(model, task);
    }

    if model.nav_tab() == crate::ui::queue::NavTab::Projects
        && matches!(model.projects_view(), ProjectsView::Overview)
    {
        return vec![
            OPEN,
            VerbEntry {
                key: "/",
                label: "search",
            },
            HELP,
        ];
    }

    // Header rows hold the selection: their own verbs only.
    if model.archived_header_selected() {
        return vec![
            VerbEntry {
                key: "enter",
                label: if model.archived_collapsed {
                    "expand"
                } else {
                    "collapse"
                },
            },
            ADD,
            HELP,
        ];
    }
    if model.inbox_header_selected() {
        return vec![
            VerbEntry {
                key: "enter",
                label: if model.inbox_collapsed {
                    "expand"
                } else {
                    "collapse"
                },
            },
            ADD,
            HELP,
        ];
    }

    // While a delete notice is on the status row, the bar leads with its undo and keeps
    // only the selected row's first truthful status verb: exactly the five-seat compact
    // budget with a selection, so no tier clips an action. The remaining status verbs stay
    // on their keys and in `:` until the notice clears. The seat is Normal-mode only,
    // because every other surface hides the notice it belongs to.
    if model.visible_delete_notice().is_some() && model.input_mode() == BoardInputMode::Normal {
        let selected_task = model
            .selected_id()
            .and_then(|id| model.tasks.iter().find(|t| t.id == id));
        let mut armed = vec![VerbEntry {
            key: "u",
            label: "undo",
        }];
        if let Some(task) = selected_task {
            armed.push(OPEN);
            if let Some(verb) = status_verbs(task.status).into_iter().next() {
                armed.push(verb);
            }
        }
        armed.push(ADD);
        armed.push(HELP);
        return armed;
    }
    let selected_task = model
        .selected_id()
        .and_then(|id| model.tasks.iter().find(|t| t.id == id));
    let mut entries = Vec::with_capacity(6);
    if let Some(task) = selected_task {
        entries.push(OPEN);
        entries.extend(status_verbs(task.status));
        // Add is useful from the backlog and inbox, but the status-heavy in-motion and
        // done legends use that seat for their truthful lifecycle actions.
        if !matches!(task.status, HumanStatus::Started | HumanStatus::Done) {
            entries.push(ADD);
        }
    }
    entries.push(HELP);
    entries
}

/// The status verbs a task's current status makes meaningful. `n` picks a task for ON DECK,
/// while `o` sends it to the inbox.
fn status_verbs(status: HumanStatus) -> Vec<VerbEntry<'static>> {
    const START: VerbEntry<'static> = VerbEntry {
        key: "s",
        label: "start",
    };
    const NEXT: VerbEntry<'static> = VerbEntry {
        key: "n",
        label: "next",
    };
    const INBOX: VerbEntry<'static> = VerbEntry {
        key: "o",
        label: "inbox",
    };
    const DONE: VerbEntry<'static> = VerbEntry {
        key: "d",
        label: "done",
    };
    match status {
        HumanStatus::Open => vec![START, NEXT, DONE],
        HumanStatus::Ready => vec![START, INBOX, DONE],
        HumanStatus::Started => vec![
            DONE,
            NEXT,
            INBOX,
            VerbEntry {
                key: "b",
                label: "block",
            },
        ],
        HumanStatus::Blocked => vec![
            DONE,
            VerbEntry {
                key: "b",
                label: "unblock",
            },
            INBOX,
        ],
        HumanStatus::Review => vec![
            DONE,
            VerbEntry {
                key: "b",
                label: "block",
            },
            INBOX,
        ],
        HumanStatus::Done => vec![
            NEXT,
            INBOX,
            VerbEntry {
                key: "u",
                label: "undo",
            },
        ],
    }
}

fn task_page_verb_items(model: &BoardModel, task: &crate::domain::Task) -> Vec<VerbEntry<'static>> {
    // A parked edit session (dirty draft, no editor open) is about saving or discarding.
    if model.task_editing() {
        return vec![
            VerbEntry {
                key: "shift+enter",
                label: "save",
            },
            VerbEntry {
                key: "a",
                label: "step",
            },
            VerbEntry {
                key: "esc",
                label: "cancel",
            },
        ];
    }
    let mut entries = Vec::with_capacity(6);
    entries.push(VerbEntry {
        key: "e",
        label: "edit",
    });
    entries.extend(status_verbs(task.status));
    entries.push(VerbEntry {
        key: "esc",
        label: "close",
    });
    entries
}

/// Build the task page's paint payload from the open task form. View mode wraps the notes
/// draft and windows it by the page scroll; field edits reuse the form's cursor windowing.
fn build_task_page_overlay<'a>(
    model: &'a BoardModel,
    form: &'a BoardForm,
    geo: &tier::TierGeometry,
    scope_dropdown: Option<FormDropdown<'a>>,
    column: bool,
) -> QueueOverlay<'a> {
    let width = geo.row_width as usize;
    let bound_task = form
        .task_id()
        .and_then(|id| model.tasks.iter().find(|task| task.id == id));
    // The section consumes extracted, word-wrapped step views, never the raw storage. The
    // reserved width is the scrollbar-safe content width less the step glyph and trailing
    // pad, matching Notes' no-truncation behavior.
    let step_text_width = width.saturating_sub(8);
    // Task-edit removals are staged, not durable, but they immediately leave the rendered
    // list. Keep the task's source index only in the session state, then derive this compact
    // visible list so the renderer never receives a row it must hide conditionally.
    let mut step_views: Vec<render::StepView> = bound_task
        .map(|task| {
            task.steps
                .iter()
                .filter(|step| !form.steps.removals.contains(&step.id))
                .map(|step| render::StepView {
                    done: step.done,
                    rows: crate::ui::edit::wrap_text(&step.text, step_text_width)
                        .into_iter()
                        .map(|row| row.text)
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();
    for text in &form.steps.pending_adds {
        step_views.push(render::StepView {
            done: false,
            rows: crate::ui::edit::wrap_text(text, step_text_width)
                .into_iter()
                .map(|row| row.text)
                .collect(),
        });
    }
    let stored_step_count = step_views.len();
    // Existing-step edits are task-session drafts. Paint every parked draft first, then the
    // active row over it. Only the active EditStep mode receives cursor metadata, so moving to
    // Title, Notes, Thread, or Scope leaves the changed step visible without a second cursor.
    if let Some(task) = bound_task {
        for (visible, step) in task
            .steps
            .iter()
            .filter(|step| !form.steps.removals.contains(&step.id))
            .enumerate()
        {
            if let Some(draft) = form.steps.drafts.get(&step.id) {
                let (rows, _, _) = wrapped_edit_rows(draft, step_text_width);
                step_views[visible].rows = rows;
            }
        }
    }
    let inline_step_editor = form.steps.editor.as_ref().and_then(|editor| {
        let index = match editor.rename {
            Some(step_id) => bound_task?
                .steps
                .iter()
                .filter(|step| !form.steps.removals.contains(&step.id))
                .position(|step| step.id == step_id)?,
            None => step_views.len(),
        };
        let (rows, cursor_row, cursor_col) = wrapped_edit_rows(&editor.buffer, step_text_width);
        if index < step_views.len() {
            step_views[index].rows = rows;
        } else {
            step_views.push(render::StepView { done: false, rows });
        }
        (model.input_mode() == BoardInputMode::EditStep).then_some(render::InlineStepEditor {
            index,
            cursor_row: u16::try_from(cursor_row).unwrap_or(u16::MAX),
            cursor_col: u16::try_from(cursor_col).unwrap_or(u16::MAX),
            refusal: editor.refusal.as_deref(),
        })
    });
    // The page cursor stores source indices, while the overlay only carries visible rows.
    // Translate at the boundary, so a staged removal cannot make the selector jump to a
    // different stored step just because its former array slot disappeared.
    let visible_step_index = |source_index: usize| {
        if let Some(task) = bound_task {
            task.steps
                .iter()
                .enumerate()
                .filter(|(_, step)| !form.steps.removals.contains(&step.id))
                .position(|(index, _)| index == source_index)
        } else {
            // Capture steps are locally staged, so their cursor is already an index into the
            // rendered pending-add rows rather than a source index in a bound task.
            (source_index < stored_step_count).then_some(source_index)
        }
    };
    // Cursor state uses source indices, so keep the recorded row counts source-aligned too.
    // Staged removals occupy zero rows; the active add row is not addressable by this cursor.
    let mut visible_counts = step_views
        .iter()
        .take(stored_step_count)
        .map(|step| step.rows.len().max(1));
    form.steps.row_counts.replace(
        bound_task
            .map(|task| {
                task.steps
                    .iter()
                    .map(|step| {
                        if form.steps.removals.contains(&step.id) {
                            0
                        } else {
                            visible_counts.next().unwrap_or(1)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
    );
    let bottom_input = (model.input_mode() == BoardInputMode::EditThread).then(|| {
        let avail = (geo.row_width as usize).saturating_sub(2);
        let (text, cursor_col) = escaped_line_window(&form.thread, avail);
        crate::ui::render::BottomInputSlot {
            text,
            cursor_col,
            placeholder: "thread…",
            refusal: None,
            above_rows: Vec::new(),
            cursor_row_offset: 0,
            message: form.thread_refusal.as_deref(),
        }
    });
    // A notes edit always keeps one row: the layout reserves it (the section caps
    // around it), so an active edit can never be scrolled/clamped out of the frame.
    // A wide column's height already reflects the shared footer's input slot.
    let page_geo = if column {
        *geo
    } else {
        render::bottom_input_geometry(*geo, bottom_input.is_some())
    };
    let status = bound_task
        .map(|task| task.status)
        .unwrap_or(HumanStatus::Ready);
    // AC-8: an archived task's header slot reads `archived` in place of the status word.
    let status_word = match bound_task {
        Some(task) if task.archived => "archived",
        _ => match status {
            HumanStatus::Open => "open",
            HumanStatus::Ready => "ready",
            HumanStatus::Started => "started",
            HumanStatus::Blocked => "blocked",
            HumanStatus::Review => "review",
            HumanStatus::Done => "done",
        },
    };
    let glyph = bound_task
        .map(render::task_status_glyph)
        .unwrap_or_else(|| render::status_glyph(status));

    // Meta footer: assignee · base · thread · scope. The PAPER TRAIL carries when the task was
    // created and last changed. The identifier belongs in the header, so it never competes with
    // footer hits.
    // A wide column moves the project up into its header slot: the scope footer paints only
    // while the edit session can change it, so its control stays reachable by mouse.
    let editing_session = !form.is_task() || form.editing || model.open_field_edit().is_some();
    let show_scope = !column || editing_session;
    let meta_scope = if show_scope {
        match &form.scope {
            TaskScope::Project { path } => render::short_project(path).to_string(),
            TaskScope::Global => "desk".to_string(),
        }
    } else {
        String::new()
    };
    let meta_scope_width = u16::try_from(render::display_width(&meta_scope)).unwrap_or(u16::MAX);
    let mut meta = String::new();
    let capture_form = !form.is_task();
    let shown_assignee = if capture_form || (form.is_task() && form.editing) {
        form.assignee.as_deref()
    } else {
        bound_task.and_then(|task| task.assignee.as_deref())
    };
    // View mode offers the quick picker on an unassigned task: a clickable `+ assign` in the
    // assignee slot, but only when a profile exists to choose. The peek never paints it.
    let assignee_segment = shown_assignee
        .map(|assignee| format!("@{}", terminal_text(assignee)))
        .or_else(|| {
            (capture_form || (form.is_task() && form.editing)).then(|| "assignee".to_string())
        })
        .or_else(|| {
            (bound_task.is_some_and(|task| !task.is_notice()) && !model.agent_names.is_empty())
                .then(|| "+ assign".to_string())
        });
    let meta_assignee_x = assignee_segment.as_ref().map(|_| 0);
    let meta_assignee_width = assignee_segment
        .as_deref()
        .map(render::display_width)
        .and_then(|width| u16::try_from(width).ok())
        .unwrap_or(0);
    if let Some(segment) = assignee_segment {
        meta.push_str(&segment);
    }

    let shown_base = if capture_form || (form.is_task() && form.editing) {
        form.base.as_deref()
    } else {
        bound_task.and_then(|task| task.base.as_deref())
    };
    let show_base = capture_form || bound_task.is_some_and(|task| !task.is_notice());
    let base_segment = show_base.then(|| {
        shown_base.map_or_else(
            || {
                let scope = if capture_form || form.editing {
                    &form.scope
                } else {
                    bound_task.map_or(&form.scope, |task| &task.scope)
                };
                let default = match scope {
                    TaskScope::Project { path } => model.default_branch_name(Path::new(path)),
                    TaskScope::Global => "default".to_string(),
                };
                if default == "default" {
                    "⎇ default".to_string()
                } else {
                    format!("⎇ {default} (default)")
                }
            },
            |base| format!("⎇ {}", terminal_text(base)),
        )
    });
    let meta_base_x = base_segment.as_ref().map(|_| {
        u16::try_from(render::display_width(&meta) + usize::from(!meta.is_empty()) * 3)
            .unwrap_or(u16::MAX)
    });
    let meta_base_width = base_segment
        .as_deref()
        .map(render::display_width)
        .and_then(|width| u16::try_from(width).ok())
        .unwrap_or(0);
    if let Some(segment) = base_segment {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str(&segment);
    }

    // `after T202, T205` beside the base, hidden when empty outside an edit session; the
    // read-only `before T203` follows, derived from the other tasks' `after`.
    let editing_links = capture_form || (form.is_task() && form.editing);
    let shown_after: &[u64] = if editing_links {
        &form.after
    } else {
        bound_task.map_or(&[], |task| task.after.as_slice())
    };
    let after_segment = if !shown_after.is_empty() {
        Some(format!(
            "after {}",
            shown_after
                .iter()
                .map(|number| format!("T{number}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    } else {
        editing_links.then(|| "after".to_string())
    };
    let before_segment = bound_task
        .filter(|_| !capture_form)
        .map(|task| render::before_numbers(task, &model.tasks))
        .filter(|before| !before.is_empty())
        .map(|before| {
            format!(
                "before {}",
                before
                    .iter()
                    .map(|number| format!("T{number}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
    for segment in [after_segment, before_segment].into_iter().flatten() {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str(&segment);
    }

    let shown_thread = if capture_form || (form.is_task() && form.editing) {
        Some(form.thread.value())
    } else {
        bound_task.and_then(|task| task.thread.as_deref())
    };
    let thread_segment = if bound_task.is_some() || capture_form {
        if let Some(thread) = shown_thread.filter(|thread| !thread.is_empty()) {
            Some(format!("#{}", terminal_text(thread)))
        } else if capture_form
            || (form.is_task() && form.editing)
            || matches!(
                model.input_mode(),
                BoardInputMode::EditTitle
                    | BoardInputMode::EditNotes
                    | BoardInputMode::SelectThread
                    | BoardInputMode::EditThread
                    | BoardInputMode::EditScope
                    | BoardInputMode::EditAssignee
                    | BoardInputMode::FormDropdown
            )
        {
            Some("thread".to_string())
        } else {
            None
        }
    } else {
        None
    };
    let thread_slot_width = thread_segment
        .as_deref()
        .map(render::display_width)
        .map(|width| u16::try_from(width).unwrap_or(u16::MAX));
    if let Some(segment) = thread_segment {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str(&segment);
    }

    if !meta_scope.is_empty() && !meta.is_empty() {
        meta.push_str(" · ");
    }
    let meta_scope_x = u16::try_from(render::display_width(&meta)).unwrap_or(u16::MAX);
    if !meta_scope.is_empty() {
        meta.push_str(&meta_scope);
    }
    // Header: indent + glyph + the WRAPPED title rows + right-aligned status word
    // on row 0. A long title wraps onto further bold rows indented under the
    // glyph instead of truncating; edit mode wraps the draft with its cursor.
    // The uniform budget keeps every row's wrap identical.
    let editing_title = model.input_mode() == BoardInputMode::EditTitle;
    let header_identifier = (!editing_title)
        .then(|| bound_task.and_then(|task| task.board_identifier()))
        .flatten();
    let mut header_rows: Vec<String> = Vec::new();
    let mut title_cursor = None;
    // The wide column paints its own two-row header (`draw_task_column`), so the in-page
    // header rows are only built for the single-pane page that actually consumes them.
    if !column {
        let word_cells = status_word.chars().count() + 1;
        let identifier_cells = header_identifier
            .as_deref()
            .map(render::display_width)
            .unwrap_or(0);
        let title_avail = width
            .saturating_sub(4 + word_cells + identifier_cells + usize::from(identifier_cells > 0));
        // The header may grow only inside the page body: it must stop one row short
        // of the lowest chrome row with one note row still living under it, or a
        // pathological title would eat the page (and the painter's chrome).
        let page_bottom = [page_geo.rule_row, page_geo.status_row, page_geo.verb_row]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(page_geo.height);
        let footer_rows = wrap_text(&meta, width.saturating_sub(2)).len();
        let footer_rows = u16::try_from(footer_rows)
            .unwrap_or(u16::MAX)
            .min(page_bottom.saturating_sub(3));
        let header_cap = page_bottom
            .saturating_sub(footer_rows.saturating_add(2))
            .max(1) as usize;
        form.title_wrap_width.set(title_avail);
        if editing_title {
            let (mut rows, cursor_row, cursor_col) = wrapped_edit_rows(&form.title, title_avail);
            let overflowed = rows.len() > header_cap;
            rows.truncate(header_cap);
            if overflowed {
                if let Some(last) = rows.last_mut() {
                    *last = present_line(last, title_avail.saturating_sub(1));
                }
            }
            for (offset, segment) in rows.iter().enumerate() {
                if offset == 0 {
                    header_rows.push(format!("{glyph} {segment}"));
                } else {
                    header_rows.push(segment.clone());
                }
            }
            // A caret hidden below the cap parks at the END of the last shown row:
            // its own hidden column would otherwise paint an unrelated position on
            // the ellipsis row.
            let shown_cursor_row = cursor_row.min(rows.len().saturating_sub(1));
            let shown_cursor_col = if cursor_row > shown_cursor_row {
                rows.last()
                    .map(|last| render::display_width(last))
                    .unwrap_or(0)
            } else {
                cursor_col
            };
            title_cursor = Some((
                u16::try_from(shown_cursor_row).unwrap_or(u16::MAX),
                u16::try_from(shown_cursor_col).unwrap_or(u16::MAX),
            ));
        } else {
            let mut rows: Vec<String> = wrap_text(form.title.value(), title_avail)
                .iter()
                .map(|row| row.text.clone())
                .collect();
            let overflowed = rows.len() > header_cap;
            rows.truncate(header_cap);
            if overflowed {
                if let Some(last) = rows.last_mut() {
                    *last = present_line(last, title_avail.saturating_sub(1));
                }
            }
            for (offset, row) in rows.iter().enumerate() {
                if offset == 0 {
                    header_rows.push(format!("{glyph} {row}"));
                } else {
                    header_rows.push(row.clone());
                }
            }
        }
    }
    let lay = if column {
        render::task_column_layout_with_meta(&page_geo, &meta)
    } else {
        render::task_page_layout_with_meta(&page_geo, header_rows.len().max(1) as u16, &meta)
    };
    // The renderer and input reducer share this viewport size for page scrolling.
    form.steps.window_rows.set(lay.notes_rows as usize);

    // View mode supplies every wrapped note row; Notes edit mode wraps too, with the
    // caret mapped into wrapped coordinates. The shared page painter combines that
    // stream with the steps, then windows it once against the fixed viewport.
    // Wrap at the width the painter can show WHOLE: the content region less its
    // two-cell gutter and one further reserved cell, taken in the scrollbar state
    // (content_width - 3 there), so an overflowing page never re-wraps rows that
    // were already painted -- and no wrapped row ever ends in the presenter's … .
    let notes_width = width.saturating_sub(6);
    let want = lay.notes_rows as usize;
    let editing_notes = model.input_mode() == BoardInputMode::EditNotes;
    let (mut notes_rows, notes_cursor, more_lines, notes_scroll) = if editing_notes {
        let (all_rows, cursor_row, cursor_column) = wrapped_edit_rows(&form.notes, notes_width);
        // Explicit pointer scrolling may leave the caret offscreen to reach steps.
        // Typing or moving the caret restores automatic following.
        let follow = if form.manual_page_scroll {
            form.notes_scroll
        } else {
            form.notes_scroll.clamp(
                cursor_row.saturating_sub(want.saturating_sub(1)),
                cursor_row,
            )
        };
        (
            all_rows,
            (!form.manual_page_scroll).then_some((
                u16::try_from(cursor_row).unwrap_or(u16::MAX),
                u16::try_from(cursor_column).unwrap_or(u16::MAX),
            )),
            0,
            follow,
        )
    } else if form.notes.value().trim().is_empty() {
        (Vec::new(), None, 0, form.notes_scroll)
    } else {
        (
            wrapped_draft_rows(&form.notes, notes_width),
            None,
            0,
            form.notes_scroll,
        )
    };
    if !editing_notes {
        if let Some(dispatch) = bound_task.and_then(|task| task.dispatch.as_ref()) {
            if !notes_rows.is_empty() {
                notes_rows.push(String::new());
            }
            let when = render::format_age(model.now(), dispatch.at);
            let mut dispatch_lines = vec![
                if dispatch.cleaned {
                    "dispatch · cleaned".to_string()
                } else {
                    "dispatch".to_string()
                },
                if dispatch.cleaned {
                    format!("worktree removed · {}", terminal_text(&dispatch.worktree))
                } else {
                    format!("worktree {}", terminal_text(&dispatch.worktree))
                },
                format!("branch {}", terminal_text(&dispatch.branch)),
            ];
            if let Some(base) = dispatch.base.as_ref() {
                let commit = dispatch
                    .base_commit
                    .as_deref()
                    .map(|commit| commit.chars().take(7).collect::<String>());
                dispatch_lines.push(match commit {
                    Some(commit) => format!("from {} @ {commit}", terminal_text(base)),
                    None => format!("from {}", terminal_text(base)),
                });
            }
            dispatch_lines.push(format!("when {when} ago"));
            for line in dispatch_lines {
                notes_rows.extend(
                    wrap_text(&line, notes_width)
                        .into_iter()
                        .map(|row| row.text),
                );
            }
        }
    }
    // A blocked task leads its page with the BLOCKED section, a task in review with the REVIEW
    // section, outside an edit session.
    let (block_rows, block_stops, block_cursor) = match bound_task {
        Some(task) if !editing_session && super::block::has_open_section(task) => {
            block_page_rows(model, form, task, notes_width)
        }
        _ => (Vec::new(), Vec::new(), None),
    };
    // The task's own form draws the trail, so the wide task column paints it too, through an
    // edit session as well (the page body never jumps as one starts or ends).
    let (trail_rows, trail_stops) = match bound_task.filter(|task| !task.is_notice()) {
        Some(task) => trail_page_rows(model, form, task, notes_width),
        None => (Vec::new(), Vec::new()),
    };
    let step_rows: usize = step_views.iter().map(|step| step.rows.len().max(1)).sum();
    // Match the painter's stream exactly: it always paints one notes row and a trailing
    // `+ step` row, even when both stored notes and stored steps are empty.
    let content = render::page_content_layout(
        block_rows.len() + notes_rows.len().max(1),
        step_rows + 1,
        trail_rows.len(),
        lay.notes_rows,
    );
    form.block.rows.replace(
        block_stops
            .into_iter()
            .chain(
                trail_stops
                    .into_iter()
                    .map(|(stop, row)| (stop, content.trail_start + row)),
            )
            .collect(),
    );
    // The reply box's caret stays in view while it is typed into.
    let notes_scroll = match block_cursor {
        Some((row, _)) if !form.manual_page_scroll => notes_scroll
            .clamp(row.saturating_sub(want.saturating_sub(1)), row)
            .min(content.max_scroll),
        _ => notes_scroll,
    };
    form.notes_max_scroll.set(content.max_scroll);
    form.steps.content_start.set(content.steps_start);
    form.notes_width.set(notes_width);

    let focus = match model.input_mode() {
        BoardInputMode::EditTitle => Some(CaptureField::Title),
        BoardInputMode::EditNotes => Some(CaptureField::Notes),
        BoardInputMode::SelectThread | BoardInputMode::EditThread => Some(CaptureField::Thread),
        BoardInputMode::SelectBase => Some(CaptureField::Base),
        BoardInputMode::SelectAfter => Some(CaptureField::After),
        BoardInputMode::EditScope => Some(CaptureField::Scope),
        BoardInputMode::EditAssignee => Some(CaptureField::Assignee),
        BoardInputMode::FormDropdown => Some(form.focus),
        _ => None,
    };

    let scope_dropdown = scope_dropdown.map(|mut dropdown| {
        let field_x = match dropdown.field {
            CaptureField::Assignee => meta_assignee_x.unwrap_or(0),
            CaptureField::Base => meta_base_x.unwrap_or(0),
            CaptureField::Scope => meta_scope_x,
            _ => 0,
        };
        dropdown.anchor_x = 2u16.saturating_add(field_x);
        dropdown
    });

    QueueOverlay::TaskPage {
        header_rows,
        header_identifier,
        header_identifier_task: (!editing_title).then(|| form.task_id()).flatten(),
        title_cursor,
        status_word,
        notes_rows,
        notes_cursor,
        more_lines,
        step_views,
        stored_step_count,
        step_cursor: form.steps.cursor.and_then(visible_step_index),
        step_add_selected: form.steps.add_selected,
        step_scroll: notes_scroll,
        step_marked: form.steps.delete_mark.and_then(visible_step_index),
        inline_step_editor,
        block_rows,
        block_cursor,
        trail_rows,
        bottom_input,
        meta,
        meta_assignee_x,
        meta_assignee_width,
        meta_base_x,
        meta_base_width,
        meta_scope_x,
        meta_scope_width,
        thread_slot_width,
        focus,
        scope_dropdown,
    }
}

/// The PAPER TRAIL section after the steps, all dim: its `PAPER TRAIL · N ▸` heading, and while
/// it is expanded (`▾`) every entry, newest first, with closed records expandable in place. Also
/// returns each stop's row, counted from the section's first row.
fn trail_page_rows(
    model: &BoardModel,
    form: &BoardForm,
    task: &crate::domain::Task,
    width: usize,
) -> (
    Vec<render::BlockPageRow>,
    Vec<(crate::ui::board::BlockTarget, usize)>,
) {
    use crate::ui::board::BlockTarget;
    use render::{BlockPageRow, BlockRowKind, QueueHitTarget};

    let width = width.max(8);
    let now = model.now();
    let entries = crate::activity::paper_trail(task);
    if entries.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let expanded = form.block.trail_expanded;
    // Selection belongs to the page form; another seat's form never paints it.
    let selected = model.block_target().filter(|_| {
        model
            .form
            .as_ref()
            .is_some_and(|own| std::ptr::eq(own, form))
    });
    let mut rows: Vec<BlockPageRow> = Vec::new();
    let mut stops = vec![(BlockTarget::TrailHeading, 0)];
    let push = |rows: &mut Vec<BlockPageRow>,
                lead: &str,
                text: &str,
                selected: bool,
                target: Option<QueueHitTarget>| {
        for (index, text) in beside_lead(lead, 0, text, width).into_iter().enumerate() {
            rows.push(BlockPageRow {
                text,
                kind: BlockRowKind::Dim,
                selected: index == 0 && selected,
                target,
            });
        }
    };
    push(
        &mut rows,
        "",
        &format!(
            "PAPER TRAIL · {} {}",
            entries.len(),
            if expanded { "▾" } else { "▸" }
        ),
        selected == Some(BlockTarget::TrailHeading),
        Some(QueueHitTarget::TrailHeading),
    );
    if !expanded {
        return (rows, stops);
    }
    for entry in &entries {
        let line = entry.line(now);
        let Some(index) = entry.record else {
            push(&mut rows, "", &line, false, None);
            continue;
        };
        let stop = BlockTarget::Trail(index);
        let open = form.block.trail_open.contains(&index);
        stops.push((stop, rows.len()));
        push(
            &mut rows,
            "",
            &format!("{line} {}", if open { "▾" } else { "▸" }),
            selected == Some(stop),
            Some(QueueHitTarget::TrailRecord(index)),
        );
        if open {
            if let Some(block) = task.past_blocks.get(index) {
                for detail in crate::activity::record_lines(block, now) {
                    push(
                        &mut rows,
                        &format!("  {}", detail.lead),
                        &detail.text,
                        false,
                        Some(QueueHitTarget::TrailRecord(index)),
                    );
                }
            }
        }
    }
    (rows, stops)
}

/// `text` wrapped beside `lead`, the lead padded to `column` cells (at least its own width) and
/// continuations aligned under the text. A lead that would leave the text fewer than a dozen
/// columns takes its own rows instead, and the text continues under it at a small indent.
fn beside_lead(lead: &str, column: usize, text: &str, width: usize) -> Vec<String> {
    const MIN_TEXT: usize = 12;
    let lead = terminal_text(lead);
    let text = terminal_text(text);
    let column = column.max(render::display_width(&lead));
    if lead.trim().is_empty() || width.saturating_sub(column) >= MIN_TEXT {
        let pad = " ".repeat(column - render::display_width(&lead));
        return wrap_text(&text, width.saturating_sub(column).max(1))
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                if index == 0 {
                    format!("{lead}{pad}{}", row.text)
                } else {
                    format!("{}{}", " ".repeat(column), row.text)
                }
            })
            .collect();
    }
    let margin = lead.len() - lead.trim_start().len();
    let indent = (margin + 2).min(width / 4);
    wrap_text(lead.trim(), width.saturating_sub(margin).max(1))
        .into_iter()
        .map(|part| format!("{}{}", " ".repeat(margin), part.text))
        .chain(
            wrap_text(&text, width.saturating_sub(indent).max(1))
                .into_iter()
                .map(|part| format!("{}{}", " ".repeat(indent), part.text)),
        )
        .collect()
}

/// The BLOCKED or REVIEW section's rows, each ring stop's row, and the reply box caret.
type BlockPageRows = (
    Vec<render::BlockPageRow>,
    Vec<(crate::ui::board::BlockTarget, usize)>,
    Option<(usize, u16)>,
);

/// Gap between the thread's `name · age` column and the reply text.
const THREAD_GAP: usize = 3;

/// The BLOCKED section of a blocked task's page or the REVIEW section of a task in review: the
/// top line (the row's live line), the body (why and needs, or done and next) as plain text, the
/// numbered options or the checks, the thread with the reply box when open, then the dim action
/// line and the closing rule. Also returns each ring stop's row and the reply box caret. Every
/// text wraps at `width`.
fn block_page_rows(
    model: &BoardModel,
    form: &BoardForm,
    task: &crate::domain::Task,
    width: usize,
) -> BlockPageRows {
    use crate::domain::OWNER;
    use crate::ui::board::BlockTarget;
    use render::{BlockPageRow, BlockRowKind};

    let Some(block) = task.block.as_ref() else {
        return (Vec::new(), Vec::new(), None);
    };
    let width = width.max(8);
    let now = model.now();
    // Selection belongs to the page form; another seat's form never paints it.
    let own = model
        .form
        .as_ref()
        .is_some_and(|own| std::ptr::eq(own, form));
    let selected = model.block_target().filter(|_| own);
    let mut rows: Vec<BlockPageRow> = Vec::new();
    let mut stops = Vec::new();
    let row = |text: String, kind: BlockRowKind, selected: bool| BlockPageRow {
        text,
        kind,
        selected,
        target: None,
    };
    // A click on a check cycles it; on the `N passed` line it shows or folds them.
    let click = |stop: Option<BlockTarget>| match stop {
        Some(BlockTarget::Check(index)) => Some(render::QueueHitTarget::PageCheck(index)),
        Some(BlockTarget::PassedFold) => Some(render::QueueHitTarget::PassedFold),
        _ => None,
    };
    // `lead` pads to `column` on the first wrapped row; continuations align under the text.
    let mut push = |rows: &mut Vec<BlockPageRow>,
                    lead: &str,
                    column: usize,
                    text: &str,
                    kind: BlockRowKind,
                    stop: Option<BlockTarget>| {
        if let Some(stop) = stop {
            stops.push((stop, rows.len()));
        }
        for (index, text) in beside_lead(lead, column, text, width)
            .into_iter()
            .enumerate()
        {
            rows.push(BlockPageRow {
                target: click(stop),
                ..row(text, kind, index == 0 && stop.is_some() && stop == selected)
            });
        }
    };
    let blank =
        |rows: &mut Vec<BlockPageRow>| rows.push(row(String::new(), BlockRowKind::Plain, false));

    let review = block.is_review();
    let top = render::status_line(task, &model.tasks, now)
        .unwrap_or_else(|| if review { "needs review" } else { "blocked" }.to_string());
    push(
        &mut rows,
        "",
        0,
        &top,
        BlockRowKind::Bold,
        Some(BlockTarget::Heading),
    );
    let body: Vec<&str> = if review {
        [block.done.as_deref(), block.next.as_deref()]
    } else {
        [block.why.as_deref(), block.needs.as_deref()]
    }
    .into_iter()
    .flatten()
    .collect();
    if !body.is_empty() {
        blank(&mut rows);
        for text in body {
            push(&mut rows, "", 0, text, BlockRowKind::Plain, None);
        }
    }
    if review {
        let (shown, passed) =
            crate::ui::board::block::review_check_order(block, form.block.fold.as_ref());
        if !block.checks.is_empty() {
            blank(&mut rows);
            push(&mut rows, "", 0, "Check", BlockRowKind::Dim, None);
        }
        for index in shown {
            let check = &block.checks[index];
            push(
                &mut rows,
                &format!(" {} ", render::check_glyph(check.state)),
                0,
                &check.text,
                BlockRowKind::Plain,
                Some(BlockTarget::Check(index)),
            );
        }
        if !passed.is_empty() {
            let open = form.block.passed_open;
            push(
                &mut rows,
                " ",
                0,
                &format!("{} passed {}", passed.len(), if open { "▾" } else { "▸" }),
                BlockRowKind::Dim,
                Some(BlockTarget::PassedFold),
            );
            if open {
                for index in passed {
                    push(
                        &mut rows,
                        &format!("   {} ", render::check_glyph(block.checks[index].state)),
                        0,
                        &block.checks[index].text,
                        BlockRowKind::Dim,
                        Some(BlockTarget::Check(index)),
                    );
                }
            }
        }
    } else if !block.options.is_empty() {
        blank(&mut rows);
        for (index, option) in block.options.iter().enumerate() {
            push(
                &mut rows,
                &format!("{:>2}  ", index + 1),
                0,
                option,
                BlockRowKind::Plain,
                Some(BlockTarget::Option(index)),
            );
        }
    }

    // The thread: `name · age` in one column wide enough for every lead, the text beside it.
    let name = |by: &str| {
        if by == OWNER {
            OWNER.to_string()
        } else {
            by.to_string()
        }
    };
    let leads: Vec<String> = block
        .replies
        .iter()
        .map(|reply| {
            let mut lead = format!(
                "{} · {}",
                name(&reply.by),
                render::format_age(now, reply.at)
            );
            if reply.edited && !reply.deleted {
                lead.push_str(" · edited");
            }
            lead
        })
        .collect();
    let editor = form.block.reply.as_ref().filter(|_| own);
    let editor_lead = editor.map(|editor| {
        if editor.edit.is_some() {
            format!("{OWNER} · edit")
        } else {
            OWNER.to_string()
        }
    });
    let column = leads
        .iter()
        .chain(editor_lead.iter())
        .map(|lead| render::display_width(&terminal_text(lead)))
        .max()
        .unwrap_or(0)
        + THREAD_GAP;
    if !leads.is_empty() || editor.is_some() {
        blank(&mut rows);
    }
    for ((index, reply), lead) in block.replies.iter().enumerate().zip(&leads) {
        if reply.deleted {
            push(&mut rows, lead, column, "deleted", BlockRowKind::Dim, None);
        } else {
            push(
                &mut rows,
                lead,
                column,
                &reply.text,
                BlockRowKind::Plain,
                Some(BlockTarget::Reply(index)),
            );
        }
    }
    let mut caret = None;
    if let (Some(editor), Some(lead)) = (editor, editor_lead) {
        // Too narrow for the column: the name takes its own row and the draft follows under it.
        let beside = width.saturating_sub(column) >= THREAD_MIN_TEXT;
        let indent = if beside { column } else { 2 };
        let field_width = width.saturating_sub(indent).max(1);
        editor.width.set(field_width);
        let (draft_rows, cursor_row, cursor_col) = wrapped_edit_rows(&editor.buffer, field_width);
        if !beside {
            rows.push(row(lead.clone(), BlockRowKind::Bold, false));
        }
        let first = rows.len();
        let empty = editor.buffer.value().is_empty();
        for (index, draft) in draft_rows.iter().enumerate() {
            let lead = if index == 0 && beside {
                format!(
                    "{lead}{}",
                    " ".repeat(column - render::display_width(&lead))
                )
            } else {
                " ".repeat(indent)
            };
            let text = if empty {
                editor.placeholder.as_str()
            } else {
                draft
            };
            rows.push(row(format!("{lead}{text}"), BlockRowKind::Bold, false));
        }
        if model.input_mode() == BoardInputMode::EditReply {
            caret = Some((
                first + cursor_row,
                u16::try_from(2 + indent + cursor_col).unwrap_or(u16::MAX),
            ));
        }
        if let Some(refusal) = editor.refusal.as_deref() {
            push(
                &mut rows,
                &" ".repeat(indent),
                0,
                refusal,
                BlockRowKind::Dim,
                None,
            );
        }
    }

    blank(&mut rows);
    let keys = section_action_keys(model, form, task, block, selected, own);
    for line in pack_keys(&keys, width) {
        rows.push(row(line, BlockRowKind::Dim, false));
    }
    rows.push(row(String::new(), BlockRowKind::Rule, false));
    blank(&mut rows);
    (rows, stops, caret)
}

/// Narrowest reply text worth keeping beside the thread's name column.
const THREAD_MIN_TEXT: usize = 12;

/// The section's action line: only keys that work right now. It follows the cursor (a check,
/// a step, the top line, your reply…) and gives way to the reply box's own keys while it is open.
fn section_action_keys(
    model: &BoardModel,
    form: &BoardForm,
    task: &crate::domain::Task,
    block: &crate::domain::Block,
    selected: Option<crate::ui::board::BlockTarget>,
    own: bool,
) -> Vec<String> {
    use crate::domain::OWNER;
    use crate::ui::board::BlockTarget;
    use crate::ui::queue::{block_wait, BlockWait};

    let review = block.is_review();
    if own && form.block.reply.is_some() {
        return super::chrome::reply_box_keys(review)
            .split(" · ")
            .map(str::to_string)
            .collect();
    }
    let mut keys: Vec<String> = Vec::new();
    if review {
        keys.push("tab next".to_string());
    }
    let cursor = if !own {
        None
    } else if form.steps.cursor.is_some() {
        Some("enter toggle step")
    } else if form.steps.add_selected {
        Some("enter add step")
    } else {
        match selected {
            Some(BlockTarget::Heading) => Some("ctrl+e edit"),
            Some(BlockTarget::Option(_)) => Some("enter choose"),
            Some(BlockTarget::Check(_)) => Some("enter mark"),
            Some(BlockTarget::PassedFold) if form.block.passed_open => Some("enter fold passed"),
            Some(BlockTarget::PassedFold) => Some("enter show passed"),
            Some(BlockTarget::Reply(index))
                if block
                    .replies
                    .get(index)
                    .is_some_and(|reply| reply.by == OWNER) =>
            {
                Some("ctrl+e edit · ctrl+x delete")
            }
            Some(BlockTarget::TrailHeading) if form.block.trail_expanded => Some("enter collapse"),
            Some(BlockTarget::TrailHeading | BlockTarget::Trail(_)) => Some("enter expand"),
            _ => None,
        }
    };
    keys.extend(
        cursor
            .into_iter()
            .flat_map(|keys| keys.split(" · "))
            .map(str::to_string),
    );
    if !review {
        match block.options.len() {
            0 => {}
            1 => keys.push("1 choose".to_string()),
            count => keys.push(format!("1-{} choose", count.min(9))),
        }
    }
    keys.push(if review { "r feedback" } else { "r reply" }.to_string());
    if review {
        keys.push("ctrl+s send back".to_string());
        keys.push("ctrl+d approve".to_string());
    } else if block_wait(task, &model.tasks) == Some(BlockWait::You) {
        keys.push("ctrl+s reply + unblock".to_string());
    } else {
        keys.push("ctrl+b unblock".to_string());
    }
    keys
}

/// Pack key legends into rows joined by ` · `, a new row where the next would not fit. A legend
/// wider than the row wraps on its own.
fn pack_keys(keys: &[String], width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for key in keys {
        let joined = if line.is_empty() {
            key.clone()
        } else {
            format!("{line} · {key}")
        };
        if render::display_width(&joined) <= width {
            line = joined;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if render::display_width(key) <= width {
            line = key.clone();
        } else {
            lines.extend(wrap_text(key, width.max(1)).into_iter().map(|row| row.text));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The reply box open under a blocked board row, with the block's why and needs above it; on a
/// review row, the feedback box with done and next above it.
fn row_reply_paint(model: &BoardModel) -> Option<render::RowReplyPaint<'_>> {
    let row = model.row_reply.as_ref()?;
    let block = model
        .tasks
        .iter()
        .find(|task| task.id == row.task)
        .and_then(|task| task.block.as_ref());
    let context = match block {
        Some(block) if block.is_review() => [
            block.done.as_deref().map(|done| ("done   ", done)),
            block.next.as_deref().map(|next| ("next   ", next)),
        ],
        _ => [
            block
                .and_then(|block| block.why.as_deref())
                .map(|why| ("why    ", why)),
            block
                .and_then(|block| block.needs.as_deref())
                .map(|needs| ("needs  ", needs)),
        ],
    };
    Some(render::RowReplyPaint {
        task: row.task,
        context,
        placeholder: &row.editor.placeholder,
        draft: &row.editor.buffer,
        width: &row.editor.width,
        refusal: row.editor.refusal.as_deref(),
        caret: model.input_mode() == BoardInputMode::EditReply,
    })
}

/// Idle status-row context for the active lens. Stored paths and thread names reach the
/// renderer only through its `present_line` path before they are painted.
fn status_idle(model: &BoardModel, surface: BoardSurface, has_message: bool) -> String {
    if !has_message {
        if let Some(notice) = model.update_notice() {
            return format!(" {notice}");
        }
    }
    footer_context(model, surface)
}

/// The idle context `status_idle` swaps in or out when a status message comes and goes
/// while an update notice is pending, so the footer can reserve rows for both.
fn status_reserve(model: &BoardModel, surface: BoardSurface, has_message: bool) -> Option<String> {
    let notice = model.update_notice()?;
    Some(if has_message {
        format!(" {notice}")
    } else {
        footer_context(model, surface)
    })
}

fn footer_context(model: &BoardModel, surface: BoardSurface) -> String {
    match surface {
        BoardSurface::Desk => " desk".to_string(),
        BoardSurface::Projects | BoardSurface::ThreadView => " projects".to_string(),
        BoardSurface::Project => {
            let path = model.archived_focus().or_else(|| model.active_project());
            let name = path
                .map(|path| render::short_project(&path.to_string_lossy()).to_string())
                .unwrap_or_else(|| "desk".to_string());
            let mut context = format!(" {name}");
            if model.focus_is_archived() {
                context.push_str(" · archived");
            } else if !model.board_filter().is_all() {
                context.push_str(&format!(" · {}", model.board_filter().label()));
            }
            context
        }
    }
}

/// The persistent navigation row's paint for this model: fixed tabs, slot 2's label,
/// and the active destination's right-side control.
fn nav_paint(model: &BoardModel) -> NavPaint {
    let slot2_label = match &model.board_location {
        // AC-41: the read-only focus says so in its slot.
        BoardLocation::ArchivedProject(path) => {
            format!("{} \u{b7} archived", project_option_label(path.as_path()))
        }
        BoardLocation::Project(path) => project_option_label(path.as_path()),
        // Slot 2 carries the remembered project even while another destination is active.
        BoardLocation::Desk | BoardLocation::Projects => model
            .selected_project()
            .map(project_option_label)
            .unwrap_or_else(|| "select project".to_string()),
    };
    let chip = match (&model.board_location, model.projects_view()) {
        (BoardLocation::Project(_), _) => Some(NavChipPaint {
            label: model.board_filter().label(),
            kind: NavChipKind::ThreadFilter,
        }),
        (BoardLocation::Projects, ProjectsView::Overview) => Some(NavChipPaint {
            label: "Overview".to_string(),
            kind: NavChipKind::ProjectsView,
        }),
        (BoardLocation::Projects, ProjectsView::Thread(name)) => Some(NavChipPaint {
            label: format!("#{name}"),
            kind: NavChipKind::ProjectsView,
        }),
        (BoardLocation::Projects, ProjectsView::Assignee(name)) => Some(NavChipPaint {
            label: format!("@{name}"),
            kind: NavChipKind::ProjectsView,
        }),
        _ => None,
    };
    NavPaint {
        active: model.nav_tab(),
        slot2_label,
        slot2_project: model.selected_project().is_some() || model.focus_is_archived(),
        chip,
    }
}

/// Which surface the list paints, from the destination and its View control.
fn board_surface(model: &BoardModel) -> BoardSurface {
    match model.effective_lens() {
        crate::ui::queue::BoardLens::Desk => BoardSurface::Desk,
        crate::ui::queue::BoardLens::Projects => BoardSurface::Projects,
        crate::ui::queue::BoardLens::ThreadView(_)
        | crate::ui::queue::BoardLens::AssigneeView(_) => BoardSurface::ThreadView,
        crate::ui::queue::BoardLens::Project(_)
        | crate::ui::queue::BoardLens::ArchivedProject(_) => BoardSurface::Project,
    }
}

/// Draw the board into any ratatui frame (live TTY or [`ratatui::backend::TestBackend`]).
///
/// the paints the queue frame via [`render::draw_queue_frame`]. Classic master-detail
/// chrome is retired; overlays that still need the classic layout (edit band, save
/// recovery banner via message) are layered lightly on top where session mode requires it.
pub fn draw_board(frame: &mut Frame, model: &BoardModel) -> render::QueueHitMap {
    let hits = draw_board_impl(frame, model);
    if let Some(selection) = model.text_selection() {
        crate::ui::text_select::paint_selection(frame, &selection, &hits.copyable);
    }
    hits
}

/// The mouse hit-map for one board frame, without a live terminal.
///
/// [`draw_board`] is the one painter every board size uses; this renders the identical
/// frame into a scratch buffer purely to recover the hit-map [`draw_board_impl`] builds
/// beside the paint, so the live mouse loop and the screen the user is looking at can
/// never disagree about where a control is -- there is no second, hand-maintained copy of
/// the geometry to drift out of step with a renderer change.
pub fn board_hit_map(area: ratatui::layout::Rect, model: &BoardModel) -> render::QueueHitMap {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let width = area.width.max(1);
    let height = area.height.max(1);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("scratch terminal");
    let mut hits = render::QueueHitMap::default();
    let _ = terminal.draw(|frame| {
        hits = draw_board_impl(frame, model);
    });
    hits
}

/// Board-level surfaces whose payloads must outlive the overlay borrowing them.
struct OverlayPayloads {
    help_lines: Vec<String>,
    palette_commands: Vec<PaletteCommandRow>,
    scope_options: Vec<String>,
    list_picker_options: Vec<String>,
    list_picker_query: Option<String>,
    list_picker_selected: usize,
    scope_tabs: Option<render::PickerTabsPaint>,
    launch_card_name: Option<String>,
    scope_selected: usize,
}

impl OverlayPayloads {
    fn collect(model: &BoardModel) -> Self {
        let help_lines = if model.input_mode() == BoardInputMode::Help {
            help_card_lines_for_query(model.help_query())
        } else {
            Vec::new()
        };
        let palette_commands: Vec<PaletteCommandRow> =
            if model.command_surface() == CommandSurface::Palette {
                let visible = model.visible_commands();
                let selected = model.command_selected();
                visible
                    .iter()
                    .enumerate()
                    .map(|(i, cmd)| PaletteCommandRow {
                        label: cmd.label.clone(),
                        selected: Some(i) == selected,
                    })
                    .collect()
            } else {
                Vec::new()
            };
        let scope_options: Vec<String> = if model.popup() == BoardPopup::ProjectPicker {
            let labels = match model.picker_tab() {
                Some(PickerTab::Archived) => model
                    .archived_project_options()
                    .iter()
                    .map(|path| {
                        project_scope_option_label(&ProjectScopeOption::Project(path.clone()))
                    })
                    .collect(),
                _ => model
                    .project_options()
                    .iter()
                    .map(project_scope_option_label)
                    .collect(),
            };
            labels
        } else if model.input_mode() == BoardInputMode::FormDropdown {
            match model.form.as_ref().map(|form| form.focus) {
                Some(CaptureField::Scope) => model
                    .form_scope_options()
                    .iter()
                    .map(|scope| match scope {
                        TaskScope::Global => "desk".to_string(),
                        TaskScope::Project { path } => render::short_project(path).to_string(),
                    })
                    .collect(),
                Some(CaptureField::Assignee) => model
                    .form
                    .as_ref()
                    .map(|form| {
                        form.assignee_options
                            .iter()
                            .map(|option| option.clone().unwrap_or_else(|| "none".to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                Some(CaptureField::Base) | None => Vec::new(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let active = model.list_picker_active();
        let list_picker_options = model
            .visible_list_picker_options()
            .into_iter()
            .map(|(index, option)| {
                if option.value == super::model::ListPickerValue::Unavailable {
                    format!("({})", option.label)
                } else {
                    let mut row = match option.count {
                        Some(count) => format!("{}  {count}", option.label),
                        None => option.label,
                    };
                    // The tabbed filter pickers mark the choice the board applies.
                    if active == Some(index) {
                        row.push_str("  \u{2713}");
                    }
                    row
                }
            })
            .collect();
        let list_picker_query = model.list_picker_query().map(str::to_string);
        let list_picker_selected = model.list_picker_selected();
        let scope_selected = if model.popup() == BoardPopup::ProjectPicker {
            model.project_picker_index().unwrap_or(0)
        } else {
            model
                .form
                .as_ref()
                .map(|form| match form.focus {
                    CaptureField::Scope => form.scope_selected,
                    CaptureField::Assignee => form.assignee_selected,
                    CaptureField::Base => 0,
                    _ => 0,
                })
                .unwrap_or(0)
        };
        let launch_card_name = model.launch_card.as_deref().map(|path| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&path.to_string_lossy())
                .to_string()
        });
        let scope_tabs = model
            .picker_tab()
            .map(|tab| render::PickerTabsPaint::Project {
                archived_active: tab == PickerTab::Archived,
                archived_count: model.archived_project_options().len(),
            });
        Self {
            help_lines,
            palette_commands,
            scope_options,
            list_picker_options,
            list_picker_query,
            list_picker_selected,
            scope_selected,
            scope_tabs,
            launch_card_name,
        }
    }

    /// The board-level modal or capture surface that outranks the list and the page, if any.
    fn modal<'a>(
        &'a self,
        model: &'a BoardModel,
        geo: &tier::TierGeometry,
    ) -> Option<QueueOverlay<'a>> {
        if model.input_mode() == BoardInputMode::Search {
            let input_width = (geo.row_width as usize).saturating_sub(2);
            let query = EditBuffer::new(model.search_query(), model.search_query().chars().count());
            let (text, cursor_col) = escaped_line_window(&query, input_width);
            return Some(QueueOverlay::Search {
                input: crate::ui::render::BottomInputSlot {
                    text,
                    cursor_col,
                    placeholder: if model.projects_overview() {
                        "search projects…"
                    } else {
                        "search tasks…"
                    },
                    refusal: None,
                    message: model.message(),
                    above_rows: Vec::new(),
                    cursor_row_offset: 0,
                },
            });
        }
        if let Some(quick_add) = model.quick_add.as_ref().filter(|_| {
            matches!(
                model.input_mode(),
                BoardInputMode::QuickAdd | BoardInputMode::SaveRecovery
            )
        }) {
            let input_width = (geo.row_width as usize).saturating_sub(2);
            // A long title wraps instead of scrolling sideways. Exactly ONE row above
            // the input is reserved (the message row: the shifted rule sits two up),
            // so the draft may span at most two painted rows, windowed by the
            // minimum that keeps the caret's wrapped row visible. Longer drafts
            // window vertically rather than touching chrome.
            const QUICK_ADD_MAX_ROWS: usize = 2;
            let (all_rows, cursor_row, cursor_column) =
                wrapped_edit_rows(&quick_add.title, input_width);
            let shown = all_rows.len().clamp(1, QUICK_ADD_MAX_ROWS);
            let start = cursor_row
                .min(all_rows.len().saturating_sub(1))
                .saturating_sub(shown - 1);
            let window: Vec<String> = all_rows[start..(start + shown).min(all_rows.len())].to_vec();
            let caret_index = cursor_row.saturating_sub(start);
            let title = window.last().cloned().unwrap_or_default();
            // Continuations paint top-first (the painter stacks them upward by index).
            let above_rows: Vec<String> = window[..window.len().saturating_sub(1)].to_vec();
            let multiline = window.len() > 1;
            // The row names its destination so Enter never has to move the user's
            // view to prove where the task went.
            let destination = match &quick_add.scope {
                TaskScope::Project { path } => project_option_label(Path::new(path)).to_string(),
                TaskScope::Global => "desk".to_string(),
            };
            return Some(QueueOverlay::QuickAdd {
                input: crate::ui::render::BottomInputSlot {
                    text: title,
                    cursor_col: u16::try_from(cursor_column).unwrap_or(u16::MAX),
                    placeholder: "title…   !p project · !t thread · !a assignee · !b base",
                    refusal: None,
                    // Save recovery owns the verb row; ordinary quick-add refusals
                    // use the shared slot's reserved row above the cursor. A wrapped
                    // draft owns those rows itself, so the message yields while it is
                    // up and returns when the draft is back under the cap.
                    message: (!multiline && model.input_mode() != BoardInputMode::SaveRecovery)
                        .then(|| model.message())
                        .flatten(),
                    above_rows,
                    cursor_row_offset: u16::try_from(
                        window.len().saturating_sub(1).saturating_sub(caret_index),
                    )
                    .unwrap_or(0),
                },
                destination,
                recovery: model.input_mode() == BoardInputMode::SaveRecovery,
            });
        }
        if model.input_mode() == BoardInputMode::Help {
            return Some(QueueOverlay::Help {
                query: model.help_query(),
                lines: &self.help_lines,
                scroll: model.help_scroll(),
            });
        }
        if model.command_surface() == CommandSurface::Palette {
            return Some(QueueOverlay::Palette {
                query: model.command_query(),
                commands: &self.palette_commands,
            });
        }
        if let Some(prompt) = model.cleanup_prompt() {
            return Some(match model.cleanup_run() {
                Some(run) => cleanup_run_overlay(prompt, &run),
                None => cleanup_overlay(prompt, home_dir().as_deref()),
            });
        }
        if let Some(card) = model.block_card() {
            return Some(block_card_overlay(model, card));
        }
        if let Some(prompt) = model.dispatch_prompt() {
            return Some(dispatch_overlay(prompt, |project| {
                model.default_branch_name(project)
            }));
        }
        if let Some(name) = self.launch_card_name.as_deref() {
            return Some(QueueOverlay::LaunchCard { name });
        }
        if model.input_mode() == BoardInputMode::ListPicker {
            let title = match model.list_picker_kind() {
                Some(crate::ui::board::ListPickerKind::ProjectsView) => "projects View",
                Some(crate::ui::board::ListPickerKind::Assignee) => "assignee",
                Some(crate::ui::board::ListPickerKind::Base) => "base",
                _ => "Filter",
            };
            return Some(QueueOverlay::ScopeDropdown {
                options: &self.list_picker_options,
                selected: self
                    .list_picker_selected
                    .min(self.list_picker_options.len().saturating_sub(1)),
                tabs: model
                    .list_picker_tab()
                    .map(|tab| render::PickerTabsPaint::Filter {
                        assignees_active: tab == crate::ui::board::FilterTab::Assignees,
                    }),
                title: Some(title),
                query: self.list_picker_query.as_deref(),
            });
        }
        if model.popup() == BoardPopup::ProjectPicker {
            return Some(QueueOverlay::ScopeDropdown {
                options: &self.scope_options,
                selected: self.scope_selected,
                tabs: self.scope_tabs,
                title: None,
                query: None,
            });
        }
        None
    }

    /// The open task page, painted from the retained form at `geo`.
    fn task_page<'a>(
        &'a self,
        model: &'a BoardModel,
        form: &'a BoardForm,
        geo: &tier::TierGeometry,
        column: bool,
    ) -> QueueOverlay<'a> {
        let scope_dropdown =
            (model.input_mode() == BoardInputMode::FormDropdown).then_some(FormDropdown {
                options: &self.scope_options,
                selected: self.scope_selected,
                field: form.focus,
                anchor_x: 0,
            });
        build_task_page_overlay(model, form, geo, scope_dropdown, column)
    }
}

/// Status row content: delete recovery notice (title + u undo hint) when armed; otherwise
/// the last action message, otherwise counts. When both channels are set (stale undo
/// refusal after delete), compose notice first then message so the refusal is visible.
fn status_row_content(model: &BoardModel) -> (Option<String>, Option<usize>, Option<usize>) {
    let editing_on_page = model.open_field_edit().is_some() && model.form.is_some();
    let bulk_count = model.visible_delete_notice_count();
    let notice = model.visible_delete_notice().map(|title| match bulk_count {
        Some(count) => format!(
            "deleted {count} {} · {BULK_DELETE_NOTICE_UNDO}",
            if count == 1 { "task" } else { "tasks" }
        ),
        None => notice_framed(title, true),
    });
    let status_owned = match (notice.as_deref(), model.message()) {
        (Some(notice), Some(msg)) => Some(format!("{notice}  ·  {msg}")),
        (Some(notice), None) => Some(notice.to_string()),
        (None, Some(msg)) => Some(msg.to_string()),
        (None, None) if editing_on_page => Some("editing…".to_string()),
        // Under a bulk cleanup card, Esc cancels the card and keeps the marks.
        (None, None)
            if model.mark_mode_active()
                && (model.cleanup_prompt().is_some() || model.dispatch_prompt().is_some()) =>
        {
            Some(format!("multi-select · {} selected", model.marked_count()))
        }
        (None, None) if model.mark_mode_active() && model.marked_count() > 0 => Some(format!(
            "multi-select · {} selected · esc clears",
            model.marked_count()
        )),
        (None, None) if model.mark_mode_active() => {
            Some("multi-select · space/click marks · esc exits".to_string())
        }
        _ => None,
    };
    // Compute the clickable control from the exact notice text, never by searching user text.
    let undo_control = bulk_count
        .map(|_| BULK_DELETE_NOTICE_UNDO)
        .or_else(|| notice.as_ref().map(|_| DELETE_NOTICE_UNDO));
    let status_undo_offset = notice
        .as_deref()
        .zip(undo_control)
        .map(|(notice, control)| row_width(notice).saturating_sub(row_width(control)));
    let status_undo_width = undo_control.map(row_width);
    (status_owned, status_undo_offset, status_undo_width)
}

/// Field named in the task header's `editing <field>` state slot while an editor is active.
fn editing_field(model: &BoardModel) -> Option<&'static str> {
    match model.input_mode() {
        BoardInputMode::EditTitle => Some("title"),
        BoardInputMode::EditNotes => Some("notes"),
        BoardInputMode::SelectThread | BoardInputMode::EditThread => Some("thread"),
        BoardInputMode::EditScope => Some("scope"),
        BoardInputMode::EditAssignee => Some("assignee"),
        BoardInputMode::FormDropdown => match model.form_focus() {
            Some(CaptureField::Scope) => Some("scope"),
            Some(CaptureField::Assignee) => Some("assignee"),
            _ => None,
        },
        BoardInputMode::EditStep => Some("step"),
        _ => None,
    }
}

/// Header state slot for the task column: `status · project`, `editing <field>`, or
/// `unsaved` while a dirty draft waits with no editor active.
fn task_header_state(model: &BoardModel, form: &BoardForm, task: &crate::domain::Task) -> String {
    if let Some(field) = editing_field(model).filter(|_| form.task_id() == Some(task.id)) {
        return format!("editing {field}");
    }
    let is_retained = model
        .form
        .as_ref()
        .is_some_and(|retained| retained.task_id() == Some(task.id));
    if is_retained && model.task_session_dirty() {
        return "unsaved".to_string();
    }
    let status = if task.archived {
        "archived"
    } else {
        match task.status {
            HumanStatus::Open => "open",
            HumanStatus::Ready => "ready",
            HumanStatus::Started => "started",
            HumanStatus::Blocked => "blocked",
            HumanStatus::Review => "review",
            HumanStatus::Done => "done",
        }
    };
    let project = match &task.scope {
        TaskScope::Project { path } => render::short_project(path).to_string(),
        TaskScope::Global => "desk".to_string(),
    };
    format!("{status} · {project}")
}

/// The dim stage crumb and key hints for the wide status row.
fn wide_status_hint(model: &BoardModel) -> (Option<&'static str>, &'static str) {
    if model.projects_overview() {
        return match model.wide_stage() {
            tier::WideStage::FullBoard => (None, "→ project pane"),
            tier::WideStage::Split => (Some("index ▸ project"), "→ project · ← close"),
            tier::WideStage::Rail if model.has_unsaved_work() => (Some("index ◂ project"), ""),
            tier::WideStage::Rail => (Some("index ◂ project"), "← index"),
            tier::WideStage::FullTask => (None, ""),
        };
    }
    // Save/cancel keys apply only while a field editor is actually open or the parked draft
    // is dirty; a clean parked session shows the stage's normal keys again.
    let field_editor =
        model.open_field_edit().is_some() || model.input_mode() == BoardInputMode::FormDropdown;
    let editing = field_editor || model.task_session_dirty();
    // Stage navigation only: the verb row already carries the surface's keys, so the hint
    // never repeats them.
    match model.wide_stage() {
        tier::WideStage::FullBoard => (None, "→ task pane"),
        tier::WideStage::Split => (Some("board ▸ task"), "→ task · ← close"),
        tier::WideStage::Rail if editing => (Some("board ◂ task"), ""),
        tier::WideStage::Rail => (Some("board ◂ task"), "← board · → full page"),
        tier::WideStage::FullTask if editing => (None, ""),
        tier::WideStage::FullTask => (None, "← rail"),
    }
}

fn draw_board_impl(frame: &mut Frame, model: &BoardModel) -> render::QueueHitMap {
    let hits = draw_board_hits(frame, model);
    // Renderer-recorded horizon for the help card, like the list's max scroll: the reducer
    // clamps with it so the offset never runs past the last page.
    if let Some(max_scroll) = hits.help_max_scroll {
        model.help_max_scroll.set(max_scroll);
    }
    if let Some(max_scroll) = hits.cleanup_max_scroll {
        model.cleanup_max_scroll.set(max_scroll);
    }
    hits
}

fn draw_board_hits(frame: &mut Frame, model: &BoardModel) -> render::QueueHitMap {
    let area = frame.area();
    // A capture draft (quick-add expanded with Tab) owns the whole frame at every width; the
    // wide task column paints task forms only. `responsive_geometry` folds that rule in.
    let responsive = model.responsive_geometry(area);
    if responsive.presentation == tier::ResponsivePresentation::WideSplit {
        return draw_wide_board(frame, model, area, responsive);
    }
    let geo = tier::resolve(area.width, area.height);
    let queue_view = model.queue_view();
    let selection_id = model.saved_task.or(model.selection_id);
    let surface = board_surface(model);
    let (status_owned, status_undo_offset, status_undo_width) = status_row_content(model);
    // The verb bar entries: computed from the selection and the open surface so the label
    // is true for the row it describes, and drawn from this one function -- the same one
    // the goldens call -- so a later edit to either side cannot silently re-open wording
    // drift.
    let verbs = board_verb_items(model);
    let payloads = OverlayPayloads::collect(model);
    let overlay = match payloads.modal(model, &geo) {
        Some(modal) => modal,
        None => match model.form.as_ref() {
            // A parked task page is not painted while the board owns the frame.
            Some(form)
                if !(model.focused_surface() == tier::FocusedSurface::Board && form.is_task()) =>
            {
                payloads.task_page(model, form, &geo, false)
            }
            _ => QueueOverlay::None,
        },
    };
    let frame_model = QueueFrameModel {
        tasks: &model.tasks,
        view: &queue_view,
        selection_id,
        marked_ids: model.marked_ids.clone(),
        nav: nav_paint(model),
        surface,
        projects: &queue_view.projects,
        projects_index: surface == BoardSurface::Projects
            && matches!(model.projects_view(), ProjectsView::Overview),
        projects_cursor: model.projects_cursor(),
        search_query: model.search_query(),
        search_pinned: model.search_pinned(),
        summary: None,
        context: status_idle(model, surface, status_owned.is_some()),
        reserve_context: status_reserve(model, surface, status_owned.is_some()),
        has_update_notice: model.update_notice().is_some(),
        status_message: status_owned.as_deref(),
        status_undo_offset,
        status_undo_width,
        verb_items: &verbs,
        now: model.now(),
        overlay,
        detail_open: model.detail_open,
        row_reply: row_reply_paint(model),
        list_scroll: model.list_scroll.get(),
        follow_list: model.follow_list.get(),
        archived_collapsed: model.archived_collapsed,
        archived_header_selected: model.archived_header_selected(),
        inbox_collapsed: model.inbox_collapsed,
        inbox_header_selected: model.inbox_header_selected(),
        rows_dim: model.focus_is_archived(),
    };
    let (hits, painted_list_scroll) = render::draw_queue_frame(frame, &frame_model, &geo, area);
    if let Some((scroll, max_scroll)) = painted_list_scroll {
        model.list_scroll.set(scroll);
        model.list_max_scroll.set(max_scroll);
    }
    paint_board_form_toast(frame, model, &geo, area);
    hits
}

/// Board-form edits use the task page's status and verb rows rather than an inline rule row.
fn paint_board_form_toast(
    frame: &mut Frame,
    model: &BoardModel,
    geo: &tier::TierGeometry,
    area: ratatui::layout::Rect,
) {
    if model.open_field_edit().is_some() && model.form.is_none() {
        if let Some(row) = geo.rule_row {
            let toast_area =
                ratatui::layout::Rect::new(area.x, area.y.saturating_add(row), area.width, 1);
            // Use the mono-only helpers so no product frame emits foreground or background
            // color SGR codes.
            frame.render_widget(
                Paragraph::new(present_line(
                    &model.edit_chrome_row(toast_area.width as usize),
                    toast_area.width as usize,
                ))
                .style(render::style_bold()),
                toast_area,
            );
        }
    }
}

/// The wide stage slider: unboxed columns, one rule column, one shared footer.
///
/// Stage 0 is the board at full width, A splits board and task page, G puts the dim rail
/// beside the page, F is the page at full width. Focus follows the stage, and the footer
/// (rule, status row with its stage crumb, verb bar) is painted once across the frame.
fn draw_wide_board(
    frame: &mut Frame,
    model: &BoardModel,
    area: ratatui::layout::Rect,
    responsive: tier::ResponsiveGeometry,
) -> render::QueueHitMap {
    if model.projects_overview()
        && matches!(
            model.wide_stage(),
            tier::WideStage::Split | tier::WideStage::Rail
        )
    {
        return draw_projects_wide_board(frame, model, area, responsive);
    }
    let stage = model.wide_stage();
    let task_focus = model.focused_surface() == tier::FocusedSurface::Task;
    let density = responsive.density;
    let queue_view = model.queue_view();
    let selection_id = model.saved_task.or(model.selection_id);
    let selected_task = selection_id.and_then(|id| model.tasks.iter().find(|task| task.id == id));
    let surface = board_surface(model);
    let (status_owned, status_undo_offset, status_undo_width) = status_row_content(model);
    let verbs = board_verb_items(model);
    let payloads = OverlayPayloads::collect(model);
    let frame_geo = tier::resolve_density(area.width, area.height, density);
    let modal = payloads.modal(model, &frame_geo);

    // The footer's owner decides its rows: a bottom input, the palette query, or the verbs
    // of the focused surface. Column heights follow the footer's (possibly shifted) rule.
    let footer_needs_input = modal.as_ref().is_some_and(render::has_bottom_input)
        || (task_focus && model.input_mode() == BoardInputMode::EditThread);
    let footer_geo = render::bottom_input_geometry(frame_geo, footer_needs_input);
    let column_height = footer_geo.rule_row.unwrap_or(area.height);
    let column_geo = |width: u16| tier::resolve_column(width, column_height, area.height, density);
    let column_rect = |rect: ratatui::layout::Rect| {
        ratatui::layout::Rect::new(rect.x, rect.y, rect.width, column_height.min(rect.height))
    };

    // The task column paints the retained page when it is bound to the selection (or owns
    // focus), otherwise a fresh view of the selected task. Nothing here touches the model.
    let retained = model
        .form
        .as_ref()
        .filter(|form| form.is_task() && (task_focus || form.task_id() == selection_id));
    let preview_form = (!task_focus && retained.is_none())
        .then(|| {
            selected_task.map(|task| {
                BoardForm::task(
                    task,
                    model.this_repo.as_deref(),
                    &model.tasks,
                    CaptureField::Title,
                    &model.archived_projects,
                    &model.agent_names,
                )
            })
        })
        .flatten();
    let task_form = retained.or(preview_form.as_ref());
    let task_area = responsive.task_content();
    let task_geo = (task_area.width > 0).then(|| column_geo(task_area.width));
    let task_overlay = match (task_geo.as_ref(), task_form) {
        (Some(geo), Some(form)) => payloads.task_page(model, form, geo, true),
        (Some(_), None) => QueueOverlay::None,
        (None, _) => QueueOverlay::None,
    };
    let header_task = task_form.and_then(|form| {
        form.task_id()
            .and_then(|id| model.tasks.iter().find(|task| task.id == id))
            .map(|task| (form, task))
    });
    let header_identifier = header_task.and_then(|(_, task)| task.board_identifier());
    let header_state = header_task.map(|(form, task)| task_header_state(model, form, task));
    let editing_title = task_focus && model.input_mode() == BoardInputMode::EditTitle;
    let header_title: Option<(String, Option<u16>)> = header_task.map(|(form, task)| {
        if editing_title {
            let width = task_geo.map(|geo| geo.row_width as usize).unwrap_or(0);
            let glyph_w = render::display_width(render::task_status_glyph(task));
            let id_w = header_identifier
                .as_deref()
                .map(render::display_width)
                .unwrap_or(0);
            // The painted state slot keeps one trailing pad cell.
            let state_w = header_state
                .as_deref()
                .map(render::display_width)
                .unwrap_or(0)
                + 1;
            let room = render::task_header_title_room(width, glyph_w, id_w, state_w);
            let (text, cursor) = escaped_line_window(&form.title, room.max(1));
            (text, Some(cursor))
        } else {
            (form.title.value().to_string(), None)
        }
    });
    let header = header_title
        .as_ref()
        .map(|(title, cursor)| render::TaskColumnHeader {
            glyph: header_task
                .map(|(_, task)| render::task_status_glyph(task))
                .unwrap_or("○"),
            identifier: header_identifier.as_deref(),
            identifier_task: header_task.and_then(|(form, _)| form.task_id()),
            title,
            title_cursor_col: *cursor,
            state: header_state.as_deref().unwrap_or_default(),
            bold: task_focus,
        });

    let board_frame = QueueFrameModel {
        tasks: &model.tasks,
        view: &queue_view,
        selection_id,
        marked_ids: model.marked_ids.clone(),
        nav: nav_paint(model),
        surface,
        projects: &queue_view.projects,
        projects_index: surface == BoardSurface::Projects
            && matches!(model.projects_view(), ProjectsView::Overview),
        projects_cursor: model.projects_cursor(),
        search_query: model.search_query(),
        search_pinned: model.search_pinned(),
        summary: None,
        context: status_idle(model, surface, status_owned.is_some()),
        reserve_context: status_reserve(model, surface, status_owned.is_some()),
        has_update_notice: model.update_notice().is_some(),
        status_message: status_owned.as_deref(),
        status_undo_offset,
        status_undo_width,
        verb_items: &verbs,
        now: model.now(),
        overlay: if task_focus {
            QueueOverlay::None
        } else {
            modal.clone().unwrap_or(QueueOverlay::None)
        },
        detail_open: None,
        row_reply: row_reply_paint(model),
        list_scroll: model.list_scroll.get(),
        follow_list: model.follow_list.get(),
        archived_collapsed: model.archived_collapsed,
        archived_header_selected: model.archived_header_selected(),
        inbox_collapsed: model.inbox_collapsed,
        inbox_header_selected: model.inbox_header_selected(),
        rows_dim: model.focus_is_archived(),
    };
    let task_frame = QueueFrameModel {
        overlay: task_overlay.clone(),
        list_scroll: 0,
        follow_list: false,
        projects: &[],
        projects_index: false,
        ..board_frame.clone()
    };
    let footer_frame = QueueFrameModel {
        overlay: match (&modal, task_focus) {
            (Some(modal), _) => modal.clone(),
            (None, true) => task_overlay.clone(),
            (None, false) => QueueOverlay::None,
        },
        ..board_frame.clone()
    };

    let mut hits = render::QueueHitMap::default();
    let board_area = responsive.board;
    if board_area.width > 0 {
        let board_geo = column_geo(board_area.width);
        if stage == tier::WideStage::Rail {
            let rail_frame = QueueFrameModel {
                follow_list: true,
                ..board_frame.clone()
            };
            let mut rail_hits =
                render::draw_rail_frame(frame, &rail_frame, &board_geo, column_rect(board_area));
            hits.regions.append(&mut rail_hits.regions);
            hits.copyable.append(&mut rail_hits.copyable);
        } else {
            let (mut board_hits, painted_list_scroll) =
                render::draw_queue_frame(frame, &board_frame, &board_geo, column_rect(board_area));
            hits.regions.append(&mut board_hits.regions);
            hits.copyable.append(&mut board_hits.copyable);
            hits.help_max_scroll = hits.help_max_scroll.or(board_hits.help_max_scroll);
            hits.cleanup_max_scroll = hits.cleanup_max_scroll.or(board_hits.cleanup_max_scroll);
            if let Some((scroll, max_scroll)) = painted_list_scroll {
                model.list_scroll.set(scroll);
                model.list_max_scroll.set(max_scroll);
            }
        }
    }
    if responsive.rule.width > 0 {
        let rule = column_rect(responsive.rule);
        for y in rule.top()..rule.bottom() {
            frame.render_widget(
                Paragraph::new(render::paint_bounded_line("│", 1, render::style_dim())),
                ratatui::layout::Rect::new(rule.x, y, 1, 1),
            );
        }
    }
    if let Some(task_geo) = task_geo.as_ref() {
        let mut task_hits = render::draw_task_column(
            frame,
            &task_frame,
            task_geo,
            column_rect(task_area),
            header,
            modal.as_ref().filter(|_| task_focus),
        );
        // A board-owned preview keeps its control hits, but only the focus router reads
        // them: a click there moves the stage first, then dispatches against this frame.
        hits.regions.append(&mut task_hits.regions);
        hits.copyable.append(&mut task_hits.copyable);
    }
    let (crumb, keys) = wide_status_hint(model);
    let mut footer_hits = render::draw_queue_footer(
        frame,
        &footer_frame,
        &footer_geo,
        area,
        Some(render::StatusHint { crumb, keys }),
    );
    hits.regions.append(&mut footer_hits.regions);
    hits.copyable.append(&mut footer_hits.copyable);
    hits.footer = footer_hits.footer;
    paint_board_form_toast(frame, model, &footer_geo, area);
    hits
}

/// Draw the projects overview's two-stage preview. The left seat remains the index, while the
/// right seat is a nested project-board session whose footer becomes the shared footer in Rail.
fn draw_projects_wide_board(
    frame: &mut Frame,
    model: &BoardModel,
    area: ratatui::layout::Rect,
    responsive: tier::ResponsiveGeometry,
) -> render::QueueHitMap {
    let stage = model.wide_stage();
    let density = responsive.density;
    let frame_geo = tier::resolve_density(area.width, area.height, density);
    let outer_view = model.queue_view();
    let outer_status = status_row_content(model);
    let outer_payloads = OverlayPayloads::collect(model);
    let outer_modal = if model.project_right_seat_focused() {
        None
    } else {
        outer_payloads.modal(model, &frame_geo)
    };

    let right = model.right_seat();
    let right_project_name = right
        .and_then(BoardModel::active_project)
        .map(project_option_label);
    let right_view = right.map(BoardModel::queue_view);
    let right_payloads = right.map(OverlayPayloads::collect);
    let right_area = responsive.task_content();
    // Modal payloads only need the right seat's row width here. The final column height is
    // resolved below after the shared footer has reserved any bottom input rows.
    let right_modal_geo = tier::resolve_density(right_area.width, area.height, density);
    let right_modal = right.and_then(|right| {
        right_payloads
            .as_ref()
            .and_then(|payloads| payloads.modal(right, &right_modal_geo))
    });
    let right_focused = stage == tier::WideStage::Rail && right.is_some();
    let footer_needs_input = if right_focused {
        right_modal.as_ref().is_some_and(render::has_bottom_input)
            || right.is_some_and(|right| right.input_mode() == BoardInputMode::EditThread)
    } else {
        outer_modal.as_ref().is_some_and(render::has_bottom_input)
    };
    let footer_geo = render::bottom_input_geometry(frame_geo, footer_needs_input);
    let column_height = footer_geo.rule_row.unwrap_or(area.height);
    let column_geo = |width: u16| tier::resolve_column(width, column_height, area.height, density);
    let column_rect = |rect: ratatui::layout::Rect| {
        ratatui::layout::Rect::new(rect.x, rect.y, rect.width, column_height.min(rect.height))
    };

    let right_geo = (right_area.width > 0).then(|| column_geo(right_area.width));
    let right_status = right.map(status_row_content);
    let right_verbs = right.map(board_verb_items);
    let right_task_overlay = match (right, right_geo.as_ref(), right_payloads.as_ref()) {
        (Some(right), Some(geo), Some(payloads)) => right
            .form
            .as_ref()
            .filter(|form| {
                !(right.focused_surface() == tier::FocusedSurface::Board && form.is_task())
            })
            .map(|form| payloads.task_page(right, form, geo, true)),
        _ => None,
    };
    let right_overlay = right_modal.clone().or(right_task_overlay.clone());

    // A task page in the preview column uses the same two-row header as the ordinary wide task
    // column. The page payload deliberately omits that header when `column` is true, so build it
    // here before the shared footer is painted.
    let mut right_header_identifier = None;
    let mut right_header_title = String::new();
    let mut right_header_state = String::new();
    let mut right_header_glyph = "○";
    let mut right_header_task = None;
    let mut right_header_visible = false;
    let mut right_header_title_cursor = None;
    if let (Some(right), Some(geo), Some(_)) =
        (right, right_geo.as_ref(), right_task_overlay.as_ref())
    {
        if let Some(form) = right.form.as_ref() {
            if let Some(task_id) = form.task_id() {
                if let Some(task) = right.tasks.iter().find(|task| task.id == task_id) {
                    right_header_visible = true;
                    right_header_task = Some(task.id);
                    right_header_glyph = render::task_status_glyph(task);
                    right_header_identifier = task.board_identifier();
                    right_header_state = task_header_state(right, form, task);
                }
            } else if !form.is_task() {
                // Expanded quick-add has no durable task to bind, but it still owns the right
                // column. Keep its draft title in the same header slot as a saved task instead
                // of letting draw_task_column replace it with the empty-pane hint.
                right_header_visible = true;
                right_header_glyph = render::status_glyph(HumanStatus::Ready);
                right_header_state = editing_field(right)
                    .map(|field| format!("editing {field}"))
                    .unwrap_or_else(|| "ready".to_string());
            }
            if right_header_visible {
                let width = geo.row_width as usize;
                let glyph_width = render::display_width(right_header_glyph);
                let identifier_width = right_header_identifier
                    .as_deref()
                    .map(render::display_width)
                    .unwrap_or(0);
                let state_width = render::display_width(&right_header_state) + 1;
                let room = render::task_header_title_room(
                    width,
                    glyph_width,
                    identifier_width,
                    state_width,
                );
                if right.input_mode() == BoardInputMode::EditTitle {
                    let (title, cursor) = escaped_line_window(&form.title, room.max(1));
                    right_header_title = title;
                    right_header_title_cursor = Some(cursor);
                } else {
                    right_header_title = form.title.value().to_string();
                }
            }
        }
    }
    let right_header = right_header_visible.then_some(render::TaskColumnHeader {
        glyph: right_header_glyph,
        identifier: right_header_identifier.as_deref(),
        identifier_task: right_header_task,
        title: &right_header_title,
        title_cursor_col: right_header_title_cursor,
        state: &right_header_state,
        bold: true,
    });

    let outer_verbs = board_verb_items(model);
    let outer_frame = QueueFrameModel {
        tasks: &model.tasks,
        view: &outer_view,
        selection_id: None,
        marked_ids: BTreeSet::new(),
        nav: nav_paint(model),
        surface: BoardSurface::Projects,
        projects: &outer_view.projects,
        projects_index: true,
        projects_cursor: model.projects_cursor(),
        search_query: model.search_query(),
        search_pinned: model.search_pinned(),
        summary: None,
        context: status_idle(model, BoardSurface::Projects, outer_status.0.is_some()),
        reserve_context: status_reserve(model, BoardSurface::Projects, outer_status.0.is_some()),
        has_update_notice: model.update_notice().is_some(),
        status_message: outer_status.0.as_deref(),
        status_undo_offset: outer_status.1,
        status_undo_width: outer_status.2,
        verb_items: &outer_verbs,
        now: model.now(),
        overlay: outer_modal.clone().unwrap_or(QueueOverlay::None),
        detail_open: None,
        row_reply: None,
        list_scroll: model.list_scroll.get(),
        follow_list: model.follow_list.get(),
        archived_collapsed: model.archived_collapsed,
        archived_header_selected: false,
        inbox_collapsed: model.inbox_collapsed,
        inbox_header_selected: false,
        rows_dim: false,
    };

    let right_frame = right.and_then(|right| {
        let view = right_view.as_ref()?;
        let status = right_status.as_ref()?;
        let verbs = right_verbs.as_ref()?;
        Some(QueueFrameModel {
            tasks: &right.tasks,
            view,
            selection_id: right.saved_task.or(right.selection_id),
            marked_ids: right.marked_ids.clone(),
            nav: nav_paint(right),
            surface: BoardSurface::Project,
            projects: &[],
            projects_index: false,
            projects_cursor: 0,
            search_query: right.search_query(),
            search_pinned: right.search_pinned(),
            summary: None,
            context: status_idle(right, BoardSurface::Project, status.0.is_some()),
            reserve_context: status_reserve(right, BoardSurface::Project, status.0.is_some()),
            has_update_notice: right.update_notice().is_some(),
            status_message: status.0.as_deref(),
            status_undo_offset: status.1,
            status_undo_width: status.2,
            verb_items: verbs,
            now: model.now(),
            overlay: right_overlay.clone().unwrap_or(QueueOverlay::None),
            detail_open: (stage == tier::WideStage::Rail)
                .then_some(right.detail_open())
                .flatten(),
            row_reply: row_reply_paint(right),
            list_scroll: right.list_scroll.get(),
            follow_list: right.follow_list.get(),
            archived_collapsed: right.archived_collapsed,
            archived_header_selected: right.archived_header_selected(),
            inbox_collapsed: right.inbox_collapsed,
            inbox_header_selected: right.inbox_header_selected(),
            rows_dim: stage == tier::WideStage::Split || right.focus_is_archived(),
        })
    });
    let footer_frame = if right_focused {
        right_frame
            .as_ref()
            .expect("project rail has a right seat")
            .clone()
    } else {
        outer_frame.clone()
    };

    let mut hits = render::QueueHitMap::default();
    let board_area = responsive.board;
    if board_area.width > 0 {
        let board_geo = column_geo(board_area.width);
        let mut board_hits = if stage == tier::WideStage::Rail {
            render::draw_rail_frame(frame, &outer_frame, &board_geo, column_rect(board_area))
        } else {
            let (board_hits, painted_list_scroll) =
                render::draw_queue_frame(frame, &outer_frame, &board_geo, column_rect(board_area));
            if let Some((scroll, max_scroll)) = painted_list_scroll {
                model.list_scroll.set(scroll);
                model.list_max_scroll.set(max_scroll);
            }
            board_hits
        };
        hits.regions.append(&mut board_hits.regions);
        hits.copyable.append(&mut board_hits.copyable);
        hits.help_max_scroll = hits.help_max_scroll.or(board_hits.help_max_scroll);
        hits.cleanup_max_scroll = hits.cleanup_max_scroll.or(board_hits.cleanup_max_scroll);
    }
    if responsive.rule.width > 0 {
        let rule = column_rect(responsive.rule);
        for y in rule.top()..rule.bottom() {
            frame.render_widget(
                Paragraph::new(render::paint_bounded_line("│", 1, render::style_dim())),
                ratatui::layout::Rect::new(rule.x, y, 1, 1),
            );
        }
    }
    if let (Some(right_frame), Some(right_geo)) = (right_frame.as_ref(), right_geo.as_ref()) {
        let (mut right_hits, painted_list_scroll) =
            if let Some(task_overlay) = right_task_overlay.as_ref() {
                let task_frame = QueueFrameModel {
                    overlay: task_overlay.clone(),
                    ..right_frame.clone()
                };
                (
                    render::draw_task_column(
                        frame,
                        &task_frame,
                        right_geo,
                        column_rect(right_area),
                        right_header,
                        right_modal.as_ref(),
                    ),
                    None,
                )
            } else if let Some(project_name) = right_project_name.as_deref() {
                render::draw_project_preview_frame(
                    frame,
                    right_frame,
                    right_geo,
                    column_rect(right_area),
                    project_name,
                    stage == tier::WideStage::Rail,
                )
            } else {
                render::draw_queue_frame_without_selector(
                    frame,
                    right_frame,
                    right_geo,
                    column_rect(right_area),
                )
            };
        hits.regions.append(&mut right_hits.regions);
        hits.copyable.append(&mut right_hits.copyable);
        hits.help_max_scroll = hits.help_max_scroll.or(right_hits.help_max_scroll);
        hits.cleanup_max_scroll = hits.cleanup_max_scroll.or(right_hits.cleanup_max_scroll);
        if let Some(right) = right {
            if let Some((scroll, max_scroll)) = painted_list_scroll {
                right.list_scroll.set(scroll);
                right.list_max_scroll.set(max_scroll);
            }
            if let Some(max_scroll) = right_hits.help_max_scroll {
                right.help_max_scroll.set(max_scroll);
            }
            if let Some(max_scroll) = right_hits.cleanup_max_scroll {
                right.cleanup_max_scroll.set(max_scroll);
            }
        }
    }

    let (crumb, keys) = wide_status_hint(model);
    let mut footer_hits = render::draw_queue_footer(
        frame,
        &footer_frame,
        &footer_geo,
        area,
        Some(render::StatusHint { crumb, keys }),
    );
    hits.regions.append(&mut footer_hits.regions);
    hits.copyable.append(&mut footer_hits.copyable);
    hits.footer = footer_hits.footer;
    hits
}
