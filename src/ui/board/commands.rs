//! Command-palette catalog, filtering, and command dispatch resolution.

use crate::domain::HumanStatus;
use crate::ui::input::BoardIntent;
use crate::ui::mouse::BoardPopup;

use super::model::BoardModel;
use uuid::Uuid;

/// Transient command surface open over the board, never durable state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandSurface {
    #[default]
    None,
    /// Searchable command discovery (`:`).
    Palette,
}

/// One discoverable board command: a label plus the existing intent it dispatches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardCommand {
    pub label: String,
    pub intent: BoardIntent,
}

fn command(label: impl Into<String>, intent: BoardIntent) -> BoardCommand {
    BoardCommand {
        label: label.into(),
        intent,
    }
}

impl BoardModel {
    /// Open command surface (action sheet or palette), if any.
    pub fn command_surface(&self) -> CommandSurface {
        self.surface
    }

    /// Current palette query (empty when the palette is closed).
    pub fn command_query(&self) -> &str {
        &self.command_query
    }

    /// Selected index into [`Self::visible_commands`] while a surface is open.
    pub fn command_selected(&self) -> Option<usize> {
        if self.surface == CommandSurface::None {
            return None;
        }
        let len = self.visible_commands().len();
        if len == 0 {
            return None;
        }
        Some(self.command_selected.min(len - 1))
    }

    /// Commands valid for the current board state.
    ///
    /// An unavailable recovery action or a command needing a selection is absent, so a
    /// surface can never expose a route the board does not already offer.
    pub fn available_commands(&self) -> Vec<BoardCommand> {
        // Unresolved save failure: only the recovery routes the boundary accepts.
        if self.popup == BoardPopup::SaveRecovery {
            return vec![
                command("retry save", BoardIntent::RetrySave),
                command("cancel save", BoardIntent::CancelSave),
            ];
        }
        let mut commands = Vec::new();
        if self.selected_id().is_some() {
            // the tail: set status x4, edit notes, change scope, assignment and optional
            // relaunch, then shared tail. No park / resume / link entries.
            commands.extend([
                command(
                    "set status: ready",
                    BoardIntent::SetStatus(HumanStatus::Ready),
                ),
                command(
                    "set status: open",
                    BoardIntent::SetStatus(HumanStatus::Open),
                ),
                command(
                    "set status: started",
                    BoardIntent::SetStatus(HumanStatus::Started),
                ),
                command(
                    "set status: blocked",
                    BoardIntent::SetStatus(HumanStatus::Blocked),
                ),
                command(
                    "set status: review",
                    BoardIntent::SetStatus(HumanStatus::Review),
                ),
                command("edit notes", BoardIntent::BeginEditNotes),
                command("move to project / desk", BoardIntent::BeginEditScope),
                command("set assignee", BoardIntent::OpenAssigneePicker),
                command("set base", BoardIntent::OpenBasePicker),
                command("set after (wait for)…", BoardIntent::OpenAfterPicker),
            ]);
            // Who an open block or review waits on: offered while a target has one. A marked
            // set with a task that has none still refuses as a whole.
            let targets: Vec<Uuid> = if self.marks_under_palette() > 0 {
                self.marked_ids.iter().copied().collect()
            } else {
                self.selected_id().into_iter().collect()
            };
            if self
                .tasks
                .iter()
                .any(|task| task.block.is_some() && targets.contains(&task.id))
            {
                commands.push(command("set waiting on…", BoardIntent::OpenOnPicker));
            }
            // Two or more marked tasks chain in the order they were marked. The palette owns
            // input while it lists this, so ask about the task list underneath it.
            if self.marks_under_palette() >= 2 {
                commands.push(command(
                    "chain marked tasks in order",
                    BoardIntent::ChainAfter,
                ));
            }
            // Starting dispatches an assigned task (`set status: started`, `ctrl+s`); dispatch
            // again is the explicit, cursor-only relaunch. A person is never dispatched.
            if self
                .selected_id()
                .and_then(|id| self.tasks.iter().find(|task| task.id == id))
                .is_some_and(|task| {
                    task.dispatch.is_some()
                        && !task.assigned().is_some_and(|assignee| assignee.is_person())
                })
            {
                commands.push(command("dispatch again", BoardIntent::DispatchAgain));
            }
        }
        // A step selected on the open task page: the alt+↑/↓ moves, findable by name.
        if self.page_step_selected() {
            commands.extend_from_slice(&[
                command("move step up", BoardIntent::MoveStepUp),
                command("move step down", BoardIntent::MoveStepDown),
            ]);
        }
        // Always-available board commands, then selection-gated delete when present.
        // The status commands above are absolute, so `set status: open` replaces the old
        // done-only `reopen` entry and stays useful from every status.
        commands.push(command("add task", BoardIntent::OpenCapture));
        if self.selected_id().is_some() {
            commands.push(command("delete task", BoardIntent::SoftDelete));
        }
        commands.extend_from_slice(&[
            command("undo", BoardIntent::Undo),
            command("show / hide done drawer", BoardIntent::ToggleDoneDrawer),
        ]);
        // Layout commands act on the task list, never over an open task page.
        if self.task_list_under_surfaces() {
            commands.extend_from_slice(&[
                command("group by…", BoardIntent::OpenGroupPicker),
                command("sort by…", BoardIntent::OpenSortPicker),
                command("fold / unfold group", BoardIntent::FoldGroup),
            ]);
        }
        commands.extend_from_slice(&[
            command("show all keys", BoardIntent::OpenHelp),
            command("quit", BoardIntent::Quit),
        ]);
        // Park / resume / link stay out of the palette.
        commands
    }

