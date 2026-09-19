//! Mouse hit-testing for the queue board and standalone capture.

use std::io::{self, stdout, Write};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::supports_keyboard_enhancement;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Block;

use crate::domain::HumanStatus;

use super::board::{board_verb_items, BoardInputMode, BoardModel};
use super::capture::{
    CaptureField, CaptureModel, CaptureScopeChoice, CAPTURE_FIELD_LABEL_WIDTH,
    CAPTURE_SCOPE_CONTROLS,
};
use super::input::{BoardIntent, CaptureIntent, PrimaryCaptureAction, PRIMARY_CAPTURE_ACTIONS};
use super::render::{form_verb_items, QueueHitMap, QueueHitTarget, QUICK_ADD_VERBS};
use super::tier::{FocusedSurface, ResponsivePresentation, WideStage};

/// Transient presentation that still exists on the V1 queue board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoardPopup {
    #[default]
    None,
    /// Session-only project scope selector.
    ProjectPicker,
    /// Board persistence failed; Retry or Cancel must resolve it before another mutation.
    SaveRecovery,
    /// Two-choice card raised at launch when the cwd default is an archived project.
    LaunchCard,
}

/// Labeled capture hit region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip<T> {
    pub rect: Rect,
    pub label: &'static str,
    pub value: T,
}

/// Capture form field and button regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureLayout {
    pub area: Rect,
    pub inner: Rect,
    pub save_recovery: bool,
    pub subtitle_area: Rect,
    pub title_area: Rect,
    pub notes_area: Rect,
    pub thread_area: Rect,
    pub scope_area: Rect,
    pub scope_chips: Vec<Chip<CaptureScopeChoice>>,
    pub this_project_available: bool,
    pub scope_path_area: Rect,
    pub message_area: Rect,
    pub save_chip: Chip<()>,
    pub cancel_chip: Chip<()>,
    pub help_area: Rect,
}

/// One terminal input capability an event loop asks the terminal for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalInputCapability {
    MouseCapture,
    BracketedPaste,
    KeyboardProtocol,
}

pub(crate) fn requested_terminal_input(
    keyboard_enhancement_supported: bool,
) -> Vec<TerminalInputCapability> {
    let mut requested = vec![
        TerminalInputCapability::MouseCapture,
        TerminalInputCapability::BracketedPaste,
    ];
    if keyboard_enhancement_supported {
        requested.push(TerminalInputCapability::KeyboardProtocol);
    }
    requested
}

fn enable_capability(out: &mut impl Write, capability: TerminalInputCapability) -> io::Result<()> {
    match capability {
        TerminalInputCapability::MouseCapture => execute!(out, EnableMouseCapture),
        TerminalInputCapability::BracketedPaste => execute!(out, EnableBracketedPaste),
        TerminalInputCapability::KeyboardProtocol => execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        ),
    }
}

fn disable_capability(out: &mut impl Write, capability: TerminalInputCapability) -> io::Result<()> {
    match capability {
        TerminalInputCapability::MouseCapture => execute!(out, DisableMouseCapture),
        TerminalInputCapability::BracketedPaste => execute!(out, DisableBracketedPaste),
        TerminalInputCapability::KeyboardProtocol => execute!(out, PopKeyboardEnhancementFlags),
    }
}

fn disable_all(out: &mut impl Write, enabled: &[TerminalInputCapability]) {
    for capability in enabled.iter().rev() {
        let _ = disable_capability(out, *capability);
    }
}

/// Restores exactly the terminal capabilities it enabled.
pub(crate) struct TerminalInputGuard<W: Write> {
    out: W,
    enabled: Vec<TerminalInputCapability>,
}

impl<W: Write> Drop for TerminalInputGuard<W> {
    fn drop(&mut self) {
        disable_all(&mut self.out, &self.enabled);
    }
}

pub(crate) fn keyboard_enhancement_supported() -> bool {
    supports_keyboard_enhancement().unwrap_or(false)
}

fn enable_terminal_input_on<W: Write>(
    out: W,
    keyboard_enhancement_supported: bool,
) -> io::Result<TerminalInputGuard<W>> {
    let mut guard = TerminalInputGuard {
        out,
        enabled: Vec::new(),
    };
    for capability in requested_terminal_input(keyboard_enhancement_supported) {
        enable_capability(&mut guard.out, capability)?;
        guard.enabled.push(capability);
    }
    Ok(guard)
}

pub(crate) fn enable_terminal_input(
    keyboard_enhancement_supported: bool,
) -> io::Result<TerminalInputGuard<std::io::Stdout>> {
    enable_terminal_input_on(stdout(), keyboard_enhancement_supported)
}

/// Left-button press at (column, row).
pub fn left_click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Compute normal capture layout matching `draw_capture`.
pub fn capture_layout(area: Rect) -> CaptureLayout {
    capture_layout_state(area, false, true, false)
}

pub fn capture_recovery_layout(area: Rect) -> CaptureLayout {
    capture_layout_state(area, true, true, false)
}

pub fn capture_layout_for_model(area: Rect, model: &CaptureModel) -> CaptureLayout {
    capture_layout_state(
        area,
        model.is_save_recovery(),
        model.this_project_available(),
        model.shows_project_path(),
    )
}

