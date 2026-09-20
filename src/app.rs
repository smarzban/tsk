//! Application entry: mode select, load store/context, run Board or Capture UI.

use std::env;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::DefaultTerminal;

use crate::context::{build_snapshot, InvocationSnapshot, RawHostContext};
use crate::domain::{DomainError, DomainState};
use crate::save_recovery::SaveRecovery;
use crate::store::{default_state_dir, StoreSignature, TaskStore};
use crate::ui::board::{
    apply_intent, board_intent_may_persist, draw_board, resolve_board_command, BoardInputMode,
    BoardModel, IntentOutcome, SaveResolution,
};
use crate::ui::capture::{CaptureField, TITLE_REQUIRED_MESSAGE};
use crate::ui::input::{
    map_edit_paste, map_key, map_task_form_key, route_responsive_key, BoardIntent,
    ResponsiveKeyRoute,
};
use crate::ui::mouse::{
    enable_terminal_input, focused_mouse_area, keyboard_enhancement_supported,
    map_responsive_board_mouse, map_scrollbar_mouse, press_on_focused_surface, scrollbar_hit_at,
    wide_mouse_focus_intent, ScrollbarMouse,
};
use crate::ui::queue::NavTab;
use crate::ui::scheduler;
use crate::ui::text_select::{
    copy_to_clipboard, copyable_line_at, frame_text_rows, selection_text,
};

/// Env var set by open-capture launcher for the quick-capture popup session.
pub const MODE_ENV: &str = "TSK_MODE";

/// One binary, two modes (Board default; Capture is quick capture: the board session
/// seeded onto the expanded quick-add page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// Primary Tasks board.
    Board,
    /// Quick-capture popup (expanded quick-add page).
    Capture,
}

/// Resolve the TUI mode from an explicit mode-env value and remaining argv (after argv0).
///
/// Default is [`AppMode::Board`]. `capture` (env string or arg) selects Capture. Process command
/// validation belongs to [`crate::cli::router`]. Prefer this pure helper in tests.
pub fn resolve_mode_from<S: AsRef<str>>(
    mode_env: Option<&str>,
    args: impl IntoIterator<Item = S>,
) -> AppMode {
    if mode_env.is_some_and(|v| v.eq_ignore_ascii_case("capture")) {
        return AppMode::Capture;
    }

    let mut iter = args.into_iter();
    // Skip program name when present.
    let _argv0 = iter.next();
    for arg in iter {
        if arg.as_ref().eq_ignore_ascii_case("capture") {
            return AppMode::Capture;
        }
    }
    AppMode::Board
}

/// Resolve the TUI mode from `TSK_MODE` and remaining argv (after argv0).
pub fn resolve_mode<S: AsRef<str>>(args: impl IntoIterator<Item = S>) -> AppMode {
    resolve_mode_from(env::var(MODE_ENV).ok().as_deref(), args)
}

fn load_snapshot() -> InvocationSnapshot {
    let raw = RawHostContext::from_env();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    build_snapshot(&raw, cwd)
}

/// Load store + snapshot into domain and board view-model (no TTY).
pub fn load_board() -> Result<(TaskStore, DomainState, BoardModel), Box<dyn Error>> {
    load_board_inner(true)
}

/// Load store + snapshot for the quick-capture popup (no TTY).
pub fn load_board_for_quick_capture() -> Result<(TaskStore, DomainState, BoardModel), Box<dyn Error>>
{
    load_board_inner(false)
}

fn load_board_inner(
    full_board_open: bool,
) -> Result<(TaskStore, DomainState, BoardModel), Box<dyn Error>> {
    let state_dir = default_state_dir();
    let store = TaskStore::new(state_dir.clone());
    if full_board_open {
        seed_notices_without_blocking_open(&store);
    }
    let state = store.load()?;
    let snapshot = load_snapshot();
    let mut model = BoardModel::from_domain_for_snapshot(&state, &snapshot);
    if full_board_open {
        model.offer_launch_card(&state, &snapshot);
    }
    model.set_update_notice(crate::update::startup(
        &state_dir,
        env!("CARGO_PKG_VERSION"),
    ));
    Ok((store, state, model))
}

fn seed_notices_without_blocking_open(store: &TaskStore) {
    let announcement_fresh = crate::announcements::is_fresh_install(store);
    let _ = crate::guides::seed_on_open(store);
    let _ = crate::announcements::seed_on_open(store, announcement_fresh);
}

fn record_notice_dismissals_without_blocking_persist(store: &TaskStore, domain: &DomainState) {
    let _ = crate::delivery::record_dismissed_notices(store, domain.tasks());
}

/// What the board's wait for input answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePoll {
    /// Nothing arrived inside the poll window: tick the background work and come round again.
    Idle,
    /// An event is queued and ready to read.
    Event,
}

/// The board loop's input-wait duration for one frame.
///
/// `active_animations` is true while a text-drag autoscroll is armed, so the wait
/// shortens to the scheduler's animation tick.
/// Wired straight through [`scheduler::next_wait`] rather than a fixed constant -- the base
/// tick and the short-tick floor stay the Frame Scheduler's, not a second copy in the loop.
pub fn board_poll_duration(active_animations: bool) -> Duration {
    scheduler::next_wait(active_animations, scheduler::DEFAULT_BASE_TICK)
}

/// Paint once at the settled size, then yield the event that interrupted a resize burst.
///
/// `run_board` uses this after [`scheduler::coalesce_resizes`] so a key or click typed
/// during a pane-edge drag is not dispatched against the pre-resize layout.
pub fn take_pending_after_paint<E>(
    pending: &mut Option<Event>,
    paint: impl FnOnce() -> Result<(), E>,
) -> Result<Option<Event>, E> {
    let Some(event) = pending.take() else {
        return Ok(None);
    };
    paint()?;
    Ok(Some(event))
}

/// One board frame in the only order survives: **settle, paint, wait**.
///
/// The settle lives here rather than at the caller's convenience because the *placement* is
/// the property, not the arithmetic. `run_board`'s loop body is a run of `continue`s -- an
/// unmapped key, an area gate, a mouse that hit nothing, and above all the poll timeout -- and
/// a settle that ended up behind any of them would wait on an event that a user who walked
/// away never produces. Owning the paint and the wait is what makes that impossible to get
/// wrong: the wait cannot answer [`FramePoll::Idle`] without the settle having already run,
/// because both are inside this call.
///
/// So the guard is structural. Moving the settle out of here to "handle it after the event"
/// means changing this signature, and the tests that drive it (`tests/e2e_persist.rs`: the
/// idle frame, and the settle-before-paint order) stop building.
///
/// This function also owns the poll duration: it computes
/// [`board_poll_duration`] itself and hands it to `wait`, so the call site cannot substitute a
/// fixed constant of its own -- the only way to change the wait is to change this function.
pub fn board_frame(
    model: &mut BoardModel,
    paint: impl FnOnce(&BoardModel) -> io::Result<()>,
    wait: impl FnOnce(Duration) -> io::Result<bool>,
    active_animations: bool,
) -> io::Result<FramePoll> {
    paint(model)?;
    if wait(board_poll_duration(active_animations))? {
        Ok(FramePoll::Event)
    } else {
        Ok(FramePoll::Idle)
    }
}

/// One frame plus, only on `FramePoll::Idle`, the store-only revalidation).
///
/// `run_board`'s loop calls this and nothing else to decide whether a tick was idle; there is
/// no separate call the loop makes only when idle for a later edit to drop in silence, unlike
/// the shape a prior round left (`board_frame`, then a sibling `revalidate_board_from_store`
/// call in the loop's own `if poll == FramePoll::Idle` arm) that a test exercising both calls
/// only through their own bodies could never prove `run_board` actually wired together. A test
/// that drives this one function and observes the merge lands exactly the same assertion
/// `run_board`'s own Idle branch depends on, because it is the same code, not a copy of it.
#[allow(clippy::too_many_arguments)]
pub fn board_idle_tick(
    model: &mut BoardModel,
    paint: impl FnOnce(&BoardModel) -> io::Result<()>,
    wait: impl FnOnce(Duration) -> io::Result<bool>,
    store: &TaskStore,
    domain: &mut DomainState,
    watch: &mut StoreWatch,
    save_recovery: &SaveRecovery<DomainState>,
    active_animations: bool,
) -> io::Result<FramePoll> {
    model.expire_ephemeral_message();
    let poll = board_frame(model, paint, wait, active_animations)?;
    if poll == FramePoll::Idle {
        revalidate_board_from_store(store, domain, model, watch, save_recovery);
    }
    Ok(poll)
}

/// Load store + context into a board view-model (no TTY).
pub fn load_board_model() -> Result<BoardModel, Box<dyn Error>> {
    let (_store, _state, model) = load_board()?;
    Ok(model)
}

/// Binary entry used by `main`. Default mode is the Tasks board.
///
/// Pass `capture` argv (or `TSK_MODE=capture`) for the quick-capture popup: the board
/// session seeded onto the expanded quick-add page (exits after save or cancel).
pub fn run(args: impl IntoIterator<Item = impl AsRef<str>>) -> Result<(), Box<dyn Error>> {
    match resolve_mode(args) {
        AppMode::Board => run_board(),
        AppMode::Capture => run_capture(),
    }
}

/// Seed the quick-capture session onto the expanded quick-add page.
///
/// These are the same intents the board routes for `+` then `Tab`, plus one: `OpenCapture`
/// stages the line from the invocation snapshot (scope and selected-text prefill) and
/// `ExpandQuickAdd` lifts it onto the task page with Notes focused, then `FocusFormField`
/// moves the cursor to Title so a name can be typed immediately. Neither of the first two
/// intents has a failure mode; their Results exist for the mutating arms the shared reducer
/// serves.
pub fn seed_quick_capture(
    domain: &mut DomainState,
    model: &mut BoardModel,
    snapshot: &InvocationSnapshot,
) {
    let _ = apply_intent(domain, model, BoardIntent::OpenCapture, Some(snapshot));
    let _ = apply_intent(domain, model, BoardIntent::ExpandQuickAdd, None);
    let _ = apply_intent(
        domain,
        model,
        BoardIntent::FocusFormField(CaptureField::Title),
        None,
    );
}

/// Whether the quick-capture popup session has ended: the draft is gone because a save
/// persisted or the user discarded it. Every other state -- editing, refusals, save
/// recovery, the retained line stash behind an expanded page -- keeps the popup open.
pub fn quick_capture_finished(model: &BoardModel) -> bool {
    !model.quick_add_open() && !model.board_form_open()
}

/// Quick capture: the board session seeded onto the expanded quick-add page.
///
/// The popup lives exactly as long as that draft: a persisted save closes it, an
/// explicit discard closes it, and a failed save keeps it open in recovery.
fn run_capture() -> Result<(), Box<dyn Error>> {
    let (store, mut domain, mut model) = load_board_for_quick_capture()?;
    let snapshot = load_snapshot();
    seed_quick_capture(&mut domain, &mut model, &snapshot);
    run_board_loop(store, domain, model, true)
}

fn run_board() -> Result<(), Box<dyn Error>> {
    let (store, domain, model) = load_board()?;
    run_board_loop(store, domain, model, false)
}

fn run_board_loop(
    store: TaskStore,
    mut domain: DomainState,
    mut model: BoardModel,
    quick_capture: bool,
) -> Result<(), Box<dyn Error>> {
    // `load_board` just read the store, so seed the watch from that snapshot: the first idle
    // tick must not immediately re-merge what is already loaded.
    let mut store_watch = StoreWatch::seeded(&store);

    // Every idle tick revalidates the store -- a `stat` on tsk.json, and only when its
    // mtime/size changed does it pay for `store.load()` + `merge_tasks_from_disk` +
    // `sync_from_domain` (see [`revalidate_board_from_store`]) -- so a quick-capture popup
    // (a separate process writing the same file) becomes visible on an open, idle board
    // without this board ever running a persisting intent. Save recovery still gates that
    // off via `board_background_work_allowed`.

    // Query before the alternate screen is entered: it can block on a terminal round-trip,
    // and a blank alternate screen is what the user would be staring at meanwhile.
    let keyboard_enhancement = keyboard_enhancement_supported();
    ratatui::run(|terminal| -> io::Result<()> {
        let _input = enable_terminal_input(keyboard_enhancement)?;
        let mut save_recovery = SaveRecovery::new();
        // The painted frame's text and its declared copyable rects, refreshed by
        // every board paint: the snapshot a finished drag selection slices its
        // copied text from ("copy what you see", inside what the painters declared
        // as content).
        let mut frame_rows: Vec<String> = Vec::new();
        let mut frame_copyable: Vec<Rect> = Vec::new();
        let mut frame_hits = crate::ui::render::QueueHitMap::default();
        // Left Down is deferred until Up (or abandoned when a text drag grows): firing
        // the click on Down made every board-row drag also peek/toggle, so copy only
        // felt reliable on the task page where Down is inert over notes.
        let mut drag_gesture = crate::ui::text_select::DragSelectGesture::new();
        let mut scrollbar_drag = false;
        let mut reflow_click = ReflowRowClick::default();
        // Non-resize event that arrived during a resize debounce window; handled
        // after one settled-size paint so a key typed mid-drag is not dropped.
        let mut pending_event: Option<Event> = None;
        loop {
            // Settle, paint, then wait. The wait is only the Frame Scheduler's idle floor.
            // All three are one call so the frame is painted before the wait can time out into
            // the `continue` below.
            let next = if let Some(event) = take_pending_after_paint(&mut pending_event, || {
                let area = terminal_area(terminal)?;
                sync_frame_presentation(area, &model);
                terminal.draw(|frame| {
                    let hits = draw_board(frame, &model);
                    frame_rows = frame_text_rows(frame.buffer_mut());
                    frame_copyable = hits.copyable.clone();
                    frame_hits = hits;
                })?;
                Ok::<(), io::Error>(())
            })? {
                event
            } else {
                let poll = board_idle_tick(
                    &mut model,
                    |model: &BoardModel| {
                        let area = terminal_area(terminal)?;
                        sync_frame_presentation(area, model);
                        terminal
                            .draw(|frame| {
                                let hits = draw_board(frame, model);
                                frame_rows = frame_text_rows(frame.buffer_mut());
                                frame_copyable = hits.copyable.clone();
                                frame_hits = hits;
                            })
                            .map(|_| ())
                    },
                    event::poll,
                    &store,
                    &mut domain,
                    &mut store_watch,
                    &save_recovery,
                    drag_gesture.has_autoscroll(),
                )?;
                if poll == FramePoll::Idle {
                    if let Some(auto) = drag_gesture.autoscroll() {
                        let area = terminal_area(terminal)?;
                        let content = drag_content_area(&model, area);
                        tick_drag_autoscroll(
                            &mut model,
                            &mut drag_gesture,
                            auto,
                            &frame_rows,
                            &frame_copyable,
                            content,
                        );
                    }
                    continue;
                }
                event::read()?
            };
            reflow_click.observe(&next);
            let event_area = match &next {
                Event::Resize(width, height) => Rect::new(0, 0, *width, *height),
                _ => terminal_area(terminal)?,
            };
            sync_frame_presentation(event_area, &model);
            match next {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // The board's new Ctrl+Q routes do not change the capture popup's keys.
                    // In particular, keep its existing Ctrl+C cancellation/exit routes intact.
                    if quick_capture
                        && key.code == KeyCode::Char('q')
                        && key.modifiers == KeyModifiers::CONTROL
                    {
                        continue;
                    }
                    // A key while the mouse button is held abandons the deferred click so
                    // Up does not fire a stale peek/select after the keyboard moved on.
                    drag_gesture.clear();
                    scrollbar_drag = false;
                    let area = terminal_area(terminal)?;
                    // The painted mode owns the keyboard: `resolve_board_surface` resolves the
                    // same input mode at every terminal size (`board_input_mode_for_area` no
                    // longer varies by area, per fix 1's gate removal), so no held-open modal
                    // can end up invisible under a shrunk pane and leave q / Esc unreachable.
                    let mode = resolve_board_surface(area, &mut model);
                    let Some(intent) = (if let Some(right) = model
                        .right_seat()
                        .filter(|_| model.project_right_seat_focused())
                    {
                        board_keyboard_intent_for_area(right, area, mode, key)
                    } else {
                        board_keyboard_intent_for_area(&model, area, mode, key)
                    }) else {
                        continue;
                    };
                    let Some(intent) = board_intent_for_presentation(area, &model, intent) else {
                        continue;
                    };
                    let RoutedBoardIntent {
                        intent,
                        target,
                        return_to_index,
                    } = route_board_intent(&model, intent);
                    let Some(intent) =
                        resolve_board_command(board_intent_target_mut(&mut model, target), intent)
                    else {
                        continue;
                    };
                    if dispatch_board_intent(
                        &store,
                        &mut domain,
                        &mut model,
                        BoardDispatchRoute { area, target },
                        intent,
                        &mut save_recovery,
                        quick_capture,
                    )? {
                        break;
                    }
                    if return_to_index
                        && dispatch_board_intent(
                            &store,
                            &mut domain,
                            &mut model,
                            BoardDispatchRoute {
                                area,
                                target: BoardIntentTarget::Outer,
                            },
                            BoardIntent::StageLeft,
                            &mut save_recovery,
                            quick_capture,
                        )?
                    {
                        break;
                    }
                }
                Event::Mouse(mouse) => {
                    // A left-button drag grows a text selection over the painted frame
                    // (highlight and copy text come from the last painted snapshot), and
                    // release hands that text to the clipboard. Clicks are deferred to Up
                    // so a drag does not also fire the Down-time peek/select path.
                    use crate::ui::text_select::{DragSelectOutcome, DragSelectPhase};
                    use crossterm::event::{MouseButton, MouseEventKind};
                    let area = terminal_area(terminal)?;
                    let continuing_row = reflow_click.target(&model, area, mouse).is_some();
                    let task_focus_candidate =
                        wide_mouse_focus_intent(&model, &frame_hits, area, mouse);
                    let (quit, mode, scrollbar) = board_scrollbar_mouse_route(
                        area,
                        &mut model,
                        &frame_hits,
                        &reflow_click,
                        mouse,
                        &mut scrollbar_drag,
                        |model, focus| {
                            handle_board_intent(
                                &store,
                                &mut domain,
                                model,
                                focus,
                                &mut save_recovery,
                                quick_capture,
                            )
                        },
                    )?;
                    if quit {
                        break;
                    }
                    match scrollbar {
                        ScrollbarMouse::Miss => {}
                        ScrollbarMouse::Intent(intent) => {
                            reflow_click.clear();
                            drag_gesture.clear();
                            let target =
                                mouse_intent_target(&model, area, mouse, &frame_hits, &intent);
                            if dispatch_board_intent(
                                &store,
                                &mut domain,
                                &mut model,
                                BoardDispatchRoute { area, target },
                                intent,
                                &mut save_recovery,
                                quick_capture,
                            )? {
                                break;
                            }
                            continue;
                        }
                        ScrollbarMouse::Consumed => {
                            reflow_click.clear();
                            drag_gesture.clear();
                            continue;
                        }
                    }
                    let mut click = mouse;
                    match mouse.kind {
                        MouseEventKind::Drag(MouseButton::Left) => {
                            let pos = clamp_position_to_area(
                                Position::new(mouse.column, mouse.row),
                                focused_mouse_area(&model, area),
                            );
                            model.drag_text_selection(pos);
                            if model.text_selection().is_none() {
                                drag_gesture.clear();
                                continue;
                            }
                            if let Some(sel) = model.text_selection() {
                                if let Some(text) =
                                    selection_text(&frame_rows, &frame_copyable, &sel)
                                {
                                    if let Some(first) = text.lines().next() {
                                        drag_gesture.ensure_copy_origin(first.to_string());
                                    }
                                }
                            }
                            let _ = drag_gesture.handle(
                                DragSelectPhase::Move,
                                pos,
                                model.text_selection(),
                            );
                            let area = terminal_area(terminal)?;
                            drag_gesture.update_autoscroll(
                                pos.y,
                                drag_content_area(&model, area),
                                model.text_selection().is_some_and(|s| s.has_area()),
                            );
                            continue;
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            let pos = if model.text_selection().is_some() {
                                clamp_position_to_area(
                                    Position::new(mouse.column, mouse.row),
                                    focused_mouse_area(&model, area),
                                )
                            } else {
                                Position::new(mouse.column, mouse.row)
                            };
                            // Some hosts omit Drag and only move between Down and Up.
                            // Grow the selection from the press cell before clearing it
                            // so copy still works there (and peek is not fired instead).
                            if model.text_selection().is_none() {
                                model.drag_text_selection(pos);
                            }
                            model.end_mouse_press();
                            match drag_gesture.handle(
                                DragSelectPhase::Release,
                                pos,
                                model.text_selection(),
                            ) {
                                DragSelectOutcome::Copy => {
                                    copy_drag_selection(
                                        &mut model,
                                        &frame_rows,
                                        &frame_copyable,
                                        drag_gesture.captured_before(),
                                        drag_gesture.captured_after(),
                                        drag_gesture.copy_origin(),
                                    );
                                    continue;
                                }
                                DragSelectOutcome::Click(down) => {
                                    model.clear_text_selection();
                                    click = crossterm::event::MouseEvent {
                                        kind: MouseEventKind::Down(MouseButton::Left),
                                        column: down.x,
                                        row: down.y,
                                        modifiers: mouse.modifiers,
                                    };
                                }
                                DragSelectOutcome::Continue => continue,
                            }
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            // Anchor a would-be selection and stash the Down for Up;
                            // do not map the click yet. A wide board-row click remains live
                            // while task-focused, every other press belongs to the focused side.
                            //
                            // INVARIANT: this Down-time gate and the Up-time dispatch
                            // (`board_mouse_click_intent_after_focus` → `board_mouse_intent` →
                            // `map_responsive_board_mouse`) are two calls of the same mapper over
                            // the same model and hit map, and no intent runs between them. They
                            // must agree: a press survives the gate below if and only if the
                            // release route maps it to a dispatchable intent. If either the gate
                            // (`press_survives_off_focus`) or the mapper's wide routing changes,
                            // both sides must change together.
                            let pos = Position::new(mouse.column, mouse.row);
                            let responsive_intent =
                                map_responsive_board_mouse(&model, &frame_hits, area, mouse);
                            let focused = press_on_focused_surface(&model, &frame_hits, area, pos);
                            if !focused
                                && !continuing_row
                                && !press_survives_off_focus(
                                    responsive_intent.as_ref(),
                                    mode,
                                    task_focus_candidate.as_ref(),
                                )
                            {
                                model.end_mouse_press();
                                drag_gesture.clear();
                                continue;
                            }
                            if focused {
                                model.begin_mouse_press(pos);
                            } else {
                                model.end_mouse_press();
                            }
                            let _ = drag_gesture.handle(DragSelectPhase::Press, pos, None);
                            if let Some(line) =
                                copyable_line_at(&frame_rows, &frame_copyable, pos.y)
                            {
                                drag_gesture.ensure_copy_origin(line);
                            }
                            continue;
                        }
                        _ => {}
                    }
                    let area = terminal_area(terminal)?;
                    let (quit, intent) = board_mouse_click_intent_after_focus(
                        area,
                        &mut model,
                        &frame_hits,
                        &mut reflow_click,
                        click,
                        |model, focus| {
                            handle_board_intent(
                                &store,
                                &mut domain,
                                model,
                                focus,
                                &mut save_recovery,
                                quick_capture,
                            )
                        },
                    )?;
                    if quit {
                        break;
                    }
                    let Some(intent) = intent else {
                        continue;
                    };
                    let target = mouse_intent_target(&model, area, click, &frame_hits, &intent);
                    if dispatch_board_intent(
                        &store,
                        &mut domain,
                        &mut model,
                        BoardDispatchRoute { area, target },
                        intent,
                        &mut save_recovery,
                        quick_capture,
                    )? {
                        break;
                    }
                }
                Event::Paste(text) => {
                    let area = terminal_area(terminal)?;
                    let Some(intent) = board_paste_intent(area, &mut model, &text) else {
                        continue;
                    };
                    let target = if model.project_right_seat_focused() {
                        BoardIntentTarget::Focused
                    } else {
                        BoardIntentTarget::Outer
                    };
                    if dispatch_board_intent(
                        &store,
                        &mut domain,
                        &mut model,
                        BoardDispatchRoute { area, target },
                        intent,
                        &mut save_recovery,
                        quick_capture,
                    )? {
                        break;
                    }
                }
                Event::Resize(_, _) => {
                    // Dragging a pane edge fires a burst of resizes; wait them out
                    // so the next paint rebuilds layout once at the settled size.
                    pending_event = scheduler::coalesce_resizes(event::poll, event::read)?;
                    continue;
                }
                _ => {}
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// Copy a finished drag selection from the last painted frame to the clipboard.
///
/// Only cells the painters declared copyable are taken, so scrollbars, borders,
/// and other chrome never reach the clipboard. A selection with no text (a bare
/// click, or a drag over blank cells) copies nothing. Success flashes a short
/// ephemeral status that clears itself; the highlight drops so it does not stick.
fn copy_drag_selection(
    model: &mut BoardModel,
    frame_rows: &[String],
    copyable: &[Rect],
    captured_before: &[String],
    captured_after: &[String],
    origin: Option<&str>,
) {
    let Some(selection) = model.text_selection() else {
        return;
    };
    let live = selection_text(frame_rows, copyable, &selection);
    let from_origin = selection.anchor.y <= selection.head.y;
    let Some(text) = crate::ui::text_select::compose_selection_copy(
        captured_before,
        live,
        captured_after,
        origin,
        from_origin,
    ) else {
        model.clear_text_selection();
        return;
    };
    if copy_to_clipboard(&text) {
        model.set_ephemeral_message("copied", Duration::from_secs(2));
    } else {
        model.set_ephemeral_message("copy failed", Duration::from_secs(2));
    }
    model.clear_text_selection();
}

fn clamp_position_to_area(position: Position, area: Rect) -> Position {
    if area.is_empty() {
        return position;
    }
    Position::new(
        position
            .x
            .clamp(area.x, area.x.saturating_add(area.width).saturating_sub(1)),
        position
            .y
            .clamp(area.y, area.y.saturating_add(area.height).saturating_sub(1)),
    )
}

/// Content rect that edge auto-scroll watches during a text drag.
pub fn drag_content_area(model: &BoardModel, area: Rect) -> Rect {
    let responsive = model.responsive_geometry(area);
    let right_seat = model.project_right_seat_focused();
    let task_focus = model.input_focused_surface() == crate::ui::tier::FocusedSurface::Task;
    let surface = if right_seat || task_focus {
        responsive.task_content()
    } else {
        responsive.board
    };
    // Only chrome row positions shape this drag viewport; they depend on the live
    // content height, not the renderer's standard/compact density decision.
    let geo = crate::ui::tier::resolve(surface.width, surface.height);
    if task_focus {
        // Approximate the shared notes/steps viewport: below the two-row header, above
        // the rule. Exact step halving is unnecessary for edge detection.
        let top = surface.y.saturating_add(geo.viewport_top).saturating_add(1);
        let bottom = surface
            .y
            .saturating_add(geo.rule_row.unwrap_or(geo.height.saturating_sub(2)));
        Rect::new(surface.x, top, surface.width, bottom.saturating_sub(top))
    } else {
        Rect::new(
            surface.x,
            surface.y.saturating_add(geo.viewport_top),
            surface.width,
            geo.viewport_height,
        )
    }
}

/// One idle tick of edge auto-scroll while a text drag sits near the content edge.
pub fn tick_drag_autoscroll(
    model: &mut BoardModel,
    gesture: &mut crate::ui::text_select::DragSelectGesture,
    auto: crate::ui::text_select::DragAutoScrollState,
    frame_rows: &[String],
    copyable: &[Rect],
    content: Rect,
) {
    let delta = match model.input_focused_surface() {
        crate::ui::tier::FocusedSurface::Task
            if matches!(
                model.input_mode(),
                BoardInputMode::TaskPage | BoardInputMode::EditStep
            ) =>
        {
            model.nudge_notes_scroll(auto.direction, auto.speed)
        }
        crate::ui::tier::FocusedSurface::Board
            if matches!(
                model.input_mode(),
                BoardInputMode::Normal | BoardInputMode::TaskPage
            ) =>
        {
            model.nudge_list_scroll(auto.direction, auto.speed)
        }
        _ => 0,
    };
    if delta == 0 {
        return;
    }
    if let Some(selection) = model.text_selection() {
        gesture.capture_leaving_rows(
            frame_rows,
            copyable,
            selection,
            content,
            auto.direction,
            delta,
        );
    }
    if let Some(y) = gesture.last_drag_row() {
        let x = model.text_selection().map(|s| s.head.x).unwrap_or(0);
        model.recompute_text_selection_head(ratatui::layout::Position::new(x, y));
    }
}

/// Whether save recovery permits the board to apply background state changes.
fn board_background_work_allowed(recovery: &SaveRecovery<DomainState>) -> bool {
    !recovery.is_pending()
}

/// Cheap idle-tick change detector for the store's on-disk document.
///
/// Holds only `tsk.json`'s last-seen [`StoreSignature`], so the frame loop's Idle
/// branch -- which runs about 4 times a second --
/// pays for a `stat`, not a parse, on every tick where nothing changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StoreWatch {
    last_seen: Option<StoreSignature>,
}

impl StoreWatch {
    /// Unseeded: the first check always reports changed. Prefer [`Self::seeded`] right after a
    /// load so the first idle tick does not immediately re-merge what was just read.
    pub fn new() -> Self {
        Self { last_seen: None }
    }

    /// Seed from the store's current on-disk signature (e.g. right after `load_board()`).
    pub fn seeded(store: &TaskStore) -> Self {
        Self {
            last_seen: store.state_signature(),
        }
    }

    /// `Some(signature)` when the store's on-disk signature differs from what was last
    /// recorded; `None` when unchanged. Does **not** update `last_seen` -- call
    /// [`Self::record`] with the returned signature only once the load it gates actually
    /// succeeds, so a transient read failure leaves the watch exactly where it was
    /// and the very next tick tries again instead of treating the failed read as caught up.
    fn poll(&self, store: &TaskStore) -> Option<Option<StoreSignature>> {
        let current = store.state_signature();
        if current == self.last_seen {
            None
        } else {
            Some(current)
        }
    }

    /// Record a signature already returned by [`Self::poll`], after the load it gated
    /// succeeded.
    fn record(&mut self, signature: Option<StoreSignature>) {
        self.last_seen = signature;
    }
}

/// On the frame loop's Idle branch: revalidate the store cheaply, no host calls.
///
/// Only when [`StoreWatch::poll`] reports a changed signature does this pay for
/// `store.load()` + [`DomainState::merge_tasks_from_disk`] + [`BoardModel::sync_from_domain`],
/// so a quick-capture popup (a separate process writing the same `tsk.json`) becomes
/// visible on an open, idle board without a persisting intent from this board and without a
/// host call.
///
/// Skips entirely while a failed save owns the displayed working state
/// ([`board_background_work_allowed`]): reloading disk under save recovery would replace the
/// working state the user is retrying. An open edit session is never redirected either --
/// `sync_from_domain` reanchors selection by id and never touches the edit binding.
///
/// A failed `store.load()` (e.g. a transient read error) does not advance the watch:
/// the signature [`StoreWatch::poll`] observed is recorded only once this load has actually
/// succeeded and merged, so a failed read leaves `last_seen` exactly where it was and the
/// very next idle tick still sees the store as changed and tries again -- instead of, as
/// before this fix, treating the failed read as caught up and staying stale until the next
/// write produces a signature this watch had not already recorded for nothing.
///
/// Returns whether a merge actually ran, so tests can assert the cheap path stayed cheap.
pub fn revalidate_board_from_store(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    watch: &mut StoreWatch,
    save_recovery: &SaveRecovery<DomainState>,
) -> bool {
    if !board_background_work_allowed(save_recovery) {
        return false;
    }
    let Some(signature) = watch.poll(store) else {
        return false;
    };
    let Ok(disk) = store.load() else {
        return false;
    };
    domain.merge_tasks_from_disk(&disk);
    model.sync_from_domain(domain);
    watch.record(signature);
    true
}

fn terminal_area(terminal: &DefaultTerminal) -> io::Result<Rect> {
    let size = terminal.size()?;
    Ok(Rect::new(0, 0, size.width, size.height))
}

fn sync_frame_presentation(area: Rect, model: &BoardModel) {
    let wide = model.responsive_geometry(area).presentation
        == crate::ui::tier::ResponsivePresentation::WideSplit;
    model.set_frame_wide(wide);
}

/// One board intent plus the persistence baseline it must recover to on a failed save.
pub struct BoardSaveContext<'a> {
    pub baseline: DomainState,
    pub intent: BoardIntent,
    pub snapshot: Option<&'a InvocationSnapshot>,
}

/// Apply one board intent through the save-recovery boundary.
///
/// A failed save transfers the working state into [`SaveRecovery`] and leaves the caller's
/// domain empty until explicit Retry promotes that state or Cancel restores the baseline. The
/// board model continues to render the retained working snapshot throughout recovery.
/// Which mapper owns a board keypress. The shared form field map applies only while a
/// field of the open form actually has focus (or its scope dropdown is open). The task
/// page's view mode keeps its own keymap even though a form is open -- otherwise a bare
/// `e` on the page would be inserted into the title draft instead of entering edit mode.
fn board_keyboard_intent_for_area(
    model: &BoardModel,
    area: Rect,
    mode: BoardInputMode,
    key: crossterm::event::KeyEvent,
) -> Option<BoardIntent> {
    let presentation = model.responsive_geometry(area).presentation;
    match route_responsive_key(mode, model.input_stage(), presentation, key) {
        ResponsiveKeyRoute::Intent(intent) => Some(intent),
        ResponsiveKeyRoute::Inert => None,
        ResponsiveKeyRoute::Surface => board_keyboard_intent(model, mode, key),
    }
}

fn board_keyboard_intent(
    model: &BoardModel,
    mode: BoardInputMode,
    key: crossterm::event::KeyEvent,
) -> Option<BoardIntent> {
    // Mark mode owns its exit keys even when a page or popup currently outranks the board.
    // This keeps progressive close behavior from leaving a latent marked set behind.
    if model.mark_mode_active() {
        let text_entry_owns_capital_m = matches!(
            mode,
            BoardInputMode::EditTitle
                | BoardInputMode::EditNotes
                | BoardInputMode::EditThread
                | BoardInputMode::EditStep
                | BoardInputMode::Palette
                | BoardInputMode::Help
                | BoardInputMode::ListPicker
                | BoardInputMode::Search
                | BoardInputMode::QuickAdd
        );
        if !text_entry_owns_capital_m
            && mode != BoardInputMode::SaveRecovery
            && key.code == KeyCode::Char('M')
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return Some(BoardIntent::ToggleMarkMode);
        }
        if mode != BoardInputMode::SaveRecovery
            && key.code == KeyCode::Esc
            && key.modifiers.is_empty()
        {
            return Some(BoardIntent::MarkClear);
        }
    }

    // Ctrl+Q belongs to the resolved surface before an open form can redirect input to its
    // retained field mapper. Text editors and save recovery remain inert because `map_key`
    // deliberately returns no Quit intent for those modes.
    if key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::CONTROL {
        return map_key(mode, key);
    }

    // Allowlist, not a denylist: the form mapper owns the keyboard ONLY while the resolved
    // mode is genuinely one of the form's own field/dropdown states. `input_mode()` lets a
    // popup or command surface OUTRANK the form's mode (see its `match self.popup`), so an
    // overriding mode can arrive with a form still open. Excluding just one such mode left
    // every other one swallowed: under `SaveRecovery` the form mapper ate `r`, `c` and Esc,
    // so `map_save_recovery`'s Retry/Cancel never ran and a failed save could not be
    // resolved (or escaped) from the keyboard at all.
    let form_field_mode = matches!(
        mode,
        BoardInputMode::EditTitle
            | BoardInputMode::EditNotes
            | BoardInputMode::EditThread
            | BoardInputMode::EditScope
            | BoardInputMode::FormScopeDropdown
    );
    // A selected task-page add target has no field mapper, but its enclosing edit session
    // still owns the one Shift+Enter task-save chord.
    if mode == BoardInputMode::TaskPage
        && model.task_editing()
        && key.code == KeyCode::Enter
        && key.modifiers == KeyModifiers::SHIFT
    {
        return Some(BoardIntent::ConfirmEdit);
    }
    // Bare Enter on a stored step toggles it. Resolved here, where the model is in reach,
    // so the persisting intent is classified before the save boundary sees it.
    if mode == BoardInputMode::TaskPage
        && key.code == KeyCode::Enter
        && key.modifiers.is_empty()
        && model.stored_step_selected()
    {
        return Some(BoardIntent::ToggleStep);
    }
    // Task-page Thread has a selected state before its text cursor opens. It owns Enter and
    // Tab itself, while capture's direct Thread editor keeps the shared form mapper.
    if matches!(
        mode,
        BoardInputMode::SelectThread | BoardInputMode::EditThread
    ) && model.task_editing()
    {
        return map_key(mode, key);
    }
    // `form_focus()` is Some whenever a form is open, so this reads as an invariant. It is still
    // not worth an `expect` here: this runs on every keypress inside the raw-mode event loop, so
    // a panic would abort with the terminal still in raw mode and take the user's shell with it.
    // Falling through to `map_key` degrades to normal-mode routing instead of dying.
    // Persistent navigation: `1` desk · `2` selected project · `3` projects, from
    // every normal-mode surface. Tab 2 with no selected project opens the picker
    // (the reducer answers the intent), so the digits never change meaning.
    if mode == BoardInputMode::Normal {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            // fall through
        } else if let KeyCode::Char(c) = key.code {
            let tab = match c {
                '1' => Some(NavTab::Desk),
                '2' => Some(NavTab::ProjectBoard),
                '3' => Some(NavTab::Projects),
                _ => None,
            };
            if let Some(tab) = tab {
                return Some(BoardIntent::SelectNavTab(tab));
            }
        }
    }

    match model.form_focus().filter(|_| form_field_mode) {
        Some(focus) => map_task_form_key(focus, mode == BoardInputMode::FormScopeDropdown, key),
        None => map_key(mode, key),
    }
}

