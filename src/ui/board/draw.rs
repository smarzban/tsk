//! Queue chrome, overlays, verb bar, and frame drawing hooks.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::SystemTime;

use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::domain::{HumanStatus, TaskScope};
use crate::ui::capture::CaptureField;
use crate::ui::edit::{
    escaped_line_window, wrap_text, wrapped_draft_rows, wrapped_edit_rows, EditBuffer,
};
use crate::ui::input::help_card_lines_for_query;
use crate::ui::mouse::BoardPopup;
use crate::ui::queue::ThreadFilter;
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
    BoardModel, PickerTab, ProjectScopeOption, ProjectsView,
};

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
        entries.extend(task_status_verbs(task));
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

fn task_status_verbs(task: &crate::domain::Task) -> Vec<VerbEntry<'static>> {
    let mut verbs = status_verbs(task.status);
    if task.assignee.is_some() && task.status != HumanStatus::Done {
        let after_start = verbs
            .iter()
            .position(|verb| verb.key == "s")
            .map_or(verbs.len(), |index| index + 1);
        verbs.insert(
            after_start,
            VerbEntry {
                key: "g",
                label: "dispatch",
            },
        );
    }
    verbs
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
    entries.extend(task_status_verbs(task));
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
        let header_cap = page_bottom.saturating_sub(3).max(1) as usize;
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
        render::task_column_layout(&page_geo)
    } else {
        render::task_page_layout(
            &page_geo,
            render::steps_section(step_views.len()),
            u16::from(model.input_mode() == BoardInputMode::EditNotes),
            header_rows.len().max(1) as u16,
        )
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
            let when = render::format_age(SystemTime::now(), dispatch.at);
            for line in [
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
                format!("when {when} ago"),
            ] {
                notes_rows.extend(
                    wrap_text(&line, notes_width)
                        .into_iter()
                        .map(|row| row.text),
                );
            }
        }
    }
    let step_rows: usize = step_views.iter().map(|step| step.rows.len().max(1)).sum();
    // Match the painter's stream exactly: it always paints one notes row and a trailing
    // `+ step` row, even when both stored notes and stored steps are empty.
    let content =
        render::page_content_layout(notes_rows.len().max(1), step_rows + 1, lay.notes_rows);
    form.notes_max_scroll.set(content.max_scroll);
    form.steps.content_start.set(content.steps_start);
    form.notes_width.set(notes_width);

    // Meta footer: assignee · thread · scope · created · updated (ages only while the task is
    // present). The identifier belongs in the header, so it never competes with footer hits.
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
    let assignee_segment = shown_assignee
        .map(|assignee| format!("@{}", terminal_text(assignee)))
        .or_else(|| {
            (capture_form || (form.is_task() && form.editing)).then(|| "assignee".to_string())
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
    if let Some(task) = bound_task {
        let now = SystemTime::now();
        let ages = format!(
            "created {} ago · updated {} ago",
            render::format_age(now, task.created_at),
            render::format_age(now, task.updated_at),
        );
        if meta.is_empty() {
            meta.push_str(&ages);
        } else {
            meta.push_str(" · ");
            meta.push_str(&ages);
        }
    }
    let focus = match model.input_mode() {
        BoardInputMode::EditTitle => Some(CaptureField::Title),
        BoardInputMode::EditNotes => Some(CaptureField::Notes),
        BoardInputMode::SelectThread | BoardInputMode::EditThread => Some(CaptureField::Thread),
        BoardInputMode::EditScope => Some(CaptureField::Scope),
        BoardInputMode::EditAssignee => Some(CaptureField::Assignee),
        BoardInputMode::FormDropdown => Some(form.focus),
        _ => None,
    };

    let scope_dropdown = scope_dropdown.map(|mut dropdown| {
        let field_x = match dropdown.field {
            CaptureField::Assignee => meta_assignee_x.unwrap_or(0),
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
        bottom_input,
        meta,
        meta_assignee_x,
        meta_assignee_width,
        meta_scope_x,
        meta_scope_width,
        thread_slot_width,
        focus,
        scope_dropdown,
    }
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
            } else if model.thread_filter() != &ThreadFilter::All {
                context.push_str(&format!(" · {}", model.thread_filter().label()));
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
            label: model.thread_filter().label(),
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
        crate::ui::queue::BoardLens::ThreadView(_) => BoardSurface::ThreadView,
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
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let list_picker_options = model
            .visible_list_picker_options()
            .into_iter()
            .map(|(_, option)| match option.count {
                Some(count) => format!("{}  {count}", option.label),
                None => option.label,
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
        let scope_tabs = model.picker_tab().map(|tab| render::PickerTabsPaint {
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
                    placeholder: "title…   !p project · !t thread · !a assignee",
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
            return Some(QueueOverlay::CleanupConfirm {
                worktree: &prompt.worktree,
                branch: &prompt.branch,
                dirty: prompt.dirty,
                branch_merged: prompt.branch_merged,
                workspace_exists: prompt.workspace_exists,
            });
        }
        if let Some(name) = self.launch_card_name.as_deref() {
            return Some(QueueOverlay::LaunchCard { name });
        }
        if model.input_mode() == BoardInputMode::ListPicker {
            let title = match model.list_picker_kind() {
                Some(crate::ui::board::ListPickerKind::ProjectsView) => "projects View",
                _ => "thread filter",
            };
            return Some(QueueOverlay::ScopeDropdown {
                options: &self.list_picker_options,
                selected: self
                    .list_picker_selected
                    .min(self.list_picker_options.len().saturating_sub(1)),
                tabs: None,
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
        has_update_notice: model.update_notice().is_some(),
        status_message: status_owned.as_deref(),
        status_undo_offset,
        status_undo_width,
        verb_items: &verbs,
        now: SystemTime::now(),
        overlay,
        detail_open: model.detail_open,
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
        has_update_notice: model.update_notice().is_some(),
        status_message: status_owned.as_deref(),
        status_undo_offset,
        status_undo_width,
        verb_items: &verbs,
        now: SystemTime::now(),
        overlay: if task_focus {
            QueueOverlay::None
        } else {
            modal.clone().unwrap_or(QueueOverlay::None)
        },
        detail_open: None,
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
        has_update_notice: model.update_notice().is_some(),
        status_message: outer_status.0.as_deref(),
        status_undo_offset: outer_status.1,
        status_undo_width: outer_status.2,
        verb_items: &outer_verbs,
        now: SystemTime::now(),
        overlay: outer_modal.clone().unwrap_or(QueueOverlay::None),
        detail_open: None,
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
            has_update_notice: right.update_notice().is_some(),
            status_message: status.0.as_deref(),
            status_undo_offset: status.1,
            status_undo_width: status.2,
            verb_items: verbs,
            now: SystemTime::now(),
            overlay: right_overlay.clone().unwrap_or(QueueOverlay::None),
            detail_open: (stage == tier::WideStage::Rail)
                .then_some(right.detail_open())
                .flatten(),
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
        if let Some(right) = right {
            if let Some((scroll, max_scroll)) = painted_list_scroll {
                right.list_scroll.set(scroll);
                right.list_max_scroll.set(max_scroll);
            }
            if let Some(max_scroll) = right_hits.help_max_scroll {
                right.help_max_scroll.set(max_scroll);
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