pub fn capture_layout_state(
    area: Rect,
    save_recovery: bool,
    this_project_available: bool,
    show_path: bool,
) -> CaptureLayout {
    let inner = Block::bordered().inner(area);
    let notes_height = capture_notes_rows(inner.height, show_path);
    let rows = [
        1,
        1,
        notes_height,
        1,
        1,
        u16::from(show_path),
        1,
        1,
        inner
            .height
            .saturating_sub(7 + notes_height + u16::from(show_path)),
        1,
    ];
    let mut y = inner.y;
    let mut areas = Vec::with_capacity(rows.len());
    for height in rows {
        let height = height.min(inner.y.saturating_add(inner.height).saturating_sub(y));
        areas.push(Rect::new(inner.x, y, inner.width, height));
        y = y.saturating_add(height);
    }
    let scope_row = areas[4];
    let scope_chips = place_scope_chips(scope_row);
    let scope_path_area = if show_path { areas[5] } else { Rect::default() };
    let buttons = areas[7];
    let save_label = if save_recovery { " Retry " } else { " Save " };
    let cancel_label = " Cancel ";
    let save_width = save_label.chars().count() as u16;
    let cancel_width = cancel_label.chars().count() as u16;
    let save_rect = Rect::new(buttons.x, buttons.y, save_width.min(buttons.width), 1);
    let cancel_x = buttons.x.saturating_add(save_width).saturating_add(2);
    let cancel_rect = if cancel_x < buttons.x.saturating_add(buttons.width) {
        Rect::new(
            cancel_x,
            buttons.y,
            cancel_width.min(
                buttons
                    .x
                    .saturating_add(buttons.width)
                    .saturating_sub(cancel_x),
            ),
            1,
        )
    } else {
        Rect::new(buttons.x, buttons.y, 0, 1)
    };
    CaptureLayout {
        area,
        inner,
        save_recovery,
        subtitle_area: areas[0],
        title_area: areas[1],
        notes_area: areas[2],
        thread_area: areas[3],
        scope_area: scope_row,
        scope_chips,
        this_project_available,
        scope_path_area,
        message_area: areas[6],
        save_chip: Chip {
            rect: save_rect,
            label: save_label,
            value: (),
        },
        cancel_chip: Chip {
            rect: cancel_rect,
            label: cancel_label,
            value: (),
        },
        help_area: areas[9],
    }
}

pub const CAPTURE_NOTES_MAX_ROWS: u16 = 3;
const CAPTURE_FIXED_ROWS: u16 = 7;

fn capture_notes_rows(inner_height: u16, show_path: bool) -> u16 {
    inner_height
        .saturating_sub(CAPTURE_FIXED_ROWS + u16::from(show_path))
        .clamp(1, CAPTURE_NOTES_MAX_ROWS)
}

fn place_scope_chips(row: Rect) -> Vec<Chip<CaptureScopeChoice>> {
    let x_end = row.x.saturating_add(row.width);
    let start = row.x.saturating_add(CAPTURE_FIELD_LABEL_WIDTH);
    let control_width = |label: &str| label.chars().count() as u16 + 4;
    let needed = |compact: bool| {
        CAPTURE_SCOPE_CONTROLS
            .iter()
            .map(|&(full, narrow, _)| control_width(if compact { narrow } else { full }) + 1)
            .sum::<u16>()
            .saturating_sub(1)
    };
    let compact = start.saturating_add(needed(false)) > x_end;
    let mut x = start;
    let mut chips = Vec::with_capacity(CAPTURE_SCOPE_CONTROLS.len());
    for &(full, narrow, choice) in CAPTURE_SCOPE_CONTROLS {
        let label = if compact { narrow } else { full };
        if x >= x_end {
            break;
        }
        let width = control_width(label).min(x_end.saturating_sub(x));
        if width == 0 {
            break;
        }
        chips.push(Chip {
            rect: Rect::new(x, row.y, width, 1),
            label,
            value: choice,
        });
        x = x.saturating_add(width).saturating_add(1);
    }
    chips
}

fn point(column: u16, row: u16) -> Position {
    Position { x: column, y: row }
}

fn verb_intent(model: &BoardModel, index: usize) -> Option<BoardIntent> {
    let entry = *board_verb_items(model).get(index)?;
    match entry.key {
        "shift+enter" => Some(BoardIntent::ConfirmEditNext),
        "enter" if model.input_mode() == BoardInputMode::EditStep => Some(BoardIntent::ConfirmEdit),
        "enter" if model.input_mode() == BoardInputMode::Search => Some(BoardIntent::PinSearch),
        "s" => Some(BoardIntent::PrimaryVerb),
        "g" => Some(BoardIntent::Dispatch),
        "enter" => Some(BoardIntent::OpenTaskPage),
        "d" => Some(BoardIntent::Complete),
        "n" => Some(BoardIntent::SetStatus(HumanStatus::Ready)),
        "o" => Some(BoardIntent::Reopen),
        "b" => Some(BoardIntent::ToggleBlock),
        "r" => Some(BoardIntent::ToggleReview),
        "x" => Some(BoardIntent::SoftDelete),
        "u" => Some(BoardIntent::Undo),
        "f" => Some(BoardIntent::File),
        "e" => Some(BoardIntent::BeginEditTitle),
        "a" => Some(BoardIntent::BeginAddStep),
        "esc" if model.input_mode() == BoardInputMode::EditStep => Some(BoardIntent::CancelEdit),
        "esc" => Some(BoardIntent::CloseLayer),
        ":" => Some(BoardIntent::OpenCommandPalette),
        "?" => Some(BoardIntent::OpenHelp),
        "/" => Some(BoardIntent::FocusSearch),
        "+" => Some(BoardIntent::OpenCapture),
        _ => None,
    }
}