pub fn apply_board_intent_with_save_recovery(
    domain: &mut DomainState,
    model: &mut BoardModel,
    recovery: &mut SaveRecovery<DomainState>,
    context: BoardSaveContext<'_>,
    mut persist: impl FnMut(&mut DomainState) -> Result<(), String>,
) -> Result<IntentOutcome, DomainError> {
    let BoardSaveContext {
        baseline,
        intent,
        snapshot,
    } = context;
    // Resolve a command-surface confirmation before the recovery gate, so the surface
    // dispatches the same intent the direct route would and gains no exemption.
    let Some(intent) = resolve_board_command(model, intent) else {
        return Ok(IntentOutcome::None);
    };
    if recovery.is_pending() {
        match intent {
            // A failed save must be resolved through Retry or Cancel. Ctrl+Q is unmapped in
            // SaveRecovery, and a Help card opened over recovery cannot bypass that gate.
            BoardIntent::Quit => return Ok(IntentOutcome::None),
            BoardIntent::RetrySave => {
                // The mouse hands this intent in already resolved, so close the surface here
                // exactly as the keyboard's ConfirmCommand route does.
                model.close_command_surface();
                if let Some(working) = recovery.retry(|working| persist(working)) {
                    *domain = working;
                    model.release_task_edit_save();
                    model.sync_from_domain(domain);
                    model.end_save_recovery(SaveResolution::Retried);
                    if !model.has_saved_task() {
                        model.set_message("saved");
                    }
                    return Ok(IntentOutcome::Persisted);
                }
                model.begin_save_recovery(recovery.error().unwrap_or("save failed"));
                return Ok(IntentOutcome::None);
            }
            BoardIntent::CancelSave => {
                model.close_command_surface();
                *domain = recovery.cancel().expect("pending recovery has a baseline");
                model.sync_from_domain(domain);
                let cancelled_quick_add = model.end_save_recovery(SaveResolution::Cancelled);
                if !cancelled_quick_add {
                    model.set_message("save cancelled");
                }
                return Ok(IntentOutcome::None);
            }
            // Navigation and presentation state mutate nothing.
            BoardIntent::SelectNext
            | BoardIntent::SelectPrev
            | BoardIntent::ToggleMarkMode
            | BoardIntent::MarkClear
            | BoardIntent::SelectIndex(_)
            | BoardIntent::FocusBoardAndSelectIndex(_)
            | BoardIntent::ListScrollTo(_)
            | BoardIntent::PageScrollTo(_)
            | BoardIntent::OpenCommandPalette
            | BoardIntent::CommandNext
            | BoardIntent::CommandPrev
            | BoardIntent::CommandQueryInsert(_)
            | BoardIntent::CommandQueryInsertText(_)
            | BoardIntent::CommandQueryBackspace
            | BoardIntent::CloseCommandSurface
            | BoardIntent::OpenHelp
            | BoardIntent::CloseHelp
            | BoardIntent::HelpQueryInsert(_)
            | BoardIntent::HelpQueryInsertText(_)
            | BoardIntent::HelpQueryBackspace
            | BoardIntent::HelpScrollUp
            | BoardIntent::HelpScrollDown
            | BoardIntent::CloseLayer
            | BoardIntent::OpenTaskPage
            | BoardIntent::StageRight
            | BoardIntent::StageLeft
            | BoardIntent::PeekDetail
            | BoardIntent::CollapseDetail
            | BoardIntent::PageScrollUp
            | BoardIntent::PageScrollDown
            | BoardIntent::PageWheelScrollUp
            | BoardIntent::PageWheelScrollDown
            | BoardIntent::ToggleDoneDrawer
            | BoardIntent::ToggleArchivedGroup
            | BoardIntent::ToggleInboxGroup => return apply_intent(domain, model, intent, None),
            _ => {
                model.begin_save_recovery(recovery.error().unwrap_or("save failed"));
                return Ok(IntentOutcome::None);
            }
        }
    }

    // decision 8: judged against the durable record, before the reducer runs, so no mutation
    // happens and the session (mode, draft, cursor, binding) survives the refusal intact. The
    // caller presents the returned error on the message row. Both save chords on a line
    // editor are this one surface, so they refuse identically: Enter (`ConfirmEdit`)
    // and Shift+Enter (`ConfirmEditNext`).
    if matches!(
        intent,
        BoardIntent::ConfirmEdit | BoardIntent::ConfirmEditNext
    ) {
        if let Some(refusal) = confirm_edit_refusal_against_the_record(&baseline, model) {
            return Err(refusal);
        }
    }

    let holds_task_edit = matches!(
        intent,
        BoardIntent::ConfirmEdit | BoardIntent::ConfirmEditNext
    ) && model.edit_target().is_some()
        // New-step adds own their save state. Existing-step renames belong to the task session,
        // so Shift+Enter must retain that complete form through the persistence boundary.
        && (model.input_mode() != BoardInputMode::EditStep
            || (intent == BoardIntent::ConfirmEditNext && model.has_active_step_rename()));
    if holds_task_edit {
        model.hold_task_edit_save();
    }
    let outcome = match apply_intent(domain, model, intent, snapshot) {
        Ok(outcome) => outcome,
        Err(error) => {
            if holds_task_edit {
                model.release_task_edit_save();
            }
            return Err(error);
        }
    };
    if outcome != IntentOutcome::Persist {
        if holds_task_edit {
            model.release_task_edit_save();
        }
        return Ok(outcome);
    }
    if let Err(error) = persist(domain) {
        let working = std::mem::take(domain);
        recovery.fail(baseline, working, error);
        model.begin_save_recovery(recovery.error().unwrap_or("save failed"));
        return Ok(IntentOutcome::None);
    }
    model.release_task_edit_save();
    model.sync_from_domain(domain);
    Ok(IntentOutcome::Persisted)
}

/// Input mode the resolved board mode owns for this area.
///
/// the legacy Resize band used to force `Normal` here on the premise that a modal open
/// before the pane shrank was no longer painted. The Tier
/// Layout Resolver's queue overlay paints an open field editor as a full-screen takeover at
/// every size regardless of `BoardMode`, so that premise no longer holds: forcing
/// `Normal` while the overlay still shows, say, an open title editor would make its keys
/// silently reach the board reducer instead of the field, both losing the keystroke and (once
/// the intent gate below is gone too) risking a stray mutation behind a visibly open editor.
/// Every area now keeps the model's own input mode, the same as Wide and Browse always did.
fn board_input_mode_for_area(_area: Rect, mode: BoardInputMode) -> BoardInputMode {
    mode
}

/// Resolve the surface this area actually paints, before any input is mapped against it.
///
/// the legacy Resize band (<50x18) used to force-close popup/help/detail/command surfaces
/// here and swallow every mutating intent in [`board_intent_for_area`] below it,
/// which left's compact-tier controls dead down to 40x10. `BoardMode` still classifies
/// the painted layout (`board_input_mode_for_area`, `board_layout*`), but no longer gates or
/// force-closes anything: every surface is routed the same way at every supported size.
fn resolve_board_surface(area: Rect, model: &mut BoardModel) -> BoardInputMode {
    board_input_mode_for_area(area, model.input_mode())
}

/// Route an intent through unchanged; the area no longer gates it.
///
/// Kept as the named choke point every key/paste/mouse route already passes through, so a
/// future area-dependent rule (if any) has one place to land, and so the call sites and their
/// tests do not need to change shape.
fn board_intent_for_area(_area: Rect, intent: BoardIntent) -> Option<BoardIntent> {
    Some(intent)
}

/// Apply the one presentation-dependent input gate before an intent reaches the reducer.
/// Projects preview opening is the only route whose meaning changes with width: a narrow
/// projects index remains a FullBoard with no right seat, while a wide frame may enter Split.
fn board_intent_for_presentation(
    area: Rect,
    model: &BoardModel,
    intent: BoardIntent,
) -> Option<BoardIntent> {
    let intent = board_intent_for_area(area, intent)?;
    if model.projects_overview()
        && model.wide_stage() == crate::ui::tier::WideStage::FullBoard
        && model.responsive_geometry(area).presentation
            != crate::ui::tier::ResponsivePresentation::WideSplit
        && intent == BoardIntent::StageRight
    {
        return None;
    }
    Some(intent)
}

/// Open a projects preview only after the input boundary has seen the responsive presentation.
/// Selection and row-click reducers stay width-agnostic, so direct model updates cannot create
/// a right seat on a narrow frame.
fn auto_open_projects_preview(area: Rect, model: &mut BoardModel, intent: &BoardIntent) {
    if !matches!(
        intent,
        BoardIntent::SelectNext
            | BoardIntent::SelectPrev
            | BoardIntent::SelectProjectRow(_)
            | BoardIntent::StageRight
    ) {
        return;
    }
    if model.projects_overview()
        && model.wide_stage() == crate::ui::tier::WideStage::FullBoard
        && model.responsive_geometry(area).presentation
            == crate::ui::tier::ResponsivePresentation::WideSplit
    {
        let _ = model.open_project_preview();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoardIntentTarget {
    Outer,
    Focused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutedBoardIntent {
    intent: BoardIntent,
    target: BoardIntentTarget,
    return_to_index: bool,
}

#[derive(Debug, Clone, Copy)]
struct BoardDispatchRoute {
    area: Rect,
    target: BoardIntentTarget,
}

/// Resolve the nested projects-preview escape and ownership rules once for every keyboard route.
/// The reducer still receives an ordinary board intent, but this helper keeps the outer/index
/// target and the second Escape transition together and directly testable.
fn route_board_intent(model: &BoardModel, intent: BoardIntent) -> RoutedBoardIntent {
    let escape_leave_requested = model.project_right_board_leave_requested();
    let arrow_leave_requested = model.project_right_board_arrow_leave_requested();
    let intent = if intent == BoardIntent::CollapseDetail && arrow_leave_requested {
        BoardIntent::StageLeft
    } else {
        intent
    };
    let return_to_index = intent == BoardIntent::CloseLayer
        && escape_leave_requested
        && model
            .right_seat()
            .is_some_and(|right| !right.mark_mode_active());
    let leave_from_arrow = intent == BoardIntent::StageLeft && arrow_leave_requested;
    let global_navigation = model.project_right_seat_focused()
        && matches!(
            intent,
            BoardIntent::SelectNavTab(_) | BoardIntent::OpenProjectSelector
        );
    let target = if leave_from_arrow || global_navigation {
        BoardIntentTarget::Outer
    } else {
        BoardIntentTarget::Focused
    };
    RoutedBoardIntent {
        intent,
        target,
        return_to_index,
    }
}

fn board_intent_target_mut(model: &mut BoardModel, target: BoardIntentTarget) -> &mut BoardModel {
    match target {
        BoardIntentTarget::Outer => model,
        BoardIntentTarget::Focused => model.input_target_mut(),
    }
}

/// Route a bracketed paste to the board intent the painted surface accepts.
///
/// A paste arrives as `Event::Paste`, so it cannot go through `map_key`; it still passes the
/// same surface resolution, area gate, and command resolution the key route applies, so a
/// paste can never reach a route a key press could not.
fn board_paste_intent(area: Rect, model: &mut BoardModel, text: &str) -> Option<BoardIntent> {
    let target = model.input_target_mut();
    let mode = resolve_board_surface(area, target);
    let intent = map_edit_paste(mode, text)?;
    let intent = board_intent_for_area(area, intent)?;
    resolve_board_command(target, intent)
}

/// Whether a press outside the focused column still reaches dispatch: an explicit row
/// select, a stage slide (`←` from the rail, `→` into the task column), a modal close
/// route, or a focus transfer.
///
/// This is the Down-time half of the press gate; the Up-time half re-maps the same press in
/// `board_mouse_intent`. Both call `map_responsive_board_mouse` over the same model and hit
/// map with no intent in between, so they must agree: keep this predicate in lockstep with
/// the mapper's wide routing (see the invariant at the `Down(MouseButton::Left)` arm).
fn press_survives_off_focus(
    responsive_intent: Option<&BoardIntent>,
    mode: BoardInputMode,
    task_focus_candidate: Option<&BoardIntent>,
) -> bool {
    let slide_or_select = matches!(
        responsive_intent,
        Some(
            BoardIntent::FocusBoardAndSelectIndex(_)
                | BoardIntent::SelectProjectRow(_)
                | BoardIntent::StageLeft
                | BoardIntent::StageRight
        )
    );
    let existing_mode_route = matches!(
        (mode, responsive_intent),
        (BoardInputMode::Help, Some(BoardIntent::CloseHelp))
            | (
                BoardInputMode::Palette,
                Some(BoardIntent::CloseCommandSurface)
            )
    );
    slide_or_select || existing_mode_route || task_focus_candidate.is_some()
}

/// Keep a row gesture attached to its original cell and task across a column reflow.
/// The reducer's existing clock authorizes continuation only after successful selection.
#[derive(Default)]
struct ReflowRowClick(Option<(Position, Rect, uuid::Uuid)>);

impl ReflowRowClick {
    fn clear(&mut self) {
        self.0 = None;
    }

    fn observe(&mut self, event: &Event) {
        use crossterm::event::{MouseButton, MouseEventKind};
        match event {
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                if self
                    .0
                    .is_some_and(|(pos, _, _)| pos != Position::new(mouse.column, mouse.row))
                {
                    self.clear();
                }
            }
            Event::Mouse(mouse)
                if matches!(
                    mouse.kind,
                    MouseEventKind::Up(MouseButton::Left) | MouseEventKind::Moved
                ) => {}
            _ => self.clear(), // keys, resize, wheel, drag, other buttons and focus changes
        }
    }

    fn target(
        &self,
        model: &BoardModel,
        area: Rect,
        mouse: crossterm::event::MouseEvent,
    ) -> Option<uuid::Uuid> {
        use crossterm::event::{MouseButton, MouseEventKind};
        let (pos, frame, id) = self.0?;
        (mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && pos == Position::new(mouse.column, mouse.row)
            && frame == area
            && model.wide_stage() == crate::ui::tier::WideStage::Split
            && model.input_mode() == BoardInputMode::Normal
            && model.pending_row_double_click(id))
        .then_some(id)
    }
}

/// Run the release-time task focus handoff before mapping the same click.
///
/// The dispatcher is the real save-recovery-aware event-loop boundary in production and a
/// direct reducer in tests. Keeping both operations in this helper pins their ordering.
fn board_mouse_click_intent_after_focus<E>(
    area: Rect,
    model: &mut BoardModel,
    painted_hits: &crate::ui::render::QueueHitMap,
    reflow_click: &mut ReflowRowClick,
    click: crossterm::event::MouseEvent,
    mut dispatch_focus: impl FnMut(&mut BoardModel, BoardIntent) -> Result<bool, E>,
) -> Result<(bool, Option<BoardIntent>), E> {
    // Consume the second release before preview controls or reflowed rows can see it.
    if let Some(id) = reflow_click.target(model, area, click) {
        reflow_click.clear();
        let intent = (model.selected_id() == Some(id))
            .then(|| {
                model
                    .visible_ids()
                    .iter()
                    .position(|&visible| visible == id)
                    .map(BoardIntent::FocusBoardAndSelectIndex)
            })
            .flatten();
        return Ok((false, intent));
    }
    reflow_click.clear();
    if let Some(focus) = wide_mouse_focus_intent(model, painted_hits, area, click) {
        if dispatch_focus(model, focus)? {
            return Ok((true, None));
        }
        // The stage moved under the pointer, so the columns may have changed width. The
        // click still names the control the user saw: dispatch it against the painted frame
        // rather than a repaint whose rows no longer line up with the press.
        resolve_board_surface(area, model);
        model.cancel_project_header_double_click();
        let intent = map_responsive_board_mouse(model, painted_hits, area, click)
            .and_then(|intent| board_intent_for_presentation(area, model, intent))
            .and_then(|intent| {
                resolve_board_command(
                    mouse_intent_target_mut(model, area, click, painted_hits, &intent),
                    intent,
                )
            });
        return Ok((false, intent));
    }
    let intent = board_mouse_intent(area, model, click);
    if matches!(
        model.wide_stage(),
        crate::ui::tier::WideStage::FullBoard | crate::ui::tier::WideStage::Rail
    ) && matches!(
        model.input_mode(),
        BoardInputMode::Normal | BoardInputMode::TaskPage
    ) {
        if let Some(BoardIntent::FocusBoardAndSelectIndex(index)) = intent.as_ref() {
            if let Some(&id) = model.visible_ids().get(*index) {
                reflow_click.0 = Some((Position::new(click.column, click.row), area, id));
            }
        }
    }
    Ok((false, intent))
}

/// Route a scrollbar event, focusing and repainting a task preview before page dispatch.
///
/// Board-focused previews paint from a clone, so their scrollbar can advertise a different
/// wrapping bound from the retained page session. Once focus transfers, one scratch paint
/// refreshes that retained bound before the clicked offset reaches the reducer.
fn board_scrollbar_mouse_route<E>(
    area: Rect,
    model: &mut BoardModel,
    painted_hits: &crate::ui::render::QueueHitMap,
    reflow_click: &ReflowRowClick,
    mouse: crossterm::event::MouseEvent,
    dragging: &mut bool,
    mut dispatch_focus: impl FnMut(&mut BoardModel, BoardIntent) -> Result<bool, E>,
) -> Result<(bool, BoardInputMode, ScrollbarMouse), E> {
    use crossterm::event::{MouseButton, MouseEventKind};

    if reflow_click.target(model, area, mouse).is_some() {
        return Ok((false, model.input_mode(), ScrollbarMouse::Miss));
    }
    let task_scrollbar_press = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && matches!(
            scrollbar_hit_at(painted_hits, Position::new(mouse.column, mouse.row)),
            Some(BoardIntent::PageScrollTo(_))
        );
    let preview_focus = if task_scrollbar_press {
        wide_mouse_focus_intent(model, painted_hits, area, mouse)
    } else {
        None
    };
    let refresh_retained_bound = preview_focus.is_some();
    if let Some(focus) = preview_focus {
        if dispatch_focus(model, focus)? {
            return Ok((true, model.input_mode(), ScrollbarMouse::Consumed));
        }
    }
    let mode = resolve_board_surface(area, model);
    let routed = if refresh_retained_bound {
        let refreshed_hits = crate::ui::board::board_hit_map(area, model);
        map_scrollbar_mouse(mode, &refreshed_hits, mouse, dragging)
    } else {
        map_scrollbar_mouse(mode, painted_hits, mouse, dragging)
    };
    Ok((false, mode, routed))
}

/// Route a mouse event to the board intent the painted frame accepts.
///
/// `resolve_board_surface`'s side effects run first, exactly as for a key press: a shrink
/// past the classic Resize threshold collapses whatever surface it left open before the
/// hit-map is rebuilt, so a click can never land on a control the shrink already withdrew.
/// The hit-map itself comes from [`crate::ui::board::board_hit_map`], the same painter
/// [`draw_board`] uses, so the click and the screen the user is looking at can never
/// disagree about where a control is. The area gate and command resolution afterward are
/// the same ones the key and paste routes already pass through.
fn mouse_intent_target(
    model: &BoardModel,
    area: Rect,
    mouse: crossterm::event::MouseEvent,
    painted_hits: &crate::ui::render::QueueHitMap,
    intent: &BoardIntent,
) -> BoardIntentTarget {
    let pos = Position::new(mouse.column, mouse.row);
    let responsive = model.responsive_geometry(area);
    let right_surface = model.project_right_seat_focused()
        && responsive.task.contains(pos)
        && !matches!(
            intent,
            BoardIntent::StageLeft | BoardIntent::StageRight | BoardIntent::SelectProjectRow(_)
        );
    let right_footer = model.project_right_seat_focused()
        && painted_hits
            .footer
            .is_some_and(|footer| footer.contains(pos))
        && !matches!(intent, BoardIntent::ListScrollTo(_));
    if right_surface || right_footer {
        BoardIntentTarget::Focused
    } else {
        BoardIntentTarget::Outer
    }
}

fn mouse_intent_target_mut<'a>(
    model: &'a mut BoardModel,
    area: Rect,
    mouse: crossterm::event::MouseEvent,
    painted_hits: &crate::ui::render::QueueHitMap,
    intent: &BoardIntent,
) -> &'a mut BoardModel {
    let target = mouse_intent_target(model, area, mouse, painted_hits, intent);
    board_intent_target_mut(model, target)
}