    /// Whether the focused task page has a stored step under its cursor (the palette's own
    /// mode is ignored: it opens over the page without changing the page's input mode).
    fn page_step_selected(&self) -> bool {
        self.focused_surface() == crate::ui::tier::FocusedSurface::Task
            && self.input_mode == crate::ui::board::BoardInputMode::TaskPage
            && self.form.as_ref().is_some_and(|form| {
                form.is_task() && form.steps.cursor.is_some() && !form.steps.add_selected
            })
    }

    /// Commands the open palette currently shows (the palette applies its query).
    pub fn visible_commands(&self) -> Vec<BoardCommand> {
        let commands = self.available_commands();
        if self.surface != CommandSurface::Palette {
            return commands;
        }
        let query = self.command_query.trim();
        if query.is_empty() {
            return commands;
        }
        commands
            .into_iter()
            .filter(|command| subsequence_match(&command.label, query))
            .collect()
    }

    /// The command a confirmation would invoke.
    /// The command a palette `ConfirmCommand` or `SelectCommand` would run, without running it.
    pub fn selected_command_for(&self, intent: &BoardIntent) -> Option<BoardCommand> {
        match intent {
            BoardIntent::ConfirmCommand => self.selected_command(),
            BoardIntent::SelectCommand(index) => self.visible_commands().get(*index).cloned(),
            _ => None,
        }
    }

    pub fn selected_command(&self) -> Option<BoardCommand> {
        let index = self.command_selected()?;
        self.visible_commands().get(index).cloned()
    }

    pub(super) fn open_command_surface(&mut self, surface: CommandSurface) {
        self.surface = surface;
        self.command_query.clear();
        self.command_selected = 0;
    }

    /// Close the palette. Domain state is never touched.
    pub fn close_command_surface(&mut self) {
        self.surface = CommandSurface::None;
        self.command_query.clear();
        self.command_selected = 0;
    }

    pub(super) fn move_command_selection(&mut self, forward: bool) {
        let len = self.visible_commands().len();
        if len == 0 {
            self.command_selected = 0;
            return;
        }
        let current = self.command_selected.min(len - 1);
        self.command_selected = if forward {
            (current + 1) % len
        } else {
            current.checked_sub(1).unwrap_or(len - 1)
        };
    }
}

/// Case-insensitive ordered subsequence match.
fn subsequence_match(label: &str, query: &str) -> bool {
    let mut chars = label.chars();
    for needle in query.chars() {
        loop {
            match chars.next() {
                Some(hay) if hay.eq_ignore_ascii_case(&needle) => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// Resolve a command-surface confirmation into the existing intent it dispatches.
///
/// Returns the intent unchanged when it is not a confirmation, and `None` when no command
/// is currently available. Confirming closes the surface before the intent is applied, so
/// the caller dispatches through the same route as the direct keyboard or chip path.
///
/// `SelectCommand(index)` resolves through the exact same steps as `ConfirmCommand`:
/// a command-surface row click names its row directly (the way `SelectIndex` names a task
/// row and `SelectProjectOption` names a dropdown option directly), but `map_board_mouse`
/// takes `&BoardModel` and so cannot set `command_selected` itself. Setting it here, in the
/// one function every route (key, paste, mouse) already calls before an intent reaches the
/// reducer, means a click tears the surface down here too -- not by falling through to
/// `apply_board_intent`'s own generic close, which a future addition to its exclusion list
/// could silently stop covering a direct command intent the way it still covers this one.
pub fn resolve_board_command(model: &mut BoardModel, intent: BoardIntent) -> Option<BoardIntent> {
    match intent {
        BoardIntent::ConfirmCommand => {}
        BoardIntent::SelectCommand(index) => {
            // Out of range against the surface this click actually opened: leave the
            // existing selection alone rather than confirm a stale index.
            model.visible_commands().get(index)?;
            model.command_selected = index;
        }
        _ => return Some(intent),
    }
    let resolved = model
        .selected_command()
        .map(|command| command.intent.clone());
    model.close_command_surface();
    resolved
}