fn quick_add_verb_intent(index: usize) -> Option<BoardIntent> {
    match QUICK_ADD_VERBS.get(index)?.key {
        "enter" => Some(BoardIntent::QuickAddSave),
        "tab" => Some(BoardIntent::ExpandQuickAdd),
        "esc" => Some(BoardIntent::CancelQuickAdd),
        _ => None,
    }
}

fn form_verb_intent(model: &BoardModel, index: usize) -> Option<BoardIntent> {
    let dropdown_open = model.input_mode() == BoardInputMode::FormDropdown;
    let focus = model.form_focus()?;
    match form_verb_items(focus, dropdown_open).get(index)?.key {
        "shift+enter" => Some(BoardIntent::ConfirmEdit),
        "enter" if dropdown_open => Some(BoardIntent::ConfirmFormDropdown),
        "enter" if matches!(focus, CaptureField::Scope | CaptureField::Assignee) => {
            Some(BoardIntent::OpenFormDropdown(focus))
        }
        "space/←→" if focus == CaptureField::Assignee => Some(BoardIntent::FormAssigneeNext),
        // The Title bar paints `enter next`: the click must do what the key does.
        "enter" if focus == CaptureField::Title => Some(BoardIntent::FormFocusNext),
        "enter" => Some(BoardIntent::ConfirmEdit),
        "esc" if dropdown_open => Some(BoardIntent::CancelFormDropdown),
        "esc" => Some(BoardIntent::CancelEdit),
        _ => None,
    }
}

fn hit_at(hits: &QueueHitMap, pos: Position) -> Option<QueueHitTarget> {
    hits.regions
        .iter()
        .rev()
        .find(|hit| hit.area.contains(pos))
        .map(|hit| hit.target)
}

/// Rectangle that currently owns pointer input for this presentation.
pub fn focused_mouse_area(model: &BoardModel, area: Rect) -> Rect {
    let responsive = model.responsive_geometry(area);
    if model.project_right_seat_focused() || model.focused_surface() == FocusedSurface::Task {
        responsive.task_content()
    } else {
        responsive.board
    }
}

/// Whether a press lands on the surface that owns pointer input. The focused column always
/// does; so does the shared wide footer, which spans the frame and routes to the focused
/// surface regardless of which column it is painted under (verbs, status controls, inputs).
pub fn press_on_focused_surface(
    model: &BoardModel,
    hits: &QueueHitMap,
    area: Rect,
    pos: Position,
) -> bool {
    if focused_mouse_area(model, area).contains(pos) {
        return true;
    }
    let responsive = model.responsive_geometry(area);
    responsive.presentation == ResponsivePresentation::WideSplit
        && area.contains(pos)
        && hits.footer.is_some_and(|footer| footer.contains(pos))
}

fn wide_footer_contains(hits: &QueueHitMap, pos: Position) -> bool {
    hits.footer.is_some_and(|footer| footer.contains(pos))
}

/// Stage move that must run before dispatching a stage A click on the task column.
///
/// Any press inside the preview column (above the shared footer) slides to G first; the
/// caller then dispatches the same click against the frame the user saw (AC-11).
pub fn wide_mouse_focus_intent(
    model: &BoardModel,
    hits: &QueueHitMap,
    area: Rect,
    mouse: MouseEvent,
) -> Option<BoardIntent> {
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        || model.wide_stage() != WideStage::Split
        || model.input_mode() != BoardInputMode::Normal
    {
        return None;
    }
    let responsive = model.responsive_geometry(area);
    if responsive.presentation != ResponsivePresentation::WideSplit {
        return None;
    }
    let pos = point(mouse.column, mouse.row);
    (responsive.task.contains(pos)
        && !wide_footer_contains(hits, pos)
        && if model.projects_overview() {
            model.selected_project_row().is_some()
        } else {
            model.selected_id().is_some()
        })
    .then_some(BoardIntent::StageRight)
}