fn board_mouse_intent(
    area: Rect,
    model: &mut BoardModel,
    mouse: crossterm::event::MouseEvent,
) -> Option<BoardIntent> {
    // crossterm's `EnableMouseCapture` turns on all-motion tracking, so
    // a bare pointer move over the pane arrives as an `Event::Mouse` too -- at a rate that
    // can saturate the event loop. `map_board_mouse` only ever acts on
    // `Down(Left)`/`ScrollUp`/`ScrollDown` (every other kind falls through its own leading
    // match to `None`), so gate on the event kind *before* paying for a full board paint
    // (`board_hit_map` renders into a scratch `TestBackend`) and before
    // `resolve_board_surface`'s side effects run for a kind that was never going to
    // dispatch anything. This also stops `resolve_board_surface` firing on motion, which
    // was itself a second reason to gate first.
    use crossterm::event::{MouseButton, MouseEventKind};
    if !matches!(
        mouse.kind,
        MouseEventKind::Down(MouseButton::Left)
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
    ) {
        return None;
    }
    resolve_board_surface(area, model);
    // Minor 3: `board_hit_map` renders a full scratch paint
    // (`TestBackend`) to recover the hit-map, but `map_board_mouse` only ever reads it for
    // `Down(Left)` -- its `ScrollUp`/`ScrollDown` arm resolves through `wheel_board_intent`
    // before the hit-map parameter is touched at all. Continuous scrolling arrives in
    // bursts, and the scratch paint cost (1.15ms at 50 tasks, 5.95ms at 200) was being paid
    // on every one of them for a value the wheel path never reads. Build it only for the
    // one event kind that actually consults it.
    let hits = if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        crate::ui::board::board_hit_map(area, model)
    } else {
        crate::ui::render::QueueHitMap::default()
    };
    let intent = map_responsive_board_mouse(model, &hits, area, mouse);
    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && !matches!(intent, Some(BoardIntent::SelectProjectRow(_)))
    {
        // Any left press that is not an index-row click disarms a pending index-row
        // double-click: inert cells and other controls (tabs, chips, search) alike.
        // The header double-click shares the same disarm.
        model.cancel_project_header_double_click();
    }
    let intent = intent?;
    let intent = board_intent_for_presentation(area, model, intent)?;
    resolve_board_command(
        mouse_intent_target_mut(model, area, mouse, &hits, &intent),
        intent,
    )
}

/// Apply one board intent and present any refusal instead of discarding it.
///
/// A refused intent changes nothing: an open edit keeps its mode, its draft, and its cursor,
/// because the domain rejected before [`crate::ui::board::apply_intent`] could clear them. All
/// this boundary adds is the reason, on the board's message line, so a field that will not
/// close says why. It owns no rule of its own: the domain decides what is
/// refused, and this only presents what came back.
fn apply_board_intent_presenting_rejection(
    domain: &mut DomainState,
    model: &mut BoardModel,
    recovery: &mut SaveRecovery<DomainState>,
    context: BoardSaveContext<'_>,
    persist: impl FnMut(&mut DomainState) -> Result<(), String>,
) -> IntentOutcome {
    match apply_board_intent_with_save_recovery(domain, model, recovery, context, persist) {
        Ok(outcome) => outcome,
        Err(error) => {
            // Safe to write here: every other producer on this line (park, dispatch, save
            // recovery) reports through `Ok`, so nothing that reaches this arm is competing
            // with a message about a different action. The line describes the intent the
            // user just issued, which is this one.
            model.set_message(board_rejection_message(&error));
            IntentOutcome::None
        }
    }
}

/// The line the board shows for a refused intent.
///
/// An empty title borrows Capture's phrasing so both surfaces say the same thing about the
/// same refusal. The two refusals that name a task by uuid get a short human phrase
/// instead: their `Display` spends more than a narrow board's whole message row on an id
/// the user cannot act on. Every other reason falls back to its own `Display`, so a refusal
/// this boundary has never seen is still explained rather than silently swallowed.
fn board_rejection_message(error: &DomainError) -> String {
    match error {
        DomainError::EmptyTitle => TITLE_REQUIRED_MESSAGE.to_string(),
        DomainError::UnknownId(_) => "that task is no longer here".to_string(),
        DomainError::SoftDeleted(_) => "that task is deleted".to_string(),
        other => other.to_string(),
    }
}

/// The intents whose **decision** depends on state another actor may have changed, and which
/// therefore must not be decided against a possibly stale in-memory snapshot.
///
/// Undo compares against the latest durable revision before mutating.
///
/// This is deliberately **narrower than "may persist"**: every intent in
/// [`board_intent_may_persist`] loads the durable baseline, because a save needs it, but only
/// these three are *decided* against it. Merging is not free and it re-derives the visible list,
/// so an intent that merely writes does not pay for one.
///
/// **`ConfirmEdit` is deliberately NOT here**, though resolved decision 8 requires it to judge
/// the bound task's availability against the durable record. Merging would satisfy the letter of
/// that and break something else: `merge_tasks_from_disk` replaces the local task wholesale, so
/// the subsequent `edit` records a merge base taken from the *disk* revision, `merge_for_save`
/// then sees a base that matches, and a concurrent same-task edit by another actor is silently
/// overwritten instead of raising a save conflict. That was measured, not assumed — see
/// [`confirm_edit_consults_the_record_without_merging_it`] and the conflict-detection test in
/// `tests/edit_target_binding.rs`. The availability judgment is made instead by
/// [`confirm_edit_refusal_against_the_record`], which *reads* the baseline and never merges it.
///
/// [`confirm_edit_consults_the_record_without_merging_it`]: self::tests
pub fn board_intent_needs_fresh_state(intent: &BoardIntent) -> bool {
    matches!(intent, BoardIntent::Undo)
}

/// resolved decision 8: a soft-deleted task is not editable, and a stale in-memory snapshot is
/// not an excuse for editing one.
///
/// Between an edit's open and its confirm, another actor can soft-delete the bound task and
/// persist it. The user can confirm first, and nothing else on this path would catch it in
/// time. So the confirm consults the freshly loaded durable record for this one judgment.
///
/// It **reads** the record and returns a verdict; it does not merge it. That distinction is the
/// whole design: merging would hand `merge_for_save` a merge base this board never earned and
/// silently defeat same-task conflict detection (see [`board_intent_needs_fresh_state`]).
/// Consulting leaves every local revision exactly where it was, so a concurrent edit still
/// collides at save time the way it always did.
///
/// Absence from the record is deliberately *not* refused here: nothing in the product hard-deletes
/// a task, so an id missing from disk means a store this board has state for and disk does not,
/// which the reducer's own unknown-id path already answers. Only the soft-deleted verdict,
/// the one a second writer can actually produce, is read from the record.
pub fn confirm_edit_refusal_against_the_record(
    record: &DomainState,
    model: &BoardModel,
) -> Option<DomainError> {
    let bound = model.edit_target()?;
    record
        .get(bound)
        .filter(|task| task.soft_deleted)
        .map(|_| DomainError::SoftDeleted(bound))
}