/// Map pointer input only through the live responsive surface and translated renderer hits.
pub fn map_responsive_board_mouse(
    model: &BoardModel,
    hits: &QueueHitMap,
    area: Rect,
    mouse: MouseEvent,
) -> Option<BoardIntent> {
    let responsive = model.responsive_geometry(area);
    if responsive.presentation != ResponsivePresentation::WideSplit {
        return map_board_mouse(model, hits, mouse);
    }
    let pos = point(mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if model.project_right_seat_focused() {
                return responsive.task.contains(pos).then(|| {
                    model
                        .right_seat()
                        .and_then(|right| map_board_mouse(right, hits, mouse))
                })?;
            }
            if model.input_mode() == BoardInputMode::Help {
                return map_board_mouse(model, hits, mouse);
            }
            return focused_mouse_area(model, area)
                .contains(pos)
                .then(|| map_board_mouse(model, hits, mouse))?;
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return None,
    }

    // The shared footer belongs to whichever surface owns input: its verbs, status controls
    // and inputs route exactly as the single-pane frame's own bottom rows do.
    if wide_footer_contains(hits, pos) {
        return if model.project_right_seat_focused() {
            model
                .right_seat()
                .and_then(|right| map_board_mouse(right, hits, mouse))
        } else {
            map_board_mouse(model, hits, mouse)
        };
    }

    // In the projects Rail the left side is still a clickable index, but every other control
    // in that side only moves the slider back. The project board is a complete narrow board
    // session, so its own mapper owns the translated hits and wheel behavior.
    if model.project_right_seat_focused() {
        if responsive.task.contains(pos) {
            return model
                .right_seat()
                .and_then(|right| map_board_mouse(right, hits, mouse));
        }
        if responsive.board.contains(pos) {
            if let Some(QueueHitTarget::ProjectRow(index)) = hit_at(hits, pos) {
                return Some(BoardIntent::SelectProjectRow(index));
            }
            return Some(BoardIntent::StageLeft);
        }
        return None;
    }

    let view_mode = matches!(
        model.input_mode(),
        BoardInputMode::Normal | BoardInputMode::TaskPage
    );
    let clean_or_dirty_task_editor = model.edit_target().is_some()
        && matches!(
            model.input_mode(),
            BoardInputMode::EditTitle
                | BoardInputMode::EditNotes
                | BoardInputMode::EditThread
                | BoardInputMode::EditScope
                | BoardInputMode::FormDropdown
        );
    // A task-row click opens or retargets the task beside the board in A (the reducer
    // moves the stage). In mark mode, a plain click stays on the board and toggles that row.
    if responsive.board.contains(pos)
        && model.input_mode() == BoardInputMode::Normal
        && model.mark_mode_active()
        && mouse.modifiers.is_empty()
    {
        if let Some(QueueHitTarget::Task(id) | QueueHitTarget::TaskNumber(id)) = hit_at(hits, pos) {
            return model
                .visible_ids()
                .iter()
                .position(|&visible| visible == id)
                .map(BoardIntent::MarkToggleAt);
        }
    }
    // The reducer refuses an ordinary retarget while a dirty draft is bound elsewhere.
    if responsive.board.contains(pos) && (view_mode || clean_or_dirty_task_editor) {
        if let Some(QueueHitTarget::Task(id)) = hit_at(hits, pos) {
            if clean_or_dirty_task_editor && model.edit_target() == Some(id) {
                return None;
            }
            return model
                .visible_ids()
                .iter()
                .position(|&visible| visible == id)
                .map(BoardIntent::FocusBoardAndSelectIndex);
        }
    }

    if !view_mode {
        if focused_mouse_area(model, area).contains(pos) {
            return map_board_mouse(model, hits, mouse);
        }
        return match model.input_mode() {
            BoardInputMode::Help => Some(BoardIntent::CloseHelp),
            BoardInputMode::Palette => Some(BoardIntent::CloseCommandSurface),
            _ => None,
        };
    }

    if responsive.board.contains(pos) {
        if model.focused_surface() == FocusedSurface::Task {
            // The task owns input, so a press on the rail takes focus left.
            if model.wide_stage() == WideStage::Rail {
                return Some(BoardIntent::StageLeft);
            }
            return None;
        }
        return map_board_mouse(model, hits, mouse);
    }
    if responsive.task.contains(pos) && model.focused_surface() == FocusedSurface::Task {
        return map_board_mouse(model, hits, mouse);
    }
    None
}

fn scrollbar_target_intent(target: QueueHitTarget) -> Option<BoardIntent> {
    match target {
        QueueHitTarget::ListScroll(offset) => Some(BoardIntent::ListScrollTo(offset)),
        QueueHitTarget::PageScroll(offset) => Some(BoardIntent::PageScrollTo(offset)),
        _ => None,
    }
}

/// Scrollbar intent for a press that actually lands on the grab zone.
pub fn scrollbar_hit_at(hits: &QueueHitMap, pos: Position) -> Option<BoardIntent> {
    hit_at(hits, pos).and_then(scrollbar_target_intent)
}

/// Scrollbar intent for a drag: clamp `row` to the track even if the pointer left it.
pub fn scrollbar_intent_at(hits: &QueueHitMap, row: u16, page: bool) -> Option<BoardIntent> {
    let mut cells: Vec<&crate::ui::render::QueueHit> = hits
        .regions
        .iter()
        .filter(|hit| match hit.target {
            QueueHitTarget::PageScroll(_) if page => true,
            QueueHitTarget::ListScroll(_) if !page => true,
            _ => false,
        })
        .collect();
    if cells.is_empty() {
        return None;
    }
    cells.sort_by_key(|hit| hit.area.y);
    let first = cells[0];
    let last = cells[cells.len() - 1];
    let chosen = if row <= first.area.y {
        first
    } else if row >= last.area.y {
        last
    } else {
        cells
            .iter()
            .copied()
            .find(|hit| {
                let end = hit.area.y.saturating_add(hit.area.height.max(1));
                (hit.area.y..end).contains(&row)
            })
            .unwrap_or(first)
    };
    scrollbar_target_intent(chosen.target)
}

/// Result of routing a pointer event through the scrollbar grab zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrollbarMouse {
    /// Not a scrollbar event; the rest of the mouse path should run.
    Miss,
    /// Dispatch this intent and skip the rest of the mouse path.
    Intent(BoardIntent),
    /// Drag ended; swallow the Up so it does not become a click.
    Consumed,
}

/// List/page scrollbar pointer state machine. Overlays (palette, picker, help)
/// miss so a gutter click still dismisses them.
pub fn map_scrollbar_mouse(
    mode: BoardInputMode,
    hits: &QueueHitMap,
    mouse: MouseEvent,
    dragging: &mut bool,
) -> ScrollbarMouse {
    if !matches!(
        mode,
        BoardInputMode::Normal
            | BoardInputMode::TaskPage
            | BoardInputMode::EditStep
            | BoardInputMode::EditTitle
            | BoardInputMode::EditNotes
            | BoardInputMode::EditScope
            | BoardInputMode::EditThread
            | BoardInputMode::SelectThread
    ) {
        *dragging = false;
        return ScrollbarMouse::Miss;
    }
    let page = mode != BoardInputMode::Normal;
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let pos = Position::new(mouse.column, mouse.row);
            if let Some(intent) = scrollbar_hit_at(hits, pos)
                .filter(|intent| matches!(intent, BoardIntent::PageScrollTo(_)) == page)
            {
                *dragging = true;
                ScrollbarMouse::Intent(intent)
            } else {
                *dragging = false;
                ScrollbarMouse::Miss
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if *dragging => {
            scrollbar_intent_at(hits, mouse.row, page)
                .map(ScrollbarMouse::Intent)
                .unwrap_or(ScrollbarMouse::Consumed)
        }
        MouseEventKind::Up(MouseButton::Left) if *dragging => {
            *dragging = false;
            ScrollbarMouse::Consumed
        }
        _ => ScrollbarMouse::Miss,
    }
}

fn wheel_board_intent(model: &BoardModel, kind: MouseEventKind) -> Option<BoardIntent> {
    match model.input_mode() {
        BoardInputMode::TaskPage
        | BoardInputMode::EditTitle
        | BoardInputMode::EditNotes
        | BoardInputMode::EditScope
        | BoardInputMode::EditThread
        | BoardInputMode::SelectThread
        | BoardInputMode::EditStep => match kind {
            MouseEventKind::ScrollUp => Some(BoardIntent::PageWheelScrollUp),
            MouseEventKind::ScrollDown => Some(BoardIntent::PageWheelScrollDown),
            _ => None,
        },
        BoardInputMode::Normal => match kind {
            MouseEventKind::ScrollUp => Some(BoardIntent::ListScrollTo(
                model.list_scroll().saturating_sub(1),
            )),
            MouseEventKind::ScrollDown => Some(BoardIntent::ListScrollTo(
                model.list_scroll().saturating_add(1),
            )),
            _ => None,
        },
        BoardInputMode::Palette => {
            let len = model.visible_commands().len();
            let selected = model.command_selected()?;
            match kind {
                MouseEventKind::ScrollUp if selected > 0 => Some(BoardIntent::CommandPrev),
                MouseEventKind::ScrollDown if selected + 1 < len => Some(BoardIntent::CommandNext),
                _ => None,
            }
        }
        BoardInputMode::Help => match kind {
            MouseEventKind::ScrollUp => Some(BoardIntent::HelpScrollUp),
            MouseEventKind::ScrollDown => Some(BoardIntent::HelpScrollDown),
            _ => None,
        },
        _ => None,
    }
}