/// Merge the freshly loaded durable baseline into local state before a mutating intent is
/// decided, for the intents [`board_intent_needs_fresh_state`] names. Returns whether it merged.
///
/// Order is load-bearing and matches the board loop's: merge, re-derive presentation, *then* run
/// the reducer. The merge never touches an open edit session — `sync_from_domain` re-derives the
/// task list and the selection and leaves the input mode, the draft, the cursor, and the binding
/// alone — so a refusal that follows still finds the draft byte-identical, which is what makes
///'s "refuse visibly and keep the draft" survivable across a merge.
///
/// A caller holding an unresolved save must not call this: that state allows only navigation plus
/// Retry/Cancel, and re-merging underneath it would move the ground the retry stands on.
pub fn refresh_before_mutation(
    intent: &BoardIntent,
    baseline: &DomainState,
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> bool {
    if !board_intent_needs_fresh_state(intent) {
        return false;
    }
    domain.merge_tasks_from_disk(baseline);
    model.sync_from_domain(domain);
    true
}

/// Copy a visible task identifier without changing selection, page state, or persistence.
fn copy_task_number(domain: &DomainState, model: &mut BoardModel, id: uuid::Uuid) {
    copy_task_number_with(domain, model, id, copy_to_clipboard);
}

fn copy_task_number_with(
    domain: &DomainState,
    model: &mut BoardModel,
    id: uuid::Uuid,
    copy: impl FnOnce(&str) -> bool,
) {
    let Some(identifier) = domain.get(id).and_then(|task| task.board_identifier()) else {
        return;
    };
    let message = if copy(&identifier) {
        format!("copy sent: {identifier}")
    } else {
        "copy failed".to_string()
    };
    model.set_ephemeral_message(message, Duration::from_secs(2));
}

/// Refresh the outer model after a right-seat action. The nested board owns a cloned task
/// snapshot so its reducer can remain the ordinary board reducer; this keeps the projects index
/// counts and the right seat on the same durable state before the next paint. Save recovery is
/// excluded because its caller intentionally leaves the working snapshot outside `domain`.
fn sync_focused_project_preview(
    model: &mut BoardModel,
    domain: &DomainState,
    recovery: &SaveRecovery<DomainState>,
) {
    if model.project_right_seat_focused() && !recovery.is_pending() {
        model.sync_from_domain(domain);
    }
}

/// Dispatch one mapped event-loop intent, then run the shared preview opening and synchronization
/// handoff before the next paint. Keeping this boundary in one function makes post-action sync
/// part of the dispatch path rather than a test-only follow-up.
fn dispatch_board_intent(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    route: BoardDispatchRoute,
    intent: BoardIntent,
    save_recovery: &mut SaveRecovery<DomainState>,
    quick_capture: bool,
) -> io::Result<bool> {
    let intent_for_preview = intent.clone();
    // Quit belongs to the whole application, even when a focused preview supplied it.
    // Its guard must see both the outer parked form and the nested preview's draft.
    let target = if intent == BoardIntent::Quit {
        BoardIntentTarget::Outer
    } else {
        route.target
    };
    let quit = handle_board_intent(
        store,
        domain,
        board_intent_target_mut(model, target),
        intent,
        save_recovery,
        quick_capture,
    )?;
    auto_open_projects_preview(route.area, model, &intent_for_preview);
    sync_focused_project_preview(model, domain, save_recovery);
    Ok(quit)
}

/// Apply a board intent. Returns `true` when the board loop should quit.
///
/// In the quick-capture popup (`quick_capture`), the loop also quits once the capture
/// session has ended: the single choke point every key, mouse, and paste dispatch
/// passes through, so a persisted save or a discard closes the popup no matter which
/// route ended the draft.
fn handle_board_intent(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    intent: BoardIntent,
    save_recovery: &mut SaveRecovery<DomainState>,
    quick_capture: bool,
) -> io::Result<bool> {
    if let BoardIntent::CopyTaskNumber(id) = intent {
        copy_task_number(domain, model, id);
        return Ok(false);
    }

    let quit_requested = intent == BoardIntent::Quit
        || (intent == BoardIntent::CloseLayer && model.root_escape_requests_quit());
    // Quick capture retains its existing Ctrl+C exit. Pending recovery owns refusals and
    // its failure banner, so do not replace it with an ordinary dirty-edit message.
    if !quick_capture
        && !save_recovery.is_pending()
        && quit_requested
        && model.refuse_quit_with_unsaved_work()
    {
        return Ok(false);
    }

    // Quick capture: Esc on the expanded draft is the top-level cancel. The board's
    // collapse-to-line fallback would strand the popup on a retained one-line draft, so
    // the whole draft is discarded and the popup closes. Nested Escapes keep their own
    // semantics: an open scope dropdown maps to CancelFormScopeDropdown, the inline step
    // editor owns its CancelEdit, and an unresolved save routes Esc to CancelSave before
    // this dispatch.
    if quick_capture
        && intent == BoardIntent::CancelEdit
        && !save_recovery.is_pending()
        && model.expanded_capture_open()
        && model.input_mode() != BoardInputMode::EditStep
    {
        let _ = apply_intent(domain, model, BoardIntent::CancelQuickAdd, None);
        return Ok(true);
    }

    let baseline = if save_recovery.is_pending() || !board_intent_may_persist(&intent) {
        DomainState::new()
    } else {
        store
            .load()
            .map_err(|error| io::Error::other(error.to_string()))?
    };

    // Do not reload while a failed save is unresolved, because only navigation plus Retry/Cancel
    // are allowed there; otherwise, bring the durable record in before the intent is decided.
    if !save_recovery.is_pending() {
        refresh_before_mutation(&intent, &baseline, domain, model);
    }

    // OpenCapture needs the invocation snapshot `load_board` seeded the board with
    // scope and provenance: the reducer stores it on `model.capture_snapshot` at
    // open and reads it back at ConfirmEdit, so a `None` here is what silently turned board
    // `a` into a no-op save that still reported success.
    let loaded_snapshot;
    let snapshot_for_intent = if !save_recovery.is_pending() && intent == BoardIntent::OpenCapture {
        loaded_snapshot = load_snapshot();
        Some(&loaded_snapshot)
    } else {
        None
    };

    let outcome = apply_board_intent_presenting_rejection(
        domain,
        model,
        save_recovery,
        BoardSaveContext {
            baseline,
            intent,
            snapshot: snapshot_for_intent,
        },
        |state| {
            store
                .reload_merge_save(state)
                .map_err(|error| error.to_string())
        },
    );
    if outcome == IntentOutcome::Persisted {
        record_notice_dismissals_without_blocking_persist(store, domain);
    }
    match outcome {
        IntentOutcome::Quit => Ok(true),
        IntentOutcome::Persist | IntentOutcome::Persisted | IntentOutcome::None => {
            Ok(quick_capture && quick_capture_finished(model))
        }
    }
}

#[cfg(test)]
mod save_recovery_tests {
    use super::board_background_work_allowed;
    use crate::domain::DomainState;
    use crate::save_recovery::SaveRecovery;

    #[test]
    fn board_background_work_defers_during_save_recovery() {
        let mut recovery = SaveRecovery::new();
        assert!(board_background_work_allowed(&recovery));
        recovery.fail(DomainState::new(), DomainState::new(), "save failed");
        assert!(!board_background_work_allowed(&recovery));
        let _ = recovery.cancel();
        assert!(board_background_work_allowed(&recovery));
    }
}

/// the idle frame tick's store-only revalidation.
#[cfg(test)]
mod idle_store_revalidation_tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{revalidate_board_from_store, StoreWatch};
    use crate::domain::{DomainState, ProvenanceOrigin, TaskScope};
    use crate::save_recovery::SaveRecovery;
    use crate::store::TaskStore;
    use crate::ui::board::{apply_intent, BoardInputMode, BoardModel};
    use crate::ui::input::BoardIntent;

    const THIS_REPO: &str = "/repos/app";

    fn temp_store_dir(label: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsk-idle-revalidate-{label}-{nanos}-{seq}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn project_scope() -> TaskScope {
        TaskScope::Project {
            path: THIS_REPO.to_string(),
        }
    }

    /// an open, idle board picks up a task a *separate* store writer (the standalone
    /// quick-capture popup, per the manifest) saved to the same `tsk.json`, with no
    /// persisting intent run on this board at all.
    #[test]
    fn idle_tick_revalidates_task_written_by_a_separate_store_writer() {
        let dir = temp_store_dir("quick-capture");
        let store = TaskStore::new(&dir);
        store.save(&DomainState::new()).unwrap();

        let mut domain = store.load().unwrap();
        let mut model = BoardModel::from_domain(&domain, Some(std::path::PathBuf::from(THIS_REPO)));
        let mut watch = StoreWatch::seeded(&store);
        let save_recovery = SaveRecovery::<DomainState>::new();

        // A separate writer (its own TaskStore handle, standing in for the quick-capture
        // popup process) saves a new task to the same on-disk document.
        let writer_store = TaskStore::new(&dir);
        let mut writer_domain = writer_store.load().unwrap();
        let captured_id = writer_domain
            .create(
                "Quick capture from another pane",
                None,
                project_scope(),
                ProvenanceOrigin::Capture,
                None,
            )
            .unwrap();
        writer_store.save(&writer_domain).unwrap();

        assert!(
            !model.visible_ids().contains(&captured_id),
            "not visible before the idle tick revalidates"
        );

        let merged = revalidate_board_from_store(
            &store,
            &mut domain,
            &mut model,
            &mut watch,
            &save_recovery,
        );

        assert!(merged, "idle tick must detect the changed store signature");
        assert!(
            domain.get(captured_id).is_some(),
            "domain must merge the separate writer's task"
        );
        assert!(
            model.visible_ids().contains(&captured_id),
            "model must sync so the quick-capture task renders without a persisting intent"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// A replace whose document has the same byte length, with the mtime forced back to the
    /// previous save's tick, must still be detected: the signature carries the inode.
    #[cfg(unix)]
    #[test]
    fn idle_tick_sees_a_replace_that_keeps_mtime_and_length() {
        let dir = temp_store_dir("same-mtime");
        let store = TaskStore::new(&dir);
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "seed",
                None,
                project_scope(),
                ProvenanceOrigin::Manual,
                None,
            )
            .unwrap();
        store.save(&domain).unwrap();

        let mut model = BoardModel::from_domain(&domain, Some(std::path::PathBuf::from(THIS_REPO)));
        let mut watch = StoreWatch::seeded(&store);
        let save_recovery = SaveRecovery::<DomainState>::new();
        let first = store.state_signature().expect("signature after seed save");

        // A separate writer saves a byte-identical document (a rename replace with no
        // content change) and forces the replaced file's mtime back to the seed tick.
        let writer_store = TaskStore::new(&dir);
        let writer_domain = writer_store.load().unwrap();
        writer_store.save(&writer_domain).unwrap();
        let file = std::fs::File::options()
            .write(true)
            .open(store.state_file())
            .unwrap();
        file.set_modified(first.modified).unwrap();
        drop(file);
        let second = store.state_signature().expect("signature after rewrite");
        assert_eq!(first.len, second.len, "same-length documents");
        assert_eq!(first.modified, second.modified, "mtime forced equal");

        let merged = revalidate_board_from_store(
            &store,
            &mut domain,
            &mut model,
            &mut watch,
            &save_recovery,
        );
        assert!(
            merged,
            "an equal-mtime equal-length replace must still read as changed"
        );
        assert!(
            domain.get(id).is_some(),
            "the merged domain still holds the task"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The cheap path stays cheap: a second idle tick over an unchanged store must not merge
    /// or sync again.
    #[test]
    fn idle_tick_skips_revalidation_when_store_is_unchanged() {
        let dir = temp_store_dir("unchanged");
        let store = TaskStore::new(&dir);
        store.save(&DomainState::new()).unwrap();

        let mut domain = store.load().unwrap();
        let mut model = BoardModel::from_domain(&domain, None);
        let mut watch = StoreWatch::seeded(&store);
        let save_recovery = SaveRecovery::<DomainState>::new();

        let first = revalidate_board_from_store(
            &store,
            &mut domain,
            &mut model,
            &mut watch,
            &save_recovery,
        );
        assert!(
            !first,
            "a watch seeded from the just-loaded snapshot must see no change"
        );

        let second = revalidate_board_from_store(
            &store,
            &mut domain,
            &mut model,
            &mut watch,
            &save_recovery,
        );
        assert!(
            !second,
            "repeated idle ticks over unchanged disk stay no-ops"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// / save recovery: a failed save owns the displayed working state, so the idle tick
    /// must not reload disk out from under it even though the store changed.
    #[test]
    fn idle_tick_skips_revalidation_while_save_recovery_is_pending() {
        let dir = temp_store_dir("save-recovery");
        let store = TaskStore::new(&dir);
        store.save(&DomainState::new()).unwrap();

        let mut domain = store.load().unwrap();
        let mut model = BoardModel::from_domain(&domain, None);
        let mut watch = StoreWatch::seeded(&store);

        let writer_store = TaskStore::new(&dir);
        let mut writer_domain = writer_store.load().unwrap();
        let captured_id = writer_domain
            .create(
                "Quick capture during save recovery",
                None,
                project_scope(),
                ProvenanceOrigin::Capture,
                None,
            )
            .unwrap();
        writer_store.save(&writer_domain).unwrap();

        let mut recovery = SaveRecovery::<DomainState>::new();
        recovery.fail(DomainState::new(), DomainState::new(), "save failed");

        let merged =
            revalidate_board_from_store(&store, &mut domain, &mut model, &mut watch, &recovery);

        assert!(
            !merged,
            "must not reload disk while a failed save owns the working state"
        );
        assert!(domain.get(captured_id).is_none());
        assert!(!model.visible_ids().contains(&captured_id));

        let _ = fs::remove_dir_all(&dir);
    }

    /// a failed `store.load()` must not burn the changed signature it never got to use --
    /// otherwise a single transient read failure leaves the board stale until some *later*
    /// write happens to produce yet another distinct signature, which is the exact failure
    /// this watch exists to survive. Corrupts `tsk.json` in place (a stand-in for any
    /// transient read failure) so `store.load()` errors while the on-disk signature has
    /// already changed, then proves the watch still reports that same signature as changed on
    /// the next poll (it was never recorded), and that a later, valid write is picked up.
    #[test]
    fn idle_tick_retries_after_a_failed_load_instead_of_recording_the_burned_signature() {
        let dir = temp_store_dir("failed-load");
        let store = TaskStore::new(&dir);
        store.save(&DomainState::new()).unwrap();

        let mut domain = store.load().unwrap();
        let mut model = BoardModel::from_domain(&domain, Some(std::path::PathBuf::from(THIS_REPO)));
        let mut watch = StoreWatch::seeded(&store);
        let save_recovery = SaveRecovery::<DomainState>::new();

        // Corrupt the document in place: the signature changes (different length), but
        // `store.load()` fails to parse it -- standing in for any transient read failure on
        // an already-changed document.
        let state_file = store.state_file();
        fs::write(&state_file, b"not valid json").unwrap();

        let merged = revalidate_board_from_store(
            &store,
            &mut domain,
            &mut model,
            &mut watch,
            &save_recovery,
        );
        assert!(
            !merged,
            "a load that fails to parse must not report a merge"
        );

        // The failed load must not have recorded the corrupt signature: the watch still
        // reports the document as changed relative to what it saw at `seeded()`, so the very
        // next tick keeps trying instead of treating the failed read as caught up.
        assert!(
            watch.poll(&store).is_some(),
            "a failed load must not burn the signature it never merged"
        );

        // Repair the document with a valid write; the watch (never having recorded the
        // corrupt signature) must still pick it up.
        let mut repaired = DomainState::new();
        let captured_id = repaired
            .create(
                "Recovered after a transient load failure",
                None,
                project_scope(),
                ProvenanceOrigin::Capture,
                None,
            )
            .unwrap();
        store.save(&repaired).unwrap();

        let merged = revalidate_board_from_store(
            &store,
            &mut domain,
            &mut model,
            &mut watch,
            &save_recovery,
        );
        assert!(merged, "the repaired document must merge on the next tick");
        assert!(domain.get(captured_id).is_some());
        assert!(model.visible_ids().contains(&captured_id));

        let _ = fs::remove_dir_all(&dir);
    }

    /// M-3(i) /: an open **title** edit must not be redirected by the idle merge this
    /// call site drives -- the doc above claims it (`sync_from_domain` reanchors selection by
    /// id and never touches the edit binding), and `tests/edit_target_binding.rs` already
    /// pins the identical merge+reanchor pair through attention polling (removed), but nothing drove
    /// it through `revalidate_board_from_store` itself. Pin it here so this call site's own
    /// risk is bound to a test, not only to the property it borrows.
    #[test]
    fn idle_tick_does_not_redirect_an_open_title_edit_and_still_merges_a_separate_writer() {
        let dir = temp_store_dir("open-title-edit");
        let store = TaskStore::new(&dir);

        let mut domain = DomainState::new();
        let alpha = domain
            .create(
                "Alpha",
                None,
                project_scope(),
                ProvenanceOrigin::Manual,
                None,
            )
            .unwrap();
        store.save(&domain).unwrap();

        let mut model = BoardModel::from_domain(&domain, Some(std::path::PathBuf::from(THIS_REPO)));
        let mut watch = StoreWatch::seeded(&store);
        let save_recovery = SaveRecovery::<DomainState>::new();

        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("begin title edit");
        assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
        assert_eq!(model.edit_target(), Some(alpha));

        // A separate writer saves a new task while the edit sits open, the same shape as the
        // idle loop's real poll.
        let writer_store = TaskStore::new(&dir);
        let mut writer_domain = writer_store.load().unwrap();
        let captured_id = writer_domain
            .create(
                "Quick capture while a title edit is open",
                None,
                project_scope(),
                ProvenanceOrigin::Capture,
                None,
            )
            .unwrap();
        writer_store.save(&writer_domain).unwrap();

        let merged = revalidate_board_from_store(
            &store,
            &mut domain,
            &mut model,
            &mut watch,
            &save_recovery,
        );
        assert!(
            merged,
            "the idle tick must still merge the separate writer's task"
        );

        assert_eq!(
            model.input_mode(),
            BoardInputMode::EditTitle,
            "AC-24: an open edit session must not be redirected by an idle merge"
        );
        assert_eq!(
            model.edit_target(),
            Some(alpha),
            "the edit must stay bound to the same task the merge ran under"
        );
        assert_eq!(
            model.edit_buffer(),
            "Alpha",
            "the open draft must survive the merge untouched"
        );
        assert!(domain.get(captured_id).is_some());
        assert!(
            model.visible_ids().contains(&captured_id),
            "the merged task still becomes visible around the open edit"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// M-3(i) /: an open **capture draft** must not be redirected by the idle merge
    /// either -- capture is not a task edit at all (there is no `edit_target` to reanchor),
    /// so the only risk is `sync_from_domain` clobbering the draft's own fields or knocking the
    /// board out of `Capture` mode; this pins that it does neither.
    #[test]
    fn idle_tick_does_not_redirect_an_open_capture_draft_and_still_merges_a_separate_writer() {
        let dir = temp_store_dir("open-capture-draft");
        let store = TaskStore::new(&dir);
        store.save(&DomainState::new()).unwrap();

        let mut domain = store.load().unwrap();
        let mut model = BoardModel::from_domain(&domain, Some(std::path::PathBuf::from(THIS_REPO)));
        let mut watch = StoreWatch::seeded(&store);
        let save_recovery = SaveRecovery::<DomainState>::new();

        apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None)
            .expect("open quick add");
        assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
        for character in "Draft in progress".chars() {
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::QuickAddInsert(character),
                None,
            )
            .expect("type into the capture draft");
        }
        assert_eq!(model.quick_add_title_value(), "Draft in progress");

        let writer_store = TaskStore::new(&dir);
        let mut writer_domain = writer_store.load().unwrap();
        let captured_id = writer_domain
            .create(
                "Quick capture while a draft is open",
                None,
                project_scope(),
                ProvenanceOrigin::Capture,
                None,
            )
            .unwrap();
        writer_store.save(&writer_domain).unwrap();

        let merged = revalidate_board_from_store(
            &store,
            &mut domain,
            &mut model,
            &mut watch,
            &save_recovery,
        );
        assert!(
            merged,
            "the idle tick must still merge the separate writer's task"
        );

        assert_eq!(
            model.input_mode(),
            BoardInputMode::QuickAdd,
            "AC-24: an open quick-add draft must not be redirected by an idle merge"
        );
        assert_eq!(
            model.quick_add_title_value(),
            "Draft in progress",
            "the open draft must survive the merge untouched"
        );
        assert!(domain.get(captured_id).is_some());
        assert!(
            model.visible_ids().contains(&captured_id),
            "the merged task still becomes visible around the open draft"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// M-3(ii): drives [`super::board_idle_tick`] itself -- the one function `run_board`'s
    /// loop calls to decide whether a tick was idle, with no separate call in the loop for a
    /// later edit to drop without a test noticing. The prior version of this test called
    /// `board_frame` and `revalidate_board_from_store` as two separate steps in the test body,
    /// which could not tell a `run_board` that had stopped wiring them together apart from one
    /// that still did (deleting the merge call from the loop kept it green). Driving
    /// `board_idle_tick` instead pins the actual call `run_board` makes: the merge below is
    /// not something this test additionally triggers, it is a postcondition of the single call
    /// under test reporting `FramePoll::Idle` at all.
    #[test]
    fn board_idle_tick_reports_idle_and_merges_a_separate_writers_task_in_one_call() {
        use super::{board_idle_tick, FramePoll};

        let dir = temp_store_dir("real-idle-path");
        let store = TaskStore::new(&dir);
        store.save(&DomainState::new()).unwrap();

        let mut domain = store.load().unwrap();
        let mut model = BoardModel::from_domain(&domain, Some(std::path::PathBuf::from(THIS_REPO)));
        let mut watch = StoreWatch::seeded(&store);
        let save_recovery = SaveRecovery::<DomainState>::new();

        let writer_store = TaskStore::new(&dir);
        let mut writer_domain = writer_store.load().unwrap();
        let captured_id = writer_domain
            .create(
                "Quick capture, real idle path",
                None,
                project_scope(),
                ProvenanceOrigin::Capture,
                None,
            )
            .unwrap();
        writer_store.save(&writer_domain).unwrap();

        let paint = |_model: &BoardModel| -> std::io::Result<()> { Ok(()) };
        // Never sees an event, matching the real Idle branch.
        let wait = |_duration: std::time::Duration| -> std::io::Result<bool> { Ok(false) };
        let poll = board_idle_tick(
            &mut model,
            paint,
            wait,
            &store,
            &mut domain,
            &mut watch,
            &save_recovery,
            false,
        )
        .unwrap();
        assert_eq!(poll, FramePoll::Idle);

        assert!(
            domain.get(captured_id).is_some(),
            "reporting Idle must already have merged the separate writer's task, with no \
             further call needed"
        );
        assert!(model.visible_ids().contains(&captured_id));

        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::Terminal;

    use crate::context::InvocationSnapshot;
    use crate::domain::{HumanStatus, ProvenanceOrigin, TaskScope};
    use crate::ui::board::{board_hit_map, CommandSurface};
    use crate::ui::capture::{apply_capture_intent, CaptureField, CaptureModel};
    use crate::ui::input::{map_capture_paste_state, map_key, CaptureIntent};
    use crate::ui::mouse::{
        focused_mouse_area, left_click, map_board_mouse, map_responsive_board_mouse,
        press_on_focused_surface,
    };
    use crate::ui::queue::NavTab;

    static TEMP_DIR_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn board_fixture(title: &str, notes: Option<String>) -> (DomainState, BoardModel) {
        let mut domain = DomainState::new();
        domain
            .create(
                title,
                notes,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create board fixture");
        let model = BoardModel::from_domain(&domain, None);
        (domain, model)
    }

    fn click_at(area: Rect) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn stage_right(domain: &mut DomainState, model: &mut BoardModel, times: usize) {
        for _ in 0..times {
            apply_intent(domain, model, BoardIntent::StageRight, None).expect("stage right");
        }
    }

    fn projects_preview_fixture() -> (DomainState, BoardModel, uuid::Uuid) {
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "preview task",
                Some("preview notes".into()),
                TaskScope::Project {
                    path: "/repos/preview".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create preview task");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("open projects overview");
        stage_right(&mut domain, &mut model, 2);
        (domain, model, id)
    }

    fn projects_preview_open_tasks_fixture() -> (DomainState, BoardModel) {
        let mut domain = DomainState::new();
        for title in ["first open task", "second open task"] {
            domain
                .create(
                    title,
                    None,
                    TaskScope::Project {
                        path: "/repos/preview".into(),
                    },
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("create open preview task");
        }
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("open projects overview");
        stage_right(&mut domain, &mut model, 2);
        (domain, model)
    }

    fn selected_row(model: &BoardModel) -> Option<uuid::Uuid> {
        model
            .selected_index()
            .and_then(|index| model.visible_ids().get(index).copied())
    }

    fn preview_key_intent(model: &mut BoardModel, area: Rect, key: KeyEvent) -> BoardIntent {
        let mode = resolve_board_surface(area, model);
        let right = model.right_seat().expect("projects rail has a right seat");
        board_keyboard_intent_for_area(right, area, mode, key).expect("preview key intent")
    }

    fn projects_overview_fixture() -> (DomainState, BoardModel) {
        let mut domain = DomainState::new();
        domain
            .create(
                "alpha task",
                None,
                TaskScope::Project {
                    path: "/repos/alpha".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create alpha task");
        domain
            .create(
                "beta task",
                None,
                TaskScope::Project {
                    path: "/repos/beta".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create beta task");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("open projects overview");
        (domain, model)
    }

    #[test]
    fn projects_preview_opening_is_gated_at_the_app_boundary() {
        let (mut domain, mut model) = projects_overview_fixture();
        let narrow = Rect::new(0, 0, 109, 24);
        let wide = Rect::new(0, 0, 110, 24);

        assert_eq!(
            board_intent_for_presentation(narrow, &model, BoardIntent::StageRight),
            None,
            "a narrow projects index has no preview stage"
        );
        apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None)
            .expect("move the narrow index cursor");
        auto_open_projects_preview(narrow, &mut model, &BoardIntent::SelectNext);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::FullBoard);
        assert!(model.right_seat().is_none());

        auto_open_projects_preview(wide, &mut model, &BoardIntent::SelectNext);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
        assert!(model.right_seat().is_some());
    }

    #[test]
    fn projects_preview_clicking_a_project_row_at_wide_width_opens_the_preview() {
        let (mut domain, mut model) = projects_overview_fixture();
        let area = Rect::new(0, 0, 110, 30);
        sync_frame_presentation(area, &model);
        let hits = board_hit_map(area, &model);
        let row = hits
            .regions
            .iter()
            .find(|hit| matches!(hit.target, crate::ui::render::QueueHitTarget::ProjectRow(1)))
            .expect("second project row hit")
            .area;
        let intent = map_responsive_board_mouse(&model, &hits, area, left_click(row.x, row.y))
            .expect("project row click");
        assert_eq!(intent, BoardIntent::SelectProjectRow(1));

        apply_intent(&mut domain, &mut model, intent, None).expect("select project row");
        auto_open_projects_preview(area, &mut model, &BoardIntent::SelectProjectRow(1));

        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
        assert_eq!(
            model
                .right_seat()
                .and_then(BoardModel::active_project)
                .map(Path::to_path_buf),
            Some(PathBuf::from("/repos/beta"))
        );
    }

    #[test]
    fn projects_preview_rail_parks_on_narrow_resize_and_restores_the_seat() {
        let (mut domain, mut model) = projects_overview_fixture();
        stage_right(&mut domain, &mut model, 2);
        let narrow = Rect::new(0, 0, 109, 30);
        let wide = Rect::new(0, 0, 110, 30);
        let project = model
            .right_seat()
            .and_then(BoardModel::active_project)
            .map(Path::to_path_buf)
            .expect("rail project");
        let selected = model
            .right_seat()
            .and_then(BoardModel::selected_id)
            .expect("rail task selection");
        assert!(model.project_right_seat_focused());

        sync_frame_presentation(narrow, &model);
        assert!(!model.project_right_seat_focused());
        let mode = resolve_board_surface(narrow, &mut model);
        let next = board_keyboard_intent_for_area(
            &model,
            narrow,
            mode,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        )
        .expect("narrow index key");
        assert_eq!(next, BoardIntent::SelectNext);
        apply_intent(&mut domain, &mut model, next, None).expect("move the parked index");
        assert_eq!(
            model.projects_cursor(),
            1,
            "narrow keys move the painted index"
        );
        assert_eq!(
            model
                .right_seat()
                .and_then(BoardModel::active_project)
                .map(Path::to_path_buf),
            Some(project.clone()),
            "narrow index movement keeps the parked seat session"
        );
        assert_eq!(
            model.right_seat().and_then(BoardModel::selected_id),
            Some(selected)
        );

        let before = domain.tasks().to_vec();
        let mode = resolve_board_surface(narrow, &mut model);
        let complete = board_keyboard_intent_for_area(
            &model,
            narrow,
            mode,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        )
        .expect("narrow verb key");
        apply_intent(&mut domain, &mut model, complete, None).expect("narrow verb");
        assert_eq!(
            domain.tasks(),
            before,
            "hidden preview receives no mutation"
        );

        sync_frame_presentation(wide, &model);
        assert!(model.project_right_seat_focused());
        assert_eq!(
            model.right_seat().and_then(BoardModel::selected_id),
            Some(selected)
        );
        assert_eq!(
            model
                .right_seat()
                .and_then(BoardModel::active_project)
                .map(Path::to_path_buf),
            Some(project),
            "widening restores the same rail session"
        );
    }

    #[test]
    fn switching_to_a_project_keeps_the_existing_task_slider_stage() {
        let mut domain = DomainState::new();
        domain
            .create(
                "project task",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("project task");
        let mut model = BoardModel::from_tasks(
            domain.tasks().to_vec(),
            Some(PathBuf::from("/repos/project")),
        );
        apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
            .expect("open task pane");
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
        assert_eq!(model.nav_tab(), NavTab::Desk);
        assert!(model.right_seat().is_none());

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::ProjectBoard),
            None,
        )
        .expect("switch to project tab");
        assert_eq!(
            model.wide_stage(),
            crate::ui::tier::WideStage::Split,
            "switching tabs must not collapse the task slider"
        );
    }

    #[test]
    fn leaving_projects_clears_its_preview_stage_and_seat() {
        let (mut domain, mut model) = projects_overview_fixture();
        stage_right(&mut domain, &mut model, 2);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Rail);
        assert!(model.right_seat().is_some());

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Desk),
            None,
        )
        .expect("leave projects");
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::FullBoard);
        assert!(model.right_seat().is_none());
    }

    #[test]
    fn nested_project_routes_keep_escape_and_global_navigation_on_the_outer_board() {
        let (_domain, model, _) = projects_preview_fixture();
        assert!(model.project_right_seat_focused());

        let collapse = route_board_intent(&model, BoardIntent::CollapseDetail);
        assert_eq!(collapse.intent, BoardIntent::StageLeft);
        assert_eq!(collapse.target, BoardIntentTarget::Outer);
        assert!(!collapse.return_to_index);

        let close = route_board_intent(&model, BoardIntent::CloseLayer);
        assert_eq!(close.intent, BoardIntent::CloseLayer);
        assert_eq!(close.target, BoardIntentTarget::Focused);
        assert!(close.return_to_index);

        let navigation = route_board_intent(&model, BoardIntent::SelectNavTab(NavTab::Desk));
        assert_eq!(navigation.target, BoardIntentTarget::Outer);
    }

    #[test]
    fn projects_preview_escape_clears_right_marks_before_returning_to_the_index() {
        let (mut domain, mut model, _) = projects_preview_fixture();
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ToggleMarkMode,
            None,
        )
        .expect("enter right-seat mark mode");
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::MarkToggle,
            None,
        )
        .expect("mark right-seat task");
        assert_eq!(model.right_seat().expect("right seat").marked_count(), 1);

        let left = route_board_intent(&model, BoardIntent::CollapseDetail);
        assert_eq!(left.intent, BoardIntent::StageLeft);
        assert_eq!(left.target, BoardIntentTarget::Outer);

        let first = route_board_intent(&model, BoardIntent::CloseLayer);
        assert_eq!(first.target, BoardIntentTarget::Focused);
        assert!(
            !first.return_to_index,
            "the first Escape is spent only on marks"
        );
        apply_intent(
            &mut domain,
            board_intent_target_mut(&mut model, first.target),
            first.intent,
            None,
        )
        .expect("clear right-seat marks");
        assert_eq!(model.right_seat().expect("right seat").marked_count(), 0);
        assert!(!model.right_seat().expect("right seat").mark_mode_active());
        assert!(model.project_right_seat_focused());

        let second = route_board_intent(&model, BoardIntent::CloseLayer);
        assert!(second.return_to_index, "the next Escape resumes closing");
    }

    #[test]
    fn empty_projects_preview_mark_mode_spends_escape_before_returning_to_index() {
        let (mut domain, mut model, _) = projects_preview_fixture();
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ToggleMarkMode,
            None,
        )
        .expect("enter empty right-seat mark mode");
        let area = Rect::new(0, 0, 110, 30);
        assert_eq!(
            preview_key_intent(
                &mut model,
                area,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            ),
            BoardIntent::MarkClear
        );
        assert_eq!(
            preview_key_intent(
                &mut model,
                area,
                KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT),
            ),
            BoardIntent::ToggleMarkMode
        );

        let close = route_board_intent(&model, BoardIntent::CloseLayer);
        assert_eq!(close.target, BoardIntentTarget::Focused);
        assert!(!close.return_to_index);
    }

    #[test]
    fn projects_preview_keyboard_uses_right_seat_for_task_actions() {
        let (mut domain, mut model, id) = projects_preview_fixture();
        let area = Rect::new(0, 0, 110, 30);
        let intent = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert_eq!(intent, BoardIntent::Complete);
        apply_intent(&mut domain, model.input_target_mut(), intent, None)
            .expect("complete preview task");
        assert_eq!(
            domain.get(id).map(|task| task.status),
            Some(HumanStatus::Done)
        );
        assert!(
            model.selected_id().is_none(),
            "the index has no task selection"
        );
    }

    #[test]
    fn projects_preview_right_seat_builds_and_spends_its_own_marked_set() {
        let (mut domain, mut model) = projects_preview_open_tasks_fixture();
        let area = Rect::new(0, 0, 110, 30);
        let header = crate::ui::queue::INBOX_HEADER_ROW_ID;
        let tasks: Vec<_> = model
            .right_seat()
            .expect("projects rail has a right seat")
            .visible_ids()
            .into_iter()
            .filter(|id| *id != header)
            .take(2)
            .collect();
        assert_eq!(tasks.len(), 2);
        let activate = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT),
        );
        assert_eq!(activate, BoardIntent::ToggleMarkMode);
        apply_intent(&mut domain, model.input_target_mut(), activate, None)
            .expect("enter right-seat mark mode");
        while selected_row(model.right_seat().expect("right seat")) != Some(tasks[0]) {
            apply_intent(
                &mut domain,
                model.input_target_mut(),
                BoardIntent::SelectNext,
                None,
            )
            .expect("select first project task");
        }

        let extend = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
        );
        assert_eq!(
            extend,
            BoardIntent::MarkExtend(crate::ui::input::MarkDirection::Down)
        );
        apply_intent(&mut domain, model.input_target_mut(), extend, None)
            .expect("mark and move in right seat");
        assert!(model
            .right_seat()
            .expect("right seat")
            .marked_ids()
            .contains(&tasks[0]));

        while selected_row(model.right_seat().expect("right seat")) != Some(tasks[1]) {
            apply_intent(
                &mut domain,
                model.input_target_mut(),
                BoardIntent::SelectNext,
                None,
            )
            .expect("select second project task");
        }
        let toggle = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert_eq!(toggle, BoardIntent::MarkToggle);
        apply_intent(&mut domain, model.input_target_mut(), toggle, None)
            .expect("mark second project task");

        let complete = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        apply_intent(&mut domain, model.input_target_mut(), complete, None)
            .expect("complete right-seat marks");
        assert!(tasks
            .iter()
            .all(|id| domain.get(*id).expect("task").status == HumanStatus::Done));
        assert_eq!(model.right_seat().expect("right seat").marked_count(), 0);
    }

    #[test]
    fn projects_preview_search_uses_the_focused_right_seat() {
        let (mut domain, mut model, _) = projects_preview_fixture();
        let area = Rect::new(0, 0, 110, 30);
        let search = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        assert_eq!(search, BoardIntent::FocusSearch);
        apply_intent(&mut domain, model.input_target_mut(), search, None)
            .expect("focus right-seat search");
        assert_eq!(model.input_mode(), BoardInputMode::Search);
        assert_eq!(
            model.right_seat().map(BoardModel::input_mode),
            Some(BoardInputMode::Search)
        );
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::SearchQueryInsertText("missing".into()),
            None,
        )
        .expect("filter right-seat tasks");
        assert!(model
            .right_seat()
            .expect("right seat")
            .visible_ids()
            .is_empty());
        assert_eq!(
            model.project_rows().len(),
            1,
            "left index remains unfiltered"
        );

        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::PinSearch,
            None,
        )
        .expect("pin right-seat search");
        let arrow = route_board_intent(&model, BoardIntent::CollapseDetail);
        assert_eq!(arrow.intent, BoardIntent::StageLeft);
        assert_eq!(arrow.target, BoardIntentTarget::Outer);
        assert!(
            !arrow.return_to_index,
            "left arrow leaves Rail even while search is pinned"
        );
        let routed = route_board_intent(&model, BoardIntent::CloseLayer);
        assert_eq!(routed.target, BoardIntentTarget::Focused);
        assert!(
            !routed.return_to_index,
            "pinned search clears before leaving"
        );
        apply_intent(&mut domain, model.input_target_mut(), routed.intent, None)
            .expect("clear right-seat search");
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Rail);
        assert_eq!(model.right_seat().map(BoardModel::search_query), Some(""));
    }

    #[test]
    fn projects_preview_right_seat_selection_stays_on_visible_task_after_sync() {
        let (mut domain, mut model) = projects_preview_open_tasks_fixture();
        let area = Rect::new(0, 0, 110, 30);
        let header = crate::ui::queue::INBOX_HEADER_ROW_ID;
        let visible = model
            .right_seat()
            .expect("projects rail has a right seat")
            .visible_ids();
        let first_task = visible
            .iter()
            .copied()
            .find(|id| *id != header)
            .expect("open task is visible");
        let second_task = visible
            .iter()
            .copied()
            .find(|id| *id != header && *id != first_task)
            .expect("second open task is visible");

        // Make the task immediately below the inbox heading the selected row. Completing it
        // must reanchor to the surviving task, not to the heading that sits between them.
        while selected_row(model.right_seat().expect("projects rail has a right seat"))
            != Some(first_task)
        {
            apply_intent(
                &mut domain,
                model.input_target_mut(),
                BoardIntent::SelectNext,
                None,
            )
            .expect("advance right-seat selection");
        }
        assert_eq!(selected_row(model.right_seat().unwrap()), Some(first_task));

        let intent = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let routed = route_board_intent(&model, intent);
        assert_eq!(routed.target, BoardIntentTarget::Focused);
        assert_eq!(routed.intent, BoardIntent::Complete);
        apply_intent(
            &mut domain,
            board_intent_target_mut(&mut model, routed.target),
            routed.intent,
            None,
        )
        .expect("complete right-seat task");

        let recovery = SaveRecovery::<DomainState>::new();
        sync_focused_project_preview(&mut model, &domain, &recovery);

        let right = model.right_seat().expect("projects rail has a right seat");
        assert_eq!(right.selected_id(), Some(second_task));
        assert!(
            right.visible_ids().contains(&second_task),
            "right-seat selection must remain on a painted row"
        );
        assert_ne!(selected_row(right), Some(header));
    }

    #[test]
    fn projects_preview_rail_mouse_dispatch_targets_right_seat_and_syncs_after_action() {
        let area = Rect::new(0, 0, 110, 30);
        let temp = TempStore::new("projects-preview-mouse");
        let (mut domain, mut model) = projects_preview_open_tasks_fixture();
        for id in domain
            .tasks()
            .iter()
            .map(|task| task.id)
            .collect::<Vec<_>>()
        {
            domain
                .set_status(id, HumanStatus::Ready)
                .expect("ready preview task");
        }
        model.sync_from_domain(&domain);
        temp.store.save(&domain).expect("persist preview tasks");
        sync_frame_presentation(area, &model);
        assert!(model.project_right_seat_focused());

        let right_visible = model
            .right_seat()
            .expect("projects rail has a right seat")
            .visible_ids();
        let task_ids: Vec<_> = right_visible
            .into_iter()
            .filter(|id| *id != crate::ui::queue::INBOX_HEADER_ROW_ID)
            .collect();
        let second_task = *task_ids.get(1).expect("second preview task");
        let hits = board_hit_map(area, &model);
        let task_hit = hits
            .regions
            .iter()
            .find(|hit| hit.target == crate::ui::render::QueueHitTarget::Task(second_task))
            .expect("right-seat task hit")
            .area;
        let task_click = left_click(task_hit.x, task_hit.y);
        let task_intent = map_responsive_board_mouse(&model, &hits, area, task_click)
            .expect("right-seat task click");
        assert!(
            matches!(task_intent, BoardIntent::SelectIndex(_)),
            "right-seat task click mapped to {task_intent:?}"
        );

        let mut recovery = SaveRecovery::new();
        let task_target = mouse_intent_target(&model, area, task_click, &hits, &task_intent);
        assert_eq!(task_target, BoardIntentTarget::Focused);
        let nested_selection_before = model.right_seat().and_then(BoardModel::selected_id);
        assert_eq!(
            mouse_intent_target_mut(&mut model, area, task_click, &hits, &task_intent)
                .selected_id(),
            nested_selection_before,
            "the Rail mouse target must be the nested board"
        );
        let quit = dispatch_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardDispatchRoute {
                area,
                target: task_target,
            },
            task_intent,
            &mut recovery,
            false,
        )
        .expect("dispatch right-seat task click");
        assert!(!quit);
        assert_eq!(
            model.selected_id(),
            None,
            "the projects index stays unselected"
        );
        assert_eq!(
            model.right_seat().and_then(BoardModel::selected_id),
            Some(second_task),
            "the task click must select the nested board's task"
        );

        let hits = board_hit_map(area, &model);
        let verb_hit = hits
            .regions
            .iter()
            .find(|hit| {
                matches!(hit.target, crate::ui::render::QueueHitTarget::Verb(_))
                    && map_responsive_board_mouse(
                        &model,
                        &hits,
                        area,
                        left_click(hit.area.x, hit.area.y),
                    ) == Some(BoardIntent::Complete)
            })
            .expect("right-seat complete verb hit")
            .area;
        assert!(
            hits.footer
                .is_some_and(|footer| footer.contains(verb_hit.as_position())),
            "the complete verb must be painted in the shared footer"
        );
        let verb_click = left_click(verb_hit.x, verb_hit.y);
        let verb_intent = map_responsive_board_mouse(&model, &hits, area, verb_click)
            .expect("right-seat footer click");
        assert_eq!(verb_intent, BoardIntent::Complete);
        let verb_target = mouse_intent_target(&model, area, verb_click, &hits, &verb_intent);
        assert_eq!(verb_target, BoardIntentTarget::Focused);
        let quit = dispatch_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardDispatchRoute {
                area,
                target: verb_target,
            },
            verb_intent,
            &mut recovery,
            false,
        )
        .expect("dispatch right-seat footer action");
        assert!(!quit);

        assert_eq!(
            domain.get(second_task).map(|task| task.status),
            Some(HumanStatus::Done)
        );
        let outer_view = model.queue_view();
        let outer_row = outer_view
            .projects
            .iter()
            .find(|row| row.path == "/repos/preview")
            .expect("outer project row after completion");
        assert_eq!(
            outer_row.on_deck, 1,
            "post-dispatch sync must refresh the outer project's ON DECK count"
        );
        let right = model.right_seat().expect("right seat after completion");
        assert!(
            right
                .selected_id()
                .is_some_and(|id| right.visible_ids().contains(&id)),
            "post-dispatch sync must leave the nested selection on a visible task"
        );
    }

    #[test]
    fn projects_preview_nested_arrows_keep_the_narrow_peek_keymap() {
        let (mut domain, mut model, _) = projects_preview_fixture();
        let area = Rect::new(0, 0, 110, 30);
        let peek = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert_eq!(peek, BoardIntent::PeekDetail);
        apply_intent(&mut domain, model.input_target_mut(), peek, None).expect("peek nested task");
        assert!(model
            .right_seat()
            .and_then(BoardModel::detail_open)
            .is_some());

        let collapse = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert_eq!(collapse, BoardIntent::CollapseDetail);
        apply_intent(&mut domain, model.input_target_mut(), collapse, None)
            .expect("collapse nested task peek");
        assert!(model
            .right_seat()
            .and_then(BoardModel::detail_open)
            .is_none());
        assert_eq!(
            model.right_seat().map(BoardModel::wide_stage),
            Some(crate::ui::tier::WideStage::FullBoard)
        );
    }

    #[test]
    fn projects_preview_esc_leaves_task_page_then_returns_to_index() {
        let (mut domain, mut model, _) = projects_preview_fixture();
        let area = Rect::new(0, 0, 110, 30);
        let enter = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        apply_intent(&mut domain, model.input_target_mut(), enter, None)
            .expect("open preview task");
        assert_eq!(
            model.right_seat().map(BoardModel::wide_stage),
            Some(crate::ui::tier::WideStage::FullTask)
        );

        let close = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert_eq!(close, BoardIntent::CloseLayer);
        apply_intent(&mut domain, model.input_target_mut(), close, None)
            .expect("close preview task page");
        assert_eq!(
            model.right_seat().map(BoardModel::wide_stage),
            Some(crate::ui::tier::WideStage::FullBoard)
        );

        let outer_close = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        let routed = route_board_intent(&model, outer_close);
        assert_eq!(routed.intent, BoardIntent::CloseLayer);
        assert_eq!(routed.target, BoardIntentTarget::Focused);
        assert!(routed.return_to_index);
        assert_eq!(
            apply_intent(
                &mut domain,
                board_intent_target_mut(&mut model, routed.target),
                routed.intent,
                None,
            )
            .expect("close the nested preview board"),
            IntentOutcome::None,
            "Esc leaves the preview, never the process"
        );
        if routed.return_to_index {
            apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None)
                .expect("return to index");
        }
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
        assert!(!model.project_right_seat_focused());
    }

    #[test]
    fn projects_preview_task_page_paints_its_wide_column_header() {
        let area = Rect::new(0, 0, 110, 30);
        let (mut domain, mut model, _) = projects_preview_fixture();
        let enter = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        apply_intent(&mut domain, model.input_target_mut(), enter, None)
            .expect("open preview task");

        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        let mut hits = crate::ui::render::QueueHitMap::default();
        terminal
            .draw(|frame| hits = draw_board(frame, &model))
            .expect("draw preview task page");
        let buffer = terminal.backend().buffer();
        let task_area = model.responsive_geometry(area).task;
        let rendered = (task_area.y..task_area.bottom())
            .map(|y| {
                (task_area.x..task_area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\\n");
        assert!(
            rendered.contains("preview task"),
            "task page header should retain the selected task title:\\n{rendered}"
        );
        assert!(hits
            .regions
            .iter()
            .any(|hit| matches!(hit.target, crate::ui::render::QueueHitTarget::FormTitle)));
    }

    /// A project preview owns the expanded capture form in the right seat. Its title must stay
    /// in the column header, and the active notes line must retain the editor's bold treatment.
    #[test]
    fn projects_preview_expanded_quick_add_paints_title_and_active_notes() {
        let area = Rect::new(0, 0, 110, 30);
        let (mut domain, mut model, _) = projects_preview_fixture();
        apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None)
            .expect("select preview project");
        apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
            .expect("focus preview project");
        assert!(model.project_right_seat_focused());

        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::OpenCapture,
            None,
        )
        .expect("open preview capture");
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::QuickAddInsertText("preview draft".to_string()),
            None,
        )
        .expect("type preview title");
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ExpandQuickAdd,
            None,
        )
        .expect("expand preview capture");
        assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::EditInsertText("first note".to_string()),
            None,
        )
        .expect("type preview note");

        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal
            .draw(|frame| {
                let _ = draw_board(frame, &model);
            })
            .expect("draw expanded preview capture");
        let buffer = terminal.backend().buffer();
        let task_area = model.responsive_geometry(area).task;
        let rendered = (task_area.y..task_area.bottom())
            .map(|y| {
                (task_area.x..task_area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\\n");
        assert!(
            rendered.contains("preview draft"),
            "expanded preview title missing from the right column:\\n{rendered}"
        );
        assert!(
            rendered.contains("first note"),
            "expanded preview notes missing from the right column:\\n{rendered}"
        );

        let (note_y, note_x) = (task_area.y..task_area.bottom())
            .find_map(|y| {
                (task_area.x..task_area.right()).find_map(|x| {
                    let remaining = task_area.right().saturating_sub(x) as usize;
                    let text = (0..remaining)
                        .map(|offset| buffer[(x + offset as u16, y)].symbol())
                        .collect::<String>();
                    text.starts_with("first note").then_some((y, x))
                })
            })
            .expect("painted note row");
        for offset in 0.."first note".chars().count() {
            let cell = &buffer[(note_x + offset as u16, note_y)];
            assert!(
                cell.modifier.contains(Modifier::BOLD),
                "active notes cell should be bold: {:?}",
                cell.symbol()
            );
        }
    }

    #[test]
    fn projects_preview_save_recovery_maps_retry_and_cancel_in_the_right_seat() {
        let area = Rect::new(0, 0, 110, 30);
        let (mut domain, mut model, _) = projects_preview_fixture();
        let mut recovery = SaveRecovery::new();
        let baseline = domain.clone();
        let outcome = apply_board_intent_with_save_recovery(
            &mut domain,
            model.input_target_mut(),
            &mut recovery,
            BoardSaveContext {
                baseline,
                intent: BoardIntent::Complete,
                snapshot: None,
            },
            |_| Err("preview save failed".into()),
        )
        .expect("failed preview save enters recovery");
        assert_eq!(outcome, IntentOutcome::None);
        assert!(recovery.is_pending());
        assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);
        let retry = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        );
        assert_eq!(retry, BoardIntent::RetrySave);
        apply_board_intent_with_save_recovery(
            &mut domain,
            model.input_target_mut(),
            &mut recovery,
            BoardSaveContext {
                baseline: DomainState::new(),
                intent: retry,
                snapshot: None,
            },
            |_| Ok(()),
        )
        .expect("retry preview save");
        assert!(!recovery.is_pending());

        let (mut domain, mut model, _) = projects_preview_fixture();
        let mut recovery = SaveRecovery::new();
        let baseline = domain.clone();
        apply_board_intent_with_save_recovery(
            &mut domain,
            model.input_target_mut(),
            &mut recovery,
            BoardSaveContext {
                baseline,
                intent: BoardIntent::Complete,
                snapshot: None,
            },
            |_| Err("preview save failed".into()),
        )
        .expect("failed preview save enters recovery");
        let cancel = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
        );
        assert_eq!(cancel, BoardIntent::CancelSave);
        apply_board_intent_with_save_recovery(
            &mut domain,
            model.input_target_mut(),
            &mut recovery,
            BoardSaveContext {
                baseline: DomainState::new(),
                intent: cancel,
                snapshot: None,
            },
            |_| panic!("CancelSave must not persist"),
        )
        .expect("cancel preview save");
        assert!(!recovery.is_pending());
    }

    #[test]
    fn projects_preview_dirty_draft_refuses_every_project_switch_route() {
        let (mut domain, mut model) = projects_overview_fixture();
        stage_right(&mut domain, &mut model, 2);
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::OpenCapture,
            None,
        )
        .expect("open preview quick add");
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::QuickAddInsertText("keep this draft".into()),
            None,
        )
        .expect("type preview draft");
        let draft = model
            .right_seat()
            .map(BoardModel::quick_add_title_value)
            .expect("preview draft");
        assert_eq!(draft, "keep this draft");
        assert!(model.has_unsaved_work());

        // Rail row changes are refused before the nested session can be rebound.
        let rail_project = model
            .right_seat()
            .and_then(BoardModel::active_project)
            .map(Path::to_path_buf);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectProjectRow(1),
            None,
        )
        .expect("refuse rail project switch");
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Rail);
        assert_eq!(
            model.right_seat().and_then(BoardModel::active_project),
            rail_project.as_deref()
        );
        assert_eq!(
            model.right_seat().map(BoardModel::quick_add_title_value),
            Some("keep this draft")
        );
        assert_eq!(
            model.message(),
            Some("save or cancel edits before switching tasks")
        );

        // Returning to Split keeps the dirty seat parked, so every index-level route below is
        // tested against the same retained draft rather than a clean replacement.
        apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None)
            .expect("park dirty preview");
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
        assert!(model.has_unsaved_work());

        let assert_refused =
            |domain: &mut DomainState, model: &mut BoardModel, intent: BoardIntent| {
                apply_intent(domain, model, intent.clone(), None).expect("dirty route refusal");
                assert_eq!(
                    model.message(),
                    Some("save or cancel edits before switching tasks"),
                    "{intent:?} must explain why the dirty preview stayed put"
                );
                assert_eq!(
                    model.right_seat().map(BoardModel::quick_add_title_value),
                    Some("keep this draft")
                );
                assert!(model.has_unsaved_work());
            };

        assert_refused(&mut domain, &mut model, BoardIntent::SelectProjectRow(1));
        assert_refused(&mut domain, &mut model, BoardIntent::OpenTaskPage);
        assert_refused(&mut domain, &mut model, BoardIntent::OpenProjectSelector);
        assert_refused(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Desk),
        );
        assert_refused(&mut domain, &mut model, BoardIntent::SearchQueryInsert('b'));
        assert_refused(
            &mut domain,
            &mut model,
            BoardIntent::SearchQueryInsertText("beta".into()),
        );
        assert_refused(&mut domain, &mut model, BoardIntent::SearchQueryBackspace);
        assert_refused(&mut domain, &mut model, BoardIntent::StageLeft);

        // A same-row second click is also a project-board jump once the double-click window is
        // satisfied, and must not bypass the dirty-seat guard.
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectProjectRow(0),
            None,
        )
        .expect("first same-row click");
        assert_refused(&mut domain, &mut model, BoardIntent::SelectProjectRow(0));

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenProjectsViewPicker,
            None,
        )
        .expect("open projects view picker");
        assert_refused(&mut domain, &mut model, BoardIntent::ConfirmListPicker);
    }

    #[test]
    fn app_keyboard_route_owns_stage_slider_keys() {
        let (mut domain, mut model) = board_fixture("keyboard stage", None);
        let wide = Rect::new(0, 0, 110, 24);
        let single = Rect::new(0, 0, 109, 24);
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let route = |model: &mut BoardModel, area, code| {
            let mode = resolve_board_surface(area, model);
            board_keyboard_intent_for_area(model, area, mode, key(code))
        };

        // Stage 0: → slides, Enter opens the full page, ← is inert at wide widths.
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::FullBoard);
        assert_eq!(
            route(&mut model, wide, KeyCode::Right),
            Some(BoardIntent::StageRight)
        );
        assert_eq!(
            route(&mut model, wide, KeyCode::Char('l')),
            Some(BoardIntent::StageRight)
        );
        assert_eq!(
            route(&mut model, wide, KeyCode::Enter),
            Some(BoardIntent::OpenTaskPage)
        );
        assert_eq!(route(&mut model, wide, KeyCode::Left), None);
        assert_eq!(route(&mut model, wide, KeyCode::Char('h')), None);
        // Below the threshold the narrow routes are untouched.
        assert_eq!(
            route(&mut model, single, KeyCode::Enter),
            Some(BoardIntent::OpenTaskPage)
        );
        assert_eq!(
            route(&mut model, single, KeyCode::Right),
            Some(BoardIntent::PeekDetail)
        );
        assert_eq!(
            route(&mut model, single, KeyCode::Char('l')),
            Some(BoardIntent::PeekDetail)
        );
        assert_eq!(
            route(&mut model, single, KeyCode::Left),
            Some(BoardIntent::CollapseDetail)
        );
        assert_eq!(
            route(&mut model, single, KeyCode::Char('h')),
            Some(BoardIntent::CollapseDetail)
        );

        // Stage G: ← and → slide, Esc is the page's own close, j/k stay page navigation.
        stage_right(&mut domain, &mut model, 2);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Rail);
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        assert_eq!(
            route(&mut model, wide, KeyCode::Left),
            Some(BoardIntent::StageLeft)
        );
        assert_eq!(
            route(&mut model, wide, KeyCode::Char('h')),
            Some(BoardIntent::StageLeft)
        );
        assert_eq!(
            route(&mut model, wide, KeyCode::Right),
            Some(BoardIntent::StageRight)
        );
        assert_eq!(
            route(&mut model, wide, KeyCode::Char('l')),
            Some(BoardIntent::StageRight)
        );
        assert_eq!(
            route(&mut model, wide, KeyCode::Esc),
            Some(BoardIntent::CloseLayer)
        );
        assert_eq!(
            route(&mut model, wide, KeyCode::Char('j')),
            Some(BoardIntent::PageScrollDown)
        );
        assert_eq!(
            route(&mut model, wide, KeyCode::Tab),
            Some(BoardIntent::FormFocusNext),
            "Tab keeps its task-page meaning"
        );
        // Narrow task page: ← stays inert, Esc closes, exactly as before the slider.
        assert_eq!(route(&mut model, single, KeyCode::Left), None);
        assert_eq!(route(&mut model, single, KeyCode::Char('h')), None);
        assert_eq!(route(&mut model, single, KeyCode::Char('l')), None);
        assert_eq!(
            route(&mut model, single, KeyCode::Esc),
            Some(BoardIntent::CloseLayer)
        );

        // Stage F: → is inert, ← goes back to the rail.
        stage_right(&mut domain, &mut model, 1);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::FullTask);
        assert_eq!(route(&mut model, wide, KeyCode::Right), None);
        assert_eq!(route(&mut model, wide, KeyCode::Char('l')), None);
        assert_eq!(
            route(&mut model, wide, KeyCode::Left),
            Some(BoardIntent::StageLeft)
        );
        assert_eq!(
            route(&mut model, wide, KeyCode::Char('h')),
            Some(BoardIntent::StageLeft)
        );
    }

    fn dispatch_reflow_test_click(
        domain: &mut DomainState,
        model: &mut BoardModel,
        reflow_click: &mut ReflowRowClick,
        area: Rect,
        click: MouseEvent,
    ) {
        reflow_click.observe(&Event::Mouse(click));
        let hits = board_hit_map(area, model);
        let continuing = reflow_click.target(model, area, click).is_some();
        let (_, mode, scrollbar) = board_scrollbar_mouse_route(
            area,
            model,
            &hits,
            reflow_click,
            click,
            &mut false,
            |model, focus| {
                apply_intent(domain, model, focus, None)?;
                Ok::<bool, DomainError>(false)
            },
        )
        .expect("production Down scrollbar route");
        assert!(matches!(scrollbar, ScrollbarMouse::Miss));
        assert!(
            continuing
                || press_on_focused_surface(
                    model,
                    &hits,
                    area,
                    Position::new(click.column, click.row),
                )
                || press_survives_off_focus(
                    map_responsive_board_mouse(model, &hits, area, click).as_ref(),
                    mode,
                    wide_mouse_focus_intent(model, &hits, area, click).as_ref()
                )
        );
        reflow_click.observe(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..click
        }));
        let (quit, intent) = board_mouse_click_intent_after_focus(
            area,
            model,
            &hits,
            reflow_click,
            click,
            |model, focus| {
                apply_intent(domain, model, focus, None)?;
                Ok::<bool, DomainError>(false)
            },
        )
        .expect("route click through production focus handoff");
        assert!(!quit);
        if let Some(intent) = intent {
            apply_intent(domain, model, intent, None).expect("dispatch click");
        }
    }

    #[test]
    fn app_reflow_double_click_right_of_split_opens_original_task() {
        for width in [110, 130] {
            let (mut domain, mut model) =
                board_fixture("original task", Some("notes ".repeat(500)));
            let id = model.selected_id();
            let mut reflow_click = ReflowRowClick::default();
            let area = Rect::new(0, 0, width, 24);
            let hits = board_hit_map(area, &model);
            let row = hits
                .regions
                .iter()
                .find(|hit| matches!(hit.target, crate::ui::render::QueueHitTarget::Task(_)))
                .unwrap()
                .area;
            let click = left_click(width - 1, row.y);
            let before = serde_json::to_value(&domain).unwrap();
            dispatch_reflow_test_click(&mut domain, &mut model, &mut reflow_click, area, click);
            assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
            dispatch_reflow_test_click(&mut domain, &mut model, &mut reflow_click, area, click);
            assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::FullTask);
            assert_eq!(model.edit_target(), id);
            assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
            assert_eq!(serde_json::to_value(&domain).unwrap(), before);
        }
    }

    #[test]
    fn app_reflow_double_click_keeps_task_below_wrapping_row() {
        for width in [110, 130] {
            let (mut domain, _) = board_fixture(&"a long preceding title ".repeat(4), None);
            domain
                .create(
                    "another long task title ".repeat(4),
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Manual,
                    None,
                )
                .unwrap();
            let mut model = BoardModel::from_domain(&domain, None);
            // The expanded inbox heading is a selectable chrome row before the two tasks.
            let id = model.visible_ids()[2];
            let mut reflow_click = ReflowRowClick::default();
            let area = Rect::new(0, 0, width, 24);
            let hits = board_hit_map(area, &model);
            let row = hits
                .regions
                .iter()
                .find(|hit| {
                    matches!(hit.target,
                crate::ui::render::QueueHitTarget::Task(task) if task == id)
                })
                .unwrap()
                .area;
            let click = left_click(20, row.y);
            dispatch_reflow_test_click(&mut domain, &mut model, &mut reflow_click, area, click);
            let new_hits = board_hit_map(area, &model);
            let moved = new_hits
                .regions
                .iter()
                .find(|hit| {
                    matches!(hit.target,
                crate::ui::render::QueueHitTarget::Task(task) if task == id)
                })
                .unwrap()
                .area;
            assert!(moved.y > row.y, "fixture must cross the wrap boundary");
            dispatch_reflow_test_click(&mut domain, &mut model, &mut reflow_click, area, click);
            assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::FullTask);
            assert_eq!(model.edit_target(), Some(id));
        }
    }

    #[test]
    fn app_reflow_double_click_different_cell_keeps_live_task_controls() {
        let (mut domain, mut model) = board_fixture("control routing", None);
        let mut reflow = ReflowRowClick::default();
        let area = Rect::new(0, 0, 130, 24);
        let hits = board_hit_map(area, &model);
        let row = hits
            .regions
            .iter()
            .find(|hit| matches!(hit.target, crate::ui::render::QueueHitTarget::Task(_)))
            .unwrap()
            .area;
        dispatch_reflow_test_click(
            &mut domain,
            &mut model,
            &mut reflow,
            area,
            left_click(80, row.y),
        );
        let hits = board_hit_map(area, &model);
        let control = hits
            .regions
            .iter()
            .find(|hit| matches!(hit.target, crate::ui::render::QueueHitTarget::StepAdd))
            .unwrap()
            .area;
        assert_ne!(control.as_position(), Position::new(80, row.y));
        dispatch_reflow_test_click(
            &mut domain,
            &mut model,
            &mut reflow,
            area,
            click_at(control),
        );
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Rail);
        assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    }

    #[test]
    fn app_reflow_double_click_cancels_for_other_gestures() {
        for event in [
            Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            Event::Resize(120, 24),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                ..left_click(21, 6)
            }),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                ..left_click(20, 6)
            }),
            Event::Mouse(left_click(21, 6)),
        ] {
            let (mut domain, mut model) = board_fixture("cancel gesture", None);
            let area = Rect::new(0, 0, 130, 24);
            let row = board_hit_map(area, &model)
                .regions
                .into_iter()
                .find(|hit| matches!(hit.target, crate::ui::render::QueueHitTarget::Task(_)))
                .unwrap()
                .area;
            let click = left_click(20, row.y);
            let mut reflow = ReflowRowClick::default();
            dispatch_reflow_test_click(&mut domain, &mut model, &mut reflow, area, click);
            assert!(reflow.target(&model, area, click).is_some());
            assert!(reflow
                .target(&model, Rect::new(0, 0, 129, 24), click)
                .is_none());
            reflow.observe(&event);
            assert!(reflow.target(&model, area, click).is_none(), "{event:?}");
        }
    }

    #[test]
    fn app_reflow_double_click_does_not_arm_on_task_number_copy() {
        let (mut domain, _) = board_fixture("copy number", None);
        domain.assign_numbers_for_persistence();
        let mut model = BoardModel::from_domain(&domain, None);
        let area = Rect::new(0, 0, 130, 24);
        let number = board_hit_map(area, &model)
            .regions
            .into_iter()
            .find(|hit| matches!(hit.target, crate::ui::render::QueueHitTarget::TaskNumber(_)))
            .unwrap()
            .area;
        let mut reflow = ReflowRowClick::default();
        dispatch_reflow_test_click(&mut domain, &mut model, &mut reflow, area, click_at(number));
        assert!(reflow.0.is_none());
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::FullBoard);
    }

    #[test]
    fn app_mouse_click_moves_stage_a_to_g_before_dispatching_same_control() {
        let (mut domain, mut model) = board_fixture("mouse stage", None);
        let area = Rect::new(0, 0, 110, 24);
        stage_right(&mut domain, &mut model, 1);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
        let task_area =
            crate::ui::tier::resolve_responsive(area.width, area.height, model.wide_stage()).task;
        let hits = crate::ui::board::board_hit_map(area, &model);
        let control = hits
            .regions
            .iter()
            .find(|hit| {
                task_area.contains(hit.area.as_position())
                    && matches!(hit.target, crate::ui::render::QueueHitTarget::StepAdd)
            })
            .expect("task-side add-step control in the preview");
        let click = click_at(control.area);
        let (quit, intent) = board_mouse_click_intent_after_focus(
            area,
            &mut model,
            &hits,
            &mut ReflowRowClick::default(),
            click,
            |model, focus| {
                assert_eq!(focus, BoardIntent::StageRight);
                apply_intent(&mut domain, model, focus, None)?;
                Ok::<bool, DomainError>(false)
            },
        )
        .expect("route click");

        assert!(!quit);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Rail);
        assert_eq!(
            model.focused_surface(),
            crate::ui::tier::FocusedSurface::Task
        );
        let intent = intent.expect("same click dispatches after the stage change");
        assert_eq!(intent, BoardIntent::BeginAddStep);
        apply_intent(&mut domain, &mut model, intent, None).expect("apply task control");
        assert_eq!(model.input_mode(), BoardInputMode::EditStep);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Rail);
    }

    #[test]
    fn app_press_gate_keeps_shared_footer_verbs_outside_the_focused_column() {
        // F-8: the shared footer spans the frame, but the Down gate only accepted presses
        // inside the focused column, so a footer verb painted under the other column died
        // before Up could dispatch it. Stage G: the left 33 cells of the verb bar sit under
        // the rail; stage A: the right cells sit under the preview.
        for (times, stage) in [
            (1, crate::ui::tier::WideStage::Split),
            (2, crate::ui::tier::WideStage::Rail),
        ] {
            let (mut domain, mut model) = board_fixture("footer gate", None);
            stage_right(&mut domain, &mut model, times);
            assert_eq!(model.wide_stage(), stage);
            let area = Rect::new(0, 0, 130, 24);
            let hits = crate::ui::board::board_hit_map(area, &model);
            let focused = focused_mouse_area(&model, area);
            let verb = hits
                .regions
                .iter()
                .find(|hit| {
                    matches!(hit.target, crate::ui::render::QueueHitTarget::Verb(_))
                        && !focused.contains(hit.area.as_position())
                })
                .unwrap_or_else(|| panic!("{stage:?}: a footer verb outside the focused column"))
                .area;
            let pos = Position::new(verb.x, verb.y);
            let intent =
                map_responsive_board_mouse(&model, &hits, area, left_click(verb.x, verb.y));
            assert!(intent.is_some(), "{stage:?}: footer verb maps an intent");
            assert!(
                press_on_focused_surface(&model, &hits, area, pos),
                "{stage:?}: the Down gate keeps a shared-footer press"
            );
        }
    }

    #[test]
    fn app_press_gate_keeps_rail_slides_and_drops_unmapped_presses() {
        let (mut domain, mut model) = board_fixture("gate rail", None);
        stage_right(&mut domain, &mut model, 2);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Rail);
        let area = Rect::new(0, 0, 110, 24);
        let hits = crate::ui::board::board_hit_map(area, &model);

        // Blank rail space maps a slide and the press gate keeps it.
        let blank = left_click(4, 9);
        let intent = map_responsive_board_mouse(&model, &hits, area, blank);
        assert_eq!(intent, Some(BoardIntent::StageLeft));
        assert!(press_survives_off_focus(
            intent.as_ref(),
            model.input_mode(),
            None
        ));

        // The rule column maps nothing and the press is dropped.
        let rule = left_click(32, 9);
        let intent = map_responsive_board_mouse(&model, &hits, area, rule);
        assert_eq!(intent, None);
        assert!(!press_survives_off_focus(
            intent.as_ref(),
            model.input_mode(),
            None
        ));

        // A rail row still routes its select.
        let row = hits
            .regions
            .iter()
            .find(|hit| matches!(hit.target, crate::ui::render::QueueHitTarget::Task(_)))
            .expect("rail row")
            .area;
        let intent = map_responsive_board_mouse(&model, &hits, area, left_click(row.x, row.y));
        assert!(matches!(
            intent,
            Some(BoardIntent::FocusBoardAndSelectIndex(_))
        ));
        assert!(press_survives_off_focus(
            intent.as_ref(),
            model.input_mode(),
            None
        ));

        // And the whole route lands the board beside the rail.
        apply_intent(&mut domain, &mut model, intent.expect("row intent"), None).expect("apply");
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
    }

    #[test]
    fn app_task_scrollbar_moves_stage_refreshes_bound_and_routes_page_scroll() {
        let (mut domain, mut model) =
            board_fixture("scrollbar stage", Some("long notes ".repeat(500)));
        // Open the full page, then slide back to A so the page is parked behind the preview.
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
            .expect("open task page");
        apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None).expect("F to G");
        apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None).expect("G to A");
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);

        let wide = Rect::new(0, 0, 110, 24);
        let preview_hits = crate::ui::board::board_hit_map(wide, &model);
        let bottom = preview_hits
            .regions
            .iter()
            .filter_map(|hit| match hit.target {
                crate::ui::render::QueueHitTarget::PageScroll(offset) => Some((offset, hit.area)),
                _ => None,
            })
            .max_by_key(|(offset, _)| *offset)
            .expect("task preview scrollbar");
        assert!(bottom.0 > 0);
        model.set_page_scroll_horizon_for_test(0);
        let mut dragging = false;
        let (quit, mode, routed) = board_scrollbar_mouse_route(
            wide,
            &mut model,
            &preview_hits,
            &ReflowRowClick::default(),
            click_at(bottom.1),
            &mut dragging,
            |model, focus| {
                assert_eq!(focus, BoardIntent::StageRight);
                apply_intent(&mut domain, model, focus, None)?;
                Ok::<bool, DomainError>(false)
            },
        )
        .expect("route task scrollbar");

        assert!(!quit);
        assert_eq!(mode, BoardInputMode::TaskPage);
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Rail);
        let ScrollbarMouse::Intent(BoardIntent::PageScrollTo(offset)) = routed else {
            panic!("scrollbar press must route a page scroll, got {routed:?}");
        };
        assert!(offset > 0);
        assert_eq!(
            model.page_scroll_horizon(),
            offset,
            "the bound is refreshed for the stage G column before the offset is applied"
        );
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::PageScrollTo(offset),
            None,
        )
        .expect("apply page scroll");
        assert_eq!(model.page_scroll(), offset);
        assert!(dragging);
    }

    #[test]
    fn default_mode_is_board() {
        assert_eq!(resolve_mode_from(None, ["tsk"]), AppMode::Board);
        assert_eq!(
            resolve_mode_from(None, ["tsk", "--something"]),
            AppMode::Board
        );
        // Non-capture env values do not select Capture.
        assert_eq!(resolve_mode_from(Some("board"), ["tsk"]), AppMode::Board);
    }

    #[test]
    fn capture_arg_selects_capture_mode() {
        assert_eq!(
            resolve_mode_from(None, ["tsk", "capture"]),
            AppMode::Capture
        );
        // Arg still wins when env is absent or non-capture.
        assert_eq!(
            resolve_mode_from(Some("board"), ["tsk", "capture"]),
            AppMode::Capture
        );
    }

    #[test]
    fn capture_env_selects_capture_mode() {
        assert_eq!(
            resolve_mode_from(Some("capture"), ["tsk"]),
            AppMode::Capture
        );
        assert_eq!(
            resolve_mode_from(Some("CAPTURE"), ["tsk", "--something"]),
            AppMode::Capture
        );
    }

    /// requires every listed control to remain operable down to 40x10, so a
    /// mutating control must reach its intent through the same route `run_board` drives
    /// (surface resolution -> key map -> area gate -> command-surface resolution) at the
    /// legacy Resize band and below, not just at Wide/Browse sizes, and it must actually be
    /// applied: routing to an intent that is never applied proves nothing moved. Formerly
    /// `resize_guidance_blocks_keyboard_task_mutation_but_keeps_quit_reachable`, which
    /// asserted the opposite (now-removed) gate.
    #[test]
    fn compact_controls_operate_down_to_40x10_and_quit_stays_reachable() {
        /// Drive one key through the live route at `area` -- surface resolution, key map,
        /// area gate, command-surface resolution -- and apply the resolved intent, exactly
        /// as `run_board`'s key arm does.
        fn drive(
            domain: &mut DomainState,
            model: &mut BoardModel,
            area: Rect,
            key: KeyEvent,
        ) -> IntentOutcome {
            let mode = resolve_board_surface(area, model);
            let intent = map_key(mode, key)
                .unwrap_or_else(|| panic!("{area:?}: {key:?} must still map to an intent"));
            let intent = board_intent_for_area(area, intent)
                .unwrap_or_else(|| panic!("{area:?}: {key:?}'s intent must route"));
            let intent = resolve_board_command(model, intent).unwrap_or_else(|| {
                panic!("{area:?}: {key:?}'s intent must resolve through the command route")
            });
            apply_intent(domain, model, intent, None)
                .unwrap_or_else(|e| panic!("{area:?}: {key:?} must apply cleanly: {e:?}"))
        }
        let ctrl = |code| KeyEvent::new(code, KeyModifiers::CONTROL);
        let bare = |code| KeyEvent::new(code, KeyModifiers::NONE);

        for area in [
            Rect::new(0, 0, 49, 18),
            Rect::new(0, 0, 50, 17),
            Rect::new(0, 0, 40, 10),
        ] {
            // 'd' (Complete): Todo -> Done, actually applied.
            let mut domain = DomainState::new();
            let id = domain
                .create(
                    "Stay todo",
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Capture,
                    None,
                )
                .expect("create task");
            let mut model = BoardModel::from_domain(&domain, None);
            drive(&mut domain, &mut model, area, ctrl(KeyCode::Char('d')));
            assert_eq!(
                domain.get(id).expect("task").status,
                HumanStatus::Done,
                "{area:?}: 'd' must actually complete the task through the live route"
            );

            // s (PrimaryVerb): Ready -> Started, actually applied.
            let mut domain = DomainState::new();
            let id = domain
                .create(
                    "Stay todo",
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Capture,
                    None,
                )
                .expect("create task");
            let mut model = BoardModel::from_domain(&domain, None);
            drive(&mut domain, &mut model, area, ctrl(KeyCode::Char('s')));
            assert_eq!(
                domain.get(id).expect("task").status,
                HumanStatus::Started,
                "{area:?}: Ctrl+S must actually start the task through the live route"
            );

            // x (SoftDelete): actually applied.
            let mut domain = DomainState::new();
            let id = domain
                .create(
                    "Stay todo",
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Capture,
                    None,
                )
                .expect("create task");
            let mut model = BoardModel::from_domain(&domain, None);
            drive(&mut domain, &mut model, area, ctrl(KeyCode::Char('x')));
            drive(&mut domain, &mut model, area, ctrl(KeyCode::Char('x')));
            assert!(
                domain.get(id).expect("task").soft_deleted,
                "{area:?}: 'x' must actually soft-delete the task through the live route"
            );

            // ':' (OpenCommandPalette): actually applied (opens the palette; not a mutation).
            let mut domain = DomainState::new();
            domain
                .create(
                    "Stay todo",
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Capture,
                    None,
                )
                .expect("create task");
            let mut model = BoardModel::from_domain(&domain, None);
            drive(&mut domain, &mut model, area, bare(KeyCode::Char(':')));
            assert_eq!(
                model.command_surface(),
                CommandSurface::Palette,
                "{area:?}: ':' must actually open the command palette through the live route"
            );

            // Quit remains reachable.
            assert_eq!(
                board_intent_for_area(area, BoardIntent::Quit),
                Some(BoardIntent::Quit),
                "{area:?}: quit remains reachable"
            );
        }
    }

    /// Minor 3: `board_mouse_intent` now builds `board_hit_map` only
    /// for `Down(Left)`, never for a wheel step, since `map_board_mouse` resolves
    /// `ScrollUp`/`ScrollDown` through `wheel_board_intent` without ever reading the
    /// hit-map parameter. This proves the fast path still dispatches the same intent the
    /// keyboard's own selection step does, end to end through `board_mouse_intent` itself
    /// -- not just through `map_board_mouse` (already covered at that layer by
    /// `wheel_step_matches_the_keyboard_selection_step` in `ui::mouse::tests`).
    #[test]
    fn board_command_surface_mouse_and_keyboard_dispatch_the_same_intent() {
        use crate::ui::board::{apply_intent, board_hit_map, resolve_board_command};
        use crate::ui::mouse::left_click;
        use crate::ui::render::QueueHitTarget;

        let mut domain = DomainState::new();
        let id = domain
            .create(
                "Command me",
                None,
                TaskScope::Project {
                    path: "/repos/app".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create task");
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/app")));
        assert_eq!(model.selected_id(), Some(id));
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenCommandPalette,
            None,
        )
        .expect("open palette");
        // Narrow to "delete" (the catalog; reopen needs a done selection and Complete is
        // key-only, so this todo fixture's unambiguous, always-available entry is delete).
        for character in "delete".chars() {
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::CommandQueryInsert(character),
                None,
            )
            .expect("type query");
        }

        // Live mouse dispatch consumes the same resolved hit-map the renderer uses.
        let area = Rect::new(0, 0, 120, 24);
        let hits = board_hit_map(area, &model);
        assert_eq!(
            model.visible_commands().first().map(|c| c.intent.clone()),
            Some(BoardIntent::SoftDelete)
        );
        let chip = hits
            .regions
            .iter()
            .find(|hit| matches!(hit.target, QueueHitTarget::Command(0)))
            .expect("command hit region");
        let mouse_intent = map_board_mouse(&model, &hits, left_click(chip.area.x + 1, chip.area.y))
            .expect("mouse command intent");

        // the legacy Resize band no longer withholds the command route either; the
        // compact queue frame paints the command surface there, so build the hit map
        // there too and resolve the same click through it -- not just an identity check on
        // the intent already resolved at 120x24 -- before anything closes the surface below.
        let resize = Rect::new(0, 0, 49, 18);
        let resize_hits = board_hit_map(resize, &model);
        let resize_chip = resize_hits
            .regions
            .iter()
            .find(|hit| matches!(hit.target, QueueHitTarget::Command(0)))
            .expect("command hit region at the resize band");
        let resize_mouse_intent = map_board_mouse(
            &model,
            &resize_hits,
            left_click(resize_chip.area.x + 1, resize_chip.area.y),
        )
        .expect("mouse command intent at the resize band");
        assert_eq!(
            resize_mouse_intent, mouse_intent,
            "the resize band must resolve the same command click as 120x24"
        );

        assert_eq!(
            board_intent_for_area(area, mouse_intent.clone()),
            Some(mouse_intent.clone())
        );
        assert_eq!(
            board_intent_for_area(resize, resize_mouse_intent.clone()),
            Some(resize_mouse_intent.clone())
        );

        // a command row click is `SelectCommand(index)`, not the row's own intent
        // directly, so it is not raw-equal to `ConfirmCommand` the way earlier commands here
        // were before that fix (the same reason `SelectIndex`/`SelectProjectOption` are never
        // raw-equal to their keyboard counterparts either). What must still match is what both
        // resolve to and the surface teardown resolving does -- resolve each on its own model
        // clone so resolving one cannot affect the other's outcome.
        let mut mouse_resolved_model = model.clone();
        let resolved_mouse_intent =
            resolve_board_command(&mut mouse_resolved_model, mouse_intent.clone())
                .expect("mouse command resolves to a dispatchable intent");
        let keyboard_intent = resolve_board_command(&mut model, BoardIntent::ConfirmCommand)
            .expect("keyboard command intent");
        assert_eq!(
            resolved_mouse_intent, keyboard_intent,
            "click and Enter must resolve to the same underlying command"
        );
        assert_eq!(
            mouse_resolved_model.command_surface(),
            model.command_surface(),
            "resolving the click must tear the surface down the same way Enter's \
             ConfirmCommand does"
        );
        assert_eq!(domain.get(id).expect("task").status, HumanStatus::Open);
    }

    #[test]
    fn projects_index_row_click_selects_and_a_second_click_opens_slot_2() {
        use crate::ui::board::{apply_intent, board_hit_map};
        use crate::ui::mouse::left_click;
        use crate::ui::render::QueueHitTarget;

        let mut domain = DomainState::new();
        domain
            .create(
                "alpha task",
                None,
                TaskScope::Project {
                    path: "/repos/alpha".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create task");
        domain
            .create(
                "beta task",
                None,
                TaskScope::Project {
                    path: "/repos/beta".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create other");
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/alpha")));
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("projects index");
        assert_eq!(
            model.selected_project(),
            Some(Path::new("/repos/alpha")),
            "slot 2 retains the invocation project while Projects is active"
        );

        let area = Rect::new(0, 0, 80, 24);
        let row_click = |model: &BoardModel, path: &str| {
            let hits = board_hit_map(area, model);
            let hit = hits
                .regions
                .iter()
                .find(|hit| {
                    matches!(hit.target, QueueHitTarget::ProjectRow(index) if model
                        .project_rows()
                        .get(index)
                        .is_some_and(|row| row.path == path))
                })
                .unwrap_or_else(|| panic!("missing index row for {path} in {hits:?}"));
            left_click(hit.area.x, hit.area.y)
        };
        let drive = |domain: &mut DomainState,
                     model: &mut BoardModel,
                     mouse: crossterm::event::MouseEvent| {
            let intent = board_mouse_intent(area, model, mouse).expect("click maps to intent");
            apply_intent(domain, model, intent, None).expect("click applies");
        };

        let mouse = row_click(&model, "/repos/beta");
        drive(&mut domain, &mut model, mouse);
        assert_eq!(
            model.selected_project(),
            Some(Path::new("/repos/alpha")),
            "a single index-row click only moves the index cursor"
        );
        assert_eq!(
            model.selected_project_row().map(|row| row.path),
            Some("/repos/beta".to_string()),
            "the clicked row is selected"
        );
        // A click on another mapped control between the two row clicks disarms the
        // pending double-click: the second row click selects again instead of opening.
        let hits = board_hit_map(area, &model);
        let tab = hits
            .regions
            .iter()
            .find(|hit| matches!(hit.target, QueueHitTarget::NavTab(NavTab::Projects)))
            .expect("projects tab hit");
        let tab_click = left_click(tab.area.x, tab.area.y);
        drive(&mut domain, &mut model, tab_click);
        drive(&mut domain, &mut model, mouse);
        assert_eq!(
            model.selected_project(),
            Some(Path::new("/repos/alpha")),
            "an intervening click on the tab disarms the row double-click"
        );

        drive(&mut domain, &mut model, mouse);
        assert_eq!(
            model.selected_project(),
            Some(Path::new("/repos/beta")),
            "a second click on the same row inside the window opens it in slot 2"
        );

        // Index rows are navigation: the click mutated no task, and the pin reanchors
        // onto the opened project's own rows.
        let pinned = model
            .selected_id()
            .and_then(|id| domain.get(id).map(|task| task.scope.clone()));
        assert_eq!(
            pinned,
            Some(TaskScope::Project {
                path: "/repos/beta".into()
            }),
            "the pin lands on a row of the project it opened"
        );
    }

    /// live dispatch reaches the project selector by mouse and keyboard in
    /// every layout that offers it, including the legacy Resize band down to 40x10.
    #[test]
    fn project_selector_mouse_and_keyboard_dispatch_the_same_intent_at_every_size() {
        use crate::ui::board::board_hit_map;
        use crate::ui::mouse::left_click;
        use crate::ui::render::QueueHitTarget;

        let mut domain = DomainState::new();
        domain
            .create(
                "Project scoped",
                None,
                TaskScope::Project {
                    path: "/repos/app".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create task");
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/app")));
        model.set_selected_project(Some(PathBuf::from("/repos/app")));

        for area in [
            Rect::new(0, 0, 120, 24),
            Rect::new(0, 0, 50, 18),
            Rect::new(0, 0, 49, 18),
            Rect::new(0, 0, 40, 10),
        ] {
            // Clicking slot 2 while its project is open opens the picker, the way the
            // old chip did.
            let hits = board_hit_map(area, &model);
            let tab2 = hits
                .regions
                .iter()
                .find(|hit| matches!(hit.target, QueueHitTarget::NavTab(NavTab::ProjectBoard)))
                .unwrap_or_else(|| panic!("no slot-2 tab hit region at {area:?}"));
            let mouse_intent =
                map_board_mouse(&model, &hits, left_click(tab2.area.x + 1, tab2.area.y))
                    .expect("slot-2 tab hit");
            assert_eq!(
                mouse_intent,
                BoardIntent::SelectNavTab(NavTab::ProjectBoard),
                "slot-2's click is the tab intent; the reducer opens the picker from it"
            );
            // `p` gives the keyboard the same destination by direct intent.
            let keyboard_intent = map_key(
                board_input_mode_for_area(area, model.input_mode()),
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            );
            assert_eq!(
                keyboard_intent,
                Some(BoardIntent::OpenProjectSelector),
                "`p` must open the project selector at {area:?}"
            );
            assert_eq!(
                board_intent_for_area(area, mouse_intent.clone()),
                Some(mouse_intent.clone()),
                "{area:?} must route the project selector"
            );
        }

        // the legacy Resize band paints a project chip too and no longer
        // withholds the route to it.
        assert_eq!(
            board_intent_for_area(RESIZE_AREA, BoardIntent::OpenProjectSelector),
            Some(BoardIntent::OpenProjectSelector),
            "the project selector must remain reachable down to the legacy Resize band"
        );
    }

    #[test]
    fn every_board_mutation_uses_a_real_persisted_baseline() {
        for intent in [
            BoardIntent::ConfirmEdit,
            BoardIntent::ConfirmEditNext,
            BoardIntent::SetStatus(HumanStatus::Blocked),
            BoardIntent::Complete,
            BoardIntent::Reopen,
            BoardIntent::SoftDelete,
            BoardIntent::Undo,
            BoardIntent::PrimaryVerb,
            BoardIntent::ToggleBlock,
            BoardIntent::ToggleReview,
            BoardIntent::ToggleStep,
        ] {
            assert!(
                board_intent_may_persist(&intent),
                "{intent:?} must load the persisted baseline before save recovery"
            );
        }
        assert!(!board_intent_may_persist(&BoardIntent::SelectNext));

        // Editing moves a draft, never the store: only ConfirmEdit above writes.
        for intent in [
            BoardIntent::EditInsert('x'),
            BoardIntent::EditInsertText("x".to_string()),
            BoardIntent::EditInsertLineBreak,
            BoardIntent::EditBackspace,
            BoardIntent::EditDeleteForward,
            BoardIntent::EditMoveLeft,
            BoardIntent::EditMoveRight,
            BoardIntent::EditMoveLineStart,
            BoardIntent::EditMoveLineEnd,
            BoardIntent::EditMoveWordLeft,
            BoardIntent::EditMoveWordRight,
            BoardIntent::CancelEdit,
        ] {
            assert!(
                !board_intent_may_persist(&intent),
                "{intent:?} must not reload a persisted baseline"
            );
        }
    }

    /// Fresh on-disk store for one test, cleaned up on drop.
    struct TempStore {
        dir: PathBuf,
        store: TaskStore,
    }

    impl TempStore {
        fn new(label: &str) -> Self {
            use std::time::{SystemTime, UNIX_EPOCH};
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let seq = TEMP_DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("tsk-{label}-{nanos}-{seq}"));
            std::fs::create_dir_all(&dir).unwrap();
            let store = TaskStore::new(&dir);
            TempStore { dir, store }
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn t64_dirty_capture_and_parked_task_drafts_refuse_quit_at_the_app_boundary() {
        let temp = TempStore::new("t64-dirty-quit");
        let mut domain = DomainState::new();
        domain
            .create(
                "park me",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("task");
        temp.store.save(&domain).expect("seed store");
        let mut recovery = SaveRecovery::new();

        let mut capture = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut capture, BoardIntent::OpenCapture, None)
            .expect("open capture");
        apply_intent(
            &mut domain,
            &mut capture,
            BoardIntent::QuickAddInsertText("captured draft".into()),
            None,
        )
        .expect("type capture");
        apply_intent(&mut domain, &mut capture, BoardIntent::ExpandQuickAdd, None)
            .expect("expand capture");
        assert!(capture.has_unsaved_work());
        assert!(!handle_board_intent(
            &temp.store,
            &mut domain,
            &mut capture,
            BoardIntent::Quit,
            &mut recovery,
            false,
        )
        .expect("refuse capture quit"));
        assert!(capture.board_form_open());
        assert_eq!(
            capture.message(),
            Some("save or cancel edits before switching tasks")
        );

        let mut task = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut task, BoardIntent::OpenTaskPage, None)
            .expect("open task page");
        apply_intent(&mut domain, &mut task, BoardIntent::BeginEditTitle, None)
            .expect("edit title");
        apply_intent(&mut domain, &mut task, BoardIntent::EditInsert('!'), None)
            .expect("dirty title");
        for _ in 0..8 {
            if task.input_mode() == BoardInputMode::TaskPage {
                break;
            }
            apply_intent(&mut domain, &mut task, BoardIntent::FormFocusNext, None)
                .expect("park editor inside task page");
        }
        assert_eq!(task.input_mode(), BoardInputMode::TaskPage);
        assert!(task.task_editing());
        for _ in 0..3 {
            apply_intent(&mut domain, &mut task, BoardIntent::StageLeft, None)
                .expect("park page toward board");
        }
        assert_eq!(task.wide_stage(), crate::ui::tier::WideStage::FullBoard);
        assert_eq!(task.input_mode(), BoardInputMode::Normal);
        assert!(task.has_unsaved_work());
        assert!(!handle_board_intent(
            &temp.store,
            &mut domain,
            &mut task,
            BoardIntent::CloseLayer,
            &mut recovery,
            false,
        )
        .expect("refuse root Esc"));
        assert!(task.has_unsaved_work());
        assert_eq!(
            task.message(),
            Some("save or cancel edits before switching tasks")
        );
    }

    #[test]
    fn t64_clean_parked_edit_quits_on_the_first_root_escape() {
        let temp = TempStore::new("t64-clean-parked-quit");
        let (mut domain, mut model) = board_with_one_task();
        temp.store.save(&domain).expect("seed store");
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("begin unchanged edit");
        for _ in 0..8 {
            if model.input_mode() == BoardInputMode::TaskPage {
                break;
            }
            apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
                .expect("park text editor");
        }
        for _ in 0..3 {
            apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None)
                .expect("return to full board");
        }
        assert!(model.task_editing());
        assert!(model.root_escape_requests_quit());
        assert!(!model.has_unsaved_work());

        apply_intent(&mut domain, &mut model, BoardIntent::FocusSearch, None)
            .expect("search over parked page");
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
            .expect("clear active search");
        assert!(
            !model.has_unsaved_work() && model.root_escape_requests_quit(),
            "closing search must restore the clean parked task-page mode"
        );
        apply_intent(&mut domain, &mut model, BoardIntent::FocusSearch, None)
            .expect("search over parked page again");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SearchQueryInsertText("behind".into()),
            None,
        )
        .expect("type parked-page search");
        apply_intent(&mut domain, &mut model, BoardIntent::PinSearch, None)
            .expect("pin parked-page search");
        assert!(
            !model.has_unsaved_work(),
            "pinning search must restore the clean parked task-page mode"
        );
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
            .expect("clear pinned search");
        assert!(
            model.root_escape_requests_quit(),
            "clearing pinned search must restore root Escape"
        );
        assert!(handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::CloseLayer,
            &mut SaveRecovery::new(),
            false,
        )
        .expect("first root Esc quits"));
    }

    #[test]
    fn t64_ctrl_q_uses_non_editor_form_modes_before_the_form_mapper() {
        let (mut domain, mut model) = board_with_one_task();
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("open task form");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::FocusFormField(CaptureField::Scope),
            None,
        )
        .expect("focus scope");
        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(model.input_mode(), BoardInputMode::EditScope);
        assert_eq!(
            board_keyboard_intent(&model, model.input_mode(), ctrl_q),
            Some(BoardIntent::Quit)
        );

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenFormScopeDropdown,
            None,
        )
        .expect("open scope picker");
        assert_eq!(model.input_mode(), BoardInputMode::FormScopeDropdown);
        assert_eq!(
            board_keyboard_intent(&model, model.input_mode(), ctrl_q),
            Some(BoardIntent::Quit)
        );

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::CancelFormScopeDropdown,
            None,
        )
        .expect("close scope picker");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::FocusFormField(CaptureField::Title),
            None,
        )
        .expect("focus title editor");
        assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
        assert_eq!(
            board_keyboard_intent(&model, model.input_mode(), ctrl_q),
            None
        );
    }

    #[test]
    fn t64_escape_closes_help_then_either_split_then_quits() {
        for projects in [false, true] {
            let temp = TempStore::new("t64-split-escape");
            let (mut domain, mut model) = if projects {
                projects_overview_fixture()
            } else {
                board_with_one_task()
            };
            temp.store.save(&domain).expect("seed store");
            stage_right(&mut domain, &mut model, 1);
            assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
            let tab = model.nav_tab();
            apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None)
                .expect("Help above split");
            let area = Rect::new(0, 0, 110, 30);
            let mut recovery = SaveRecovery::new();
            for press in 0..3 {
                let mode = resolve_board_surface(area, &mut model);
                let intent = board_keyboard_intent_for_area(
                    &model,
                    area,
                    mode,
                    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                )
                .expect("Esc route");
                let routed = route_board_intent(&model, intent);
                let quit = dispatch_board_intent(
                    &temp.store,
                    &mut domain,
                    &mut model,
                    BoardDispatchRoute {
                        area,
                        target: routed.target,
                    },
                    routed.intent,
                    &mut recovery,
                    false,
                )
                .expect("dispatch Esc");
                assert_eq!(quit, press == 2, "projects={projects}, press={press}");
                assert_eq!(
                    model.wide_stage(),
                    if press == 0 {
                        crate::ui::tier::WideStage::Split
                    } else {
                        crate::ui::tier::WideStage::FullBoard
                    }
                );
                assert_eq!(model.nav_tab(), tab);
                if projects && press == 1 {
                    assert!(
                        model.right_seat().is_none(),
                        "collapsed preview stays closed after app sync"
                    );
                }
            }
        }
    }

    #[test]
    fn t64_narrowed_split_root_quits_without_collapsing_hidden_state() {
        for projects in [false, true] {
            for stages in 1..=if projects { 2 } else { 1 } {
                for width in [78, 109] {
                    for help in [false, true] {
                        let temp = TempStore::new("t64-narrow-root");
                        let (mut domain, mut model) = if projects {
                            projects_overview_fixture()
                        } else {
                            board_with_one_task()
                        };
                        temp.store.save(&domain).unwrap();
                        stage_right(&mut domain, &mut model, stages);
                        sync_frame_presentation(Rect::new(0, 0, 110, 30), &model);
                        assert!(model.frame_wide());
                        let parked_stage = model.wide_stage();
                        let tab = model.nav_tab();
                        let area = Rect::new(0, 0, width, 30);
                        sync_frame_presentation(area, &model);
                        assert!(!model.frame_wide());
                        assert_eq!(model.input_mode(), BoardInputMode::Normal);
                        if help {
                            apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None)
                                .unwrap();
                        }
                        for press in 0..=usize::from(help) {
                            let mode = resolve_board_surface(area, &mut model);
                            let intent = board_keyboard_intent_for_area(
                                &model,
                                area,
                                mode,
                                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                            )
                            .unwrap();
                            let routed = route_board_intent(&model, intent);
                            let quit = dispatch_board_intent(
                                &temp.store,
                                &mut domain,
                                &mut model,
                                BoardDispatchRoute {
                                    area,
                                    target: routed.target,
                                },
                                routed.intent,
                                &mut SaveRecovery::new(),
                                false,
                            )
                            .unwrap();
                            assert_eq!(quit, press == usize::from(help), "projects={projects}, stages={stages}, width={width}, help={help}, press={press}");
                            assert_eq!(
                                model.wide_stage(),
                                parked_stage,
                                "Esc does not mutate an unpainted stage"
                            );
                            assert_eq!(model.nav_tab(), tab);
                            assert_eq!(model.right_seat().is_some(), projects);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn t64_narrow_task_page_is_not_a_board_root() {
        let temp = TempStore::new("t64-narrow-page");
        let (mut domain, mut model) = board_with_one_task();
        temp.store.save(&domain).unwrap();
        stage_right(&mut domain, &mut model, 2);
        sync_frame_presentation(Rect::new(0, 0, 78, 30), &model);
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        assert!(!model.root_escape_requests_quit());
        assert!(!handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::CloseLayer,
            &mut SaveRecovery::new(),
            false,
        )
        .unwrap());
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
    }

    #[test]
    fn t64_narrow_header_escape_preserves_the_parked_split() {
        let (mut domain, mut model) = board_with_one_task();
        stage_right(&mut domain, &mut model, 1);
        sync_frame_presentation(Rect::new(0, 0, 110, 30), &model);
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleInboxGroup, None).unwrap();
        assert!(model.inbox_header_selected());
        sync_frame_presentation(Rect::new(0, 0, 78, 30), &model);
        assert_eq!(
            apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).unwrap(),
            IntentOutcome::None
        );
        assert_eq!(
            model.wide_stage(),
            crate::ui::tier::WideStage::Split,
            "a hidden split is not an Escape layer"
        );
        sync_frame_presentation(Rect::new(0, 0, 110, 30), &model);
        assert!(model.frame_wide());
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
    }

    #[test]
    fn t64_narrow_root_refuses_hidden_drafts_and_widening_restores_them() {
        for projects in [false, true] {
            let temp = TempStore::new("t64-narrow-draft");
            let (mut domain, mut model) = if projects {
                projects_overview_fixture()
            } else {
                board_with_one_task()
            };
            temp.store.save(&domain).unwrap();
            stage_right(&mut domain, &mut model, 2);
            sync_frame_presentation(Rect::new(0, 0, 110, 30), &model);
            apply_intent(
                &mut domain,
                model.input_target_mut(),
                BoardIntent::BeginEditTitle,
                None,
            )
            .unwrap();
            apply_intent(
                &mut domain,
                model.input_target_mut(),
                BoardIntent::EditInsert('!'),
                None,
            )
            .unwrap();
            let draft = model.input_target_mut().edit_buffer().to_owned();
            for _ in 0..8 {
                if model.input_mode() == BoardInputMode::TaskPage {
                    break;
                }
                apply_intent(
                    &mut domain,
                    model.input_target_mut(),
                    BoardIntent::FormFocusNext,
                    None,
                )
                .unwrap();
            }
            assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
            apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None).unwrap();
            assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
            sync_frame_presentation(Rect::new(0, 0, 78, 30), &model);
            assert_eq!(model.input_mode(), BoardInputMode::Normal);
            assert!(model.has_unsaved_work());
            assert!(!handle_board_intent(
                &temp.store,
                &mut domain,
                &mut model,
                BoardIntent::CloseLayer,
                &mut SaveRecovery::new(),
                false
            )
            .unwrap());
            assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
            assert!(model.has_unsaved_work());
            assert_eq!(
                model.message(),
                Some("save or cancel edits before switching tasks")
            );
            sync_frame_presentation(Rect::new(0, 0, 110, 30), &model);
            stage_right(&mut domain, &mut model, 1);
            apply_intent(
                &mut domain,
                model.input_target_mut(),
                BoardIntent::FocusFormField(CaptureField::Title),
                None,
            )
            .unwrap();
            assert_eq!(model.input_target_mut().edit_buffer(), draft);
        }
    }

    #[test]
    fn t64_split_escape_keeps_parked_drafts() {
        let temp = TempStore::new("t64-split-drafts");
        let (mut domain, mut model) = board_with_one_task();
        temp.store.save(&domain).expect("seed store");
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).unwrap();
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None).unwrap();
        for _ in 0..8 {
            if model.input_mode() == BoardInputMode::TaskPage {
                break;
            }
            apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).unwrap();
        }
        for _ in 0..2 {
            apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None).unwrap();
        }
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
        assert!(model.task_session_dirty());
        assert!(!handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::CloseLayer,
            &mut SaveRecovery::new(),
            false
        )
        .unwrap());
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::FullBoard);
        assert!(
            model.task_session_dirty(),
            "collapsing a task split parks its draft"
        );
        assert!(!handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::CloseLayer,
            &mut SaveRecovery::new(),
            false
        )
        .unwrap());
        assert!(
            model.task_session_dirty(),
            "root Esc refuses the parked dirty draft"
        );

        let (mut domain, mut model, _) = projects_preview_fixture();
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::BeginEditTitle,
            None,
        )
        .unwrap();
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::EditInsert('!'),
            None,
        )
        .unwrap();
        let draft = model.right_seat().unwrap().edit_buffer().to_owned();
        apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None).unwrap();
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);
        assert_eq!(
            apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).unwrap(),
            IntentOutcome::None
        );
        assert_eq!(
            model.wide_stage(),
            crate::ui::tier::WideStage::Split,
            "a project preview with unsaved work cannot be discarded"
        );
        assert_eq!(model.right_seat().unwrap().edit_buffer(), draft);
        assert_eq!(
            model.message(),
            Some("save or cancel edits before switching tasks")
        );
    }

    #[test]
    fn t64_ctrl_q_from_a_clean_nested_preview_quits_the_whole_board() {
        let temp = TempStore::new("t64-nested-global-quit");
        let (mut domain, mut model, _) = projects_preview_fixture();
        temp.store.save(&domain).expect("seed preview store");
        let area = Rect::new(0, 0, 110, 30);
        let intent = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        );
        assert_eq!(intent, BoardIntent::Quit);
        let routed = route_board_intent(&model, intent);
        assert_eq!(routed.target, BoardIntentTarget::Focused);

        let mut recovery = SaveRecovery::new();
        assert!(dispatch_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardDispatchRoute {
                area,
                target: routed.target,
            },
            routed.intent,
            &mut recovery,
            false,
        )
        .expect("nested global quit"));
    }

    #[test]
    fn t64_focused_preview_quit_preserves_a_dirty_outer_task() {
        let temp = TempStore::new("t64-outer-dirty-quit");
        let (mut domain, mut model) = board_with_one_task();
        temp.store.save(&domain).expect("seed store");
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("edit outer task");
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None)
            .expect("dirty outer title");
        let draft = model.edit_buffer().to_owned();
        for _ in 0..8 {
            if model.input_mode() == BoardInputMode::TaskPage {
                break;
            }
            apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
                .expect("park editor");
        }
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        for _ in 0..3 {
            apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None)
                .expect("park task page");
        }
        assert!(model.root_escape_requests_quit());
        assert!(model.has_unsaved_work());
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("switch to projects with outer draft parked");
        stage_right(&mut domain, &mut model, 2);
        let area = Rect::new(0, 0, 110, 30);
        let intent = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        );
        assert_eq!(intent, BoardIntent::Quit);
        assert!(model.project_right_seat_focused());
        assert!(!model.right_seat().unwrap().has_unsaved_work());
        let routed = route_board_intent(&model, intent);
        assert_eq!(routed.target, BoardIntentTarget::Focused);
        assert!(!dispatch_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardDispatchRoute {
                area,
                target: routed.target
            },
            routed.intent,
            &mut SaveRecovery::new(),
            false,
        )
        .expect("outer draft refuses focused quit"));
        assert!(model.task_session_dirty());
        assert_eq!(
            model.message(),
            Some("save or cancel edits before switching tasks")
        );
        // Return to the parked task and inspect the actual title, not just a dirty flag.
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::ProjectBoard),
            None,
        )
        .expect("return to outer task");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::FocusFormField(CaptureField::Title),
            None,
        )
        .expect("inspect parked title");
        assert_eq!(model.edit_buffer(), draft);
    }

    #[test]
    fn t64_quit_over_failed_save_preserves_the_recovery_banner() {
        for capture in [false, true] {
            let temp = TempStore::new("t64-recovery-banner");
            let (mut domain, mut model) = board_with_one_task();
            temp.store.save(&domain).expect("seed store");
            let baseline = domain.clone();
            let save = if capture {
                let snapshot = crate::context::build_snapshot(
                    &crate::context::RawHostContext::default(),
                    temp.dir.to_str().expect("scratch path"),
                );
                apply_intent(
                    &mut domain,
                    &mut model,
                    BoardIntent::OpenCapture,
                    Some(&snapshot),
                )
                .expect("open quick add");
                apply_intent(
                    &mut domain,
                    &mut model,
                    BoardIntent::QuickAddInsertText("unsaved capture".into()),
                    None,
                )
                .expect("type quick add");
                BoardIntent::QuickAddSave
            } else {
                apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
                    .expect("edit task");
                apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None)
                    .expect("dirty task");
                BoardIntent::ConfirmEdit
            };
            let mut recovery = SaveRecovery::new();
            apply_board_intent_with_save_recovery(
                &mut domain,
                &mut model,
                &mut recovery,
                BoardSaveContext {
                    baseline,
                    intent: save,
                    snapshot: None,
                },
                |_| Err("disk full".into()),
            )
            .expect("failed persistence enters recovery");
            assert!(recovery.is_pending());
            assert!(model.has_unsaved_work());
            let banner = model.message().expect("failure banner").to_owned();
            assert!(banner.contains("disk full"));
            apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None)
                .expect("Help over failed save");
            for key in ['q', 'c'] {
                let intent = board_keyboard_intent(
                    &model,
                    model.input_mode(),
                    KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL),
                )
                .expect("Help quit shortcut");
                assert_eq!(intent, BoardIntent::Quit);
                assert!(!dispatch_board_intent(
                    &temp.store,
                    &mut domain,
                    &mut model,
                    BoardDispatchRoute {
                        area: Rect::new(0, 0, 78, 24),
                        target: BoardIntentTarget::Focused
                    },
                    intent,
                    &mut recovery,
                    false,
                )
                .expect("recovery prevents quit"));
                assert_eq!(
                    model.message(),
                    Some(banner.as_str()),
                    "capture={capture}, key={key}"
                );
                assert_eq!(model.input_mode(), BoardInputMode::Help);
                assert!(recovery.is_pending());
                assert!(model.has_unsaved_work());
            }
            apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
                .expect("Esc closes Help");
            assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);
            assert_eq!(model.message(), Some(banner.as_str()));
        }
    }

    #[test]
    fn t64_dirty_nested_preview_refuses_global_quit_and_keeps_its_draft() {
        let temp = TempStore::new("t64-nested-dirty-quit");
        let (mut domain, mut model, _) = projects_preview_fixture();
        temp.store.save(&domain).expect("seed preview store");
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::OpenCapture,
            None,
        )
        .expect("open nested quick add");
        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::QuickAddInsertText("nested draft".into()),
            None,
        )
        .expect("type nested draft");
        apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None)
            .expect("park nested preview");
        assert_eq!(model.wide_stage(), crate::ui::tier::WideStage::Split);

        let mut recovery = SaveRecovery::new();
        assert!(!handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::Quit,
            &mut recovery,
            false,
        )
        .expect("refuse nested quit"));
        assert_eq!(
            model.right_seat().map(BoardModel::quick_add_title_value),
            Some("nested draft")
        );
        assert_eq!(
            model.message(),
            Some("save or cancel edits before switching tasks")
        );
    }

    #[test]
    fn t64_save_recovery_refuses_quit_but_quick_capture_keeps_ctrl_c() {
        let temp = TempStore::new("t64-recovery-capture-quit");
        let mut domain = DomainState::new();
        temp.store.save(&domain).expect("seed store");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery = SaveRecovery::new();
        recovery.fail(DomainState::new(), DomainState::new(), "save failed");
        model.begin_save_recovery("save failed");
        apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None)
            .expect("open Help over recovery");
        assert_eq!(model.input_mode(), BoardInputMode::Help);
        assert_eq!(
            apply_board_intent_with_save_recovery(
                &mut domain,
                &mut model,
                &mut recovery,
                BoardSaveContext {
                    baseline: DomainState::new(),
                    intent: BoardIntent::Quit,
                    snapshot: None,
                },
                |_| Ok(()),
            )
            .expect("recovery quit is inert"),
            IntentOutcome::None
        );
        assert!(recovery.is_pending());
        assert_eq!(
            model.input_mode(),
            BoardInputMode::Help,
            "Ctrl+Q remains inert without dismissing Help over recovery"
        );
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
            .expect("Esc closes Help");
        assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);

        let mut recovery = SaveRecovery::new();
        let mut capture = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut capture, BoardIntent::OpenCapture, None)
            .expect("open capture");
        apply_intent(
            &mut domain,
            &mut capture,
            BoardIntent::QuickAddInsertText("unsaved popup draft".into()),
            None,
        )
        .expect("type capture");
        apply_intent(&mut domain, &mut capture, BoardIntent::ExpandQuickAdd, None)
            .expect("expand capture");
        apply_intent(&mut domain, &mut capture, BoardIntent::FormFocusNext, None)
            .expect("focus non-editor step selection");
        assert_eq!(capture.input_mode(), BoardInputMode::CapturePage);
        let intent = board_keyboard_intent(
            &capture,
            capture.input_mode(),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        )
        .expect("existing Ctrl+C quit shortcut");
        assert!(handle_board_intent(
            &temp.store,
            &mut domain,
            &mut capture,
            intent,
            &mut recovery,
            true,
        )
        .expect("quick capture retains Ctrl+C exit"));
        assert!(capture.board_form_open());
        assert_eq!(capture.message(), None);
    }

    #[test]
    fn copy_task_number_sends_the_exact_displayed_identifier() {
        let temp = TempStore::new("copy-task-number-payload");
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "Copy me",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("seed task");
        temp.store.save(&domain).expect("persist numbered task");
        let domain = temp.store.load().expect("reload numbered task");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut copied = None;

        copy_task_number_with(&domain, &mut model, id, |text| {
            copied = Some(text.to_string());
            true
        });

        assert_eq!(copied.as_deref(), Some("T1"));
        assert_eq!(model.message(), Some("copy sent: T1"));
    }

    #[test]
    fn copy_task_number_intent_uses_the_app_loop_handoff_without_mutating() {
        let temp = TempStore::new("copy-task-number");
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "Copy me",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("seed task");
        temp.store.save(&domain).expect("persist numbered task");
        let mut domain = temp.store.load().expect("reload numbered task");
        let number = domain.get(id).expect("task").number.expect("task number");
        let mut model = BoardModel::from_domain(&domain, None);
        let selection = model.selected_id();
        let mut recovery = SaveRecovery::new();

        let quit = handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::CopyTaskNumber(id),
            &mut recovery,
            false,
        )
        .expect("copy intent");

        assert!(!quit);
        assert_eq!(
            model.selected_id(),
            selection,
            "copy must not move selection"
        );
        assert_eq!(domain.get(id).expect("task").number, Some(number));
        let message = format!("copy sent: T{number}");
        assert_eq!(model.message(), Some(message.as_str()));
    }

    #[test]
    fn copy_task_number_intent_sends_a_notice_identifier_through_the_app_handoff() {
        let temp = TempStore::new("copy-notice-number");
        let mut domain = DomainState::new();
        let id = domain
            .create_notice(
                "guide.copy",
                "Copy notice",
                None,
                HumanStatus::Ready,
                TaskScope::Global,
                Vec::new(),
            )
            .expect("seed notice");
        temp.store.save(&domain).expect("persist notice");
        let mut domain = temp.store.load().expect("reload numbered notice");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery = SaveRecovery::new();

        let quit = handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::CopyTaskNumber(id),
            &mut recovery,
            false,
        )
        .expect("copy intent");

        assert!(!quit);
        assert_eq!(model.message(), Some("copy sent: N1"));
    }

    /// Reverting the dismissal to the done-only flow leaves the archive and delete
    /// guides unrecorded here: the record was dropped before the handoff, so only the
    /// real board persistence handoff can put the catalog id back.
    #[test]
    fn archiving_or_deleting_a_guide_on_the_board_records_its_dismissal() {
        let run_case = |label: &str, intents: &[BoardIntent]| {
            let temp = TempStore::new(label);
            assert_eq!(crate::guides::seed_on_open(&temp.store), Ok(4));
            std::fs::remove_file(temp.dir.join(crate::delivery::DELIVERY_FILE))
                .expect("drop the delivery record");
            let mut domain = temp.store.load().expect("load seeded board");
            let mut model = BoardModel::from_domain(&domain, None);
            let selected = model.selected_id().expect("seeded selection");
            let mut recovery = SaveRecovery::new();
            for intent in intents {
                handle_board_intent(
                    &temp.store,
                    &mut domain,
                    &mut model,
                    intent.clone(),
                    &mut recovery,
                    false,
                )
                .expect("dismiss intent");
            }
            let task = domain.get(selected).expect("selected guide");
            assert!(
                crate::delivery::is_dismissed(task),
                "{label}: the guide must be dismissed on the board"
            );
            let catalog_id = task.notice.as_ref().expect("notice").catalog_id.clone();
            let recorded = crate::delivery::load(temp.store.path());
            assert_eq!(
                recorded.guides,
                [catalog_id]
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>(),
                "{label}: the persistence handoff must record the dismissed guide"
            );
            // A dismissed guide never comes back, even when the record was lost.
            assert_eq!(
                crate::guides::seed_on_open(&temp.store),
                Ok(0),
                "{label}: the other guides stay present, none is created"
            );
        };
        // ctrl+f files the selected guide; two ctrl+x arm, then delete it.
        run_case("dismiss-archive", &[BoardIntent::File]);
        run_case(
            "dismiss-delete",
            &[BoardIntent::SoftDelete, BoardIntent::SoftDelete],
        );
    }

    #[test]
    fn shift_enter_step_save_uses_the_real_app_save_boundary() {
        let temp = TempStore::new("shift-enter-step");
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "Shift Enter",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("seed task");
        temp.store.save(&domain).expect("seed store");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery = SaveRecovery::new();
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("enter task edit mode");
        apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
            .expect("return to task page");
        apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None).expect("edit");
        for ch in "next step".chars() {
            apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None).expect("type");
        }
        handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmEditNext,
            &mut recovery,
            false,
        )
        .expect("real shift-enter save");
        assert_eq!(
            temp.store
                .load()
                .expect("reload")
                .get(id)
                .expect("task")
                .steps[0]
                .text,
            "next step"
        );
        assert!(!recovery.is_pending());
        assert_eq!(
            model.input_mode(),
            BoardInputMode::TaskPage,
            "successful Shift+Enter saves the step and exits task editing"
        );
    }

    /// fix2 B1: `a` on the real board must create a task through the exact intent
    /// route the running app takes — `handle_board_intent`, which decides the snapshot the
    /// same way the live loop does — not through `apply_intent` hand-fed a snapshot the
    /// product never supplies. This is the app-loop boundary test the second review asked
    /// for: no snapshot is constructed or injected here, only real board intents.
    #[test]
    fn board_plus_title_enter_creates_one_task_through_the_real_app_intent_route() {
        let temp = TempStore::new("board-a-real-route");
        let mut domain = DomainState::new();
        temp.store.save(&domain).expect("seed empty store");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut save_recovery = SaveRecovery::new();

        assert_eq!(model.input_mode(), BoardInputMode::Normal);

        // `a`: OpenCapture, exactly as NORMAL_KEYMAP binds it.
        let quit = handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::OpenCapture,
            &mut save_recovery,
            false,
        )
        .expect("open capture");
        assert!(!quit);
        assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);

        // Type a title one key at a time, the way the real keyboard loop feeds it in.
        for ch in "Real app-route capture".chars() {
            handle_board_intent(
                &temp.store,
                &mut domain,
                &mut model,
                BoardIntent::QuickAddInsert(ch),
                &mut save_recovery,
                false,
            )
            .expect("type title");
        }

        // Enter saves and closes the status-row line.
        let quit = handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::QuickAddSave,
            &mut save_recovery,
            false,
        )
        .expect("save quick add");
        assert!(!quit);
        assert_eq!(
            model.input_mode(),
            BoardInputMode::Normal,
            "a successful capture returns to Normal"
        );
        assert!(
            !save_recovery.is_pending(),
            "a real, writable temp store must not enter save recovery"
        );

        let saved = temp.store.load().expect("load persisted domain");
        let created: Vec<_> = saved
            .tasks()
            .iter()
            .filter(|t| t.title == "Real app-route capture")
            .collect();
        assert_eq!(
            created.len(),
            1,
            "board `+` must create exactly one task through the real app intent path, not zero"
        );
        assert!(
            model.visible_ids().contains(&created[0].id)
                || model.nav_tab() == crate::ui::queue::NavTab::Desk,
            "the board either shows the saved task or keeps the user's destination: \
             a save must not switch tabs to prove where a row went"
        );
    }

    #[test]
    fn selected_text_provenance_survives_standalone_and_board_capture_save_paths() {
        let raw = crate::context::RawHostContext {
            cwd: Some("/tmp/no-repo-selected-capture".into()),
            selected_text: Some("Selected capture".into()),
            ..crate::context::RawHostContext::default()
        };
        let snapshot = crate::context::build_snapshot(&raw, "/tmp/no-repo-selected-capture");
        assert_eq!(snapshot.provenance, ProvenanceOrigin::Selection);

        let standalone = TempStore::new("selected-standalone-capture");
        let mut standalone_domain = DomainState::new();
        let mut capture_model = CaptureModel::from_snapshot(&snapshot);
        assert_eq!(capture_model.title(), "Selected capture");
        apply_capture_intent(
            &mut standalone_domain,
            Some(&standalone.store),
            &snapshot,
            &mut capture_model,
            CaptureIntent::Save,
        )
        .expect("save standalone selected-text capture");
        let saved = standalone.store.load().expect("reload standalone capture");
        assert_eq!(saved.tasks()[0].provenance, ProvenanceOrigin::Selection);

        let board = TempStore::new("selected-board-capture");
        let mut board_domain = DomainState::new();
        let mut board_model = BoardModel::from_domain(&board_domain, None);
        let mut recovery = SaveRecovery::new();
        apply_intent(
            &mut board_domain,
            &mut board_model,
            BoardIntent::OpenCapture,
            Some(&snapshot),
        )
        .expect("open board capture with selected-text snapshot");
        assert_eq!(board_model.quick_add_title_value(), "Selected capture");

        let outcome = apply_board_intent_with_save_recovery(
            &mut board_domain,
            &mut board_model,
            &mut recovery,
            BoardSaveContext {
                baseline: DomainState::new(),
                intent: BoardIntent::QuickAddSave,
                snapshot: None,
            },
            |working| {
                board
                    .store
                    .reload_merge_save(working)
                    .map_err(|error| error.to_string())
            },
        )
        .expect("save board selected-text capture");
        assert_eq!(outcome, IntentOutcome::Persisted);
        let saved = board.store.load().expect("reload board capture");
        assert_eq!(saved.tasks()[0].provenance, ProvenanceOrigin::Selection);
    }

    #[test]
    fn quick_add_project_token_matches_a_project_basename_case_insensitively() {
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Project {
                path: "/repos/tsk-board".into(),
            },
            this_repo: Some(PathBuf::from("/repos/tsk-board")),
            title_prefill: None,
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, snapshot.this_repo.clone());

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenCapture,
            Some(&snapshot),
        )
        .expect("open quick add");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::QuickAddInsertText("Case insensitive scope !p TSK-Board".into()),
            Some(&snapshot),
        )
        .expect("type title and scope token");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::QuickAddSave,
            Some(&snapshot),
        )
        .expect("save quick add");

        let task = domain.tasks().first().expect("quick add creates a task");
        assert_eq!(task.title, "Case insensitive scope");
        assert_eq!(
            task.scope,
            TaskScope::Project {
                path: "/repos/tsk-board".into()
            }
        );
    }

    #[test]
    fn quick_add_refusals_keep_the_line_open_and_esc_drops_their_message() {
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: None,
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenCapture,
            Some(&snapshot),
        )
        .expect("open quick add");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::QuickAddSave,
            Some(&snapshot),
        )
        .expect("reject empty title");
        assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
        assert_eq!(model.message(), Some(TITLE_REQUIRED_MESSAGE));

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::CancelQuickAdd,
            Some(&snapshot),
        )
        .expect("close refused quick add");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert_eq!(model.message(), None, "no refusal leaks onto the board");

        apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None)
            .expect("open snapshot-less quick add");
        apply_intent(&mut domain, &mut model, BoardIntent::QuickAddSave, None)
            .expect("refuse unavailable capture context");
        assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
        assert_eq!(
            model.message(),
            Some("capture context unavailable; press Esc and try again")
        );
    }

    #[test]
    fn expanded_quick_add_save_recovery_cancel_keeps_the_complete_draft_stash() {
        use std::fs;

        let dir = temp_state_dir("expanded-quick-add-save-recovery");
        let blocked = dir.join("not-a-directory");
        fs::write(&blocked, "not a state directory").expect("blocking file");
        let store = TaskStore::new(&blocked);
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: Some(PathBuf::from("/repos/chosen")),
            title_prefill: Some("Retained title".into()),
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, snapshot.this_repo.clone());
        let mut recovery = SaveRecovery::new();

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenCapture,
            Some(&snapshot),
        )
        .expect("open quick add");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ExpandQuickAdd,
            Some(&snapshot),
        )
        .expect("expand quick add");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("retained notes".into()),
            Some(&snapshot),
        )
        .expect("write notes");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::FocusFormField(CaptureField::Scope),
            Some(&snapshot),
        )
        .expect("focus scope");
        // Startup inside a repo opens that project's board, so quick-add starts
        // scoped to it; cycling moves to the next option (the desk).
        assert_eq!(
            model.form_scope(),
            Some(&TaskScope::Project {
                path: "/repos/chosen".into()
            }),
            "quick-add inherits the invocation project as its destination"
        );
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::FormCycleScope,
            Some(&snapshot),
        )
        .expect("cycle scope");
        assert_eq!(model.form_scope(), Some(&TaskScope::Global));

        let outcome = apply_board_intent_with_save_recovery(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardSaveContext {
                baseline: DomainState::new(),
                intent: BoardIntent::ConfirmEdit,
                snapshot: Some(&snapshot),
            },
            |working| store.save(working).map_err(|error| error.to_string()),
        )
        .expect("failed save enters recovery");
        assert_eq!(outcome, IntentOutcome::None);
        assert!(recovery.is_pending());
        assert!(
            model.board_form_open(),
            "recovery retains the expanded form"
        );

        apply_board_intent_with_save_recovery(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardSaveContext {
                baseline: DomainState::new(),
                intent: BoardIntent::CancelSave,
                snapshot: None,
            },
            |_| panic!("CancelSave must not persist"),
        )
        .expect("cancel failed save");

        assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
        assert_eq!(model.quick_add_title_value(), "Retained title");
        assert!(
            model.board_form_open(),
            "complete expanded draft remains stashed"
        );
        assert_eq!(
            model.form_scope(),
            Some(&TaskScope::Global),
            "the stashed draft keeps the scope the user chose"
        );
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ExpandQuickAdd,
            Some(&snapshot),
        )
        .expect("reopen retained draft");
        assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
        assert_eq!(model.edit_buffer(), "retained notes");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirmed_expanded_quick_add_clears_the_form_and_selects_the_new_task() {
        let temp = TempStore::new("confirmed-expanded-quick-add");
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: Some("Expanded saved task".into()),
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery = SaveRecovery::new();

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenCapture,
            Some(&snapshot),
        )
        .expect("open quick add");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ExpandQuickAdd,
            Some(&snapshot),
        )
        .expect("expand quick add");
        let outcome = apply_board_intent_with_save_recovery(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardSaveContext {
                baseline: DomainState::new(),
                intent: BoardIntent::ConfirmEdit,
                snapshot: Some(&snapshot),
            },
            |working| temp.store.save(working).map_err(|error| error.to_string()),
        )
        .expect("save expanded quick add");

        let id = domain.tasks()[0].id;
        assert_eq!(outcome, IntentOutcome::Persisted);
        assert!(!recovery.is_pending());
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert!(!model.board_form_open());
        assert_eq!(model.selected_id(), Some(id));
    }

    /// Quick capture (prefix+c) seeds the popup with the expanded quick-add page and the
    /// cursor in Title, snapshot defaults intact. The board's `+` then `Tab` keeps its
    /// Notes focus; the popup opens on the title so a name can be typed immediately.
    #[test]
    fn quick_capture_seeds_the_expanded_draft_page_with_snapshot_defaults() {
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: Some("Popup draft".into()),
            provenance: ProvenanceOrigin::Selection,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        seed_quick_capture(&mut domain, &mut model, &snapshot);

        assert_eq!(model.quick_add_title_value(), "Popup draft");
        assert_eq!(model.form_focus(), Some(CaptureField::Title));
        assert_eq!(
            model.input_mode(),
            BoardInputMode::EditTitle,
            "the popup opens with the cursor in the title"
        );
        assert!(model.board_form_open(), "the expanded page owns the popup");
        assert!(!quick_capture_finished(&model), "the session is live");
        assert!(domain.tasks().is_empty(), "seeding never creates a task");
    }

    /// The popup frame is the task-page takeover painting the expanded draft, not the
    /// legacy capture form card.
    #[test]
    fn quick_capture_popup_paints_the_task_page_takeover() {
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: Some("Popup paint".into()),
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        seed_quick_capture(&mut domain, &mut model, &snapshot);

        // Also cover a smaller 50x16 viewport above the compact board's
        // 40x10 operable floor, so the takeover stays readable there.
        let width = 50u16;
        let height = 16u16;
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                crate::ui::board::draw_board(frame, &model);
            })
            .expect("draw popup frame");
        let buffer = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer.cell((x, y)).expect("cell").symbol())
                    .collect()
            })
            .collect();
        let frame_text = rows.join("\n");
        assert!(
            frame_text.contains("+ step"),
            "expanded page must paint its step target"
        );
        assert!(
            frame_text.contains("Popup paint"),
            "the prefill paints on the page: {frame_text}"
        );
        assert!(
            frame_text.contains("desk"),
            "the snapshot's default destination paints: {frame_text}"
        );
    }

    #[test]
    fn quick_capture_save_persists_and_ends_the_popup_session() {
        let temp = TempStore::new("quick-capture-popup-save");
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: Some("Popup saved task".into()),
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery = SaveRecovery::new();
        seed_quick_capture(&mut domain, &mut model, &snapshot);

        let outcome = apply_board_intent_with_save_recovery(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardSaveContext {
                baseline: DomainState::new(),
                intent: BoardIntent::ConfirmEdit,
                snapshot: Some(&snapshot),
            },
            |working| {
                temp.store
                    .reload_merge_save(working)
                    .map_err(|e| e.to_string())
            },
        )
        .expect("save the popup draft");

        assert_eq!(outcome, IntentOutcome::Persisted);
        assert!(!recovery.is_pending());
        assert!(
            quick_capture_finished(&model),
            "a persisted save closes the popup"
        );
        assert_eq!(temp.store.load().expect("reload").tasks().len(), 1);
    }

    #[test]
    fn quick_capture_cancel_closes_without_creating_a_task() {
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: Some("Popup cancelled".into()),
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        seed_quick_capture(&mut domain, &mut model, &snapshot);

        // Esc first returns to the retained one-line draft (the board's existing flow).
        apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
            .expect("cancel the page back to the line");
        assert!(!quick_capture_finished(&model), "the line is still open");
        assert!(domain.tasks().is_empty());

        // Second Esc discards the line and ends the session.
        apply_intent(&mut domain, &mut model, BoardIntent::CancelQuickAdd, None)
            .expect("discard the draft line");
        assert!(quick_capture_finished(&model), "cancel closes the popup");
        assert!(domain.tasks().is_empty(), "cancel creates nothing");
    }

    /// The popup's Esc on the expanded draft is one press: the whole draft is discarded
    /// and the session ends, instead of the board's fallback to the retained one-line
    /// draft.
    #[test]
    fn popup_step_escape_keeps_the_parent_draft() {
        let temp = TempStore::new("popup-step-escape");
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: Some("retained title".into()),
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery = SaveRecovery::new();
        seed_quick_capture(&mut domain, &mut model, &snapshot);
        for intent in [
            BoardIntent::BeginAddStep,
            BoardIntent::EditInsertText("unsaved step".into()),
        ] {
            apply_intent(&mut domain, &mut model, intent, None).unwrap();
        }
        let intent = crate::ui::input::map_key(
            model.input_mode(),
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
            ),
        )
        .expect("Escape maps");
        let quit = handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            intent,
            &mut recovery,
            true,
        )
        .unwrap();
        assert!(!quit);
        assert!(model.expanded_capture_open());
        assert_eq!(model.quick_add_title_value(), "retained title");
        assert!(!quick_capture_finished(&model));
    }

    #[test]
    fn quick_capture_esc_on_the_expanded_draft_closes_the_popup_in_one_press() {
        let temp = TempStore::new("quick-capture-popup-esc");
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: Some("Popup esc".into()),
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery = SaveRecovery::new();
        seed_quick_capture(&mut domain, &mut model, &snapshot);

        let quit = handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::CancelEdit,
            &mut recovery,
            true,
        )
        .expect("cancel the popup draft");

        assert!(quit, "one Esc closes the popup");
        assert!(
            quick_capture_finished(&model),
            "the expanded page and the retained line are both gone"
        );
        assert!(domain.tasks().is_empty(), "Esc creates nothing");
    }

    /// The board keeps its two-press Esc: the expanded page falls back to the retained
    /// one-line draft, only the second press discards it, and no cancel quits the board.
    #[test]
    fn board_esc_from_the_expanded_page_still_returns_to_the_retained_line() {
        let temp = TempStore::new("board-expanded-esc");
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: Some("Board esc".into()),
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery = SaveRecovery::new();
        seed_quick_capture(&mut domain, &mut model, &snapshot);

        let quit = handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::CancelEdit,
            &mut recovery,
            false,
        )
        .expect("cancel the page");

        assert!(!quit);
        assert!(model.quick_add_open(), "the one-line draft is retained");
        assert!(
            model.board_form_open(),
            "the page stays stashed so Tab can restore it"
        );
        assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);

        let quit = handle_board_intent(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::CancelQuickAdd,
            &mut recovery,
            false,
        )
        .expect("discard the line");
        assert!(!quit, "a draft cancel never quits the board");
        assert!(domain.tasks().is_empty());
    }

    #[test]
    fn quick_capture_failed_save_keeps_the_editable_draft_in_recovery() {
        let snapshot = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: Some("Popup recovery".into()),
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery = SaveRecovery::new();
        seed_quick_capture(&mut domain, &mut model, &snapshot);

        let outcome = apply_board_intent_with_save_recovery(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardSaveContext {
                baseline: DomainState::new(),
                intent: BoardIntent::ConfirmEdit,
                snapshot: Some(&snapshot),
            },
            |_| Err("disk full".to_string()),
        )
        .expect("the failed save stays in the popup");

        assert_eq!(outcome, IntentOutcome::None);
        assert!(recovery.is_pending());
        assert!(
            !quick_capture_finished(&model),
            "a failed save keeps the popup"
        );
        assert!(model.board_form_open(), "the draft stays editable");

        // Cancel restores the baseline and returns to the retained line; the session is
        // still live until the draft itself is discarded.
        apply_board_intent_with_save_recovery(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardSaveContext {
                baseline: DomainState::new(),
                intent: BoardIntent::CancelSave,
                snapshot: None,
            },
            |working| unreachable!("cancelling recovery never persists: {working:?}"),
        )
        .expect("cancel the failed save");
        assert!(!recovery.is_pending());
        assert!(!quick_capture_finished(&model));
        assert!(domain.tasks().is_empty(), "the baseline has no new task");

        apply_intent(&mut domain, &mut model, BoardIntent::CancelQuickAdd, None)
            .expect("discard the draft line");
        assert!(quick_capture_finished(&model));
    }

    /// Without a snapshot, quick-add save must neither call `capture_save` nor report
    /// `Persist`. It keeps its draft and says why.
    #[test]
    fn quick_add_save_without_a_capture_snapshot_neither_saves_nor_reports_persist() {
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None)
            .expect("open quick add with no snapshot");
        assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::QuickAddInsert('x'),
            None,
        )
        .expect("type into title");

        let outcome = apply_intent(&mut domain, &mut model, BoardIntent::QuickAddSave, None)
            .expect("confirm without a snapshot");

        assert_eq!(
            outcome,
            IntentOutcome::None,
            "a snapshot-less confirm must not claim Persist"
        );
        assert!(domain.tasks().is_empty(), "nothing was ever saved");
        assert_eq!(
            model.input_mode(),
            BoardInputMode::QuickAdd,
            "quick add stays open so the draft is not lost"
        );
        assert_eq!(
            model.quick_add_title_value(),
            "x",
            "the draft the user typed must survive the refusal"
        );
        assert!(
            model.message().is_some(),
            "the refusal must say why, not silently no-op"
        );
    }

    /// Area below the 50x18 floor: the board paints resize guidance here.
    const RESIZE_AREA: Rect = Rect {
        x: 0,
        y: 0,
        width: 49,
        height: 18,
    };

    /// Host offering two eligible park sources, so Park opens the ambiguity picker.
    fn board_with_one_task() -> (DomainState, BoardModel) {
        let mut domain = DomainState::new();
        domain
            .create(
                "Behind an open surface",
                None,
                TaskScope::Project {
                    path: "/repos/app".into(),
                },
                ProvenanceOrigin::Capture,
                None,
            )
            .expect("create task");
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/app")));
        model.set_selected_project(Some(PathBuf::from("/repos/app")));
        (domain, model)
    }

    /// Every board state whose input mode is a modal surface, each already open when the
    /// pane shrinks below the resize floor.
    #[allow(clippy::type_complexity)]
    #[test]
    fn task_page_view_mode_routes_keys_through_the_page_keymap_not_the_form_field_map() {
        let (mut domain, mut model) = board_with_one_task();
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        assert!(model.board_form_open(), "the page keeps its form open");

        for (key, expected) in [
            (
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
                BoardIntent::BeginEditTitle,
            ),
            (
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
                BoardIntent::SetStatus(HumanStatus::Ready),
            ),
            (
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                BoardIntent::Complete,
            ),
            (
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                BoardIntent::PrimaryVerb,
            ),
        ] {
            assert_eq!(
                board_keyboard_intent(&model, BoardInputMode::TaskPage, key),
                Some(expected.clone()),
                "page view mode must route {key:?} through the page keymap"
            );
        }

        // A focused field hands back to the form field map: bare characters insert.
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("enter title edit");
        assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
        assert_eq!(
            board_keyboard_intent(
                &model,
                BoardInputMode::EditTitle,
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)
            ),
            Some(BoardIntent::EditInsert('e'))
        );
    }

    #[test]
    fn expanded_quick_add_tabs_past_the_selected_step_target() {
        let mut domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None)
            .expect("open quick add");
        apply_intent(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None)
            .expect("expand quick add");
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
            .expect("Tab from Notes selects + step");
        assert_eq!(model.input_mode(), BoardInputMode::CapturePage);

        let intent = board_keyboard_intent(
            &model,
            model.input_mode(),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        assert_eq!(
            intent,
            Some(BoardIntent::FormFocusNext),
            "Tab on expanded capture's + step must reach Thread"
        );
        apply_intent(&mut domain, &mut model, intent.expect("Tab intent"), None)
            .expect("advance past + step");
        assert_eq!(model.input_mode(), BoardInputMode::EditThread);
    }

    #[test]
    fn expanded_quick_add_step_selection_cannot_reach_the_hidden_board() {
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "behind the draft",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create hidden board task");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None)
            .expect("open quick add");
        apply_intent(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None)
            .expect("expand quick add");
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
            .expect("select + step");
        apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None)
            .expect("open first step");
        for text in ["first", "second"] {
            for character in text.chars() {
                apply_intent(
                    &mut domain,
                    &mut model,
                    BoardIntent::EditInsert(character),
                    None,
                )
                .expect("type step");
            }
            apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
                .expect("stage step and open next");
        }
        apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
            .expect("close empty next step");
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
            .expect("select first staged step");
        assert_eq!(model.input_mode(), BoardInputMode::CapturePage);
        for key in [
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ] {
            let intent = board_keyboard_intent(&model, model.input_mode(), key);
            if let Some(intent) = intent {
                apply_intent(&mut domain, &mut model, intent, None).expect("capture owns key");
            }
            assert!(
                model.capture_draft_open(),
                "{key:?} keeps the capture draft"
            );
            assert_ne!(
                domain
                    .tasks()
                    .iter()
                    .find(|task| task.id == id)
                    .expect("hidden task")
                    .status,
                HumanStatus::Done,
                "{key:?} does not mutate hidden task"
            );
        }
    }

    /// A form can remain allocated while a popup or view owns the resolved input mode. The
    /// keyboard must hand keys to the form only in its genuine field/dropdown modes, never just
    /// because `form_focus()` is present.
    #[test]
    fn board_keyboard_uses_the_form_mapper_only_for_form_field_and_dropdown_modes() {
        let (mut domain, mut model) = board_with_one_task();
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("open task form");
        assert!(model.board_form_open());

        for mode in [BoardInputMode::EditTitle, BoardInputMode::EditNotes] {
            assert_eq!(
                board_keyboard_intent(
                    &model,
                    mode,
                    KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
                ),
                Some(BoardIntent::FormFocusNext),
                "{mode:?} must route through the shared form mapper"
            );
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::FocusFormField(CaptureField::Scope),
            None,
        )
        .expect("focus scope field");
        assert_eq!(
            board_keyboard_intent(
                &model,
                BoardInputMode::EditScope,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            Some(BoardIntent::FormCycleScope),
            "EditScope must route through the shared form mapper"
        );
        assert_eq!(
            board_keyboard_intent(
                &model,
                BoardInputMode::FormScopeDropdown,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)
            ),
            Some(BoardIntent::FormScopeNext),
            "the form scope dropdown must route through the shared form mapper"
        );

        // These modes may overlay an open form, but their own map must win. Each key is one the
        // form mapper would handle differently, so this is an allowlist regression guard rather
        // than a test of identical fallthroughs.
        for (mode, code, expected) in [
            (BoardInputMode::Normal, KeyCode::Char('r'), None),
            (
                BoardInputMode::TaskPage,
                KeyCode::Char('e'),
                Some(BoardIntent::BeginEditTitle),
            ),
            (
                BoardInputMode::ProjectPicker,
                KeyCode::Char('q'),
                Some(BoardIntent::CancelProjectPicker),
            ),
            (
                BoardInputMode::SaveRecovery,
                KeyCode::Char('r'),
                Some(BoardIntent::RetrySave),
            ),
            (
                BoardInputMode::Palette,
                KeyCode::Char('r'),
                Some(BoardIntent::CommandQueryInsert('r')),
            ),
            (
                BoardInputMode::Help,
                KeyCode::Char('r'),
                Some(BoardIntent::HelpQueryInsert('r')),
            ),
            (
                BoardInputMode::Help,
                KeyCode::Esc,
                Some(BoardIntent::CloseLayer),
            ),
        ] {
            let mods = if mode == BoardInputMode::TaskPage {
                KeyModifiers::CONTROL
            } else {
                KeyModifiers::NONE
            };
            assert_eq!(
                board_keyboard_intent(&model, mode, KeyEvent::new(code, mods)),
                expected,
                "{mode:?} must not be swallowed by the open form"
            );
        }
    }

    #[test]
    fn mark_mode_does_not_shadow_text_entry_or_save_recovery_keys() {
        let (mut domain, mut model) = board_with_one_task();
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
            .expect("enter mark mode");
        apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None)
            .expect("open quick add while marks remain available");
        assert!(model.mark_mode_active());
        assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
        assert_eq!(
            board_keyboard_intent(
                &model,
                model.input_mode(),
                KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT)
            ),
            Some(BoardIntent::QuickAddInsert('M'))
        );

        model.begin_save_recovery("injected save failure");
        assert_eq!(
            board_keyboard_intent(
                &model,
                model.input_mode(),
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Some(BoardIntent::CancelSave),
            "save recovery must remain escapable while mark mode is retained"
        );
    }

    /// SaveRecovery outranks an open form, so Retry and both Cancel keys must retain the only
    /// routes that can resolve a failed save.
    #[test]
    fn save_recovery_with_an_open_form_routes_r_c_and_esc_to_its_own_mapper() {
        let (mut domain, mut model) = board_with_one_task();
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("open task form");
        model.begin_save_recovery("injected save failure");
        assert!(
            model.board_form_open(),
            "save recovery retains the failed form"
        );
        assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);

        for (code, expected) in [
            (KeyCode::Char('r'), BoardIntent::RetrySave),
            (KeyCode::Char('c'), BoardIntent::CancelSave),
            (KeyCode::Esc, BoardIntent::CancelSave),
        ] {
            assert_eq!(
                board_keyboard_intent(
                    &model,
                    model.input_mode(),
                    KeyEvent::new(code, KeyModifiers::NONE)
                ),
                Some(expected),
                "SaveRecovery must own {code:?} while a form stays open"
            );
        }
    }

    /// Bare Enter on the task page becomes `ToggleStep` only while a stored step is
    /// selected. Resolved here, at the keyboard boundary, so the persisting intent is
    /// classified before the save baseline loads; everywhere else Enter keeps its route.
    #[test]
    fn bare_enter_on_a_stored_step_resolves_to_toggle_step_at_the_keyboard_boundary() {
        use crate::ui::board::apply_intent;

        let (mut domain, mut model) = board_fixture("Stepped", None);
        let id = model.selected_id().expect("task");
        domain.add_step(id, "alpha").expect("step");
        domain.add_step(id, "bravo").expect("step");
        model.sync_from_domain(&domain);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        // Board: Enter opens the page.
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, enter),
            Some(BoardIntent::OpenTaskPage)
        );
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

        // Page with no step selected: Enter is still the page route.
        assert!(!model.stored_step_selected());
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::TaskPage, enter),
            Some(BoardIntent::OpenTaskPage)
        );

        // Tab selects the first stored step: Enter now toggles it.
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("tab");
        assert!(model.stored_step_selected());
        let intent = board_keyboard_intent(&model, BoardInputMode::TaskPage, enter)
            .expect("enter on a step");
        assert_eq!(intent, BoardIntent::ToggleStep);
        assert!(board_intent_may_persist(&intent));
        apply_intent(&mut domain, &mut model, intent, None).expect("toggle");
        assert!(domain.get(id).expect("task").steps[0].done, "alpha toggled");
        assert_eq!(
            domain.get(id).expect("task").status,
            HumanStatus::Open,
            "task status untouched"
        );

        // On the trailing `+ step` target Enter keeps its add route (not a toggle).
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("bravo");
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("+ step");
        assert!(!model.stored_step_selected());
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::TaskPage, enter),
            Some(BoardIntent::OpenTaskPage)
        );

        // Shift+Enter never becomes a toggle.
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("wrap");
        assert!(model.stored_step_selected());
        assert_ne!(
            board_keyboard_intent(
                &model,
                BoardInputMode::TaskPage,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
            ),
            Some(BoardIntent::ToggleStep)
        );
    }

    #[test]
    fn navigation_digits_route_from_project_focus_and_preserve_input_interception() {
        let (mut domain, _) = board_fixture("digits", None);
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/alpha")));
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::ProjectBoard),
            None,
        )
        .expect("project board");
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, key(KeyCode::Char('1'))),
            Some(BoardIntent::SelectNavTab(NavTab::Desk))
        );
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, key(KeyCode::Char('2'))),
            Some(BoardIntent::SelectNavTab(NavTab::ProjectBoard))
        );
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, key(KeyCode::Char('3'))),
            Some(BoardIntent::SelectNavTab(NavTab::Projects))
        );
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("edit");
        assert!(matches!(
            board_keyboard_intent(&model, BoardInputMode::EditTitle, key(KeyCode::Char('1'))),
            Some(BoardIntent::EditInsert('1'))
        ));
    }

    #[test]
    fn projects_search_owns_bound_keys_paste_and_mouse_focus() {
        let (mut domain, _) = board_fixture("search", None);
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/beta")));
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("projects index");
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let hits = board_hit_map(Rect::new(0, 0, 80, 24), &model);
        let search = hits
            .regions
            .iter()
            .find(|hit| hit.target == crate::ui::render::QueueHitTarget::Verb(1))
            .expect("closed footer search affordance");
        assert!(
            search.area.y >= 20,
            "projects search belongs in the footer slot, got hit at y={}",
            search.area.y
        );
        assert_eq!(
            map_board_mouse(&model, &hits, click_at(search.area)),
            Some(BoardIntent::FocusSearch)
        );
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, key(KeyCode::Char('/'))),
            Some(BoardIntent::FocusSearch)
        );
        // Normal mode does not own an implicit query. Bound and unbound letters
        // retain their ordinary routes or are inert until slash/footer search focus.
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, key(KeyCode::Char('e'))),
            None
        );
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, key(KeyCode::Char('w'))),
            None
        );
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, key(KeyCode::Backspace)),
            None
        );
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, key(KeyCode::Char('v'))),
            Some(BoardIntent::OpenProjectsViewPicker)
        );
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Normal, key(KeyCode::Char('3'))),
            Some(BoardIntent::SelectNavTab(NavTab::Projects))
        );
        apply_intent(&mut domain, &mut model, BoardIntent::FocusSearch, None)
            .expect("focus search");
        let compact_hits = board_hit_map(Rect::new(0, 0, 40, 10), &model);
        let compact_search = compact_hits
            .regions
            .iter()
            .find(|hit| hit.target == crate::ui::render::QueueHitTarget::Search)
            .expect("compact footer search input");
        assert!(
            compact_search.area.y >= 7,
            "compact search must stay in the footer slot"
        );
        for c in ['b', 'e', 't', 'a', 'j', 'k', 'v', '1', '2', '3'] {
            assert_eq!(
                board_keyboard_intent(&model, BoardInputMode::Search, key(KeyCode::Char(c))),
                Some(BoardIntent::SearchQueryInsert(c)),
                "bound search character should be text: {c}"
            );
        }
        let intent = board_paste_intent(Rect::new(0, 0, 80, 24), &mut model, "beta")
            .expect("paste search query");
        assert_eq!(intent, BoardIntent::SearchQueryInsertText("beta".into()));
        apply_intent(&mut domain, &mut model, intent, None).expect("insert query");
        assert_eq!(model.search_query(), "beta");
        assert_eq!(model.project_rows().len(), 1);
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Search, key(KeyCode::Backspace)),
            Some(BoardIntent::SearchQueryBackspace)
        );
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Search, key(KeyCode::Enter)),
            Some(BoardIntent::PinSearch)
        );
        let hits = board_hit_map(Rect::new(0, 0, 80, 24), &model);
        let search = hits
            .regions
            .iter()
            .find(|hit| hit.target == crate::ui::render::QueueHitTarget::Search)
            .expect("open footer search input");
        assert!(
            search.area.y >= 20,
            "open projects search must remain in the footer slot, got y={}",
            search.area.y
        );
        assert_eq!(
            map_board_mouse(&model, &hits, click_at(search.area)),
            Some(BoardIntent::FocusSearch)
        );
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("clear search");
        assert_eq!(model.search_query(), "");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        apply_intent(&mut domain, &mut model, BoardIntent::FocusSearch, None)
            .expect("refocus search");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SearchQueryInsertText("beta".into()),
            None,
        )
        .expect("restore query");
        apply_intent(&mut domain, &mut model, BoardIntent::PinSearch, None)
            .expect("pin selected search match");
        assert_eq!(model.nav_tab(), NavTab::Projects);
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert!(model.search_pinned());
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
            .expect("open selected search match");
        assert_eq!(model.nav_tab(), NavTab::ProjectBoard);
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert_eq!(model.search_query(), "");
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::Search, key(KeyCode::Esc)),
            Some(BoardIntent::CloseLayer)
        );
    }

    #[test]
    fn search_filters_every_task_surface_and_enter_pins_the_query() {
        use crate::ui::board::apply_intent;

        let mut domain = DomainState::new();
        let login = domain
            .create(
                "Ship login flow",
                Some("wire the form".into()),
                TaskScope::Project {
                    path: "/repos/app".into(),
                },
                ProvenanceOrigin::Manual,
                Some("auth".into()),
            )
            .expect("login task");
        domain
            .set_status(login, HumanStatus::Started)
            .expect("start login task");
        domain.add_step(login, "cover redirect").expect("step");
        let other = domain
            .create(
                "Unrelated work",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("other task");
        domain
            .set_status(other, HumanStatus::Started)
            .expect("start other task");
        let mut model = BoardModel::from_domain(&domain, None);

        apply_intent(&mut domain, &mut model, BoardIntent::FocusSearch, None)
            .expect("focus search from desk");
        assert_eq!(model.input_mode(), BoardInputMode::Search);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SearchQueryInsertText("   \t".into()),
            None,
        )
        .expect("type whitespace search");
        apply_intent(&mut domain, &mut model, BoardIntent::PinSearch, None)
            .expect("close whitespace search");
        assert_eq!(model.search_query(), "");
        assert!(model.root_escape_requests_quit());
        apply_intent(&mut domain, &mut model, BoardIntent::FocusSearch, None)
            .expect("refocus content search");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SearchQueryInsertText("login auth redirect".into()),
            None,
        )
        .expect("type task search");
        assert_eq!(model.visible_ids(), vec![login]);
        assert_eq!(model.queue_view().counts.in_motion, 1);

        apply_intent(&mut domain, &mut model, BoardIntent::PinSearch, None).expect("pin search");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert!(model.search_pinned());
        assert_eq!(model.search_query(), "login auth redirect");
        assert_eq!(model.selected_id(), Some(login));

        let arriving = domain
            .create(
                "Login follow-up",
                Some("auth redirect".into()),
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("arriving matching task");
        domain
            .set_status(arriving, HumanStatus::Started)
            .expect("start arriving task");
        model.sync_from_domain(&domain);
        assert_eq!(model.selected_id(), Some(login));
        assert!(model.visible_ids().contains(&arriving));

        assert_eq!(
            apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
                .expect("clear pinned search"),
            IntentOutcome::None
        );
        assert_eq!(model.search_query(), "");
        assert!(!model.search_pinned());
        let restored = model.visible_ids();
        assert_eq!(restored.len(), 3);
        assert!(
            restored.contains(&login) && restored.contains(&other) && restored.contains(&arriving)
        );

        apply_intent(&mut domain, &mut model, BoardIntent::FocusSearch, None)
            .expect("search again");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SearchQueryInsertText("login".into()),
            None,
        )
        .expect("restore task query");
        apply_intent(&mut domain, &mut model, BoardIntent::PinSearch, None)
            .expect("pin restored task query");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("switch tabs");
        assert_eq!(model.search_query(), "");
        assert!(!model.search_pinned());
    }

    #[test]
    fn pinned_search_clears_before_a_task_page_closes() {
        use crate::ui::board::apply_intent;

        let mut domain = DomainState::new();
        let id = domain
            .create(
                "matching task",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Capture,
                None,
            )
            .expect("create matching task");
        domain
            .set_status(id, HumanStatus::Ready)
            .expect("ready matching task");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut model, BoardIntent::FocusSearch, None)
            .expect("focus search");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SearchQueryInsertText("matching".into()),
            None,
        )
        .expect("filter task rows");
        apply_intent(&mut domain, &mut model, BoardIntent::PinSearch, None).expect("pin search");
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
            .expect("open matching task");
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
            .expect("clear pinned search from task page");
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        assert_eq!(model.search_query(), "");
        assert!(!model.search_pinned());
    }

    #[test]
    fn task_row_click_during_search_selects_without_clearing_the_query() {
        use crate::ui::board::apply_intent;

        let mut domain = DomainState::new();
        let id = domain
            .create(
                "matching row",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Capture,
                None,
            )
            .expect("create matching task");
        domain
            .set_status(id, HumanStatus::Ready)
            .expect("ready matching task");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut model, BoardIntent::FocusSearch, None)
            .expect("focus search");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SearchQueryInsertText("matching".into()),
            None,
        )
        .expect("filter task rows");
        let hits = board_hit_map(Rect::new(0, 0, 80, 24), &model);
        let task = hits
            .regions
            .iter()
            .find(|hit| hit.target == crate::ui::render::QueueHitTarget::Task(id))
            .expect("matching task hit");

        assert_eq!(
            map_board_mouse(&model, &hits, click_at(task.area)),
            Some(BoardIntent::SelectIndex(0))
        );
        assert_eq!(model.search_query(), "matching");
        assert_eq!(model.input_mode(), BoardInputMode::Search);
    }

    #[test]
    fn a_board_paste_routes_to_the_edit_buffer_and_is_inert_outside_an_edit_mode() {
        let area = Rect::new(0, 0, 120, 40);
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "Original",
                None,
                TaskScope::Project {
                    path: "/repos/app".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create task");
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/app")));
        assert_eq!(model.selected_id(), Some(id));

        // Normal board: a paste is not an edit and changes nothing.
        assert_eq!(board_paste_intent(area, &mut model, "pasted"), None);
        assert_eq!(model.edit_buffer(), "");
        assert_eq!(domain.get(id).expect("task").title, "Original");

        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("begin title edit");
        let intent =
            board_paste_intent(area, &mut model, "one\ntwo").expect("a paste in an edit mode");
        assert_eq!(intent, BoardIntent::EditInsertText("one\ntwo".to_string()));
        apply_intent(&mut domain, &mut model, intent, None).expect("insert the paste");
        assert!(
            model.edit_buffer().contains("one"),
            "the paste must land in the draft: {:?}",
            model.edit_buffer()
        );

        // the queue overlay paints an open editor as a full-screen takeover at every
        // size, so a paste it accepted before the pane shrank still lands at the
        // legacy Resize band and below.
        let intent = board_paste_intent(RESIZE_AREA, &mut model, "more")
            .expect("a paste still reaches the open editor down to 40x10");
        assert_eq!(intent, BoardIntent::EditInsertText("more".to_string()));
    }

    /// pasting into the open palette narrows the query exactly as typing it would.
    ///
    /// Bracketed paste routes the payload to `Event::Paste`, so without this route the
    /// palette search would silently swallow a paste that worked before the protocol was on.
    #[test]
    fn a_board_paste_into_the_open_palette_narrows_the_query_like_typing() {
        let area = Rect::new(0, 0, 120, 40);
        let mut domain = DomainState::new();
        domain
            .create(
                "Original",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create task");

        let open_palette = |model: &mut BoardModel, domain: &mut DomainState| {
            apply_intent(domain, model, BoardIntent::OpenCommandPalette, None)
                .expect("open the palette");
        };

        // Typed baseline: the same characters entered one key press at a time.
        let mut typed = BoardModel::from_domain(&domain, None);
        open_palette(&mut typed, &mut domain);
        for character in "park".chars() {
            apply_intent(
                &mut domain,
                &mut typed,
                BoardIntent::CommandQueryInsert(character),
                None,
            )
            .expect("type into the query");
        }

        let mut pasted = BoardModel::from_domain(&domain, None);
        open_palette(&mut pasted, &mut domain);
        let intent = board_paste_intent(area, &mut pasted, "park").expect("a paste in the palette");
        apply_intent(&mut domain, &mut pasted, intent, None).expect("insert the paste");

        assert_eq!(pasted.command_query(), typed.command_query());
        assert_eq!(
            pasted
                .visible_commands()
                .iter()
                .map(|command| command.label)
                .collect::<Vec<_>>(),
            typed
                .visible_commands()
                .iter()
                .map(|command| command.label)
                .collect::<Vec<_>>(),
            "a paste must narrow the palette exactly as typing the same run does"
        );

        // The query is one search line, so each break folds to a single space (CRLF included).
        let mut broken = BoardModel::from_domain(&domain, None);
        open_palette(&mut broken, &mut domain);
        let intent =
            board_paste_intent(area, &mut broken, "a\r\nb").expect("a paste in the palette");
        apply_intent(&mut domain, &mut broken, intent, None).expect("insert the paste");
        assert_eq!(broken.command_query(), "a b");
    }

    /// pasting a project path while the scope path editor is active extends the path.
    ///
    /// This is the most likely paste in the product; before bracketed paste it arrived as key
    /// presses and worked, so the paste route must keep it working.
    #[test]
    fn a_capture_paste_extends_the_scope_path_while_the_path_editor_is_active() {
        use crate::ui::capture::{CaptureField, CaptureScopeChoice};
        use crate::ui::input::CaptureIntent;

        let snap = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: None,
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::SelectScope(CaptureScopeChoice::Other),
        )
        .expect("begin the path edit");
        assert_eq!(model.focused(), CaptureField::Scope);
        assert!(model.is_path_editing());

        let intent =
            map_capture_paste_state(model.focused(), model.is_path_editing(), "/repos/app")
                .expect("a paste into the path");
        apply_capture_intent(&mut domain, None, &snap, &mut model, intent)
            .expect("insert the paste");
        assert_eq!(model.scope_path_edit(), Some("/repos/app"));

        // The path is one line: a pasted break folds to a single space, CRLF included.
        let intent = map_capture_paste_state(model.focused(), model.is_path_editing(), "\r\nmore")
            .expect("a paste into the path");
        apply_capture_intent(&mut domain, None, &snap, &mut model, intent)
            .expect("insert the paste");
        assert_eq!(model.scope_path_edit(), Some("/repos/app more"));

        // Scope focus without an active path editor is not a text field: inert, as before.
        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::FocusField(CaptureField::Title),
        )
        .expect("leave the path edit");
        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::FocusField(CaptureField::Scope),
        )
        .expect("focus scope");
        assert!(!model.is_path_editing());
        assert_eq!(
            map_capture_paste_state(model.focused(), model.is_path_editing(), "ignored"),
            None
        );
    }

    /// a Capture paste reaches the focused text field, and nowhere else.
    #[test]
    fn a_capture_paste_routes_to_the_focused_text_field_and_is_inert_elsewhere() {
        use crate::ui::capture::CaptureField;
        use crate::ui::input::CaptureIntent;

        let snap = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: None,
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        assert_eq!(model.focused(), CaptureField::Title);

        let intent = map_capture_paste_state(model.focused(), model.is_path_editing(), "one\ntwo")
            .expect("a paste into Title");
        assert_eq!(intent, CaptureIntent::InsertText("one\ntwo".to_string()));
        apply_capture_intent(&mut domain, None, &snap, &mut model, intent)
            .expect("insert the paste");
        // Title is a single-line field:'s insert flattens the pasted newline.
        assert_eq!(model.title(), "one two");

        // The scope row is not a text field: a paste there changes nothing.
        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::FocusField(CaptureField::Scope),
        )
        .expect("focus scope");
        assert_eq!(
            map_capture_paste_state(model.focused(), model.is_path_editing(), "ignored"),
            None
        );
        assert_eq!(model.title(), "one two");
    }

    /// A failed save owns the form until Retry or Cancel, so a paste must be inert too.
    #[test]
    fn a_capture_paste_is_inert_while_a_save_failure_is_unresolved() {
        use std::fs;

        use crate::ui::capture::CaptureField;
        use crate::ui::input::CaptureIntent;

        let dir = temp_state_dir("capture-paste-recovery");
        // A plain file where the state directory should be: every save attempt fails.
        let blocked = dir.join("not-a-directory");
        fs::write(&blocked, "not a state directory").expect("blocking file");
        let store = TaskStore::new(&blocked);

        let snap = InvocationSnapshot {
            default_scope: TaskScope::Global,
            this_repo: None,
            title_prefill: None,
            provenance: ProvenanceOrigin::Capture,
        };
        let mut domain = DomainState::new();
        let mut model = CaptureModel::from_snapshot(&snap);
        apply_capture_intent(
            &mut domain,
            None,
            &snap,
            &mut model,
            CaptureIntent::InsertText("Pending".to_string()),
        )
        .expect("seed a title");
        apply_capture_intent(
            &mut domain,
            Some(&store),
            &snap,
            &mut model,
            CaptureIntent::Save,
        )
        .expect("a save failure stays on the form");
        assert!(model.is_save_recovery());
        assert_eq!(model.focused(), CaptureField::Title);

        let intent = map_capture_paste_state(model.focused(), model.is_path_editing(), "pasted")
            .expect("the paste still routes to the focused field");
        apply_capture_intent(&mut domain, Some(&store), &snap, &mut model, intent)
            .expect("recovery answers the paste without editing");
        assert_eq!(
            model.title(),
            "Pending",
            "recovery accepts only Retry or Cancel, by key or by paste"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Temp state dir for one open-refresh test.
    fn temp_state_dir(tag: &str) -> PathBuf {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = TEMP_DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("tsk-{tag}-{nanos}-{seq}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// One task linked to `w0:p1`, saved to a fresh store.
    #[test]
    fn refused_title_edit_keeps_its_draft_and_cursor_and_says_a_title_is_required() {
        for draft in ["", "   "] {
            let (mut domain, mut model) = board_with_one_task();
            let id = domain.tasks()[0].id;
            let before = domain.get(id).expect("task").clone();

            apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
                .expect("open the title edit");
            for _ in 0..before.title.chars().count() {
                apply_intent(&mut domain, &mut model, BoardIntent::EditBackspace, None)
                    .expect("clear the seeded title");
            }
            for character in draft.chars() {
                apply_intent(
                    &mut domain,
                    &mut model,
                    BoardIntent::EditInsert(character),
                    None,
                )
                .expect("type the draft");
            }
            // Leave the cursor somewhere other than the end, so "unchanged" is a real claim.
            apply_intent(&mut domain, &mut model, BoardIntent::EditMoveLeft, None)
                .expect("move the cursor");
            let cursor = model.edit_cursor();

            let mut recovery = SaveRecovery::new();
            let outcome = apply_board_intent_presenting_rejection(
                &mut domain,
                &mut model,
                &mut recovery,
                BoardSaveContext {
                    baseline: DomainState::new(),
                    intent: BoardIntent::ConfirmEdit,
                    snapshot: None,
                },
                |_| panic!("a refused edit must never persist"),
            );

            assert_eq!(outcome, IntentOutcome::None, "draft {draft:?}");
            assert_eq!(
                model.input_mode(),
                BoardInputMode::EditTitle,
                "draft {draft:?}: the field must stay open"
            );
            assert_eq!(
                model.edit_buffer(),
                draft,
                "draft {draft:?}: the typed text must survive the refusal"
            );
            assert_eq!(
                model.edit_cursor(),
                cursor,
                "draft {draft:?}: the cursor must not jump"
            );
            assert_eq!(
                model.message(),
                Some(TITLE_REQUIRED_MESSAGE),
                "draft {draft:?}: the refusal must explain itself"
            );
            assert_eq!(
                domain.get(id).expect("task"),
                &before,
                "draft {draft:?}: a refused edit changes no task"
            );
        }
    }

    /// `ConfirmEdit` reads the durable record for its availability verdict and is **not**
    /// merged from it.
    ///
    /// The distinction is the whole design, and neither half is safe to leave implicit. Adding
    /// `ConfirmEdit` to the merge set is the obvious-looking way to satisfy decision 8 and it
    /// silently defeats same-task save-conflict detection (measured: see
    /// `a_concurrent_edit_to_the_bound_task_still_reaches_the_save_conflict` in
    /// `tests/edit_target_binding.rs`). Dropping the verdict entirely puts the stale-snapshot
    /// hole back. This pins both sides.
    #[test]
    fn a_refusals_line_is_readable_and_never_a_uuid_dump() {
        let id = uuid::Uuid::from_u128(7);

        assert_eq!(
            board_rejection_message(&DomainError::EmptyTitle),
            TITLE_REQUIRED_MESSAGE,
            "both surfaces state the same refusal in the same words"
        );
        for error in [DomainError::UnknownId(id), DomainError::SoftDeleted(id)] {
            let line = board_rejection_message(&error);
            assert!(
                !line.contains(&id.to_string()),
                "{error:?} reported an id the user cannot act on: {line:?}"
            );
            assert!(
                line.len() <= 40,
                "{error:?} needs more than a narrow board row: {line:?}"
            );
        }
        // Unmapped: the domain's own words, but never nothing.
        let unmapped = DomainError::StaleUndo(id);
        assert_eq!(
            board_rejection_message(&unmapped),
            unmapped.to_string(),
            "a refusal this boundary has never seen must still be explained"
        );
    }
}

#[cfg(test)]
mod projects_view_tests {
    use super::{apply_intent, BoardIntent, DomainState, NavTab};
    use crate::domain::ProvenanceOrigin;
    use std::path::PathBuf;

    /// The projects index's View selector replaces the index with one thread's flat
    /// cross-project task board.
    #[test]
    fn projects_view_thread_renders_cross_project_matches() {
        use crate::ui::queue::SectionKind;

        let mut domain = DomainState::new();
        for (title, path) in [
            ("alpha release task", "/repos/alpha"),
            ("beta release task", "/repos/beta"),
        ] {
            domain
                .create(
                    title,
                    None,
                    crate::domain::TaskScope::Project { path: path.into() },
                    ProvenanceOrigin::Manual,
                    Some("release".into()),
                )
                .expect("create threaded task");
        }
        let mut model =
            crate::ui::board::BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/alpha")));
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("projects index");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenProjectsViewPicker,
            None,
        )
        .expect("open view picker");
        assert!(model.list_picker_open(), "the View picker opens");
        // The picker's first row is Overview; move once to the first thread, then confirm.
        apply_intent(&mut domain, &mut model, BoardIntent::ListPickerNext, None)
            .expect("move to first thread");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ConfirmListPicker,
            None,
        )
        .expect("apply thread view");

        let view = model.queue_view();
        assert!(
            view.projects.is_empty(),
            "the thread view replaces the project index rows"
        );
        let listed: Vec<_> = view
            .sections
            .iter()
            .flat_map(|section| section.task_ids.iter().copied())
            .collect();
        assert_eq!(listed.len(), 2, "both projects' release tasks list flat");
        assert!(
            view.sections
                .iter()
                .all(|section| section.project_label.is_none()),
            "no thread/project nesting"
        );
        let _ = SectionKind::NeedsYou;
    }
}