/// Map a board mouse event through the current frame's renderer-owned hit map.
pub fn map_board_mouse(
    model: &BoardModel,
    hits: &QueueHitMap,
    mouse: MouseEvent,
) -> Option<BoardIntent> {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {}
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            return wheel_board_intent(model, mouse.kind)
        }
        _ => return None,
    }
    let pos = point(mouse.column, mouse.row);
    if model.expanded_capture_open()
        && matches!(
            model.input_mode(),
            BoardInputMode::EditTitle
                | BoardInputMode::EditNotes
                | BoardInputMode::EditScope
                | BoardInputMode::EditThread
                | BoardInputMode::SelectThread
                | BoardInputMode::EditStep
        )
    {
        let target = hit_at(hits, pos);
        if let Some(hit) = hits.regions.iter().find(|hit| {
            Some(hit.target) == target && hit.area.contains(Position::new(mouse.column, mouse.row))
        }) {
            match hit.target {
                QueueHitTarget::FormTitle => {
                    return Some(BoardIntent::FocusFormCursor(
                        CaptureField::Title,
                        mouse.row.saturating_sub(hit.area.y) as usize,
                        mouse.column.saturating_sub(hit.area.x + 4) as usize,
                    ))
                }
                QueueHitTarget::FormNotes(row) => {
                    return Some(BoardIntent::FocusFormCursor(
                        CaptureField::Notes,
                        row,
                        mouse.column.saturating_sub(hit.area.x + 2) as usize,
                    ))
                }
                _ => {}
            }
        }
    }
    match model.input_mode() {
        BoardInputMode::Palette => match hit_at(hits, pos) {
            Some(QueueHitTarget::Command(index)) => model
                .visible_commands()
                .get(index)
                .map(|_| BoardIntent::SelectCommand(index)),
            Some(QueueHitTarget::CommandChrome) => None,
            // The card's own border/title/footer rule: inert, the same as the palette's
            // pre-card `CommandChrome` furniture just above.
            Some(QueueHitTarget::ModalChrome) => None,
            Some(QueueHitTarget::ModalClose) => Some(BoardIntent::CloseCommandSurface),
            _ => Some(BoardIntent::CloseCommandSurface),
        },
        BoardInputMode::ProjectPicker => match hit_at(hits, pos) {
            Some(QueueHitTarget::ProjectOption(index)) => {
                Some(BoardIntent::SelectProjectOption(index))
            }
            Some(QueueHitTarget::PickerTab(tab)) => Some(BoardIntent::SelectPickerTab(tab)),
            Some(QueueHitTarget::ModalChrome) => None,
            Some(QueueHitTarget::ModalClose) => Some(BoardIntent::CancelProjectPicker),
            _ => Some(BoardIntent::CancelProjectPicker),
        },
        // The searchable card's own chrome and body are inert. Its `[x]` and the
        // full-frame fallback outside the card close it.
        BoardInputMode::Help => match hit_at(hits, pos) {
            Some(QueueHitTarget::ModalChrome) => None,
            _ => Some(BoardIntent::CloseHelp),
        },
        BoardInputMode::QuickAdd => match hit_at(hits, pos) {
            // The line already owns keyboard focus, so its click is intentionally inert.
            Some(QueueHitTarget::QuickAddInput) => None,
            Some(QueueHitTarget::TaskNumber(id)) => Some(BoardIntent::CopyTaskNumber(id)),
            Some(QueueHitTarget::Task(id)) => model
                .visible_ids()
                .iter()
                .position(|&visible| visible == id)
                .map(BoardIntent::QuickAddSelectIndex),
            Some(QueueHitTarget::Verb(index)) => quick_add_verb_intent(index),
            // A refusal holding the verb row is a notice, not the outside.
            Some(QueueHitTarget::ModalChrome) => None,
            // Chosen policy: outside clicks discard the draft and are swallowed, rather than
            // triggering a second board action behind the capture surface.
            _ => Some(BoardIntent::CancelQuickAdd),
        },
        BoardInputMode::EditTitle
        | BoardInputMode::EditNotes
        | BoardInputMode::EditScope
        | BoardInputMode::EditAssignee => match hit_at(hits, pos) {
            Some(QueueHitTarget::FormTitle) => {
                Some(BoardIntent::FocusFormField(CaptureField::Title))
            }
            Some(QueueHitTarget::FormNotes(_)) => {
                Some(BoardIntent::FocusFormField(CaptureField::Notes))
            }
            Some(QueueHitTarget::FormScope) => {
                Some(BoardIntent::OpenFormDropdown(CaptureField::Scope))
            }
            Some(QueueHitTarget::FormAssignee) => {
                Some(BoardIntent::OpenFormDropdown(CaptureField::Assignee))
            }
            Some(QueueHitTarget::FormThread) => {
                Some(BoardIntent::FocusFormField(CaptureField::Thread))
            }
            Some(QueueHitTarget::Step(index)) if model.task_editing() => {
                Some(BoardIntent::SelectStep(index))
            }
            Some(QueueHitTarget::StepAdd) => Some(BoardIntent::BeginAddStep),
            Some(QueueHitTarget::Verb(index)) => form_verb_intent(model, index),
            _ => None,
        },
        BoardInputMode::SelectThread | BoardInputMode::EditThread => match hit_at(hits, pos) {
            Some(QueueHitTarget::FormThread) => Some(BoardIntent::ToggleThreadEditing),
            Some(QueueHitTarget::FormTitle) => {
                Some(BoardIntent::FocusFormField(CaptureField::Title))
            }
            Some(QueueHitTarget::FormNotes(_)) => {
                Some(BoardIntent::FocusFormField(CaptureField::Notes))
            }
            Some(QueueHitTarget::FormScope) => {
                Some(BoardIntent::OpenFormDropdown(CaptureField::Scope))
            }
            Some(QueueHitTarget::FormAssignee) => {
                Some(BoardIntent::OpenFormDropdown(CaptureField::Assignee))
            }
            Some(QueueHitTarget::Step(index)) if model.task_editing() => {
                Some(BoardIntent::SelectStep(index))
            }
            Some(QueueHitTarget::StepAdd) => Some(BoardIntent::BeginAddStep),
            Some(QueueHitTarget::Verb(index)) => form_verb_intent(model, index),
            _ => None,
        },
        BoardInputMode::TaskPage | BoardInputMode::CapturePage => match hit_at(hits, pos) {
            Some(QueueHitTarget::TaskNumber(id)) => Some(BoardIntent::CopyTaskNumber(id)),
            Some(QueueHitTarget::FormTitle) if model.task_editing() => {
                Some(BoardIntent::FocusFormField(CaptureField::Title))
            }
            Some(QueueHitTarget::FormNotes(_)) if model.task_editing() => {
                Some(BoardIntent::FocusFormField(CaptureField::Notes))
            }
            Some(QueueHitTarget::FormThread) if model.task_editing() => {
                Some(BoardIntent::FocusFormField(CaptureField::Thread))
            }
            Some(QueueHitTarget::FormScope) if model.task_editing() => {
                Some(BoardIntent::OpenFormDropdown(CaptureField::Scope))
            }
            Some(QueueHitTarget::FormAssignee) if model.task_editing() => {
                Some(BoardIntent::OpenFormDropdown(CaptureField::Assignee))
            }
            // Step clicks always select. In view mode this remains read-only; the reducer opens
            // the inline editor only when the task edit session is already active.
            Some(QueueHitTarget::Step(index)) => Some(BoardIntent::SelectStep(index)),
            Some(QueueHitTarget::StepAdd) => Some(BoardIntent::BeginAddStep),
            Some(QueueHitTarget::PageScroll(offset)) => Some(BoardIntent::PageScrollTo(offset)),
            Some(QueueHitTarget::Verb(index)) => verb_intent(model, index),
            _ => None,
        },
        BoardInputMode::FormDropdown => match hit_at(hits, pos) {
            Some(QueueHitTarget::FormDropdownOption(index)) => {
                Some(BoardIntent::SelectFormDropdownOption(index))
            }
            Some(QueueHitTarget::Verb(index)) => form_verb_intent(model, index),
            _ => None,
        },
        BoardInputMode::EditStep => match hit_at(hits, pos) {
            Some(QueueHitTarget::FormTitle) => {
                Some(BoardIntent::FocusFormField(CaptureField::Title))
            }
            Some(QueueHitTarget::FormNotes(_)) => {
                Some(BoardIntent::FocusFormField(CaptureField::Notes))
            }
            Some(QueueHitTarget::FormThread) => {
                Some(BoardIntent::FocusFormField(CaptureField::Thread))
            }
            Some(QueueHitTarget::FormScope) => {
                Some(BoardIntent::OpenFormDropdown(CaptureField::Scope))
            }
            Some(QueueHitTarget::FormAssignee) => {
                Some(BoardIntent::OpenFormDropdown(CaptureField::Assignee))
            }
            Some(QueueHitTarget::Step(index)) => Some(BoardIntent::SelectStep(index)),
            Some(QueueHitTarget::StepAdd) => Some(BoardIntent::BeginAddStep),
            Some(QueueHitTarget::Verb(index)) => verb_intent(model, index),
            Some(QueueHitTarget::TaskNumber(id)) if model.empty_add_step_editor() => {
                Some(BoardIntent::CopyTaskNumber(id))
            }
            Some(QueueHitTarget::Task(id)) if model.empty_add_step_editor() => model
                .visible_ids()
                .iter()
                .position(|&visible| visible == id)
                .map(BoardIntent::SelectIndex),
            Some(QueueHitTarget::PageScroll(offset)) if model.empty_add_step_editor() => {
                Some(BoardIntent::PageScrollTo(offset))
            }
            _ => None,
        },
        BoardInputMode::SaveRecovery => None,
        BoardInputMode::LaunchCard => match hit_at(hits, pos) {
            Some(QueueHitTarget::LaunchOption(0)) => Some(BoardIntent::LaunchUnarchive),
            Some(QueueHitTarget::LaunchOption(1)) => Some(BoardIntent::LaunchKeepArchived),
            _ => None,
        },
        BoardInputMode::ListPicker => match hit_at(hits, pos) {
            Some(QueueHitTarget::ListPickerOption(index)) => {
                Some(BoardIntent::SelectListOption(index))
            }
            Some(QueueHitTarget::ModalChrome) => None,
            Some(QueueHitTarget::ModalClose) => Some(BoardIntent::CancelListPicker),
            _ => Some(BoardIntent::CancelListPicker),
        },
        BoardInputMode::Search => match hit_at(hits, pos) {
            Some(QueueHitTarget::Search) => Some(BoardIntent::FocusSearch),
            Some(QueueHitTarget::ProjectRow(index)) => Some(BoardIntent::SelectProjectRow(index)),
            Some(QueueHitTarget::TaskNumber(id)) => Some(BoardIntent::CopyTaskNumber(id)),
            Some(QueueHitTarget::Task(id)) => model
                .visible_ids()
                .iter()
                .position(|&visible| visible == id)
                .map(BoardIntent::SelectIndex),
            // The painted `enter pin · esc clear` row dispatches like the keys.
            Some(QueueHitTarget::Verb(index)) => verb_intent(model, index),
            _ => Some(BoardIntent::CloseLayer),
        },
        BoardInputMode::Normal => match hit_at(hits, pos) {
            Some(QueueHitTarget::NavTab(tab)) => Some(BoardIntent::SelectNavTab(tab)),
            Some(QueueHitTarget::NavChip) => match model.nav_chip_kind() {
                Some(crate::ui::render::NavChipKind::ThreadFilter) => {
                    Some(BoardIntent::OpenThreadFilterPicker)
                }
                Some(crate::ui::render::NavChipKind::ProjectsView) => {
                    Some(BoardIntent::OpenProjectsViewPicker)
                }
                None => None,
            },
            Some(QueueHitTarget::Search) => Some(BoardIntent::FocusSearch),
            Some(QueueHitTarget::ProjectRow(index)) => Some(BoardIntent::SelectProjectRow(index)),
            Some(QueueHitTarget::Drawer) => Some(BoardIntent::ToggleDoneDrawer),
            Some(QueueHitTarget::ArchivedHeader) => Some(BoardIntent::ToggleArchivedGroup),
            Some(QueueHitTarget::InboxHeader) => Some(BoardIntent::ToggleInboxGroup),
            Some(QueueHitTarget::TaskNumber(id))
                if model.mark_mode_active() && mouse.modifiers.is_empty() =>
            {
                model
                    .visible_ids()
                    .iter()
                    .position(|&visible| visible == id)
                    .map(BoardIntent::MarkToggleAt)
            }
            Some(QueueHitTarget::TaskNumber(id)) => Some(BoardIntent::CopyTaskNumber(id)),
            Some(QueueHitTarget::Task(id))
                if model.mark_mode_active() && mouse.modifiers.is_empty() =>
            {
                model
                    .visible_ids()
                    .iter()
                    .position(|&visible| visible == id)
                    .map(BoardIntent::MarkToggleAt)
            }
            Some(QueueHitTarget::Task(id)) => model
                .visible_ids()
                .iter()
                .position(|&visible| visible == id)
                .map(BoardIntent::SelectIndex),
            Some(QueueHitTarget::ListScroll(offset)) => Some(BoardIntent::ListScrollTo(offset)),
            Some(QueueHitTarget::Verb(index)) => verb_intent(model, index),
            Some(QueueHitTarget::DeleteNoticeUndo) => Some(BoardIntent::Undo),
            _ => None,
        },
    }
}

/// Map a mouse event to a capture intent (left click only).
pub fn map_capture_mouse(layout: &CaptureLayout, mouse: MouseEvent) -> Option<CaptureIntent> {
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        return None;
    }
    let pos = point(mouse.column, mouse.row);
    if layout.save_chip.rect.width > 0 && layout.save_chip.rect.contains(pos) {
        return Some(if layout.save_recovery {
            CaptureIntent::RetrySave
        } else {
            CaptureIntent::Save
        });
    }
    if layout.cancel_chip.rect.width > 0 && layout.cancel_chip.rect.contains(pos) {
        return Some(if layout.save_recovery {
            CaptureIntent::CancelSave
        } else {
            CaptureIntent::Cancel
        });
    }
    if layout.save_recovery {
        return None;
    }
    if layout.title_area.contains(pos) {
        return Some(CaptureIntent::FocusField(CaptureField::Title));
    }
    if layout.notes_area.contains(pos) {
        return Some(CaptureIntent::FocusField(CaptureField::Notes));
    }
    if layout.thread_area.contains(pos) {
        return Some(CaptureIntent::FocusField(CaptureField::Thread));
    }
    for chip in &layout.scope_chips {
        if chip.rect.contains(pos) {
            if chip.value == CaptureScopeChoice::ThisProject && !layout.this_project_available {
                return None;
            }
            return Some(CaptureIntent::SelectScope(chip.value));
        }
    }
    if !layout.scope_path_area.is_empty() && layout.scope_path_area.contains(pos) {
        return Some(CaptureIntent::SelectScope(CaptureScopeChoice::Other));
    }
    if layout.scope_area.contains(pos) {
        return Some(CaptureIntent::CycleScope);
    }
    None
}

pub fn primary_capture_action_sample_mouse(
    action: PrimaryCaptureAction,
    layout: &CaptureLayout,
) -> MouseEvent {
    let (column, row) = match action {
        PrimaryCaptureAction::SaveCapture => (layout.save_chip.rect.x, layout.save_chip.rect.y),
        PrimaryCaptureAction::CancelCapture => {
            (layout.cancel_chip.rect.x, layout.cancel_chip.rect.y)
        }
        PrimaryCaptureAction::EditField => (layout.title_area.x, layout.title_area.y),
        PrimaryCaptureAction::ChangeScope => (layout.scope_area.x, layout.scope_area.y),
    };
    left_click(column, row)
}

pub fn capture_mouse_paths_complete(layout: &CaptureLayout) -> bool {
    PRIMARY_CAPTURE_ACTIONS.iter().all(|&action| {
        let mouse = primary_capture_action_sample_mouse(action, layout);
        map_capture_mouse(layout, mouse)
            .and_then(|intent| super::input::intent_primary_capture_action(&intent))
            == Some(action)
    })
}
