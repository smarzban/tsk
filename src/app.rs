//! Application entry: mode select, load store/context, run Board or Capture UI.

use std::env;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::DefaultTerminal;

use crate::agents::AgentProfiles;
use crate::context::{build_snapshot, InvocationSnapshot, RawHostContext};
use crate::dispatch::{
    self, BranchCleanup, CleanupError, CleanupResult, DispatchError, DispatchHost, DispatchResult,
    SystemDispatchHost, WorktreeCleanup,
};
use crate::domain::{DomainError, DomainState, HumanStatus};
use crate::save_recovery::SaveRecovery;
use crate::store::{default_state_dir, StoreSignature, TaskStore};
use crate::ui::board::{
    apply_intent, board_intent_may_persist, draw_board, resolve_board_command, BoardInputMode,
    BoardModel, BulkCleanup, CleanupPrompt, CleanupRow, CleanupRowState, CleanupRun, CleanupRunRow,
    IntentOutcome, SaveResolution,
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

/// The invocation snapshot, its default destination rewritten to the stored project it
/// aliases (`/tmp/repo` for a launch from `/private/tmp/repo`). Board open, the quick-capture
/// popup, and the board's quick add all take their snapshot from here.
pub fn load_snapshot(state: &DomainState) -> InvocationSnapshot {
    let raw = RawHostContext::from_env();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut snapshot = build_snapshot(&raw, cwd);
    crate::scope::adopt_stored_identity(&mut snapshot, state);
    snapshot
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
        seed_full_board_files_without_blocking_open(&store);
        seed_notices_without_blocking_open(&store);
    }
    let state = store.load()?;
    let snapshot = load_snapshot(&state);
    let mut model = BoardModel::from_domain_for_snapshot(&state, &snapshot);
    match crate::agents::AgentProfiles::load(&state_dir) {
        Ok(profiles) => model.set_agent_profiles(&profiles),
        Err(error) => model.set_message(error.to_string()),
    }
    if full_board_open {
        model.offer_launch_card(&state, &snapshot);
    }
    model.set_update_notice(crate::update::startup(
        &state_dir,
        env!("CARGO_PKG_VERSION"),
    ));
    Ok((store, state, model))
}

fn seed_full_board_files_without_blocking_open(store: &TaskStore) {
    let _ = crate::agents::seed_on_open(store.path());
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
    model.poll_base_picker_results();
    model.poll_cleanup_check();
    model.poll_dispatch_checks();
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
    let snapshot = load_snapshot(&domain);
    seed_quick_capture(&mut domain, &mut model, &snapshot);
    run_board_loop(store, domain, model, true)
}

fn run_board() -> Result<(), Box<dyn Error>> {
    let (store, domain, model) = load_board()?;
    crate::git_base::remember_fetches_in(store.path());
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
            // Apply completed branch discovery and cleanup checks only on the board thread,
            // before painting.
            board_background_step(
                &store,
                &mut domain,
                &mut model,
                &mut save_recovery,
                dispatch::running_inside_herdr(),
                &mut board_dispatch_host(&store),
                &mut |naming| drop(dispatch::spawn_agent_naming(naming)),
            )?;
            if model.quit_after_cleanup_due() {
                break;
            }
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
                | BoardInputMode::CleanupConfirm
                | BoardInputMode::CleanupDirtyConfirm
                | BoardInputMode::DispatchConfirm
                | BoardInputMode::BlockCard
                | BoardInputMode::EditReply
        );
        // A list picker's or bulk cleanup card's Esc is its own cancel: clearing marks
        // underneath would leave it open, still bound to the set it captured.
        let surface_owns_escape = matches!(
            mode,
            BoardInputMode::ListPicker
                | BoardInputMode::CleanupConfirm
                | BoardInputMode::CleanupDirtyConfirm
                | BoardInputMode::DispatchConfirm
                | BoardInputMode::BlockCard
                | BoardInputMode::EditReply
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
            && !surface_owns_escape
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
            | BoardInputMode::SelectBase
            | BoardInputMode::EditScope
            | BoardInputMode::EditAssignee
            | BoardInputMode::FormDropdown
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
    // Bare Enter on a block option opens the reply box prefilled with it.
    if mode == BoardInputMode::TaskPage
        && key.code == KeyCode::Enter
        && key.modifiers.is_empty()
        && model.block_option_selected()
    {
        return Some(BoardIntent::ReplyWithOption);
    }
    // Bare Enter on a review check cycles it; on the `N passed` line it folds or unfolds them.
    if mode == BoardInputMode::TaskPage && key.code == KeyCode::Enter && key.modifiers.is_empty() {
        if model.review_check_selected() {
            return Some(BoardIntent::CycleCheck);
        }
        if model.passed_checks_selected() {
            return Some(BoardIntent::TogglePassedChecks);
        }
        if model.trail_record_selected() {
            return Some(BoardIntent::ToggleTrailRecord(None));
        }
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
        Some(focus) => map_task_form_key(focus, mode == BoardInputMode::FormDropdown, key),
        None => map_key(mode, key),
    }
}

/// Resolve a failed save on the board that owns it, which need not be the board the user
/// answered from: a project preview parked by a narrowing frame keeps its held form and
/// drafts while the visible board shows a proxy of its banner (and the reverse after the
/// frame widens). Owners unwind their session; proxies only take the banner down.
fn resolve_save_recovery(model: &mut BoardModel, domain: &DomainState, resolution: SaveResolution) {
    let seat_owns = model
        .preview_seat_mut()
        .is_some_and(|seat| seat.owns_save_recovery());
    // A board answering with no other owner in sight owns it.
    let outer_owns =
        model.owns_save_recovery() || (!seat_owns && !model.shows_save_recovery_proxy());
    let retried = resolution == SaveResolution::Retried;
    let before_sync = |board: &mut BoardModel| {
        if retried {
            board.release_task_edit_save();
        }
    };
    let after_sync = |board: &mut BoardModel| {
        board.finish_form_assignee_sync(retried);
        board.finish_form_base_sync(retried);
        let cancelled_quick_add = board.end_save_recovery(resolution);
        if retried && !board.has_saved_task() {
            board.set_message("saved");
        } else if !retried && !cancelled_quick_add {
            board.set_message("save cancelled");
        }
    };
    let proxy_message = if retried { "saved" } else { "save cancelled" };
    if outer_owns {
        before_sync(model);
    }
    if seat_owns {
        before_sync(model.preview_seat_mut().expect("seat owns recovery"));
    }
    // The outer sync carries the preview seat with it.
    model.sync_from_domain(domain);
    if outer_owns {
        after_sync(model);
    } else if model.shows_save_recovery_proxy() {
        model.end_proxy_save_recovery();
        model.set_message(proxy_message);
    }
    if let Some(seat) = model.preview_seat_mut() {
        if seat_owns {
            after_sync(seat);
        } else if seat.shows_save_recovery_proxy() {
            seat.end_proxy_save_recovery();
            seat.set_message(proxy_message);
        }
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
                    resolve_save_recovery(model, domain, SaveResolution::Retried);
                    return Ok(IntentOutcome::Persisted);
                }
                model.begin_save_recovery(recovery.error().unwrap_or("save failed"));
                return Ok(IntentOutcome::None);
            }
            BoardIntent::CancelSave => {
                model.close_command_surface();
                *domain = recovery.cancel().expect("pending recovery has a baseline");
                resolve_save_recovery(model, domain, SaveResolution::Cancelled);
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
        fail_board_save(domain, model, recovery, baseline, error);
        return Ok(IntentOutcome::None);
    }
    model.release_task_edit_save();
    model.sync_from_domain(domain);
    model.finish_form_assignee_sync(true);
    model.finish_form_base_sync(true);
    model.finish_form_after_sync(true);
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
    matches!(
        intent,
        BoardIntent::Undo
            | BoardIntent::DispatchAgain
            | BoardIntent::ConfirmDispatch
            | BoardIntent::StartWithoutRelaunch
            | BoardIntent::ConfirmCleanup
            | BoardIntent::KeepCleanup
    )
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
    // A palette Retry or Cancel is the same answer as the key: resolve it on the board whose
    // palette chose it, then route it like the key below.
    let palette_recovery = matches!(
        intent,
        BoardIntent::ConfirmCommand | BoardIntent::SelectCommand(_)
    ) && board_intent_target_mut(model, route.target)
        .selected_command_for(&intent)
        .is_some_and(|command| {
            matches!(
                command.intent,
                BoardIntent::RetrySave | BoardIntent::CancelSave
            )
        });
    let intent = if palette_recovery {
        match resolve_board_command(board_intent_target_mut(model, route.target), intent) {
            Some(resolved) => resolved,
            None => return Ok(false),
        }
    } else {
        intent
    };
    // Quit belongs to the whole application, even when a focused preview supplied it.
    // Its guard must see both the outer parked form and the nested preview's draft.
    // Retry and Cancel resolve the failed save on whichever board owns it, so they start from
    // the outer board, which reaches its preview seat whether or not that seat has focus.
    let target = if matches!(
        intent,
        BoardIntent::Quit | BoardIntent::RetrySave | BoardIntent::CancelSave
    ) {
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

/// Dispatch the task that was under the cursor when the verb was invoked. Marks are cleared and
/// never become targets, even if a refresh moves the visible cursor before host work begins.
pub fn dispatch_task_with_host(
    domain: &mut DomainState,
    model: &mut BoardModel,
    id: uuid::Uuid,
    profiles: &AgentProfiles,
    again: bool,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<DispatchResult, DispatchError> {
    model.clear_marks();
    dispatch::run_with_host(domain, id, profiles, again, in_herdr, host)
}

pub use crate::ui::board::CLEANUP_BUSY;

// Returned once per `ctrl+d`; boxing the converged result would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOffer {
    None,
    Prompted,
    /// A confirmed cleanup still runs: nothing opens and nothing completes.
    Busy,
    MissingConverged(CleanupResult),
}

/// Offer cleanup only when one cursor task would newly become done. A missing recorded
/// worktree converges the retained dispatch and completion for one save without opening a popup.
pub fn offer_cleanup_prompt_with_host(
    domain: &mut DomainState,
    model: &mut BoardModel,
    id: uuid::Uuid,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<CleanupOffer, CleanupError> {
    if model.bulk_verb_active() || model.focus_is_archived() {
        return Ok(CleanupOffer::None);
    }
    let Some(task) = domain.get(id) else {
        return Ok(CleanupOffer::None);
    };
    if task.archived || task.status == HumanStatus::Done {
        return Ok(CleanupOffer::None);
    }
    let Some(record) = task.dispatch.as_ref() else {
        return Ok(CleanupOffer::None);
    };
    if record.cleaned {
        return Ok(CleanupOffer::None);
    }
    if model.cleanup_running() {
        return Ok(CleanupOffer::Busy);
    }
    // Decide on the background check before the snapshot, as the base picker does: a fetch
    // that lands between the two then still has a check to fill the card, rather than a
    // fresh answer sitting over pre-fetch ancestry with nothing left to refresh it.
    let merge_check = begin_cleanup_merge_check(task, record, host);
    // Cached refs only: the card opens at once and the background check fills merged status.
    let preview = dispatch::inspect_cleanup_cached_with_host(domain, id, in_herdr, host)?;
    if !preview.inspection.worktree_exists {
        let workspace = dispatch::untouched_workspace(in_herdr, &preview.inspection);
        domain
            .record_dispatch_cleaned(id, crate::domain::CleanupOutcome::Missing)
            .and_then(|()| domain.complete_after_cleanup(id))
            .map_err(|error| CleanupError::Store(error.to_string()))?;
        return Ok(CleanupOffer::MissingConverged(CleanupResult {
            remote: None,
            warning: preview.inspection.warning,
            branch_reason: Some(dispatch::BranchRetentionReason::MissingWorktree),
            number: preview.number,
            title: preview.title,
            worktree_path: preview.record.worktree,
            branch_name: preview.record.branch,
            base: preview.record.base.or(preview.record.base_ref),
            workspace_id: preview.record.herdr_workspace_id,
            worktree: WorktreeCleanup::Missing,
            branch: BranchCleanup::Kept,
            workspace,
        }));
    }
    model.begin_cleanup_prompt(CleanupPrompt::single(cleanup_row(id, preview, merge_check)));
    Ok(CleanupOffer::Prompted)
}

/// Start a dispatched task's background merged check, when it has a recorded base to check.
fn begin_cleanup_merge_check(
    task: &crate::domain::Task,
    record: &crate::domain::Dispatch,
    host: &mut impl DispatchHost,
) -> Option<dispatch::MergeCheck> {
    match (
        &task.scope,
        record.base.is_some() || record.base_ref.is_some(),
    ) {
        (crate::domain::TaskScope::Project { path }, true) => {
            host.begin_merge_check(std::path::Path::new(path), record)
        }
        _ => None,
    }
}

fn cleanup_row(
    id: uuid::Uuid,
    preview: dispatch::CleanupPreview,
    merge_check: Option<dispatch::MergeCheck>,
) -> CleanupRow {
    CleanupRow {
        merge_check,
        check_failed: false,
        unreachable_remote: preview.inspection.unreachable_remote.clone(),
        inspected: Some(preview.record.clone()),
        task_id: id,
        number: preview.number,
        worktree: preview.record.worktree,
        branch: preview.record.branch,
        base: preview
            .record
            .base
            .or(preview.record.base_ref)
            .unwrap_or_else(|| "unknown".to_string()),
        dirty: preview.inspection.dirty,
        branch_merged: preview.inspection.branch_merged,
        base_available: preview.inspection.base_available,
        warning: preview.inspection.warning,
        workspace_exists: preview.inspection.workspace_exists,
    }
}

/// A board save failed: keep the working state aside and show save recovery for it.
fn fail_board_save(
    domain: &mut DomainState,
    model: &mut BoardModel,
    recovery: &mut SaveRecovery<DomainState>,
    baseline: DomainState,
    error: String,
) {
    let working = std::mem::take(domain);
    recovery.fail(baseline, working, error);
    model.begin_save_recovery(recovery.error().unwrap_or("save failed"));
}

/// Whether the task still carries the dispatch its cleanup row inspected. Another board or the
/// CLI may have cleaned or relaunched it while the card was open; cleanup then must not touch
/// the new worktree or agent.
fn dispatch_unchanged(
    domain: &DomainState,
    task_id: uuid::Uuid,
    inspected: Option<&crate::domain::Dispatch>,
) -> bool {
    inspected.is_some() && domain.get(task_id).and_then(|task| task.dispatch.as_ref()) == inspected
}

/// What a bulk `ctrl+d` on a marked set did before the reducer's plain batch completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BulkCleanupOffer {
    /// No target has a live dispatch: complete the set as a plain batch, no card.
    None,
    /// The card is open over the marked set, which stays marked until `y` or `n`.
    Prompted,
    /// A confirmed cleanup still runs: nothing opens and nothing completes.
    Busy,
    /// Every live dispatch's worktree was already gone: they converged to cleaned and the
    /// whole set completed in this transaction, still one undo entry. The caller saves.
    MissingConverged { done: usize, missing: usize },
}

/// Bulk `ctrl+d`: when the marked set holds tasks with a live dispatch, open one card listing
/// each of them (cached refs plus a background merged check per task). Tasks without one are
/// listed as just marked done. Nothing is mutated while the card is open.
pub fn offer_bulk_cleanup_prompt_with_host(
    domain: &mut DomainState,
    model: &mut BoardModel,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<BulkCleanupOffer, DomainError> {
    if !model.bulk_verb_active() || model.focus_is_archived() {
        return Ok(BulkCleanupOffer::None);
    }
    // The tasks this verb would newly complete, in the reducer's target order.
    let targets = model
        .marked_ids()
        .iter()
        .copied()
        .filter(|id| {
            domain
                .get(*id)
                .is_some_and(|task| task.status != HumanStatus::Done)
        })
        .collect::<Vec<_>>();
    let live = |task: &crate::domain::Task| {
        !task.archived
            && matches!(task.scope, crate::domain::TaskScope::Project { .. })
            && task.dispatch.as_ref().is_some_and(|record| !record.cleaned)
    };
    if !targets.iter().any(|id| domain.get(*id).is_some_and(&live)) {
        return Ok(BulkCleanupOffer::None);
    }
    if model.cleanup_running() {
        return Ok(BulkCleanupOffer::Busy);
    }
    let mut rows = Vec::new();
    let mut bulk = BulkCleanup {
        targets: targets.clone(),
        ..BulkCleanup::default()
    };
    for &id in &targets {
        let task = domain.get(id).expect("targets exist");
        let identifier = task
            .board_identifier()
            .unwrap_or_else(|| "new task".to_string());
        let Some(record) = task.dispatch.as_ref().filter(|_| live(task)) else {
            bulk.plain.push(identifier);
            continue;
        };
        // Check before the snapshot, as the single card does, so a fetch landing between the
        // two still has a check left to refresh the row.
        let merge_check = begin_cleanup_merge_check(task, record, host);
        match dispatch::inspect_cleanup_cached_with_host(domain, id, in_herdr, host) {
            Ok(preview) if !preview.inspection.worktree_exists => {
                bulk.missing.push((id, identifier, preview.record));
            }
            Ok(preview) => rows.push(cleanup_row(id, preview, merge_check)),
            Err(error) => bulk.refused.push((identifier, error.reason())),
        }
    }
    if rows.is_empty() && bulk.refused.is_empty() {
        let missing = bulk.missing.len();
        for (id, _, _) in &bulk.missing {
            domain.record_dispatch_cleaned(*id, crate::domain::CleanupOutcome::Missing)?;
        }
        domain.complete_batch_after_cleanup(&targets)?;
        model.clear_marks();
        return Ok(BulkCleanupOffer::MissingConverged {
            done: targets.len(),
            missing,
        });
    }
    model.begin_cleanup_prompt(CleanupPrompt {
        rows,
        bulk: Some(bulk),
        scroll: 0,
    });
    Ok(BulkCleanupOffer::Prompted)
}

/// What a cleanup card's `y` or `n` did to the domain, before its one save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupConfirmed {
    /// `y`: the run that reports the card's rows. Its worker starts after the save, once
    /// every merged check landed ([`poll_cleanup_runs`]).
    pub run: Option<CleanupRun>,
    /// `n`: the status-row outcome.
    pub message: Option<String>,
}

/// Apply a cleanup card's choice to the domain: complete the card's task, or the whole marked
/// set as one batch and one undo entry, and converge worktrees that were already gone. With
/// `y`, also plan each cleanable row for the worker. Completion never waits on a merged
/// check: it is saved at once, and the worker starts later ([`poll_cleanup_runs`]), so the
/// event loop never waits on a removal. A row kept here (uncommitted changes, changed since
/// the card opened) never stops the others.
pub fn confirm_cleanup_with_host(
    domain: &mut DomainState,
    model: &mut BoardModel,
    clean: bool,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<Option<CleanupConfirmed>, DomainError> {
    let Some(prompt) = model.cleanup_prompt().cloned() else {
        return Ok(None);
    };
    let targets = match &prompt.bulk {
        // A target another actor removed since the card opened is dropped before any host
        // work, so a refused batch can never follow a cleanup.
        Some(bulk) => bulk
            .targets
            .iter()
            .copied()
            .filter(|id| domain.get(*id).is_some())
            .collect::<Vec<_>>(),
        None => match prompt.rows.first() {
            Some(row) => vec![row.task_id],
            None => return Ok(None),
        },
    };
    let mut plan = Vec::new();
    let mut waiting = Vec::new();
    let mut rows = Vec::new();
    for row in prompt
        .rows
        .iter()
        .filter(|row| clean && targets.contains(&row.task_id))
    {
        let kept = |error: CleanupError| CleanupRowState::kept(&error);
        let state = if row.dirty {
            Some(kept(CleanupError::DirtyWorktree))
        } else if !dispatch_unchanged(domain, row.task_id, row.inspected.as_ref()) {
            Some(kept(CleanupError::DispatchChanged))
        } else {
            // The refs are settled when the worker starts, from this row's merged check.
            match dispatch::cleanup_plan(domain, row.task_id, dispatch::CleanupRefs::Unconfirmed) {
                Ok(planned) => {
                    plan.push(planned);
                    waiting.push(row.clone());
                    None
                }
                Err(error) => Some(kept(error)),
            }
        };
        let state = state.unwrap_or(if row.checking() {
            CleanupRowState::Checking
        } else {
            CleanupRowState::Queued
        });
        rows.push(CleanupRunRow {
            task_id: row.task_id,
            number: row.number,
            slot: matches!(state, CleanupRowState::Checking | CleanupRowState::Queued)
                .then(|| plan.len() - 1),
            inspected: row.inspected.clone(),
            state,
        });
    }
    let mut missing = 0;
    if let Some(bulk) = &prompt.bulk {
        for (id, _, inspected) in &bulk.missing {
            // The card's snapshot may be stale: another board can have relaunched this task
            // since. Converge only the dispatch that was inspected, and only while its
            // worktree is still gone; a relaunched or reappeared worktree is left live.
            let same_dispatch = domain
                .get(*id)
                .and_then(|task| task.dispatch.as_ref())
                .is_some_and(|record| record == inspected);
            let still_missing = same_dispatch
                && dispatch::inspect_cleanup_cached_with_host(domain, *id, in_herdr, host)
                    .is_ok_and(|preview| !preview.inspection.worktree_exists);
            if still_missing {
                domain.record_dispatch_cleaned(*id, crate::domain::CleanupOutcome::Missing)?;
                missing += 1;
            }
        }
    }
    let done = targets
        .iter()
        .filter(|id| {
            domain
                .get(**id)
                .is_some_and(|task| task.status != HumanStatus::Done)
        })
        .count();
    if prompt.bulk.is_some() {
        domain.complete_batch_after_cleanup(&targets)?;
        model.clear_marks();
    } else {
        domain.complete_after_cleanup(targets[0])?;
    }
    if !clean {
        model.close_popup();
        let message = match prompt.bulk {
            Some(_) if missing > 0 => {
                format!("done {done} · worktrees kept · {missing} already gone")
            }
            Some(_) => format!("done {done} · worktrees kept"),
            None => format!(
                "done T{} · worktree kept",
                prompt.rows.first().map_or(0, |row| row.number)
            ),
        };
        return Ok(Some(CleanupConfirmed {
            run: None,
            message: Some(message),
        }));
    }
    // The run owns the merged checks from here: the card's copies stop competing for them.
    if let Some(prompt) = model.cleanup_prompt_mut() {
        for row in &mut prompt.rows {
            row.merge_check = None;
        }
    }
    let refused = prompt
        .bulk
        .as_ref()
        .map(|bulk| bulk.refused.clone())
        .unwrap_or_default();
    let run = CleanupRun {
        job: dispatch::CleanupJob::new(plan.len()),
        rows,
        bulk: prompt.bulk.as_ref().map(|_| (done, missing)),
        refused,
        settled: false,
        waiting,
        plan,
        start_deadline: Instant::now() + dispatch::MERGE_CHECK_TIMEOUT,
        started: false,
    };
    Ok(Some(CleanupConfirmed {
        run: Some(run),
        message: None,
    }))
}

/// Start the run's worker once its merged checks landed (or their bound passed), cleaning each
/// row with the refs its check confirmed: an unfinished, failed or offline check keeps the
/// branch. Rows whose task's dispatch changed since `y` are withdrawn before any host work.
fn start_due_cleanup(
    store: &TaskStore,
    domain: &DomainState,
    model: &mut BoardModel,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) {
    let (job, plan) = {
        let Some(mut run) = model.cleanup_run_mut() else {
            return;
        };
        if run.started || !run.poll_checks() {
            return;
        }
        let job = dispatch::CleanupJob::new(run.plan.len()).bound_to_store(store.path());
        let run = &mut *run;
        for (planned, row) in run.plan.iter_mut().zip(&run.waiting) {
            planned.refs = row.cleanup_refs();
        }
        for row in &mut run.rows {
            if row.state == CleanupRowState::Checking {
                row.state = CleanupRowState::Queued;
            }
            if let Some(slot) = row.slot {
                if !dispatch_unchanged(domain, row.task_id, row.inspected.as_ref()) {
                    job.cancel(slot);
                }
            }
        }
        run.job = job.clone();
        run.started = true;
        (job, run.plan.clone())
    };
    if !plan.is_empty() {
        host.begin_cleanup(job, plan, in_herdr);
    }
}

/// One board-loop step for the running cleanup (one slot, shared with the project preview,
/// so a dropped or rebound preview never loses it): start the worker when due, withdraw rows
/// whose dispatch changed, apply landed rows (mark the dispatch cleaned while it is still the
/// one inspected, then save), and keep the status row current. A finished run posts its
/// summary and goes, unless its card is open with something kept: then the card shows why
/// until Esc. Waits for an unresolved failed save, like every other background step.
pub fn poll_cleanup_runs(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    save_recovery: &mut SaveRecovery<DomainState>,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) {
    if !model.cleanup_running() {
        model.set_cleanup_status(None);
        model.drop_cleanup_refusals();
        return;
    }
    if save_recovery.is_pending() {
        return;
    }
    start_due_cleanup(store, domain, model, in_herdr, host);
    // A preview's card counts only while the preview is painted and focused: a parked one
    // (narrowed frame) is invisible and unreachable, so its run reports on the visible board.
    let preview_focused = model.project_right_seat_focused();
    let card_open = model.cleanup_card_open()
        || (preview_focused
            && model
                .preview_seat_mut()
                .is_some_and(|seat| seat.cleanup_card_open()));
    let mut baseline = None;
    let mut mutated = false;
    let (finished, clean, progress) = {
        let Some(mut run) = model.cleanup_run_mut() else {
            return;
        };
        if run.started {
            if let Some((slots, settled)) = run.job.snapshot() {
                let job = run.job.clone();
                for row in &mut run.rows {
                    if row.state.landed() {
                        continue;
                    }
                    let Some(index) = row.slot else {
                        continue;
                    };
                    match slots.get(index) {
                        Some(dispatch::CleanupSlot::Queued) => {
                            // Not reached yet: withdraw it if its dispatch changed meanwhile.
                            if !dispatch_unchanged(domain, row.task_id, row.inspected.as_ref()) {
                                job.cancel(index);
                            }
                        }
                        Some(dispatch::CleanupSlot::Running) => {
                            row.state = CleanupRowState::Removing;
                        }
                        Some(dispatch::CleanupSlot::Done(Ok(result))) => {
                            if dispatch_unchanged(domain, row.task_id, row.inspected.as_ref()) {
                                // The pre-mutation state, for save recovery.
                                baseline.get_or_insert_with(|| domain.clone());
                                if domain
                                    .record_dispatch_cleaned_keeping_undo(
                                        row.task_id,
                                        result.outcome(),
                                    )
                                    .is_ok()
                                {
                                    mutated = true;
                                }
                            }
                            row.state = CleanupRowState::Cleaned {
                                branch_kept: result.branch_reason.map(|reason| {
                                    (
                                        reason.short().to_string(),
                                        reason.message(
                                            result.base.as_deref(),
                                            result.remote.as_deref(),
                                        ),
                                    )
                                }),
                            };
                        }
                        Some(dispatch::CleanupSlot::Done(Err(error))) => {
                            row.state = CleanupRowState::kept(error);
                        }
                        None => {}
                    }
                }
                run.settled = settled;
            }
        }
        (run.finished(), run.summary().1, run.progress_message())
    };
    if mutated {
        if let Err(error) = store.reload_merge_save(domain) {
            fail_board_save(
                domain,
                model,
                save_recovery,
                baseline.unwrap_or_default(),
                error.to_string(),
            );
            return;
        }
        model.sync_from_domain(domain);
    }
    if finished && (!card_open || clean) {
        if let Some((summary, clean)) = model.finish_cleanup_run() {
            model.post_cleanup_summary(summary, clean);
        }
        return;
    }
    let status = if model.quitting_after_cleanup() {
        Some(crate::ui::board::FINISHING_CLEANUP.to_string())
    } else if !card_open && !finished {
        Some(progress)
    } else {
        None
    };
    model.set_cleanup_status(status);
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// One board-loop step for cleanup cards: apply landed merged checks to an open card (this
/// board's or its project preview's), then advance the running cleanup. `run_board_loop`
/// calls this every iteration, before paint.
pub fn finish_queued_cleanup_with_host(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    save_recovery: &mut SaveRecovery<DomainState>,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> io::Result<()> {
    model.poll_cleanup_check();
    model.present_save_recovery(save_recovery.error());
    poll_cleanup_runs(store, domain, model, save_recovery, in_herdr, host);
    model.present_save_recovery(save_recovery.error());
    Ok(())
}

/// The board's single-task launch: `ctrl+s` on an assigned task never dispatched, `y` on the
/// relaunch card (`again`), or the palette's **dispatch again**. Launches `target` through
/// the real host and saves its record with `started`. A start that moved the status is
/// undoable: `ctrl+u` restores the status and leaves the agent running. A refusal or a
/// failed launch changes nothing and says why on the status row.
#[allow(clippy::too_many_arguments)]
/// Start what the unsaved completions in `domain` released, through the start route (plain, or
/// a dispatch for an assigned task never dispatched). Call it right before the save that makes
/// the done durable, so each start lands in that same save.
fn start_released(
    store: &TaskStore,
    domain: &mut DomainState,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Vec<dispatch::Released> {
    dispatch::start_released_with_host(domain, store.path(), crate::domain::OWNER, in_herdr, host)
}

/// Once the save that carried them landed: name each launched agent and say what started.
fn report_released(
    released: &[dispatch::Released],
    name_agent: &mut dyn FnMut(dispatch::AgentNaming),
) -> Option<String> {
    for released in released {
        if let dispatch::ReleasedStart::Dispatched(result) = &released.start {
            if let Some(naming) = result.naming.clone() {
                name_agent(naming);
            }
        }
    }
    dispatch::released_message(released)
}

/// `message`, then what the completion released.
fn with_released(message: String, note: Option<String>) -> String {
    match note {
        Some(note) => format!("{message} · {note}"),
        None => message,
    }
}

fn run_board_dispatch(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    save_recovery: &mut SaveRecovery<DomainState>,
    baseline: DomainState,
    target: Option<uuid::Uuid>,
    again: bool,
    in_herdr: bool,
    host: &mut impl DispatchHost,
    name_agent: &mut dyn FnMut(dispatch::AgentNaming),
) {
    model.clear_marks();
    if model.bulk_dispatch_running() {
        model.set_message(format!("{BULK_DISPATCH_RUNNING} · wait for it to finish"));
        return;
    }
    let Some(target) = target else {
        model.set_message(DispatchError::UnknownTask.to_string());
        return;
    };
    // A launch into a worktree the running cleanup has yet to remove would lose it.
    let cleaning = model.cleanup_run().is_some_and(|run| {
        run.rows
            .iter()
            .any(|row| row.task_id == target && !row.state.landed())
    });
    if cleaning {
        model.set_message(CLEANUP_BUSY);
        return;
    }
    if let Err(error) = dispatch::ensure_platform_supported() {
        model.set_message(error.to_string());
        return;
    }
    let profiles = match AgentProfiles::load(store.path()) {
        Ok(profiles) => profiles,
        Err(error) => {
            model.set_message(error.to_string());
            return;
        }
    };
    let previous = domain.get(target).map(|task| task.status);
    match dispatch::run_with_host(domain, target, &profiles, again, in_herdr, host) {
        Ok(result) => {
            if let Some(previous) = previous.filter(|status| *status != HumanStatus::Started) {
                if let Err(error) = domain.push_start_undo(target, previous) {
                    model.set_message(error.to_string());
                }
            }
            if let Err(error) = store.reload_merge_save(domain) {
                fail_board_save(domain, model, save_recovery, baseline, error.to_string());
                return;
            }
            if let Some(naming) = result.naming.clone() {
                name_agent(naming);
            }
            model.sync_from_domain(domain);
            let mut message = format!("dispatched T{} to @{}", result.number, result.assignee);
            if let Some(warning) = result.warning {
                message.push_str(" · ");
                message.push_str(&warning);
            }
            model.set_message(message);
            record_notice_dismissals_without_blocking_persist(store, domain);
        }
        Err(DispatchError::NoAssignee) => model.set_message(dispatch::BOARD_NO_ASSIGNEE),
        Err(error) => model.set_message(error.to_string()),
    }
}

/// Whether a start verb moves `task` to started: `ctrl+s` starts open and ready tasks, the
/// palette's **set status: started** any task not started yet.
fn start_moves(task: &crate::domain::Task, any_status: bool) -> bool {
    if any_status {
        task.status != HumanStatus::Started
    } else {
        matches!(task.status, HumanStatus::Open | HumanStatus::Ready)
    }
}

/// Whether a start of `task` would launch its agent: assigned, never dispatched, not done, not
/// archived, and dispatch can work here. A done task set back to started is a status
/// correction and never launches.
fn start_launches(task: &crate::domain::Task, in_herdr: bool) -> bool {
    start_assigns_a_launch(task) && dispatch::launch_unavailable(task, in_herdr).is_none()
}

/// Assigned, never dispatched, not done, not archived: a start that would launch if it could.
fn start_assigns_a_launch(task: &crate::domain::Task) -> bool {
    task.assignee.is_some()
        && task.dispatch.is_none()
        && task.status != HumanStatus::Done
        && !task.archived
}

/// Why a start of `task` launches nothing although it is assigned and never dispatched:
/// outside Herdr, or a desk task. Such a start is plain and says why.
fn start_no_launch(task: &crate::domain::Task, in_herdr: bool) -> Option<&'static str> {
    start_assigns_a_launch(task)
        .then(|| dispatch::launch_unavailable(task, in_herdr))
        .flatten()
}

/// `started · no launch: <reason> (T3, T4)` for the rows of a set that could not launch,
/// grouped by reason.
fn no_launch_rows_message(rows: &[(String, &'static str)]) -> Option<String> {
    let mut reasons: Vec<&'static str> = rows.iter().map(|(_, reason)| *reason).collect();
    reasons.dedup();
    reasons.sort_unstable();
    reasons.dedup();
    let parts = reasons
        .iter()
        .map(|reason| {
            let ids = rows
                .iter()
                .filter(|(_, row)| row == reason)
                .map(|(identifier, _)| identifier.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{reason} ({ids})")
        })
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| dispatch::no_launch_message(&parts.join(" · ")))
}

/// Route a start (`ctrl+s`, palette **set status: started**) before the reducer sees it.
/// Returns `true` when the start was handled here: it dispatched, opened the relaunch or bulk
/// card, or refused. `false` leaves it to the reducer as a plain status change.
///
/// - Unassigned, a running agent, or a done task: plain.
/// - Assigned and never dispatched: dispatch, which starts it.
/// - Dispatched and the agent is gone: the relaunch card (`y` relaunch, `n` just start).
/// - A marked set where any start would launch: the bulk card listing what launches and what
///   only starts. Dispatched tasks in a set only start; relaunching stays cursor-only.
#[allow(clippy::too_many_arguments)]
fn route_board_start(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    save_recovery: &mut SaveRecovery<DomainState>,
    baseline: &DomainState,
    any_status: bool,
    in_herdr: bool,
    host: &mut impl DispatchHost,
    name_agent: &mut dyn FnMut(dispatch::AgentNaming),
) -> bool {
    // Decide against the durable record: an assignment made elsewhere must launch. The
    // invoked targets are kept; if the refresh drops or reanchors them, the start is refused
    // rather than handed to a reducer that would read the new cursor.
    let mut targets = model.verb_target_ids();
    targets.sort();
    domain.merge_tasks_from_disk(baseline);
    model.sync_from_domain(domain);
    let mut refreshed = model.verb_target_ids();
    refreshed.sort();
    if refreshed != targets {
        model.clear_marks();
        model.set_message(START_TARGET_CHANGED);
        return true;
    }
    let launches = |task: &crate::domain::Task| {
        start_moves(task, any_status) && start_launches(task, in_herdr)
    };
    if model.bulk_verb_active() {
        // A set where nothing would launch is the plain batch, with no card and no
        // dispatch-only checks. Assigned rows that cannot launch here say why.
        if !targets
            .iter()
            .filter_map(|id| domain.get(*id))
            .any(launches)
        {
            let no_launch = targets
                .iter()
                .filter_map(|id| domain.get(*id))
                .filter(|task| start_moves(task, any_status))
                .filter_map(|task| {
                    start_no_launch(task, in_herdr).map(|reason| {
                        (
                            task.board_identifier()
                                .unwrap_or_else(|| "task".to_string()),
                            reason,
                        )
                    })
                })
                .collect::<Vec<_>>();
            let Some(message) = no_launch_rows_message(&no_launch) else {
                return false;
            };
            let corrections = targets
                .iter()
                .filter(|id| {
                    any_status
                        && domain
                            .get(**id)
                            .is_some_and(|task| task.status == HumanStatus::Done || task.archived)
                })
                .copied()
                .collect::<Vec<_>>();
            model.clear_marks();
            if start_plain_and_save(
                store,
                domain,
                model,
                save_recovery,
                baseline.clone(),
                &targets,
                &corrections,
                any_status,
                false,
                in_herdr,
            )
            .is_some()
            {
                model.set_message(message);
            }
            return true;
        }
        return open_bulk_start_card(domain, model, store, &targets, any_status, in_herdr, host);
    }
    let Some(task) = targets.first().and_then(|id| domain.get(*id)) else {
        return false;
    };
    if task.assignee.is_none()
        || !start_moves(task, any_status)
        || task.status == HumanStatus::Done
        || task.archived
    {
        return false;
    }
    match dispatch::start_route(task, crate::domain::OWNER, in_herdr, host) {
        dispatch::StartRoute::Plain => false,
        dispatch::StartRoute::NoLaunch { reason } => {
            let target = task.id;
            if start_plain_and_save(
                store,
                domain,
                model,
                save_recovery,
                baseline.clone(),
                &[target],
                &[],
                any_status,
                false,
                in_herdr,
            )
            .is_some()
            {
                model.set_message(dispatch::no_launch_message(reason));
            }
            true
        }
        dispatch::StartRoute::Dispatch => {
            let target = task.id;
            run_board_dispatch(
                store,
                domain,
                model,
                save_recovery,
                baseline.clone(),
                Some(target),
                false,
                in_herdr,
                host,
                name_agent,
            );
            true
        }
        dispatch::StartRoute::AgentGone { assignee } => {
            let target = task.id;
            open_relaunch_card(domain, model, target, assignee, any_status);
            true
        }
    }
}

/// Open the start-anyway card when a start (`ctrl+s`, palette **set status: started**) would
/// move a task that still waits on others: `T203 runs after T202 (started). Start anyway?`.
/// The marks stay for the replay. `y` sets [`BoardModel::start_anyway`] for one pass.
fn ask_before_starting_waiting(
    domain: &DomainState,
    model: &mut BoardModel,
    intent: &BoardIntent,
) -> bool {
    if model.start_anyway {
        return false;
    }
    let any_status = *intent != BoardIntent::PrimaryVerb;
    let waiting: Vec<String> = model
        .verb_target_ids()
        .iter()
        .filter_map(|id| domain.get(*id))
        .filter(|task| !task.is_notice() && start_moves(task, any_status))
        .filter_map(|task| domain.waiting_text(task))
        .collect();
    if waiting.is_empty() {
        return false;
    }
    model.begin_dispatch_prompt(crate::ui::board::DispatchPrompt::start_anyway(
        crate::ui::board::StartAnywayPrompt {
            waiting,
            intent: intent.clone(),
        },
    ));
    true
}

/// The status row when a refresh before a start dropped or moved its target.
const START_TARGET_CHANGED: &str = "that task changed elsewhere · nothing started";

/// Ask before relaunching `target`'s gone agent: `y` relaunches, `n` only starts.
fn open_relaunch_card(
    domain: &DomainState,
    model: &mut BoardModel,
    target: uuid::Uuid,
    assignee: String,
    any_status: bool,
) {
    let Some(task) = domain.get(target) else {
        return;
    };
    let prompt = crate::ui::board::RelaunchPrompt {
        task_id: target,
        number: task.number.unwrap_or_default(),
        any_status,
        assignee,
        worktree: task
            .dispatch
            .as_ref()
            .map(|record| record.worktree.clone())
            .unwrap_or_default(),
    };
    model.clear_marks();
    model.begin_dispatch_prompt(crate::ui::board::DispatchPrompt::relaunch(prompt));
}

/// The feedback box's `ctrl+d`: store any feedback (held until it lands, like `shift+enter`),
/// close the box, then complete the task through the cleanup card when its dispatch is live.
#[allow(clippy::too_many_arguments)]
fn approve_review(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    save_recovery: &mut SaveRecovery<DomainState>,
    id: uuid::Uuid,
    quick_capture: bool,
    in_herdr: bool,
    host: &mut impl DispatchHost,
    name_agent: &mut dyn FnMut(dispatch::AgentNaming),
) -> io::Result<()> {
    if model.cleanup_running() {
        model.set_message(CLEANUP_BUSY);
        return Ok(());
    }
    // The box approves the round it was opened on, never a newer one merged in meanwhile.
    if !model.reply_box_current(domain) {
        return Ok(());
    }
    let has_feedback = model
        .reply_draft()
        .is_some_and(|draft| !draft.trim().is_empty());
    if has_feedback {
        handle_board_intent_with_host(
            store,
            domain,
            model,
            BoardIntent::ReplySave,
            save_recovery,
            quick_capture,
            in_herdr,
            host,
            name_agent,
        )?;
        // A refused or failed save keeps the box and its draft; approve again after.
        if save_recovery.is_pending() || model.reply_task_id().is_some() {
            return Ok(());
        }
    } else {
        model.close_reply_box();
    }
    match offer_cleanup_prompt_with_host(domain, model, id, in_herdr, host) {
        Ok(CleanupOffer::Prompted) => return Ok(()),
        Ok(CleanupOffer::MissingConverged(result)) => {
            let Some(baseline) = load_baseline(store, model)? else {
                return Ok(());
            };
            let released = start_released(store, domain, in_herdr, host);
            if let Err(error) = store.reload_merge_save(domain) {
                fail_board_save(domain, model, save_recovery, baseline, error.to_string());
                return Ok(());
            }
            model.sync_from_domain(domain);
            model.set_message(with_released(
                format!("done T{} · worktree missing · branch kept", result.number),
                report_released(&released, name_agent),
            ));
            record_notice_dismissals_without_blocking_persist(store, domain);
            return Ok(());
        }
        Ok(CleanupOffer::Busy) => {
            model.set_message(CLEANUP_BUSY);
            return Ok(());
        }
        Ok(CleanupOffer::None) => {}
        Err(error) => {
            model.set_message(error.to_string());
            return Ok(());
        }
    }
    handle_board_intent_with_host(
        store,
        domain,
        model,
        BoardIntent::ApproveReview(id),
        save_recovery,
        quick_capture,
        in_herdr,
        host,
        name_agent,
    )?;
    Ok(())
}

/// How the reply box's `ctrl+s` starts its task: `None` for an unassigned (or archived) task,
/// which unblocks to ready as before.
fn reply_unblock_route(
    domain: &DomainState,
    model: &BoardModel,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Option<(uuid::Uuid, dispatch::StartRoute, dispatch::AgentCheck)> {
    let id = model.reply_task_id()?;
    let task = domain.get(id)?;
    if task.assignee.is_none() || task.archived {
        return None;
    }
    let (route, check) = dispatch::start_route_checked(task, crate::domain::OWNER, in_herdr, host);
    Some((id, route, check))
}

/// Send the reply this `ctrl+s` stored (or just the unblock, for an empty box) to `id`'s
/// dispatched agent once the start is durable, and say on the status row how it went.
/// Nothing is sent when `check` found no live dispatch.
fn deliver_reply_after_start(
    domain: &DomainState,
    model: &mut BoardModel,
    id: uuid::Uuid,
    check: &dispatch::AgentCheck,
    reply: Option<&str>,
    host: &mut impl DispatchHost,
) {
    let Some(task) = domain.get(id) else {
        return;
    };
    let Some(assignee) = task.assignee.as_deref() else {
        return;
    };
    // A send-back closed the review round this action left: it carries the failed checks.
    let sent_back = task
        .past_blocks
        .last()
        .filter(|round| round.resolution == Some(crate::domain::Resolution::SentBack));
    let (label, noun, text) = match sent_back {
        Some(round) => (
            "sent back",
            "feedback",
            dispatch::send_back_text(reply, &round.failed_checks()),
        ),
        None => ("unblocked", "reply", reply.map(str::to_string)),
    };
    if let Some(delivery) = dispatch::deliver_reply(task, check, label, text.as_deref(), host) {
        model.set_message(dispatch::delivery_message(assignee, noun, &delivery));
    }
}

/// Start `ids` as a plain status change in one save and one undo step: the relaunch card's
/// `n`, and the start-only rows of the bulk card. Each row is rechecked against the refreshed
/// board with the rule the card was opened under: a row that no longer moves (started, done,
/// archived, deleted, or out of `ctrl+s`'s open/ready) is left alone, and a row that now
/// needs a launch the card did not show (assigned meanwhile) is refused on the status row.
/// Returns the refusals, or `None` when the save failed into recovery.
#[allow(clippy::too_many_arguments)]
fn start_plain_and_save(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    save_recovery: &mut SaveRecovery<DomainState>,
    baseline: DomainState,
    ids: &[uuid::Uuid],
    corrections: &[uuid::Uuid],
    any_status: bool,
    undoable: bool,
    in_herdr: bool,
) -> Option<Vec<String>> {
    let mut moving = Vec::new();
    let mut refused = Vec::new();
    for id in ids {
        let Some(task) = domain.get(*id) else {
            continue;
        };
        // A done or archived row starts only when the card listed it as a correction; one
        // that finished or was archived after the card opened is left alone.
        let corrected = corrections.contains(id);
        if task.soft_deleted
            || ((task.archived || task.status == HumanStatus::Done) && !corrected)
            || !start_moves(task, any_status)
        {
            continue;
        }
        if start_launches(task, in_herdr) {
            refused.push(
                task.board_identifier()
                    .unwrap_or_else(|| "task".to_string()),
            );
            continue;
        }
        moving.push(*id);
    }
    if !refused.is_empty() {
        model.set_message(format!(
            "not started: {} assigned since the card opened · ctrl+s launches",
            refused.join(", ")
        ));
    }
    if moving.is_empty() {
        return Some(refused);
    }
    let started = if undoable {
        domain.start_batch(&moving)
    } else {
        moving
            .iter()
            .try_for_each(|id| domain.set_status(*id, HumanStatus::Started))
            .map(|()| true)
    };
    if let Err(error) = started {
        model.set_message(board_rejection_message(&error));
        return Some(refused);
    }
    if let Err(error) = store.reload_merge_save(domain) {
        fail_board_save(domain, model, save_recovery, baseline, error.to_string());
        return None;
    }
    model.sync_from_domain(domain);
    record_notice_dismissals_without_blocking_persist(store, domain);
    Some(refused)
}

const BULK_DISPATCH_RUNNING: &str = "a bulk dispatch is still running";

/// Why a marked task is skipped: the single-task refusal, in the card's short words.
fn bulk_dispatch_skip_reason(error: &DispatchError) -> String {
    match error {
        DispatchError::NoAssignee => "unassigned".into(),
        DispatchError::AlreadyDispatched(_) => "already dispatched (use dispatch again)".into(),
        DispatchError::NeedsGitProject => "not a project in a git repo".into(),
        DispatchError::DoneTask => "done".into(),
        DispatchError::ArchivedTask => "archived".into(),
        DispatchError::SoftDeletedTask => "deleted".into(),
        error => error.to_string(),
    }
}

/// Check every marked task with the single-task rules from task state alone (the git check runs
/// off the event loop), then return the ones that pass, in board order, and the skipped ones
/// with their reason.
fn check_marked_dispatches(
    domain: &DomainState,
    ids: impl IntoIterator<Item = uuid::Uuid>,
    profiles: &AgentProfiles,
    in_herdr: bool,
) -> (Vec<dispatch::EligibleDispatch>, Vec<(String, String)>) {
    let mut ids = ids.into_iter().collect::<Vec<_>>();
    ids.sort_by_key(|id| domain.get(*id).and_then(|task| task.number));
    let mut launch = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        match dispatch::check_task(domain, id, profiles, in_herdr) {
            Ok(eligible) => launch.push(eligible),
            Err(error) => {
                let identifier = domain
                    .get(id)
                    .and_then(|task| task.board_identifier())
                    .unwrap_or_else(|| "task".to_string());
                skipped.push((identifier, bulk_dispatch_skip_reason(&error)));
            }
        }
    }
    (launch, skipped)
}

/// `ctrl+s` on a marked set where a start would launch: one card listing what `y` launches,
/// what it only starts, and what stays unstarted with the reason. It opens at once; the
/// git-repository check runs off the event loop and its rows show `checking…` until it lands.
/// Nothing changes and the marks stay until `y`. With nothing to launch or start there is no
/// card, only the refusal. Returns `true` (the start was handled here).
#[allow(clippy::too_many_arguments)]
fn open_bulk_start_card(
    domain: &DomainState,
    model: &mut BoardModel,
    store: &TaskStore,
    targets: &[uuid::Uuid],
    any_status: bool,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> bool {
    if model.bulk_dispatch_running() {
        model.set_message(format!("{BULK_DISPATCH_RUNNING} · wait for it to finish"));
        return true;
    }
    if model.cleanup_running() {
        model.set_message(CLEANUP_BUSY);
        return true;
    }
    // The palette's absolute start corrects done and archived tasks to started, as its plain
    // batch does: they are start-only rows, never launches. `ctrl+s` leaves them out.
    let mut starting = targets
        .iter()
        .filter_map(|id| domain.get(*id))
        .filter(|task| {
            start_moves(task, any_status)
                && (any_status || (task.status != HumanStatus::Done && !task.archived))
        })
        .collect::<Vec<_>>();
    starting.sort_by_key(|task| task.number);
    let corrections = starting
        .iter()
        .filter(|task| task.status == HumanStatus::Done || task.archived)
        .map(|task| task.id)
        .collect::<Vec<_>>();
    let (launching, start_only): (Vec<_>, Vec<_>) = starting
        .into_iter()
        .partition(|task| start_launches(task, in_herdr));
    let no_launch = start_only
        .iter()
        .filter_map(|task| start_no_launch(task, in_herdr).map(|reason| (task.id, reason)))
        .collect::<Vec<_>>();
    let start_only = start_only
        .into_iter()
        .map(|task| {
            (
                task.board_identifier()
                    .unwrap_or_else(|| "task".to_string()),
                task.id,
            )
        })
        .collect::<Vec<_>>();
    let launching = launching.iter().map(|task| task.id).collect::<Vec<_>>();
    if let Err(error) = dispatch::ensure_platform_supported() {
        model.set_message(error.to_string());
        return true;
    }
    let profiles = match AgentProfiles::load(store.path()) {
        Ok(profiles) => profiles,
        Err(error) => {
            model.set_message(error.to_string());
            return true;
        }
    };
    open_bulk_dispatch_card(
        domain,
        model,
        launching,
        start_only,
        corrections,
        no_launch,
        any_status,
        &profiles,
        in_herdr,
        host,
    );
    true
}

/// The bulk start card over `launching` (assigned, never dispatched) and `start_only`.
#[allow(clippy::too_many_arguments)]
pub fn open_bulk_dispatch_card(
    domain: &DomainState,
    model: &mut BoardModel,
    launching: Vec<uuid::Uuid>,
    start_only: Vec<(String, uuid::Uuid)>,
    corrections: Vec<uuid::Uuid>,
    no_launch: Vec<(uuid::Uuid, &'static str)>,
    any_status: bool,
    profiles: &AgentProfiles,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) {
    let (launch, skipped) = check_marked_dispatches(domain, launching, profiles, in_herdr);
    if launch.is_empty() && start_only.is_empty() {
        model.set_message(crate::ui::board::nothing_to_dispatch(&skipped));
        return;
    }
    let mut projects = launch
        .iter()
        .map(|eligible| eligible.project().to_path_buf())
        .collect::<Vec<_>>();
    projects.sort();
    projects.dedup();
    let git_checks = (!projects.is_empty()).then(|| host.begin_git_checks(projects));
    model.begin_dispatch_prompt(crate::ui::board::DispatchPrompt {
        launch,
        skipped,
        start_only,
        corrections,
        no_launch,
        any_status,
        relaunch: None,
        start_anyway: None,
        git_checks,
        scroll: 0,
    });
    // A host that checked inline has its answer now.
    model.poll_dispatch_checks();
}

/// `y` on the start card. The relaunch card relaunches its one task. The bulk card starts its
/// start-only rows in one save, then rechecks each listed launch against the refreshed board
/// (another board or the CLI may have dispatched or changed it), clears the marks, and starts
/// the launches through the host. Each launch confirms its repository before creating
/// anything. Outcomes are recorded as they land ([`land_bulk_dispatch_with_host`]).
#[allow(clippy::too_many_arguments)]
fn start_bulk_dispatch(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    save_recovery: &mut SaveRecovery<DomainState>,
    baseline: DomainState,
    in_herdr: bool,
    host: &mut impl DispatchHost,
    name_agent: &mut dyn FnMut(dispatch::AgentNaming),
) -> io::Result<()> {
    let Some(prompt) = model.take_dispatch_prompt() else {
        return Ok(());
    };
    if let Some(relaunch) = prompt.relaunch {
        run_board_dispatch(
            store,
            domain,
            model,
            save_recovery,
            baseline,
            Some(relaunch.task_id),
            true,
            in_herdr,
            host,
            name_agent,
        );
        return Ok(());
    }
    let mut start_refused = Vec::new();
    if !prompt.start_only.is_empty() {
        let ids = prompt
            .start_only
            .iter()
            .map(|(_, id)| *id)
            .collect::<Vec<_>>();
        model.clear_marks();
        let before = domain.clone();
        let Some(refused) = start_plain_and_save(
            store,
            domain,
            model,
            save_recovery,
            baseline,
            &ids,
            &prompt.corrections,
            prompt.any_status,
            true,
            in_herdr,
        ) else {
            return Ok(());
        };
        if prompt.launch.is_empty() {
            let started = ids
                .iter()
                .filter(|id| {
                    domain.get(**id).map(|task| task.status)
                        != before.get(**id).map(|task| task.status)
                })
                .count();
            let mut message = format!("started {}", plural(started, "task"));
            if !refused.is_empty() {
                message.push_str(&format!(
                    " · not started: {} assigned since the card opened",
                    refused.join(", ")
                ));
            }
            model.set_message(message);
            return Ok(());
        }
        start_refused = refused;
    }
    let profiles = match AgentProfiles::load(store.path()) {
        Ok(profiles) => profiles,
        Err(error) => {
            model.set_message(error.to_string());
            return Ok(());
        }
    };
    let total = prompt.launch.len();
    let (jobs, failed) = check_marked_dispatches(
        domain,
        prompt.launch.iter().map(|eligible| eligible.id),
        &profiles,
        in_herdr,
    );
    model.clear_marks();
    if jobs.is_empty() {
        model.set_message(crate::ui::board::nothing_to_dispatch(&failed));
        return Ok(());
    }
    let mut run = crate::ui::board::BulkDispatchRun::new(host.begin_launches(jobs), total);
    run.failed = failed;
    run.failed.extend(
        start_refused
            .into_iter()
            .map(|identifier| (identifier, "assigned since the card opened".to_string())),
    );
    model.set_message(run.message());
    model.begin_bulk_dispatch(run);
    // A host that launched inline has every outcome ready now.
    land_bulk_dispatch_with_host(store, domain, model, save_recovery, name_agent)
}

/// A launch whose record is durable: count it and name its agent.
fn settle_saved_launch(
    run: &mut crate::ui::board::BulkDispatchRun,
    pending: crate::ui::board::PendingLaunch,
    name_agent: &mut dyn FnMut(dispatch::AgentNaming),
) {
    if let Some(kept) = pending.kept_status {
        run.kept_status.push((pending.identifier.clone(), kept));
    }
    run.launched.push((pending.identifier, pending.assignee));
    if let Some(naming) = pending.naming {
        name_agent(naming);
    }
}

/// Record the bulk launches that landed, one save per poll, like a single dispatch: the record
/// and `started`. Polled every board-loop iteration, wherever the batch was started: the outer
/// board and its project preview share it, so navigation never drops it.
///
/// - A launch whose task vanished or was dispatched elsewhere meanwhile is reported, never
///   recorded over. A task whose status the human changed after `y` (or archived or deleted)
///   gets its record but keeps that status, and the outcome says so.
/// - Outcomes stay on the batch while a save failure is unresolved. A failed read or save goes
///   to save recovery holding the recorded launches; a later poll settles them: kept if Retry
///   saved them, reported as launched but not recorded (with the workspace whose agent is
///   running) if Cancel discarded them. Agents are named only once their record is saved.
pub fn land_bulk_dispatch_with_host(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    save_recovery: &mut SaveRecovery<DomainState>,
    name_agent: &mut dyn FnMut(dispatch::AgentNaming),
) -> io::Result<()> {
    if save_recovery.is_pending() {
        return Ok(());
    }
    let Some(mut run) = model.take_bulk_dispatch() else {
        return Ok(());
    };
    let before = run.message();
    // A failed save left these recorded; recovery has resolved it since.
    for pending in std::mem::take(&mut run.pending) {
        let saved = domain
            .get(pending.task_id)
            .and_then(|task| task.dispatch.as_ref())
            == Some(&pending.record);
        if saved {
            settle_saved_launch(&mut run, pending, name_agent);
        } else {
            run.unrecorded
                .push((pending.identifier, pending.record.herdr_workspace_id));
        }
    }
    let landed = run.batch.take_landed();
    if !landed.is_empty() {
        // Best effort: a store that cannot be read fails the save below, into recovery.
        if let Ok(disk) = store.load() {
            domain.merge_tasks_from_disk(&disk);
        }
        let baseline = domain.clone();
        for (job, outcome) in landed {
            let identifier = format!("T{}", job.number);
            let launched = match outcome {
                Ok(launched) => launched,
                Err(error) => {
                    run.failed.push((identifier, error.to_string()));
                    continue;
                }
            };
            let worktree = launched.record.worktree.clone();
            run.warnings.extend(launched.warning.clone());
            let kept_status = match domain.get(job.id) {
                None => {
                    run.failed.push((
                        identifier,
                        format!("launched in {worktree}, but the task is gone"),
                    ));
                    continue;
                }
                Some(task) if task.dispatch.is_some() => {
                    run.failed.push((
                        identifier,
                        format!(
                            "launched in {worktree}, but it was dispatched elsewhere meanwhile"
                        ),
                    ));
                    continue;
                }
                Some(task) if task.soft_deleted => Some("deleted"),
                Some(task) if task.archived => Some("archived"),
                Some(task) if job.status_touched_since(task) => {
                    Some(crate::cli::presenter::status_name(task.status))
                }
                Some(_) => None,
            };
            let task_id = job.id;
            match dispatch::commit_launch_with_status(domain, job, launched, kept_status.is_none())
            {
                Ok(result) => run.pending.push(crate::ui::board::PendingLaunch {
                    task_id,
                    identifier,
                    assignee: result.assignee,
                    record: result.record,
                    naming: result.naming,
                    kept_status: kept_status.map(str::to_string),
                }),
                Err(error) => run.failed.push((identifier, error.to_string())),
            }
        }
        if !run.pending.is_empty() {
            if let Err(error) = store.reload_merge_save(domain) {
                let working = std::mem::take(domain);
                save_recovery.fail(baseline, working, error.to_string());
                // No board's form or draft caused this save, so none owns its recovery: the
                // banner is a proxy on the outer board and the input target, and Retry or
                // Cancel ends it without touching any open form.
                let error = save_recovery.error().unwrap_or("save failed").to_string();
                model.begin_proxy_save_recovery(&error);
                model.present_save_recovery(Some(&error));
                model.begin_bulk_dispatch(run);
                return Ok(());
            }
            model.sync_from_domain(domain);
            for pending in std::mem::take(&mut run.pending) {
                settle_saved_launch(&mut run, pending, name_agent);
            }
            record_notice_dismissals_without_blocking_persist(store, domain);
        }
    }
    let message = run.message();
    let settled = run.settled();
    if !settled {
        model.begin_bulk_dispatch(run);
    }
    if message != before || settled {
        model.input_target_mut().set_message(message);
    }
    Ok(())
}

/// Load the save baseline for a mutating intent. While a bulk dispatch is landing, a store that
/// cannot be read refuses the intent on the status row (`None`, nothing changed) instead of
/// ending the board: exiting would kill launches whose records are still to be saved.
fn load_baseline(store: &TaskStore, model: &mut BoardModel) -> io::Result<Option<DomainState>> {
    match store.load() {
        Ok(baseline) => Ok(Some(baseline)),
        Err(error) if model.bulk_dispatch_running() => {
            model.set_message(format!(
                "can't read the task store: {error} · nothing changed · {BULK_DISPATCH_RUNNING}"
            ));
            Ok(None)
        }
        Err(error) => Err(io::Error::other(error.to_string())),
    }
}

/// The board loop's background work before each paint, on the board thread: landed branch
/// lookups, queued cleanups, and bulk dispatch outcomes.
#[allow(clippy::too_many_arguments)]
fn board_background_step(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    save_recovery: &mut SaveRecovery<DomainState>,
    in_herdr: bool,
    host: &mut impl DispatchHost,
    name_agent: &mut dyn FnMut(dispatch::AgentNaming),
) -> io::Result<()> {
    model.poll_base_picker_results();
    model.poll_dispatch_checks();
    finish_queued_cleanup_with_host(store, domain, model, save_recovery, in_herdr, host)?;
    land_bulk_dispatch_with_host(store, domain, model, save_recovery, name_agent)
}

/// The real dispatch host for a board: launchers live beside the board's own store.
fn board_dispatch_host(store: &TaskStore) -> SystemDispatchHost {
    SystemDispatchHost::in_state_dir(store.path())
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
    handle_board_intent_with_host(
        store,
        domain,
        model,
        intent,
        save_recovery,
        quick_capture,
        dispatch::running_inside_herdr(),
        &mut board_dispatch_host(store),
        // Detached: the board never waits on Herdr's agent detection.
        &mut |naming| drop(dispatch::spawn_agent_naming(naming)),
    )
}

/// [`handle_board_intent`] over an explicit git/Herdr seam, so tests drive dispatch and
/// cleanup through the real boundary with a fake host.
#[allow(clippy::too_many_arguments)]
fn handle_board_intent_with_host(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
    intent: BoardIntent,
    save_recovery: &mut SaveRecovery<DomainState>,
    quick_capture: bool,
    in_herdr: bool,
    host: &mut impl DispatchHost,
    name_agent: &mut dyn FnMut(dispatch::AgentNaming),
) -> io::Result<bool> {
    let Some(intent) = resolve_board_command(model, intent) else {
        return Ok(false);
    };
    if let BoardIntent::CopyTaskNumber(id) = intent {
        copy_task_number(domain, model, id);
        return Ok(false);
    }
    let dispatch_target = if intent == BoardIntent::DispatchAgain {
        model.selected_id()
    } else {
        None
    };

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
    // Quitting mid-run would leave launched agents without their records.
    if quit_requested && model.bulk_dispatch_running() {
        model.set_message(format!("{BULK_DISPATCH_RUNNING} · quit when it finishes"));
        return Ok(false);
    }
    // A running cleanup finishes its git and Herdr steps before the board exits (bounded), so
    // no removed worktree is left without its cleaned marker. The run loop exits once due.
    if quit_requested && model.cleanup_running() {
        model.begin_quit_after_cleanup();
        return Ok(false);
    }

    // Quick capture: Esc on the expanded draft is the top-level cancel. The board's
    // collapse-to-line fallback would strand the popup on a retained one-line draft, so
    // the whole draft is discarded and the popup closes. Nested Escapes keep their own
    // semantics: an open scope dropdown maps to CancelFormDropdown, the inline step
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

    // A running batch refuses another dispatch before anything reads the store.
    if matches!(
        intent,
        BoardIntent::DispatchAgain | BoardIntent::ConfirmDispatch
    ) && !save_recovery.is_pending()
        && model.bulk_dispatch_running()
    {
        model.set_message(format!("{BULK_DISPATCH_RUNNING} · wait for it to finish"));
        return Ok(false);
    }

    let baseline = if save_recovery.is_pending() || !board_intent_may_persist(model, &intent) {
        DomainState::new()
    } else {
        match load_baseline(store, model)? {
            Some(baseline) => baseline,
            None => return Ok(false),
        }
    };

    // Do not reload while a failed save is unresolved, because only navigation plus Retry/Cancel
    // are allowed there; otherwise, bring the durable record in before the intent is decided.
    if !save_recovery.is_pending() {
        refresh_before_mutation(&intent, &baseline, domain, model);
    }

    // `ctrl+d` in the feedback box approves the review: the feedback is stored first, then the
    // task completes the way `ctrl+d` completes it, cleanup card included. Nothing is sent.
    if intent == BoardIntent::ReplyApprove && !save_recovery.is_pending() {
        let target = model.reply_task_id().filter(|id| {
            model.reply_is_feedback()
                && domain
                    .get(*id)
                    .is_some_and(|task| task.status == HumanStatus::Review)
        });
        if let Some(id) = target {
            // Approve against the durable record: a round replaced elsewhere must refuse.
            domain.merge_tasks_from_disk(&baseline);
            model.sync_from_domain(domain);
            approve_review(
                store,
                domain,
                model,
                save_recovery,
                id,
                quick_capture,
                in_herdr,
                host,
                name_agent,
            )?;
            return Ok(false);
        }
    }

    // `ctrl+d` waits for a running cleanup, whatever it targets: a marked set, a task with or
    // without a dispatch, here or in a project preview. Checked before any eligibility.
    if intent == BoardIntent::Complete && !save_recovery.is_pending() && model.cleanup_running() {
        model.set_message(CLEANUP_BUSY);
        return Ok(false);
    }

    if intent == BoardIntent::Complete && !save_recovery.is_pending() && model.bulk_verb_active() {
        match offer_bulk_cleanup_prompt_with_host(domain, model, in_herdr, host) {
            Ok(BulkCleanupOffer::Prompted) => return Ok(false),
            Ok(BulkCleanupOffer::MissingConverged { done, missing }) => {
                let released = start_released(store, domain, in_herdr, host);
                if let Err(error) = store.reload_merge_save(domain) {
                    fail_board_save(domain, model, save_recovery, baseline, error.to_string());
                    return Ok(false);
                }
                model.sync_from_domain(domain);
                model.set_message(with_released(
                    format!("done {done} · {} already gone", plural(missing, "worktree")),
                    report_released(&released, name_agent),
                ));
                record_notice_dismissals_without_blocking_persist(store, domain);
                return Ok(false);
            }
            Ok(BulkCleanupOffer::Busy) => {
                model.set_message(CLEANUP_BUSY);
                return Ok(false);
            }
            Ok(BulkCleanupOffer::None) => {}
            Err(error) => {
                model.set_message(board_rejection_message(&error));
                return Ok(false);
            }
        }
    }

    if intent == BoardIntent::Complete && !save_recovery.is_pending() {
        if let Some(target) = model.selected_id() {
            match offer_cleanup_prompt_with_host(domain, model, target, in_herdr, host) {
                Ok(CleanupOffer::Prompted) => return Ok(false),
                Ok(CleanupOffer::MissingConverged(result)) => {
                    let released = start_released(store, domain, in_herdr, host);
                    if let Err(error) = store.reload_merge_save(domain) {
                        fail_board_save(domain, model, save_recovery, baseline, error.to_string());
                        return Ok(false);
                    }
                    model.sync_from_domain(domain);
                    model.set_message(with_released(
                        format!("done T{} · worktree missing · branch kept", result.number),
                        report_released(&released, name_agent),
                    ));
                    record_notice_dismissals_without_blocking_persist(store, domain);
                    return Ok(false);
                }
                Ok(CleanupOffer::Busy) => {
                    model.set_message(CLEANUP_BUSY);
                    return Ok(false);
                }
                Ok(CleanupOffer::None) => {}
                Err(error) => model.set_message(error.to_string()),
            }
        }
    }

    if matches!(
        intent,
        BoardIntent::ConfirmCleanup | BoardIntent::KeepCleanup
    ) && !save_recovery.is_pending()
    {
        // The card already confirmed: it now reports the run, and only Esc acts on it.
        if model.cleanup_run().is_some() {
            return Ok(false);
        }
        let clean = intent == BoardIntent::ConfirmCleanup;
        let confirmed = match confirm_cleanup_with_host(domain, model, clean, in_herdr, host) {
            Ok(confirmed) => confirmed,
            Err(error) => {
                model.close_popup();
                model.set_message(board_rejection_message(&error));
                return Ok(false);
            }
        };
        // What the completion released starts in the same save.
        let released = start_released(store, domain, in_herdr, host);
        // Completion is durable before any worktree is touched.
        if let Err(error) = store.reload_merge_save(domain) {
            model.close_popup();
            fail_board_save(domain, model, save_recovery, baseline, error.to_string());
            return Ok(false);
        }
        model.sync_from_domain(domain);
        // The cleanup card owns the status row: its summary carries what started.
        model.released_note = report_released(&released, name_agent);
        match confirmed {
            Some(CleanupConfirmed { run: Some(run), .. }) => {
                model.begin_cleanup_run(run);
                poll_cleanup_runs(store, domain, model, save_recovery, in_herdr, host);
            }
            // Worktrees kept on purpose are still kept: the outcome stays until the next action.
            Some(CleanupConfirmed {
                message: Some(message),
                ..
            }) => model.post_cleanup_summary(message, false),
            _ => {}
        }
        record_notice_dismissals_without_blocking_persist(store, domain);
        return Ok(false);
    }

    // OpenCapture needs the invocation snapshot `load_board` seeded the board with
    // scope and provenance: the reducer stores it on `model.capture_snapshot` at
    // open and reads it back at ConfirmEdit, so a `None` here is what silently turned board
    // `a` into a no-op save that still reported success.
    // `y` on the start-anyway card: replay the start that asked, past the after check once.
    if intent == BoardIntent::ConfirmDispatch && !save_recovery.is_pending() {
        if let Some(start) = model
            .dispatch_prompt()
            .and_then(|prompt| prompt.start_anyway.clone())
        {
            model.take_dispatch_prompt();
            model.start_anyway = true;
            let replayed = handle_board_intent_with_host(
                store,
                domain,
                model,
                start.intent,
                save_recovery,
                quick_capture,
                in_herdr,
                host,
                name_agent,
            );
            model.start_anyway = false;
            return replayed;
        }
    }
    if intent == BoardIntent::ConfirmDispatch && !save_recovery.is_pending() {
        start_bulk_dispatch(
            store,
            domain,
            model,
            save_recovery,
            baseline,
            in_herdr,
            host,
            name_agent,
        )?;
        return Ok(false);
    }

    // `n` on the relaunch card: start the task, leave its agent alone.
    if intent == BoardIntent::StartWithoutRelaunch && !save_recovery.is_pending() {
        if let Some(relaunch) = model
            .dispatch_prompt()
            .and_then(|prompt| prompt.relaunch.clone())
        {
            model.take_dispatch_prompt();
            start_plain_and_save(
                store,
                domain,
                model,
                save_recovery,
                baseline,
                &[relaunch.task_id],
                &[],
                relaunch.any_status,
                false,
                in_herdr,
            );
        }
        return Ok(false);
    }

    if intent == BoardIntent::DispatchAgain && !save_recovery.is_pending() {
        run_board_dispatch(
            store,
            domain,
            model,
            save_recovery,
            baseline,
            dispatch_target,
            true,
            in_herdr,
            host,
            name_agent,
        );
        return Ok(false);
    }

    // A start of a task that still waits on others asks first.
    if matches!(
        intent,
        BoardIntent::PrimaryVerb | BoardIntent::SetStatus(HumanStatus::Started)
    ) && !save_recovery.is_pending()
        && !model.focus_is_archived()
        && ask_before_starting_waiting(domain, model, &intent)
    {
        return Ok(false);
    }

    // A start on an assigned task dispatches it, asks before relaunching a gone agent, or
    // opens the bulk start card; everything else is the reducer's plain status change.
    if matches!(
        intent,
        BoardIntent::PrimaryVerb | BoardIntent::SetStatus(HumanStatus::Started)
    ) && !save_recovery.is_pending()
        && !model.focus_is_archived()
        && route_board_start(
            store,
            domain,
            model,
            save_recovery,
            &baseline,
            intent != BoardIntent::PrimaryVerb,
            in_herdr,
            host,
            name_agent,
        )
    {
        return Ok(false);
    }

    // The reply box's `ctrl+s` on a task with an agent unblocks to started, not ready: a
    // running agent gets a plain start; otherwise the reply is stored first and the task
    // then dispatches, or asks before relaunching a gone agent.
    let mut after_reply = None;
    let mut reply_no_launch = None;
    // A plain start because the agent still runs: the reply goes to it once the start is
    // durable. Shift+Enter (save only) never sends.
    let mut deliver_to = None;
    // Only the reply this action stores is ever sent, never earlier ones.
    let mut reply_text = (intent == BoardIntent::ReplySaveUnblock && !save_recovery.is_pending())
        .then(|| model.reply_draft().map(|draft| draft.trim().to_string()))
        .flatten();
    let intent = if intent == BoardIntent::ReplySaveUnblock && !save_recovery.is_pending() {
        // Route against the durable record: an assignment made elsewhere must launch.
        domain.merge_tasks_from_disk(&baseline);
        model.sync_from_domain(domain);
        match reply_unblock_route(domain, model, in_herdr, host) {
            None => intent,
            Some((id, dispatch::StartRoute::Plain, check)) => {
                deliver_to = Some((id, check));
                BoardIntent::ReplySaveStart
            }
            Some((_, dispatch::StartRoute::NoLaunch { reason }, _)) => {
                reply_no_launch = Some(reason);
                BoardIntent::ReplySaveStart
            }
            Some((id, route, _)) => {
                after_reply = Some((id, route));
                // A review keeps its send-back through this save: an empty box with a failed
                // check still launches.
                if model.reply_is_feedback() {
                    BoardIntent::ReplySaveBeforeLaunch
                } else {
                    BoardIntent::ReplySave
                }
            }
        }
    } else {
        intent
    };

    let resolves_recovery = match intent {
        BoardIntent::RetrySave => Some(true),
        BoardIntent::CancelSave => Some(false),
        _ => None,
    };

    let loaded_snapshot;
    let snapshot_for_intent = if !save_recovery.is_pending() && intent == BoardIntent::OpenCapture {
        loaded_snapshot = load_snapshot(domain);
        Some(&loaded_snapshot)
    } else {
        None
    };

    // A completion the reducer made starts what it released in the same save.
    let mut released = Vec::new();
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
            released.extend(start_released(store, state, in_herdr, host));
            store
                .reload_merge_save(state)
                .map_err(|error| error.to_string())
        },
    );
    if outcome == IntentOutcome::Persisted {
        if let Some(note) = report_released(&released, name_agent) {
            model.set_message(note);
        }
    }
    // A reply whose save failed keeps its start for save recovery: Retry resumes it, Cancel
    // drops it with the rolled-back reply.
    // A delivery waits on the same recovery: Retry sends it once, against a fresh check.
    let mut resumed_check = dispatch::AgentCheck::NotChecked;
    match resolves_recovery {
        Some(false) => {
            model.pending_reply_start = None;
            model.pending_reply_delivery = None;
            model.pending_reply_text = None;
        }
        Some(true) if outcome == IntentOutcome::Persisted && !save_recovery.is_pending() => {
            reply_text = model.pending_reply_text.take();
            if let Some(target) = model.pending_reply_start.take() {
                after_reply = domain
                    .get(target)
                    .filter(|task| task.assignee.is_some() && !task.archived)
                    .map(|task| {
                        let (route, check) = dispatch::start_route_checked(
                            task,
                            crate::domain::OWNER,
                            in_herdr,
                            host,
                        );
                        resumed_check = check;
                        (target, route)
                    });
            }
            if let Some(target) = model.pending_reply_delivery.take() {
                deliver_to = domain.get(target).map(|task| {
                    let (_, check) =
                        dispatch::start_route_checked(task, crate::domain::OWNER, in_herdr, host);
                    (target, check)
                });
            }
        }
        _ => {}
    }
    if save_recovery.is_pending() {
        if let Some((target, _)) = after_reply.take() {
            model.pending_reply_start = Some(target);
        }
        if let Some((target, _)) = deliver_to.take() {
            model.pending_reply_delivery = Some(target);
        }
        if model.pending_reply_start.is_some() || model.pending_reply_delivery.is_some() {
            if let Some(text) = reply_text.take() {
                model.pending_reply_text = Some(text);
            }
        }
    }
    if outcome == IntentOutcome::Persisted {
        record_notice_dismissals_without_blocking_persist(store, domain);
        if let Some(reason) = reply_no_launch {
            model.set_message(dispatch::no_launch_message(reason));
        }
        if let Some((target, check)) = deliver_to.take() {
            deliver_reply_after_start(domain, model, target, &check, reply_text.as_deref(), host);
        }
        if let Some((target, route)) = after_reply.filter(|_| !save_recovery.is_pending()) {
            match route {
                dispatch::StartRoute::AgentGone { assignee } => {
                    // The reply box unblocks: any status but started moves.
                    open_relaunch_card(domain, model, target, assignee, true);
                }
                // Only a resumed start lands here: its agent turned out to be running, or
                // dispatch cannot work here.
                dispatch::StartRoute::Plain | dispatch::StartRoute::NoLaunch { .. } => {
                    let Some(baseline) = load_baseline(store, model)? else {
                        return Ok(false);
                    };
                    let started = start_plain_and_save(
                        store,
                        domain,
                        model,
                        save_recovery,
                        baseline,
                        &[target],
                        &[],
                        true,
                        false,
                        in_herdr,
                    );
                    match (started, route) {
                        (Some(_), dispatch::StartRoute::NoLaunch { reason }) => {
                            model.set_message(dispatch::no_launch_message(reason));
                        }
                        (Some(_), _) if !save_recovery.is_pending() => {
                            deliver_reply_after_start(
                                domain,
                                model,
                                target,
                                &resumed_check,
                                reply_text.as_deref(),
                                host,
                            );
                        }
                        _ => {}
                    }
                }
                dispatch::StartRoute::Dispatch => {
                    let Some(baseline) = load_baseline(store, model)? else {
                        return Ok(false);
                    };
                    run_board_dispatch(
                        store,
                        domain,
                        model,
                        save_recovery,
                        baseline,
                        Some(target),
                        false,
                        in_herdr,
                        host,
                        name_agent,
                    );
                }
            }
        }
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
    fn projects_preview_right_seat_inherits_agent_profiles_when_it_is_created() {
        let temp = TempStore::new("projects-preview-agents");
        std::fs::write(
            temp.dir.join("config.toml"),
            "[agent.reviewer]\ncommand = [\"true\"]\n",
        )
        .expect("write profiles");
        let profiles = crate::agents::AgentProfiles::load(&temp.dir).expect("load profiles");
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "preview assignment",
                None,
                TaskScope::Project {
                    path: "/repos/preview".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create preview task");
        let mut model = BoardModel::from_domain(&domain, None);
        model.set_agent_profiles(&profiles);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("open projects overview");
        stage_right(&mut domain, &mut model, 2);

        apply_intent(
            &mut domain,
            model.input_target_mut(),
            BoardIntent::OpenAssigneePicker,
            None,
        )
        .expect("open right-seat assignee picker");
        assert_eq!(
            model
                .input_target_mut()
                .selected_list_picker_option()
                .map(|(_, option)| option.label),
            Some("@reviewer".to_string()),
            "the inherited profile is offered"
        );
        assert_eq!(
            apply_intent(
                &mut domain,
                model.input_target_mut(),
                BoardIntent::ConfirmListPicker,
                None,
            )
            .expect("assign in right seat"),
            IntentOutcome::Persist
        );
        assert_eq!(
            domain.get(id).expect("preview task").assignee.as_deref(),
            Some("reviewer")
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
    fn projects_preview_h_and_l_keep_the_narrow_peek_keymap() {
        let (mut domain, mut model, _) = projects_preview_fixture();
        let area = Rect::new(0, 0, 110, 30);
        let peek = preview_key_intent(
            &mut model,
            area,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
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
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
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
        // Narrow task page: ← stays inert, Esc closes.
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
        let model = BoardModel::from_domain(&DomainState::new(), None);
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
                board_intent_may_persist(&model, &intent),
                "{intent:?} must load the persisted baseline before save recovery"
            );
        }
        assert!(!board_intent_may_persist(&model, &BoardIntent::SelectNext));

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
                !board_intent_may_persist(&model, &intent),
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
            BoardIntent::OpenFormDropdown(CaptureField::Scope),
            None,
        )
        .expect("open scope picker");
        assert_eq!(model.input_mode(), BoardInputMode::FormDropdown);
        assert_eq!(
            board_keyboard_intent(&model, model.input_mode(), ctrl_q),
            Some(BoardIntent::Quit)
        );

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::CancelFormDropdown,
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
    fn task_and_expanded_capture_edit_rings_follow_footer_order_in_both_directions() {
        let mut domain = DomainState::new();
        domain
            .create(
                "ring task",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("task");
        let mut task = BoardModel::from_domain(&domain, None);
        apply_intent(&mut domain, &mut task, BoardIntent::OpenTaskPage, None).expect("page");
        apply_intent(&mut domain, &mut task, BoardIntent::BeginEditTitle, None).expect("edit");
        assert_eq!(task.input_mode(), BoardInputMode::EditTitle);
        for expected in [
            BoardInputMode::EditNotes,
            BoardInputMode::TaskPage,
            BoardInputMode::EditAssignee,
            BoardInputMode::SelectBase,
            BoardInputMode::SelectAfter,
            BoardInputMode::SelectThread,
            BoardInputMode::EditScope,
            BoardInputMode::EditTitle,
        ] {
            apply_intent(&mut domain, &mut task, BoardIntent::FormFocusNext, None).expect("tab");
            assert_eq!(task.input_mode(), expected);
        }
        for expected in [
            BoardInputMode::EditScope,
            BoardInputMode::SelectThread,
            BoardInputMode::SelectAfter,
            BoardInputMode::SelectBase,
            BoardInputMode::EditAssignee,
            BoardInputMode::TaskPage,
            BoardInputMode::EditNotes,
            BoardInputMode::EditTitle,
        ] {
            apply_intent(&mut domain, &mut task, BoardIntent::FormFocusPrev, None)
                .expect("shift tab");
            assert_eq!(task.input_mode(), expected);
        }

        let mut capture_domain = DomainState::new();
        let mut capture = BoardModel::from_domain(&capture_domain, None);
        apply_intent(
            &mut capture_domain,
            &mut capture,
            BoardIntent::OpenCapture,
            None,
        )
        .expect("open quick add");
        apply_intent(
            &mut capture_domain,
            &mut capture,
            BoardIntent::ExpandQuickAdd,
            None,
        )
        .expect("expand quick add");
        assert_eq!(capture.input_mode(), BoardInputMode::EditNotes);
        for expected in [
            BoardInputMode::CapturePage,
            BoardInputMode::EditAssignee,
            BoardInputMode::SelectBase,
            BoardInputMode::SelectAfter,
            BoardInputMode::EditThread,
            BoardInputMode::EditScope,
            BoardInputMode::EditTitle,
        ] {
            apply_intent(
                &mut capture_domain,
                &mut capture,
                BoardIntent::FormFocusNext,
                None,
            )
            .expect("capture tab");
            assert_eq!(capture.input_mode(), expected);
        }
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
                BoardInputMode::FormDropdown,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)
            ),
            Some(BoardIntent::FormDropdownNext),
            "the form scope dropdown must route through the shared form mapper"
        );

        // These modes may overlay an open form, but their own map must win. Each key is one the
        // form mapper would handle differently, so this is an allowlist regression guard rather
        // than a test of identical fallthroughs.
        for (mode, code, expected) in [
            (
                BoardInputMode::Normal,
                KeyCode::Char('r'),
                Some(BoardIntent::BeginReply),
            ),
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

    /// Bare Enter on a selected block option opens the prefilled reply box, which then types
    /// letters such as a capital M.
    #[test]
    fn enter_on_a_block_option_resolves_to_reply_with_option_at_the_keyboard_boundary() {
        use crate::ui::board::apply_intent;

        let (mut domain, mut model) = board_fixture("Asked", None);
        let id = model.selected_id().expect("task");
        domain
            .block(
                id,
                crate::domain::BlockDraft {
                    options: vec!["yes".into()],
                    ..Default::default()
                },
                "claude",
            )
            .expect("block");
        model.sync_from_domain(&domain);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("tab");
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::TaskPage, enter),
            Some(BoardIntent::OpenTaskPage),
            "the heading keeps Enter's page route"
        );
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("tab");
        let intent = board_keyboard_intent(&model, BoardInputMode::TaskPage, enter)
            .expect("enter on an option");
        assert_eq!(intent, BoardIntent::ReplyWithOption);
        apply_intent(&mut domain, &mut model, intent, None).expect("reply");
        assert_eq!(model.input_mode(), BoardInputMode::EditReply);
        assert_eq!(model.reply_draft(), Some("yes"));
        let capital_m = KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT);
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::EditReply, capital_m),
            Some(BoardIntent::EditInsert('M')),
            "the reply box types a capital M"
        );
    }

    /// With mark mode on, the block card and the reply box own capital M and Esc: typing
    /// `Meeting…` never toggles mark mode, and Esc cancels the surface, never the marked set
    /// the card captured.
    #[test]
    fn mark_mode_keys_belong_to_the_block_card_and_the_reply_box() {
        use crate::ui::board::apply_intent;

        let (mut domain, mut model) = board_fixture("Asked", None);
        let id = model.selected_id().expect("task");
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None).expect("mark");
        apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark one");
        assert_eq!(model.marked_count(), 1);
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleBlock, None).expect("card");
        assert_eq!(model.input_mode(), BoardInputMode::BlockCard);
        let capital_m = KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::BlockCard, capital_m),
            Some(BoardIntent::EditInsert('M')),
            "the card's why types a capital M"
        );
        let cancel = board_keyboard_intent(&model, BoardInputMode::BlockCard, esc);
        assert_eq!(cancel, Some(BoardIntent::BlockCardCancel));
        apply_intent(&mut domain, &mut model, BoardIntent::BlockCardCancel, None).expect("esc");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert!(
            model.mark_mode_active() && model.marked_count() == 1,
            "Esc kept the marks"
        );

        domain
            .block(id, crate::domain::BlockDraft::default(), "claude")
            .expect("block");
        model.sync_from_domain(&domain);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("reply");
        assert_eq!(model.input_mode(), BoardInputMode::EditReply);
        assert!(model.mark_mode_active(), "the marks are still armed");
        assert_eq!(
            board_keyboard_intent(&model, BoardInputMode::EditReply, capital_m),
            Some(BoardIntent::EditInsert('M'))
        );
        let reply_esc = board_keyboard_intent(&model, BoardInputMode::EditReply, esc);
        assert_ne!(reply_esc, Some(BoardIntent::MarkClear));
        assert_eq!(
            reply_esc,
            crate::ui::input::map_key(BoardInputMode::EditReply, esc),
            "Esc is the reply box's own cancel"
        );
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
        assert!(board_intent_may_persist(&model, &intent));
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
                .map(|command| command.label.clone())
                .collect::<Vec<_>>(),
            typed
                .visible_commands()
                .iter()
                .map(|command| command.label.clone())
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

        // Scope focus without an active path editor is not a text field: inert.
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

#[cfg(test)]
mod quick_assign_tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    use super::{
        apply_board_intent_with_save_recovery, apply_intent, board_keyboard_intent,
        handle_board_intent_with_host, BoardIntent, BoardSaveContext, DomainState,
    };
    use crate::agents::AgentProfiles;
    use crate::dispatch::{self, CreatedWorktree, DispatchHost};
    use crate::domain::{HumanStatus, ProvenanceOrigin, TaskScope, UndoEntry};
    use crate::save_recovery::SaveRecovery;
    use crate::store::TaskStore;
    use crate::ui::board::IntentOutcome;
    use crate::ui::board::{board_hit_map, BoardInputMode, BoardModel, ListPickerKind};
    use crate::ui::input::map_key;
    use crate::ui::mouse::{left_click, map_board_mouse};
    use crate::ui::render::QueueHitTarget;

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    const PROJECT: &str = "/repos/app";

    struct Temp {
        dir: PathBuf,
        store: TaskStore,
    }

    impl Temp {
        fn new(label: &str, profiles: &[&str]) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("tsk-quick-assign-{label}-{nanos}-{seq}"));
            std::fs::create_dir_all(&dir).expect("state dir");
            let content = profiles
                .iter()
                .map(|name| format!("[agent.{name}]\ncommand = [\"true\"]\n"))
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(dir.join("config.toml"), content).expect("write profiles");
            let store = TaskStore::new(&dir);
            Temp { dir, store }
        }

        fn profiles(&self) -> AgentProfiles {
            AgentProfiles::load(&self.dir).expect("load profiles")
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Saved project tasks (so each carries a number), and a board with the profiles loaded.
    fn board(temp: &Temp, titles: &[&str]) -> (DomainState, BoardModel, Vec<uuid::Uuid>) {
        let mut domain = DomainState::new();
        let ids = titles
            .iter()
            .map(|title| {
                domain
                    .create(
                        title,
                        None,
                        TaskScope::Project {
                            path: PROJECT.into(),
                        },
                        ProvenanceOrigin::Manual,
                        None,
                    )
                    .expect("create task")
            })
            .collect();
        temp.store
            .reload_merge_save(&mut domain)
            .expect("save tasks");
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(PROJECT)));
        model.set_selected_project(Some(PathBuf::from(PROJECT)));
        model.set_agent_profiles(&temp.profiles());
        (domain, model, ids)
    }

    fn select(domain: &mut DomainState, model: &mut BoardModel, id: uuid::Uuid) {
        let index = model
            .visible_ids()
            .iter()
            .position(|visible| *visible == id)
            .expect("task visible");
        apply_intent(domain, model, BoardIntent::SelectIndex(index), None).expect("select");
    }

    fn key(model: &BoardModel, code: KeyCode, modifiers: KeyModifiers) -> BoardIntent {
        map_key(model.input_mode(), KeyEvent::new(code, modifiers)).expect("mapped key")
    }

    /// Records what the store held when the launch began, proving the assignment was durable
    /// before any host work.
    struct FakeHost {
        store: TaskStore,
        assignee_on_disk_at_launch: Rc<RefCell<Vec<Option<String>>>>,
        launched: usize,
        fail_launch: Option<String>,
        /// Fail only the launch whose branch contains this text.
        fail_branch: Option<String>,
        /// Every command typed into a pane: one per launch, a relaunch included.
        ran: usize,
        /// Herdr's answer to "is an agent in this pane": `None` cannot say.
        agent: Option<bool>,
        /// Herdr no longer has the recorded workspace.
        workspace_gone: bool,
        /// Herdr cannot answer the root-pane query at all.
        root_failed: bool,
        /// The last reply of each task as saved when a launch began.
        replies_on_disk_at_launch: Vec<String>,
        /// Workspaces opened on a kept worktree (a relaunch after the workspace closed).
        reopened: usize,
        /// Every prompt submitted to an agent: (pane, text).
        prompts: Vec<(String, String)>,
        /// How Herdr answers a prompt: `None` accepts it.
        prompt_error: Option<crate::dispatch::PromptError>,
        /// Live agent names by pane; a pane missing here holds an unnamed agent.
        names: std::collections::HashMap<String, String>,
        /// Names the next agent lookups report, ahead of `names` (a pane changing hands).
        next_names: std::collections::VecDeque<Option<String>>,
        /// Every workspace whose root pane was asked for, and every pane asked about.
        root_queries: Vec<String>,
        agent_queries: Vec<String>,
        /// Answer cleanup inspections with a clean, merged worktree.
        cleanup: bool,
        /// The task's durable status and last reply on disk when each prompt was submitted.
        disk_at_prompt: Vec<(HumanStatus, Option<String>)>,
    }

    impl DispatchHost for FakeHost {
        fn inspect_cleanup_cached(
            &mut self,
            _: &Path,
            _: &crate::domain::Dispatch,
            _: bool,
        ) -> Result<crate::dispatch::CleanupInspection, String> {
            if !self.cleanup {
                return Err("cleanup inspection is not supported".into());
            }
            Ok(crate::dispatch::CleanupInspection {
                unreachable_remote: None,
                worktree_exists: true,
                dirty: false,
                branch_merged: true,
                workspace_exists: true,
                target_matches: true,
                warning: None,
                base_available: true,
            })
        }
        fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
            Ok(true)
        }

        fn resolve_base(&mut self, _: &Path) -> Result<String, String> {
            Ok("main".into())
        }

        fn create_worktree(
            &mut self,
            _: &Path,
            branch: &str,
            _: Option<&str>,
            _: &str,
        ) -> Result<CreatedWorktree, String> {
            let disk = self.store.load().expect("load during launch");
            self.replies_on_disk_at_launch
                .extend(disk.tasks().iter().filter_map(|task| {
                    let block = task.block.as_ref().or(task.past_blocks.last())?;
                    block.replies.last().map(|reply| reply.text.clone())
                }));
            self.assignee_on_disk_at_launch.borrow_mut().extend(
                disk.tasks()
                    .iter()
                    .filter(|task| !task.is_notice())
                    .map(|task| task.assignee.clone()),
            );
            self.launched += 1;
            if let Some(error) = self.fail_launch.clone() {
                return Err(error);
            }
            if self
                .fail_branch
                .as_deref()
                .is_some_and(|text| branch.contains(text))
            {
                return Err(format!("herdr refused {branch}"));
            }
            Ok(CreatedWorktree {
                path: "/tmp/tsk-quick-assign-worktree".into(),
                branch: branch.into(),
                workspace_id: "w1".into(),
                root_pane_id: "w1:p1".into(),
            })
        }

        fn root_pane(&mut self, workspace: &str) -> Result<String, crate::dispatch::RootPaneError> {
            self.root_queries.push(workspace.into());
            if self.workspace_gone {
                return Err(crate::dispatch::RootPaneError::WorkspaceGone(
                    "workspace not found".into(),
                ));
            }
            if self.root_failed {
                return Err("could not run herdr".into());
            }
            Ok(format!("{workspace}:p1"))
        }

        fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
            self.ran += 1;
            Ok(())
        }

        fn pane_has_agent(&mut self, _: &str) -> Result<bool, String> {
            self.agent.ok_or_else(|| "herdr did not answer".to_string())
        }

        fn pane_agent(&mut self, pane: &str) -> Result<crate::dispatch::PaneAgent, String> {
            self.agent_queries.push(pane.into());
            match self.agent {
                None => Err("herdr did not answer".into()),
                Some(false) => Ok(crate::dispatch::PaneAgent::Absent),
                Some(true) => Ok(crate::dispatch::PaneAgent::Present {
                    name: self
                        .next_names
                        .pop_front()
                        .unwrap_or_else(|| self.names.get(pane).cloned()),
                }),
            }
        }

        fn prompt_agent(
            &mut self,
            pane: &str,
            text: &str,
        ) -> Result<(), crate::dispatch::PromptError> {
            // What an agent acting on this prompt would read from the board right now.
            let number: Option<u64> = text
                .strip_prefix("[tsk T")
                .and_then(|rest| rest.split(' ').next())
                .and_then(|digits| digits.trim_end_matches(']').parse().ok());
            let disk = self.store.load().expect("load at prompt");
            if let Some(task) = disk.tasks().iter().find(|task| task.number == number) {
                let block = task.block.as_ref().or(task.past_blocks.last());
                self.disk_at_prompt.push((
                    task.status,
                    block.and_then(|block| block.replies.last().map(|reply| reply.text.clone())),
                ));
            }
            self.prompts.push((pane.into(), text.into()));
            self.prompt_error.clone().map_or(Ok(()), Err)
        }

        fn open_worktree(
            &mut self,
            _: &Path,
            worktree: &Path,
            _: &str,
        ) -> Result<CreatedWorktree, String> {
            self.reopened += 1;
            Ok(CreatedWorktree {
                path: worktree.to_path_buf(),
                branch: "ignored".into(),
                workspace_id: "w2".into(),
                root_pane_id: "w2:p1".into(),
            })
        }
    }

    fn fake_host(temp: &Temp) -> FakeHost {
        FakeHost {
            store: TaskStore::new(&temp.dir),
            assignee_on_disk_at_launch: Rc::default(),
            launched: 0,
            fail_launch: None,
            fail_branch: None,
            ran: 0,
            agent: None,
            workspace_gone: false,
            root_failed: false,
            replies_on_disk_at_launch: Vec::new(),
            reopened: 0,
            prompts: Vec::new(),
            prompt_error: None,
            names: Default::default(),
            next_names: Default::default(),
            root_queries: Vec::new(),
            agent_queries: Vec::new(),
            disk_at_prompt: Vec::new(),
            cleanup: false,
        }
    }

    /// The dispatched agent of task `id` runs, under its dispatch name, in workspace `w0`'s
    /// root pane (the one `dispatched_before` records).
    fn agent_running(domain: &DomainState, host: &mut FakeHost, id: uuid::Uuid) -> String {
        let number = domain.get(id).and_then(|task| task.number).expect("number");
        host.agent = Some(true);
        host.names.insert(
            "w0:p1".into(),
            crate::dispatch::agent_name(number, "builder"),
        );
        format!("[tsk T{number} unblocked]")
    }

    fn handle(
        temp: &Temp,
        domain: &mut DomainState,
        model: &mut BoardModel,
        intent: BoardIntent,
        host: &mut FakeHost,
    ) {
        let mut recovery = SaveRecovery::new();
        handle_board_intent_with_host(
            &temp.store,
            domain,
            model,
            intent,
            &mut recovery,
            false,
            true,
            host,
            &mut |_| {},
        )
        .expect("board intent");
        assert!(
            !recovery.is_pending(),
            "no save failure expected: {:?}",
            model.message()
        );
    }

    /// A saved earlier launch on `id`, with the task left at `status`.
    fn dispatched_before(
        temp: &Temp,
        domain: &mut DomainState,
        model: &mut BoardModel,
        id: uuid::Uuid,
        status: HumanStatus,
    ) {
        domain
            .assign(id, Some("builder".into()))
            .expect("assign builder");
        temp.store.reload_merge_save(domain).expect("save");
        domain
            .record_dispatch(
                id,
                crate::domain::Dispatch {
                    argv: vec!["true".into()],
                    worktree: "/tmp/tsk-start-earlier".into(),
                    branch: "tsk/earlier".into(),
                    base: Some("main".into()),
                    base_ref: None,
                    base_commit: None,
                    base_remote: None,
                    herdr_workspace_id: "w0".into(),
                    at: std::time::SystemTime::now(),
                    cleaned: false,
                },
            )
            .expect("record earlier launch");
        temp.store.reload_merge_save(domain).expect("save");
        domain.set_status(id, status).expect("status");
        temp.store.reload_merge_save(domain).expect("save");
        model.sync_from_domain(domain);
    }

    fn ctrl_s(model: &BoardModel) -> BoardIntent {
        let intent = key(model, KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(intent, BoardIntent::PrimaryVerb);
        intent
    }

    /// Block `id` on you, save, open its page, and type `reply` in the reply box.
    fn reply_on_blocked(
        temp: &Temp,
        domain: &mut DomainState,
        model: &mut BoardModel,
        id: uuid::Uuid,
        reply: &str,
    ) {
        domain
            .block(
                id,
                crate::domain::BlockDraft::from_input(
                    Some("which db?"),
                    None,
                    &[],
                    Default::default(),
                )
                .expect("draft"),
                "builder",
            )
            .expect("block");
        temp.store.reload_merge_save(domain).expect("save");
        model.sync_from_domain(domain);
        select(domain, model, id);
        apply_intent(domain, model, BoardIntent::OpenTaskPage, None).expect("page");
        apply_intent(domain, model, BoardIntent::BeginReply, None).expect("reply box");
        apply_intent(
            domain,
            model,
            BoardIntent::EditInsertText(reply.into()),
            None,
        )
        .expect("type");
        assert_eq!(model.input_mode(), BoardInputMode::EditReply);
    }

    /// Block `id` on you with a why and needs, save, and select its board row.
    fn blocked_row(temp: &Temp, domain: &mut DomainState, model: &mut BoardModel, id: uuid::Uuid) {
        domain
            .block(
                id,
                crate::domain::BlockDraft::from_input(
                    Some("which db?"),
                    Some("a decision"),
                    &[],
                    Default::default(),
                )
                .expect("draft"),
                "builder",
            )
            .expect("block");
        temp.store.reload_merge_save(domain).expect("save");
        model.sync_from_domain(domain);
        select(domain, model, id);
    }

    fn board_screen(model: &BoardModel, width: u16, height: u16) -> (String, Option<(u16, u16)>) {
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| {
                crate::ui::board::draw_board(frame, model);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let text = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let cursor = terminal
            .get_cursor_position()
            .ok()
            .map(|position| (position.x, position.y));
        (text, cursor)
    }

    /// `r` on a blocked board row opens the reply box under the row, with why and needs
    /// above it; `ctrl+s` stores the reply, starts the task and sends it to the running agent,
    /// then hands the keys back to the board.
    #[test]
    fn r_on_a_blocked_row_replies_inline_and_delivers() {
        for width in [80, 140] {
            let temp = Temp::new("row-reply", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["agent asks", "other work"]);
            dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
            blocked_row(&temp, &mut domain, &mut model, ids[0]);
            assert_eq!(model.input_mode(), BoardInputMode::Normal);
            let intent = key(&model, KeyCode::Char('r'), KeyModifiers::NONE);
            assert_eq!(intent, BoardIntent::BeginReply);
            apply_intent(&mut domain, &mut model, intent, None).expect("open");
            assert_eq!(model.input_mode(), BoardInputMode::EditReply);
            assert_eq!(model.row_reply_task(), Some(ids[0]));
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::EditInsertText("postgres".into()),
                None,
            )
            .expect("type");
            let (screen, cursor) = board_screen(&model, width, 30);
            let row = screen
                .lines()
                .position(|line| line.contains("agent asks"))
                .expect("row painted");
            let lines: Vec<&str> = screen.lines().collect();
            assert!(lines[row + 1].contains("why    which db?"), "{screen}");
            assert!(lines[row + 2].contains("needs  a decision"), "{screen}");
            assert!(lines[row + 3].contains("└ you  postgres"), "{screen}");
            let (_, y) = cursor.expect("caret placed");
            assert_eq!(usize::from(y), row + 3, "the caret sits in the draft");

            let mut host = fake_host(&temp);
            agent_running(&domain, &mut host, ids[0]);
            let save = key(&model, KeyCode::Char('s'), KeyModifiers::CONTROL);
            assert_eq!(save, BoardIntent::ReplySaveUnblock);
            handle(&temp, &mut domain, &mut model, save, &mut host);
            assert_eq!(host.prompts.len(), 1);
            assert!(host.prompts[0].1.ends_with("unblocked] postgres"));
            assert_eq!(model.message(), Some("started · reply sent to @builder"));
            assert_eq!(model.input_mode(), BoardInputMode::Normal);
            assert_eq!(model.row_reply_task(), None, "the box closed on landing");
            let saved = temp.store.load().expect("reload");
            assert_eq!(
                saved.get(ids[0]).expect("task").status,
                HumanStatus::Started
            );
            assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("postgres"));
        }
    }

    /// A long multi-line draft scrolls the list so the caret stays on screen, even after
    /// moving back to its first line.
    #[test]
    fn the_row_reply_caret_stays_visible_in_a_long_draft() {
        let temp = Temp::new("row-reply-caret", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["agent asks"]);
        blocked_row(&temp, &mut domain, &mut model, ids[0]);
        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
        let draft = (1..=40)
            .map(|line| format!("line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText(draft),
            None,
        )
        .expect("type");
        let caret_line = |model: &BoardModel| {
            let (screen, cursor) = board_screen(model, 80, 18);
            let (_, y) = cursor.expect("caret placed");
            screen
                .lines()
                .nth(usize::from(y))
                .expect("caret row on screen")
                .to_string()
        };
        assert!(caret_line(&model).contains("line 40"), "the end is in view");
        for _ in 0..45 {
            apply_intent(&mut domain, &mut model, BoardIntent::EditMoveUp, None).expect("up");
        }
        assert!(
            caret_line(&model).contains("line 01"),
            "the first line scrolled into view"
        );
    }

    /// A refresh that takes the row off the list (finished elsewhere) keeps the draft on
    /// screen with the changed-block refusal, and the box still owns input.
    #[test]
    fn the_row_reply_box_survives_its_row_leaving_the_list() {
        let temp = Temp::new("row-reply-orphan", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["finished elsewhere"]);
        blocked_row(&temp, &mut domain, &mut model, ids[0]);
        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("my draft".into()),
            None,
        )
        .expect("type");
        elsewhere(&temp, |other| other.complete(ids[0]).expect("done"));
        let disk = temp.store.load().expect("load");
        domain.merge_tasks_from_disk(&disk);
        model.sync_from_domain(&domain);
        assert_eq!(model.input_mode(), BoardInputMode::EditReply);
        assert_eq!(model.reply_draft(), Some("my draft"));
        let (screen, cursor) = board_screen(&model, 80, 24);
        assert!(screen.contains("not on this list"), "{screen}");
        assert!(screen.contains("└ you  my draft"), "{screen}");
        assert!(
            screen.contains("this block was closed or replaced elsewhere"),
            "{screen}"
        );
        let (_, y) = cursor.expect("caret placed");
        assert!(screen
            .lines()
            .nth(usize::from(y))
            .is_some_and(|line| line.contains("my draft")));
        // Typing keeps the refusal; Esc closes the box back to the board.
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("!".into()),
            None,
        )
        .expect("type");
        assert!(board_screen(&model, 80, 24)
            .0
            .contains("this block was closed or replaced elsewhere"));
        apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("esc");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
    }

    /// Shift+Enter on the row's box stores the reply only; Esc discards a draft. Both return
    /// the keys to the board.
    #[test]
    fn the_row_reply_box_saves_without_sending_and_cancels() {
        let temp = Temp::new("row-reply-save", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["agent asks"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        blocked_row(&temp, &mut domain, &mut model, ids[0]);
        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("open");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("later".into()),
            None,
        )
        .expect("type");
        apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("esc");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert_eq!(model.row_reply_task(), None);

        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("open");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("postgres".into()),
            None,
        )
        .expect("type");
        let mut host = fake_host(&temp);
        host.agent = Some(true);
        let save = key(&model, KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(save, BoardIntent::ReplySave);
        handle(&temp, &mut domain, &mut model, save, &mut host);
        assert!(host.prompts.is_empty());
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Blocked);
        assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("postgres"));
    }

    /// The row's box outlives a failed save: Cancel keeps the draft open, Retry lands it.
    #[cfg(unix)]
    #[test]
    fn the_row_reply_box_outlives_a_failed_save() {
        use std::os::unix::fs::PermissionsExt;
        for retry in [true, false] {
            let temp = Temp::new("row-reply-recovery", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["agent asks"]);
            blocked_row(&temp, &mut domain, &mut model, ids[0]);
            apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::EditInsertText("postgres".into()),
                None,
            )
            .expect("type");
            let mut host = fake_host(&temp);
            let mut recovery = SaveRecovery::new();
            let mut step = |domain: &mut DomainState,
                            model: &mut BoardModel,
                            recovery: &mut SaveRecovery<DomainState>,
                            intent: BoardIntent| {
                handle_board_intent_with_host(
                    &temp.store,
                    domain,
                    model,
                    intent,
                    recovery,
                    false,
                    true,
                    &mut host,
                    &mut |_| {},
                )
                .expect("intent");
            };
            std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o555))
                .expect("lock the state dir");
            step(
                &mut domain,
                &mut model,
                &mut recovery,
                BoardIntent::ReplySave,
            );
            std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o700))
                .expect("unlock");
            assert!(recovery.is_pending());
            assert_eq!(model.reply_draft(), Some("postgres"), "the box is held");
            let answer = if retry {
                BoardIntent::RetrySave
            } else {
                BoardIntent::CancelSave
            };
            step(&mut domain, &mut model, &mut recovery, answer);
            assert!(!recovery.is_pending());
            if retry {
                assert_eq!(model.row_reply_task(), None, "the reply landed");
                assert_eq!(model.input_mode(), BoardInputMode::Normal);
            } else {
                assert_eq!(model.reply_draft(), Some("postgres"), "the draft stays");
                assert_eq!(model.input_mode(), BoardInputMode::EditReply);
            }
        }
    }

    /// `r` on a row that is not blocked says so; with marks it answers the cursor row only.
    #[test]
    fn r_answers_the_cursor_row_only_and_refuses_unblocked_rows() {
        let temp = Temp::new("row-reply-refuse", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["blocked one", "plain one"]);
        blocked_row(&temp, &mut domain, &mut model, ids[0]);
        select(&mut domain, &mut model, ids[1]);
        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
        assert_eq!(model.message(), Some("not blocked or in review"));
        assert_eq!(model.input_mode(), BoardInputMode::Normal);

        apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None).expect("M");
        apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark");
        assert!(!model.marked_ids().is_empty());
        select(&mut domain, &mut model, ids[0]);
        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
        assert_eq!(model.row_reply_task(), Some(ids[0]));
        assert!(
            !model.marked_ids().is_empty(),
            "marks stay; the box ignores them"
        );
    }

    fn last_reply(domain: &DomainState, id: uuid::Uuid) -> Option<String> {
        let task = domain.get(id)?;
        let block = task.block.as_ref().or(task.past_blocks.last())?;
        block.replies.last().map(|reply| reply.text.clone())
    }

    /// Put `id` up for review by `builder` with `checks`, save, select its row and open its page.
    fn review_page(
        temp: &Temp,
        domain: &mut DomainState,
        model: &mut BoardModel,
        id: uuid::Uuid,
        checks: &[&str],
    ) {
        let checks: Vec<String> = checks.iter().map(|check| check.to_string()).collect();
        domain
            .review(
                id,
                crate::domain::ReviewDraft::from_input(
                    Some("built the card"),
                    &checks,
                    Some("docs"),
                    Default::default(),
                )
                .expect("draft"),
                "builder",
            )
            .expect("review");
        temp.store.reload_merge_save(domain).expect("save");
        model.sync_from_domain(domain);
        select(domain, model, id);
        apply_intent(domain, model, BoardIntent::OpenTaskPage, None).expect("page");
    }

    /// One key on the task page, resolved at the keyboard boundary and applied like the loop.
    fn page_key(
        temp: &Temp,
        domain: &mut DomainState,
        model: &mut BoardModel,
        host: &mut FakeHost,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> BoardIntent {
        let intent =
            board_keyboard_intent(model, model.input_mode(), KeyEvent::new(code, modifiers))
                .expect("mapped key");
        handle(temp, domain, model, intent.clone(), host);
        intent
    }

    fn checks_of(domain: &DomainState, id: uuid::Uuid) -> Vec<crate::domain::CheckState> {
        domain
            .get(id)
            .and_then(|task| task.block.as_ref())
            .map(|block| block.checks.iter().map(|check| check.state).collect())
            .unwrap_or_default()
    }

    /// A sent-back review round lands on the PAPER TRAIL: Tab past `+ step` selects it, Enter
    /// (resolved at the keyboard boundary) expands it in place and folds it again, and bare `a`
    /// never touches the store.
    #[test]
    fn enter_on_a_paper_trail_record_expands_it_in_place() {
        let temp = Temp::new("paper-trail", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["trail me"]);
        review_page(&temp, &mut domain, &mut model, ids[0], &["tests pass"]);
        domain
            .set_status(ids[0], HumanStatus::Started)
            .expect("send back");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        let mut host = fake_host(&temp);
        let none = KeyModifiers::NONE;
        // `+ step`, then the round.
        for _ in 0..2 {
            page_key(
                &temp,
                &mut domain,
                &mut model,
                &mut host,
                KeyCode::Tab,
                none,
            );
        }
        assert_eq!(
            model.block_target(),
            Some(crate::ui::board::BlockTarget::Trail(0))
        );
        let enter = page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Enter,
            none,
        );
        assert_eq!(enter, BoardIntent::ToggleTrailRecord(None));
        let (screen, _) = board_screen(&model, 90, 40);
        assert!(
            screen.contains("review round 1 · sent back · you"),
            "{screen}"
        );
        assert!(screen.contains("○ tests pass"), "{screen}");
        assert!(screen.contains("done   built the card"), "{screen}");
        page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Enter,
            none,
        );
        let (screen, _) = board_screen(&model, 90, 40);
        assert!(!screen.contains("done   built the card"), "{screen}");
        assert_eq!(
            model.input_mode(),
            BoardInputMode::TaskPage,
            "the page stays open"
        );

        let before = temp.store.load().expect("load");
        let a = page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Char('a'),
            none,
        );
        assert_eq!(a, BoardIntent::ToggleTrailAll);
        assert!(!crate::ui::board::board_intent_may_persist(&model, &a));
        assert_eq!(temp.store.load().expect("reload").tasks(), before.tasks());
    }

    /// Enter on a check cycles it open → passed → failed → open in its own row, and the cursor
    /// never moves. Passed checks fold into the `N passed ▸` line only when the page is next
    /// painted fresh; Enter on that line unfolds it (`▾`) so a folded check can be cycled again.
    #[test]
    fn enter_cycles_checks_in_place_and_passed_checks_fold_on_return() {
        use crate::domain::CheckState::{Failed, Open, Passed};
        use crate::ui::board::BlockTarget::{Check, Heading, PassedFold};
        let temp = Temp::new("review-checks", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        review_page(
            &temp,
            &mut domain,
            &mut model,
            ids[0],
            &["tests pass", "no flicker"],
        );
        let mut host = fake_host(&temp);
        let none = KeyModifiers::NONE;
        let key =
            |domain: &mut DomainState,
             model: &mut BoardModel,
             host: &mut FakeHost,
             code: KeyCode| { page_key(&temp, domain, model, host, code, none) };
        key(&mut domain, &mut model, &mut host, KeyCode::Tab);
        assert_eq!(model.block_target(), Some(Heading));
        key(&mut domain, &mut model, &mut host, KeyCode::Tab);
        assert_eq!(model.block_target(), Some(Check(0)));

        let enter = key(&mut domain, &mut model, &mut host, KeyCode::Enter);
        assert_eq!(enter, BoardIntent::CycleCheck);
        assert_eq!(checks_of(&domain, ids[0]), [Passed, Open]);
        assert_eq!(
            checks_of(&temp.store.load().expect("reload"), ids[0]),
            [Passed, Open],
            "the check is durable"
        );
        assert_eq!(model.block_target(), Some(Check(0)), "the cursor stays");
        let (screen, _) = board_screen(&model, 90, 30);
        assert!(
            screen.contains("✓ tests pass"),
            "passed in place:\n{screen}"
        );
        assert!(!screen.contains("passed ▸"), "{screen}");
        let at = |screen: &str, text: &str| {
            screen
                .find(text)
                .unwrap_or_else(|| panic!("{text}:\n{screen}"))
        };
        assert!(at(&screen, "✓ tests pass") < at(&screen, "○ no flicker"));

        key(&mut domain, &mut model, &mut host, KeyCode::Enter);
        assert_eq!(checks_of(&domain, ids[0]), [Failed, Open]);
        assert_eq!(model.block_target(), Some(Check(0)), "the cursor stays");
        let (screen, _) = board_screen(&model, 90, 30);
        assert!(at(&screen, "✗ tests pass") < at(&screen, "○ no flicker"));
        key(&mut domain, &mut model, &mut host, KeyCode::Enter);
        assert_eq!(checks_of(&domain, ids[0]), [Open, Open], "failed → open");
        assert_eq!(model.block_target(), Some(Check(0)));
        key(&mut domain, &mut model, &mut host, KeyCode::Enter);
        assert_eq!(checks_of(&domain, ids[0]), [Passed, Open]);
        key(&mut domain, &mut model, &mut host, KeyCode::Tab);
        assert_eq!(
            model.block_target(),
            Some(Check(1)),
            "the ring keeps page order"
        );

        // Leave the page and come back: the passed check is folded now.
        key(&mut domain, &mut model, &mut host, KeyCode::Esc);
        assert_ne!(
            model.input_mode(),
            BoardInputMode::TaskPage,
            "left the page"
        );
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
        let (screen, _) = board_screen(&model, 90, 30);
        assert!(screen.contains("1 passed ▸"), "{screen}");
        assert!(!screen.contains("tests pass"), "{screen}");
        assert!(at(&screen, "○ no flicker") < at(&screen, "1 passed ▸"));
        key(&mut domain, &mut model, &mut host, KeyCode::Tab);
        key(&mut domain, &mut model, &mut host, KeyCode::Tab);
        assert_eq!(model.block_target(), Some(Check(1)));
        key(&mut domain, &mut model, &mut host, KeyCode::Tab);
        assert_eq!(model.block_target(), Some(PassedFold));

        let enter = key(&mut domain, &mut model, &mut host, KeyCode::Enter);
        assert_eq!(enter, BoardIntent::TogglePassedChecks);
        assert_eq!(model.block_target(), Some(PassedFold));
        let (screen, _) = board_screen(&model, 90, 30);
        assert!(screen.contains("1 passed ▾"), "{screen}");
        assert!(screen.contains("✓ tests pass"), "{screen}");
        key(&mut domain, &mut model, &mut host, KeyCode::Tab);
        assert_eq!(model.block_target(), Some(Check(0)));
        key(&mut domain, &mut model, &mut host, KeyCode::Enter);
        assert_eq!(checks_of(&domain, ids[0]), [Failed, Open]);
        assert_eq!(model.block_target(), Some(Check(0)), "the cursor stays");
        let (screen, _) = board_screen(&model, 90, 30);
        assert!(
            at(&screen, "1 passed ▾") < at(&screen, "✗ tests pass"),
            "failed under the open fold, in place:\n{screen}"
        );

        // Folding paints the fold fresh: the failed check leaves it.
        key(&mut domain, &mut model, &mut host, KeyCode::BackTab);
        assert_eq!(model.block_target(), Some(PassedFold));
        key(&mut domain, &mut model, &mut host, KeyCode::Enter);
        let (screen, _) = board_screen(&model, 90, 30);
        assert!(screen.contains("✗ tests pass"), "{screen}");
        assert!(!screen.contains("passed ▸"), "{screen}");
        assert_eq!(
            model.block_target(),
            Some(Check(0)),
            "the fold line went away; the cursor rests on the check that left it"
        );
        let (screen, _) = board_screen(&model, 90, 30);
        let selected = screen
            .lines()
            .find(|line| line.starts_with('▸'))
            .unwrap_or_else(|| panic!("a painted selection:\n{screen}"));
        assert!(selected.contains("✗ tests pass"), "{selected}");
        key(&mut domain, &mut model, &mut host, KeyCode::Tab);
        assert_eq!(
            model.block_target(),
            Some(Check(1)),
            "Tab walks on in order"
        );
        assert_eq!(host.prompts, [], "checks never send anything");
    }

    /// A review round that arrives on an open page takes its fold then; a refresh that marks the
    /// selected check passed elsewhere keeps its row and the selection until the page is left.
    #[test]
    fn a_review_arriving_on_an_open_page_freezes_its_fold() {
        use crate::domain::CheckState::{Open, Passed};
        use crate::ui::board::BlockTarget::{Check, Heading};
        let temp = Temp::new("review-arrives", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        select(&mut domain, &mut model, ids[0]);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
        let mut other = temp.store.load().expect("load");
        let checks = vec!["tests pass".to_string(), "no flicker".to_string()];
        other
            .review(
                ids[0],
                crate::domain::ReviewDraft::from_input(
                    Some("built the card"),
                    &checks,
                    Some("docs"),
                    Default::default(),
                )
                .expect("draft"),
                "builder",
            )
            .expect("review");
        temp.store.reload_merge_save(&mut other).expect("save");
        domain = temp.store.load().expect("reload");
        model.sync_from_domain(&domain);
        let mut host = fake_host(&temp);
        let none = KeyModifiers::NONE;
        page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Tab,
            none,
        );
        assert_eq!(model.block_target(), Some(Heading));
        page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Tab,
            none,
        );
        assert_eq!(model.block_target(), Some(Check(0)));

        other.set_check(ids[0], 0, Passed).expect("pass");
        temp.store.reload_merge_save(&mut other).expect("save");
        domain = temp.store.load().expect("reload");
        model.sync_from_domain(&domain);
        assert_eq!(checks_of(&domain, ids[0]), [Passed, Open]);
        assert_eq!(model.block_target(), Some(Check(0)), "the selection holds");
        let (screen, _) = board_screen(&model, 90, 30);
        let selected = screen
            .lines()
            .find(|line| line.starts_with('▸'))
            .unwrap_or_else(|| panic!("a painted selection:\n{screen}"));
        assert!(selected.contains("✓ tests pass"), "{selected}\n{screen}");
        assert!(!screen.contains("passed ▸"), "{screen}");

        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close");
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
        let (screen, _) = board_screen(&model, 90, 30);
        assert!(screen.contains("1 passed ▸"), "{screen}");
        assert!(!screen.contains("tests pass"), "{screen}");
    }

    /// The REVIEW section leads the page: round, who it is on, author, the PR it names, then
    /// done, the checks, next and the feedback, above the notes.
    #[test]
    fn the_review_section_leads_the_task_page() {
        let temp = Temp::new("review-page", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        domain
            .edit(
                ids[0],
                "review me",
                Some("see PR #41 for the diff".into()),
                TaskScope::Project {
                    path: PROJECT.into(),
                },
                None,
            )
            .expect("notes");
        temp.store.reload_merge_save(&mut domain).expect("save");
        review_page(&temp, &mut domain, &mut model, ids[0], &["tests pass"]);
        let (screen, _) = board_screen(&model, 100, 30);
        let heading = screen
            .lines()
            .find(|line| line.contains("REVIEW · round 1 · on you · @builder"))
            .unwrap_or_else(|| panic!("heading:\n{screen}"));
        assert!(heading.contains("PR #41 · r feedback"), "{heading}");
        let at = |text: &str| {
            screen
                .find(text)
                .unwrap_or_else(|| panic!("{text}:\n{screen}"))
        };
        assert!(at("REVIEW") < at("done   built the card"));
        assert!(at("done   built the card") < at("○ tests pass"));
        assert!(at("○ tests pass") < at("next   docs"));
        assert!(at("next   docs") < at("see PR #41 for the diff"));
    }

    /// The REVIEW section paints at 40 and 120 columns: the heading and the fold line stay,
    /// long text wraps instead of truncating.
    #[test]
    fn the_review_section_paints_narrow_and_wide() {
        use crate::domain::CheckState::Passed;
        let temp = Temp::new("review-widths", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        review_page(
            &temp,
            &mut domain,
            &mut model,
            ids[0],
            &["a check long enough to wrap at forty columns for sure", "b"],
        );
        domain.set_check(ids[0], 1, Passed).expect("pass");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        let (screen, _) = board_screen(&model, 120, 30);
        assert!(
            screen.contains("✓ b") && !screen.contains("passed ▸"),
            "a check passed while the page is open keeps its row:\n{screen}"
        );
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close");
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
        for width in [40, 120] {
            let (screen, _) = board_screen(&model, width, 30);
            assert!(screen.contains("REVIEW · round 1"), "{width}:\n{screen}");
            assert!(screen.contains("1 passed ▸"), "{width}:\n{screen}");
            assert!(screen.contains("wrap"), "{width}:\n{screen}");
            assert!(
                screen.contains("for sure"),
                "wrapped, not cut ({width}):\n{screen}"
            );
        }
    }

    /// The feedback box's keys: Esc discards, Shift+Enter stores and stays in review, and
    /// neither sends anything.
    #[test]
    fn the_feedback_box_saves_without_sending_and_cancels() {
        let temp = Temp::new("review-feedback-save", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        review_page(&temp, &mut domain, &mut model, ids[0], &["tests pass"]);
        let mut host = fake_host(&temp);
        agent_running(&domain, &mut host, ids[0]);
        let r = page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        );
        assert_eq!(r, BoardIntent::BeginReply);
        assert_eq!(model.input_mode(), BoardInputMode::EditReply);
        assert!(model.reply_is_feedback());
        let (screen, _) = board_screen(&model, 100, 30);
        assert!(screen.contains("feedback to @builder…"), "{screen}");
        assert!(
            screen.contains("ctrl+s send back") && screen.contains("ctrl+d approve"),
            "{screen}"
        );
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("drop me".into()),
            None,
        )
        .expect("type");
        let esc = page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Esc,
            KeyModifiers::NONE,
        );
        assert_eq!(esc, BoardIntent::CancelEdit);
        assert_eq!(model.reply_draft(), None);
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("looks close".into()),
            None,
        )
        .expect("type");
        let save = page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Enter,
            KeyModifiers::SHIFT,
        );
        assert_eq!(save, BoardIntent::ReplySave);
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(
            task.status,
            HumanStatus::Review,
            "shift+enter stays in review"
        );
        assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("looks close"));
        assert_eq!(model.reply_draft(), None, "the box closed on landing");
        assert_eq!(host.prompts, [], "shift+enter never sends");
        // Your feedback is the last word: the row says so instead of the author and checks.
        assert_eq!(
            crate::ui::render::block_trailer(task, saved.tasks()).as_deref(),
            Some("feedback")
        );
    }

    /// `ctrl+s` sends the review back: the round closes as sent back, the task starts, and the
    /// running agent gets this feedback plus the failed checks, once.
    #[test]
    fn ctrl_s_sends_back_with_the_failed_checks_to_the_running_agent() {
        use crate::domain::CheckState::{Failed, Passed};
        let temp = Temp::new("review-send-back", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        review_page(
            &temp,
            &mut domain,
            &mut model,
            ids[0],
            &["tests pass", "no flicker", "docs build"],
        );
        for (index, state) in [(0, Passed), (1, Failed), (2, Failed)] {
            domain.set_check(ids[0], index, state).expect("check");
            temp.store.reload_merge_save(&mut domain).expect("save");
        }
        model.sync_from_domain(&domain);
        // An earlier note of yours on this round is never re-sent.
        domain
            .reply(ids[0], "earlier note", crate::domain::OWNER)
            .expect("note");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        let mut host = fake_host(&temp);
        agent_running(&domain, &mut host, ids[0]);
        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("fix the flicker".into()),
            None,
        )
        .expect("type");
        let send = page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(send, BoardIntent::ReplySaveUnblock);
        assert_eq!(host.ran, 0, "a running agent is not relaunched");
        assert_eq!(
            host.prompts,
            [(
                "w0:p1".to_string(),
                "[tsk T1 sent back] fix the flicker Failed checks: no flicker; docs build"
                    .to_string()
            )]
        );
        assert_eq!(model.message(), Some("started · feedback sent to @builder"));
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        assert_eq!(task.block, None);
        let closed = task.past_blocks.last().expect("closed round");
        assert_eq!(closed.round, 1);
        assert_eq!(closed.resolution, Some(crate::domain::Resolution::SentBack));
        assert_eq!(
            closed.replies.last().map(|reply| reply.text.as_str()),
            Some("fix the flicker")
        );

        // The agent sets review again: round 2 opens, round 1 stays in history.
        domain = temp.store.load().expect("reload");
        domain
            .set_status_by(ids[0], HumanStatus::Review, "builder")
            .expect("review again");
        temp.store.reload_merge_save(&mut domain).expect("save");
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.block.as_ref().map(|round| round.round), Some(2));
        assert_eq!(task.past_blocks.len(), 1);
    }

    /// `ctrl+s` on an unassigned review still sends it back to started, with nothing to send.
    #[test]
    fn sending_back_an_unassigned_review_starts_it() {
        let temp = Temp::new("review-send-back-plain", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        failed_check_feedback_box(&temp, &mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        assert_eq!(
            task.past_blocks.last().and_then(|round| round.resolution),
            Some(crate::domain::Resolution::SentBack)
        );
        assert_eq!(host.prompts, []);
    }

    /// `ctrl+d` approves: the feedback is stored, the task is done, the round closes as
    /// approved, and nothing is sent to the running agent.
    #[test]
    fn ctrl_d_in_the_feedback_box_approves_without_sending() {
        let temp = Temp::new("review-approve", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        review_page(&temp, &mut domain, &mut model, ids[0], &["tests pass"]);
        let mut host = fake_host(&temp);
        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("ship it".into()),
            None,
        )
        .expect("type");
        let approve = page_key(
            &temp,
            &mut domain,
            &mut model,
            &mut host,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(approve, BoardIntent::ReplyApprove);
        assert_eq!(model.reply_draft(), None, "the box closed");
        assert_ne!(model.input_mode(), BoardInputMode::EditReply);
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Done);
        let closed = task.past_blocks.last().expect("closed round");
        assert_eq!(closed.resolution, Some(crate::domain::Resolution::Approved));
        assert_eq!(
            closed.replies.last().map(|reply| reply.text.as_str()),
            Some("ship it")
        );
        assert_eq!(host.prompts, []);

        // On a blocked task's reply box the chord approves nothing.
        let temp = Temp::new("review-approve-blocked", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["blocked"]);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "x");
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplyApprove,
            &mut host,
        );
        assert_eq!(
            model.message(),
            Some("only a task in review can be approved")
        );
        assert_eq!(
            temp.store
                .load()
                .expect("reload")
                .get(ids[0])
                .expect("task")
                .status,
            HumanStatus::Blocked
        );
    }

    /// The feedback box opens inline under a review row too, with done and next above it.
    #[test]
    fn r_on_a_review_row_opens_the_feedback_box_inline() {
        let temp = Temp::new("review-row", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        review_page(&temp, &mut domain, &mut model, ids[0], &["tests pass"]);
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        let intent = key(&model, KeyCode::Char('r'), KeyModifiers::NONE);
        apply_intent(&mut domain, &mut model, intent, None).expect("r");
        assert_eq!(model.row_reply_task(), Some(ids[0]));
        let (screen, _) = board_screen(&model, 100, 24);
        assert!(screen.contains("done   built the card"), "{screen}");
        assert!(screen.contains("feedback to @builder…"), "{screen}");
    }

    /// A review handed to another agent rides IN MOTION with `△` and `on @pi`; on you it is
    /// `▲` in NEEDS YOU with the author and passed checks.
    #[test]
    fn a_review_on_another_agent_rides_in_motion() {
        let temp = Temp::new("review-elsewhere", &["builder", "pi"]);
        let (mut domain, mut model, ids) = board(&temp, &["mine", "theirs"]);
        for (id, on) in [
            (ids[0], crate::domain::BlockOn::You),
            (ids[1], crate::domain::BlockOn::Agent("pi".into())),
        ] {
            domain
                .review(
                    id,
                    crate::domain::ReviewDraft::from_input(
                        Some("done"),
                        &["a".into(), "b".into()],
                        None,
                        on,
                    )
                    .expect("draft"),
                    "claude",
                )
                .expect("review");
        }
        temp.store.reload_merge_save(&mut domain).expect("save");
        domain
            .set_check(ids[0], 0, crate::domain::CheckState::Passed)
            .expect("pass");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        let (screen, _) = board_screen(&model, 100, 24);
        let needs = screen.find("NEEDS YOU").expect("needs you");
        let motion = screen.find("IN MOTION").expect("in motion");
        let mine = screen
            .lines()
            .find(|line| line.contains("T1 mine"))
            .expect("mine row");
        let theirs = screen
            .lines()
            .find(|line| line.contains("T2 theirs"))
            .expect("theirs row");
        assert!(
            mine.contains("▲") && mine.contains("@claude · 1/2 ✓"),
            "{mine}"
        );
        assert!(
            theirs.contains("△") && theirs.contains("on @pi"),
            "{theirs}"
        );
        let at = |line: &str| screen.find(line).expect("row");
        assert!(needs < at(mine) && at(mine) < motion, "{screen}");
        assert!(motion < at(theirs), "{screen}");
    }

    /// A marked set's `ctrl+r` card puts every task up for review with one draft in one save,
    /// and one `ctrl+u` reverses the whole set.
    #[test]
    fn a_marked_set_review_card_is_one_batch_and_one_undo() {
        let temp = Temp::new("review-batch", &["builder", "pi"]);
        let (mut domain, mut model, ids) = board(&temp, &["one", "two"]);
        domain
            .set_status(ids[0], HumanStatus::Ready)
            .expect("ready");
        domain
            .set_status(ids[1], HumanStatus::Started)
            .expect("started");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ToggleMarkMode,
            &mut host,
        );
        for id in &ids {
            select(&mut domain, &mut model, *id);
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::MarkToggle,
                &mut host,
            );
        }
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ToggleReview,
            &mut host,
        );
        assert_eq!(model.input_mode(), BoardInputMode::BlockCard);
        let (screen, _) = board_screen(&model, 100, 24);
        assert!(screen.contains("review 2 tasks"), "{screen}");
        for text in ["shared work", "\n", "x"] {
            if text == "\n" {
                handle(
                    &temp,
                    &mut domain,
                    &mut model,
                    BoardIntent::BlockCardNextField,
                    &mut host,
                );
                continue;
            }
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::EditInsertText(text.into()),
                None,
            )
            .expect("type");
        }
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::BlockCardNewline,
            &mut host,
        );
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("y".into()),
            None,
        )
        .expect("type");
        // on: agent pi.
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::BlockCardNextField,
            &mut host,
        );
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::BlockCardNextField,
            &mut host,
        );
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::BlockCardRight,
            &mut host,
        );
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("pi".into()),
            None,
        )
        .expect("type");
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::BlockCardConfirm,
            &mut host,
        );
        assert_ne!(
            model.input_mode(),
            BoardInputMode::BlockCard,
            "the card closed"
        );
        assert_eq!(model.marked_count(), 0, "the marks cleared");
        let saved = temp.store.load().expect("reload");
        for id in &ids {
            let task = saved.get(*id).expect("task");
            assert_eq!(task.status, HumanStatus::Review);
            let round = task.block.as_ref().expect("round");
            assert_eq!(round.done.as_deref(), Some("shared work"));
            assert_eq!(
                round
                    .checks
                    .iter()
                    .map(|check| check.text.as_str())
                    .collect::<Vec<_>>(),
                ["x", "y"]
            );
            assert_eq!(round.on, crate::domain::BlockOn::Agent("pi".into()));
        }
        assert!(matches!(
            saved.last_undo(),
            Some(crate::domain::UndoEntry::Batch { entries }) if entries.len() == 2
        ));

        handle(&temp, &mut domain, &mut model, BoardIntent::Undo, &mut host);
        let saved = temp.store.load().expect("reload");
        assert_eq!(saved.get(ids[0]).expect("one").status, HumanStatus::Ready);
        assert_eq!(saved.get(ids[1]).expect("two").status, HumanStatus::Started);
        assert!(ids
            .iter()
            .all(|id| saved.get(*id).expect("task").block.is_none()));
    }

    /// The agent-visible `tsk list <n> --json` row of task `id`.
    fn agent_json(temp: &Temp, domain: &DomainState, id: uuid::Uuid) -> serde_json::Value {
        let number = domain.get(id).and_then(|task| task.number).expect("number");
        let output = crate::cli::run_with(
            [
                "tsk".to_string(),
                "list".into(),
                number.to_string(),
                "--json".into(),
                "--state-dir".into(),
                temp.dir.to_string_lossy().into_owned(),
            ],
            std::io::Cursor::new(Vec::<u8>::new()),
            true,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        serde_json::from_str::<serde_json::Value>(&output.stdout).expect("json")[0].clone()
    }

    /// What a launched agent is told and can read after a send-back that launched it: the
    /// prompt points at `past_reviews`, whose last entry holds this feedback and the failed
    /// checks, and `past_blocks` holds no review round.
    fn assert_send_back_visible(
        temp: &Temp,
        domain: &DomainState,
        id: uuid::Uuid,
        feedback: Option<&str>,
    ) {
        let saved = temp.store.load().expect("reload");
        let task = saved.get(id).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        let prompt = task
            .dispatch
            .as_ref()
            .and_then(|record| record.argv.last())
            .expect("launch prompt");
        assert!(prompt.contains("past_reviews"), "{prompt}");
        let json = agent_json(temp, domain, id);
        assert!(json.get("past_blocks").is_none(), "{json}");
        let round = json["past_reviews"]
            .as_array()
            .and_then(|rounds| rounds.last())
            .cloned()
            .expect("closed round");
        assert_eq!(round["resolution"], "sent_back");
        assert_eq!(
            round["checks"],
            serde_json::json!([
                {"text": "tests pass", "state": "open"},
                {"text": "no flicker", "state": "failed"}
            ])
        );
        match feedback {
            Some(text) => assert_eq!(round["feedback"][0]["text"], text),
            None => assert_eq!(round["feedback"], serde_json::json!([])),
        }
    }

    /// Review `id` with two checks, fail the second, save, and open the empty feedback box.
    fn failed_check_feedback_box(
        temp: &Temp,
        domain: &mut DomainState,
        model: &mut BoardModel,
        id: uuid::Uuid,
    ) {
        review_page(temp, domain, model, id, &["tests pass", "no flicker"]);
        domain
            .set_check(id, 1, crate::domain::CheckState::Failed)
            .expect("fail");
        temp.store.reload_merge_save(domain).expect("save");
        model.sync_from_domain(domain);
        apply_intent(domain, model, BoardIntent::BeginReply, None).expect("r");
    }

    /// `ctrl+s` with an empty box on a review never dispatched: the failed check is enough, the
    /// round closes as sent back and the dispatched agent is pointed at it.
    #[test]
    fn an_empty_send_back_with_a_failed_check_dispatches_the_agent() {
        let temp = Temp::new("review-send-back-dispatch", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        failed_check_feedback_box(&temp, &mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        assert_eq!(host.ran, 1, "one launch");
        assert_eq!(
            host.prompts,
            [],
            "a fresh launch is pointed at the record instead"
        );
        assert_eq!(model.reply_draft(), None, "the box closed");
        assert_send_back_visible(&temp, &domain, ids[0], None);
    }

    /// `ctrl+s` on a review whose agent is gone saves the feedback, asks, and the relaunched
    /// agent is pointed at the round with that feedback and the failed check.
    #[test]
    fn a_send_back_to_a_gone_agent_relaunches_it_onto_the_feedback() {
        let temp = Temp::new("review-send-back-gone", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        failed_check_feedback_box(&temp, &mut domain, &mut model, ids[0]);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText("fix the flicker".into()),
            None,
        )
        .expect("type");
        let mut host = fake_host(&temp);
        host.agent = Some(false);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        assert!(model
            .dispatch_prompt()
            .is_some_and(|prompt| prompt.relaunch.is_some()));
        assert_eq!(
            temp.store
                .load()
                .expect("reload")
                .get(ids[0])
                .expect("task")
                .status,
            HumanStatus::Review,
            "nothing starts before the relaunch is confirmed"
        );
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmDispatch,
            &mut host,
        );
        assert_eq!(host.ran, 1);
        assert_eq!(host.prompts, []);
        assert_send_back_visible(&temp, &domain, ids[0], Some("fix the flicker"));
    }

    /// An empty send-back with no failed check says what is missing and keeps the box, on
    /// every route; Shift+Enter keeps its own empty-reply refusal.
    #[test]
    fn an_empty_send_back_without_a_failed_check_is_refused() {
        let temp = Temp::new("review-send-back-empty", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["plain", "assigned"]);
        domain
            .assign(ids[1], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        let mut host = fake_host(&temp);
        for id in ids.clone() {
            review_page(&temp, &mut domain, &mut model, id, &["tests pass"]);
            apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ReplySaveUnblock,
                &mut host,
            );
            assert_eq!(model.reply_draft(), Some(""), "the box stays");
            let (screen, _) = board_screen(&model, 100, 30);
            assert!(
                screen.contains(crate::ui::board::NOTHING_TO_SEND_BACK),
                "{screen}"
            );
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ReplySave,
                &mut host,
            );
            let (screen, _) = board_screen(&model, 100, 30);
            assert!(screen.contains("type a reply first"), "{screen}");
            assert_eq!(
                temp.store
                    .load()
                    .expect("reload")
                    .get(id)
                    .expect("task")
                    .status,
                HumanStatus::Review
            );
            apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("esc");
            apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close");
        }
        assert_eq!(host.ran, 0, "nothing launched");
    }

    /// `ctrl+d` with an empty box approves only the round the box was opened on: a newer round
    /// merged in meanwhile is refused and the box stays.
    #[test]
    fn approving_an_empty_box_refuses_a_round_replaced_meanwhile() {
        let temp = Temp::new("review-approve-stale", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        review_page(&temp, &mut domain, &mut model, ids[0], &["tests pass"]);
        apply_intent(&mut domain, &mut model, BoardIntent::BeginReply, None).expect("r");
        elsewhere(&temp, |other| {
            other
                .set_status_by(ids[0], HumanStatus::Started, "builder")
                .expect("started elsewhere");
        });
        elsewhere(&temp, |other| {
            other
                .set_status_by(ids[0], HumanStatus::Review, "builder")
                .expect("round 2 elsewhere");
        });
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplyApprove,
            &mut host,
        );
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Review, "round 2 is not approved");
        assert_eq!(task.block.as_ref().map(|round| round.round), Some(2));
        assert_eq!(model.reply_draft(), Some(""), "the box stays");
        assert_eq!(model.message(), Some(crate::ui::board::BLOCK_REPLACED));
    }

    /// `ctrl+d` on a dispatched review with its agent running: the feedback is stored, the
    /// cleanup card asks, Esc approves nothing, and confirming closes the round as approved.
    /// Nothing is ever sent to the agent.
    #[test]
    fn approving_a_dispatched_review_goes_through_the_cleanup_card() {
        let temp = Temp::new("review-approve-dispatched", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["review me"]);
        let worktree = temp.dir.join("worktree");
        std::fs::create_dir_all(&worktree).expect("worktree");
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign builder");
        temp.store.reload_merge_save(&mut domain).expect("save");
        domain
            .record_dispatch(
                ids[0],
                crate::domain::Dispatch {
                    argv: vec!["true".into()],
                    worktree: worktree.to_string_lossy().into_owned(),
                    branch: "tsk/earlier".into(),
                    base: Some("main".into()),
                    base_ref: None,
                    base_commit: None,
                    base_remote: None,
                    herdr_workspace_id: "w0".into(),
                    at: std::time::SystemTime::now(),
                    cleaned: false,
                },
            )
            .expect("record the launch");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        review_page(&temp, &mut domain, &mut model, ids[0], &["tests pass"]);
        let mut host = fake_host(&temp);
        host.cleanup = true;
        agent_running(&domain, &mut host, ids[0]);
        let approve = |domain: &mut DomainState, model: &mut BoardModel, host: &mut FakeHost| {
            apply_intent(domain, model, BoardIntent::BeginReply, None).expect("r");
            apply_intent(
                domain,
                model,
                BoardIntent::EditInsertText("ship it".into()),
                None,
            )
            .expect("type");
            handle(&temp, domain, model, BoardIntent::ReplyApprove, host);
        };
        approve(&mut domain, &mut model, &mut host);
        assert!(
            model.cleanup_prompt().is_some(),
            "the cleanup card asks: {:?}",
            model.message()
        );
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(
            task.status,
            HumanStatus::Review,
            "nothing changes before confirm"
        );
        assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("ship it"));

        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::CancelCleanup,
            &mut host,
        );
        assert!(model.cleanup_prompt().is_none());
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Review,
            "cancel approves nothing"
        );

        approve(&mut domain, &mut model, &mut host);
        assert!(model.cleanup_prompt().is_some());
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::KeepCleanup,
            &mut host,
        );
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Done);
        let round = task.past_blocks.last().expect("closed round");
        assert_eq!(round.resolution, Some(crate::domain::Resolution::Approved));
        assert_eq!(host.prompts, [], "approving never sends");
    }

    #[test]
    fn reply_and_unblock_on_an_unassigned_task_still_goes_to_ready() {
        let temp = Temp::new("reply-unassigned", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["mine"]);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        let saved = temp.store.load().expect("reload");
        assert_eq!(saved.get(ids[0]).expect("task").status, HumanStatus::Ready);
        assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("postgres"));
        assert_eq!(host.ran, 0);
    }

    #[test]
    fn reply_and_unblock_on_an_assigned_task_saves_the_reply_then_dispatches() {
        let temp = Temp::new("reply-dispatch", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["send after answer"]);
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        assert_eq!(host.ran, 1, "one launch");
        assert_eq!(
            host.replies_on_disk_at_launch,
            ["postgres"],
            "the reply was saved before the launch began"
        );
        assert!(host.prompts.is_empty(), "a fresh launch reads past_blocks");
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        assert!(task.dispatch.is_some());
        assert!(task.block.is_none(), "the start closed the block");
        assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("postgres"));
    }

    #[test]
    fn reply_and_unblock_with_a_failed_launch_keeps_the_reply_and_the_block() {
        let temp = Temp::new("reply-fails", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["launch fails"]);
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        host.fail_launch = Some("herdr is down".into());
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        assert_eq!(model.message(), Some("herdr is down"));
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Blocked);
        assert!(task.dispatch.is_none());
        assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("postgres"));
    }

    #[test]
    fn reply_and_unblock_with_a_running_agent_starts_without_a_launch() {
        let temp = Temp::new("reply-running", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["agent waits"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        let tag = agent_running(&domain, &mut host, ids[0]);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        assert_eq!(host.ran, 0);
        assert_eq!(model.dispatch_prompt(), None);
        assert_eq!(
            host.prompts,
            [("w0:p1".to_string(), format!("{tag} postgres"))],
            "the reply went to the recorded workspace's agent once"
        );
        assert_eq!(
            host.root_queries,
            ["w0"],
            "the recorded workspace was asked"
        );
        assert_eq!(
            host.disk_at_prompt,
            [(HumanStatus::Started, Some("postgres".to_string()))],
            "the reply and the start were durable before the prompt"
        );
        assert_eq!(model.message(), Some("started · reply sent to @builder"));
    }

    /// `ctrl+s` on an empty box only unblocks, and tells the agent so with the bare tag.
    #[test]
    fn an_empty_reply_box_unblocks_and_sends_the_bare_tag() {
        let temp = Temp::new("reply-empty", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["agent waits"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "");
        let mut host = fake_host(&temp);
        let tag = agent_running(&domain, &mut host, ids[0]);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        assert_eq!(host.prompts, [("w0:p1".to_string(), tag)]);
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        assert!(task.past_blocks.last().expect("closed").replies.is_empty());
        assert_eq!(model.reply_draft(), None, "the box closed on landing");
    }

    /// Each send carries only the reply that action stored: an earlier answer is never sent
    /// again, even with no agent reply in between.
    #[test]
    fn two_sends_in_a_row_never_repeat_a_reply() {
        let temp = Temp::new("reply-twice", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["agent asks twice"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "use postgres");
        let mut host = fake_host(&temp);
        let tag = agent_running(&domain, &mut host, ids[0]);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        // Blocked again; the owner's earlier reply sits on the closed block and a new one
        // is typed with a line break.
        reply_on_blocked(
            &temp,
            &mut domain,
            &mut model,
            ids[0],
            "add an index\n\"quoted\" too",
        );
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        let texts: Vec<&str> = host.prompts.iter().map(|(_, text)| text.as_str()).collect();
        assert_eq!(
            texts,
            [
                format!("{tag} use postgres"),
                format!("{tag} add an index\n\"quoted\" too")
            ]
        );
    }

    /// The agent waits on a permission prompt, Herdr fails, Herdr cannot list the panes, or
    /// cannot say who runs: the task stays started, the reply stays on the task, and nothing
    /// is tried twice.
    #[test]
    fn a_refused_or_failed_delivery_keeps_the_task_started() {
        use crate::dispatch::PromptError;
        // (agent, root pane fails, prompt error, sends, message)
        let cases = [
            (
                Some(true),
                false,
                Some(PromptError::AgentBlocked),
                1,
                "started · @builder is waiting on a prompt; reply kept on the task",
            ),
            (
                Some(true),
                false,
                Some(PromptError::Failed("herdr: boom".into())),
                1,
                "started · could not reach @builder; reply kept on the task",
            ),
            (
                None,
                false,
                None,
                0,
                "started · could not reach @builder; reply kept on the task",
            ),
            (
                Some(true),
                true,
                None,
                0,
                "started · could not reach @builder; reply kept on the task",
            ),
        ];
        for (agent, root_failed, error, sends, message) in cases {
            let temp = Temp::new("reply-delivery-fails", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["agent busy"]);
            dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
            reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
            let mut host = fake_host(&temp);
            agent_running(&domain, &mut host, ids[0]);
            host.agent = agent;
            host.root_failed = root_failed;
            host.prompt_error = error;
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ReplySaveUnblock,
                &mut host,
            );
            assert_eq!(host.prompts.len(), sends, "{message}");
            assert_eq!(host.ran, 0, "never relaunches");
            assert_eq!(model.dispatch_prompt(), None);
            assert_eq!(model.message(), Some(message));
            let saved = temp.store.load().expect("reload");
            assert_eq!(
                saved.get(ids[0]).expect("task").status,
                HumanStatus::Started
            );
            assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("postgres"));
        }
    }

    /// Another agent in the recorded pane, an unnamed one (naming failed), or a pane that
    /// changes hands between the check and the send: nothing is sent, and the start stays.
    #[test]
    fn a_reply_never_reaches_an_agent_tsk_did_not_dispatch() {
        let not_in_pane =
            "started · reply not sent: @builder is not in its pane; reply kept on the task";
        for case in ["other", "unnamed", "swapped"] {
            let temp = Temp::new("reply-wrong-agent", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["pane reused"]);
            dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
            reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "secret");
            let mut host = fake_host(&temp);
            let number = domain.get(ids[0]).and_then(|task| task.number).expect("n");
            let expected = crate::dispatch::agent_name(number, "builder");
            host.agent = Some(true);
            match case {
                "other" => {
                    host.names.insert("w0:p1".into(), "reviewer".into());
                }
                "unnamed" => {}
                _ => {
                    host.next_names = [Some(expected.clone()), Some("t99-other".into())].into();
                }
            }
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ReplySaveUnblock,
                &mut host,
            );
            assert!(host.prompts.is_empty(), "{case}: nothing sent");
            assert!(
                host.agent_queries.iter().all(|pane| pane == "w0:p1"),
                "{case}: only the recorded pane was asked"
            );
            assert_eq!(model.message(), Some(not_in_pane), "{case}");
            assert_eq!(model.dispatch_prompt(), None, "{case}: the start stays");
            let saved = temp.store.load().expect("reload");
            assert_eq!(
                saved.get(ids[0]).expect("task").status,
                HumanStatus::Started,
                "{case}"
            );
        }
    }

    /// Outside Herdr a dispatched task still starts, and the status row says the reply did
    /// not go anywhere.
    #[test]
    fn a_reply_outside_herdr_starts_and_says_it_was_not_sent() {
        let temp = Temp::new("reply-outside-herdr", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["plain terminal"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        agent_running(&domain, &mut host, ids[0]);
        let mut recovery = SaveRecovery::new();
        handle_board_intent_with_host(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut recovery,
            false,
            false,
            &mut host,
            &mut |_| {},
        )
        .expect("intent");
        assert!(host.prompts.is_empty());
        assert!(host.root_queries.is_empty(), "Herdr is never asked");
        assert_eq!(
            model.message(),
            Some("started · reply not sent: not in Herdr")
        );
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );
    }

    /// Shift+Enter stores the reply only: nothing is sent, even to a running agent.
    #[test]
    fn a_reply_saved_without_unblocking_sends_nothing() {
        let temp = Temp::new("reply-save-only", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["agent waits"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        agent_running(&domain, &mut host, ids[0]);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySave,
            &mut host,
        );
        assert!(host.prompts.is_empty());
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Blocked
        );
        assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("postgres"));
    }

    #[test]
    fn reply_and_unblock_with_the_agent_gone_saves_the_reply_and_asks() {
        let temp = Temp::new("reply-gone", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["agent left"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        host.agent = Some(false);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        assert!(model
            .dispatch_prompt()
            .is_some_and(|prompt| prompt.relaunch.is_some()));
        assert_eq!(host.ran, 0);
        assert!(host.prompts.is_empty(), "a gone agent gets nothing");
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Blocked
        );
        assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("postgres"));
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmDispatch,
            &mut host,
        );
        assert_eq!(host.ran, 1);
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );
    }

    #[test]
    fn ctrl_s_on_the_task_page_dispatches_the_page_task() {
        let temp = Temp::new("page-start", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["other", "on the page"]);
        domain
            .assign(ids[1], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        select(&mut domain, &mut model, ids[1]);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        let mut host = fake_host(&temp);
        let intent = key(&model, KeyCode::Char('s'), KeyModifiers::CONTROL);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        assert_eq!(host.ran, 1);
        let saved = temp.store.load().expect("reload");
        assert!(saved.get(ids[1]).expect("task").dispatch.is_some());
        assert!(saved.get(ids[0]).expect("task").dispatch.is_none());
    }

    /// Another writer's change on disk, saved through a fresh copy of the store.
    fn elsewhere(temp: &Temp, change: impl FnOnce(&mut DomainState)) {
        let mut other = temp.store.load().expect("load");
        change(&mut other);
        temp.store
            .reload_merge_save(&mut other)
            .expect("save elsewhere");
    }

    fn assigned_board(temp: &Temp, titles: &[&str]) -> (DomainState, BoardModel, Vec<uuid::Uuid>) {
        let (mut domain, mut model, ids) = board(temp, titles);
        for id in &ids {
            domain.assign(*id, Some("builder".into())).expect("assign");
        }
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        (domain, model, ids)
    }

    /// F-9: a refresh that drops the invoked task never starts the cursor's new task.
    #[test]
    fn a_start_whose_target_was_archived_elsewhere_starts_nothing() {
        let temp = Temp::new("start-target-gone", &["builder"]);
        let (mut domain, mut model, ids) = assigned_board(&temp, &["archived elsewhere", "next"]);
        select(&mut domain, &mut model, ids[0]);
        elsewhere(&temp, |other| {
            other.archive_task(ids[0]).expect("archive");
        });
        let mut host = fake_host(&temp);
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        assert_eq!(model.message(), Some(super::START_TARGET_CHANGED));
        let saved = temp.store.load().expect("reload");
        assert_eq!(saved.get(ids[1]).expect("task").status, HumanStatus::Open);
        assert_eq!(saved.get(ids[0]).expect("task").status, HumanStatus::Open);
        assert_eq!(host.ran, 0);
    }

    /// F-12: an assignment made elsewhere launches, though this board has not refreshed.
    #[test]
    fn a_start_on_a_task_assigned_elsewhere_launches() {
        let temp = Temp::new("start-assigned-elsewhere", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["assigned by the cli"]);
        select(&mut domain, &mut model, ids[0]);
        elsewhere(&temp, |other| {
            other
                .assign(ids[0], Some("builder".into()))
                .expect("assign");
        });
        let mut host = fake_host(&temp);
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        assert_eq!(host.ran, 1);
        let saved = temp.store.load().expect("reload");
        assert!(saved.get(ids[0]).expect("task").dispatch.is_some());
    }

    #[test]
    fn a_reply_on_a_task_assigned_elsewhere_dispatches() {
        let temp = Temp::new("reply-assigned-elsewhere", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["assigned by the cli"]);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        elsewhere(&temp, |other| {
            other
                .assign(ids[0], Some("builder".into()))
                .expect("assign");
        });
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut host,
        );
        assert_eq!(host.ran, 1);
        assert_eq!(host.replies_on_disk_at_launch, ["postgres"]);
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );
    }

    /// F-3/F-4/F-8: only Herdr's `workspace_not_found` means gone; any other root-pane failure
    /// is a plain start, and an explicit relaunch refuses instead of opening a second workspace.
    #[test]
    fn a_failed_root_pane_query_is_a_plain_start_and_never_reopens() {
        let temp = Temp::new("root-failed", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["herdr is busy"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Ready);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        host.root_failed = true;
        host.agent = Some(false);
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        assert_eq!(model.dispatch_prompt(), None, "no relaunch card");
        assert_eq!(
            domain.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );

        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::DispatchAgain,
            &mut host,
        );
        assert_eq!(model.message(), Some("could not run herdr"));
        assert_eq!((host.reopened, host.ran, host.launched), (0, 0, 0));
    }

    /// F-16: outside Herdr a dispatched task's start is plain; nothing is asked or launched.
    #[test]
    fn a_dispatched_start_outside_herdr_is_plain() {
        let temp = Temp::new("dispatched-no-herdr", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["no herdr"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Ready);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        host.agent = Some(false);
        host.workspace_gone = true;
        let mut recovery = SaveRecovery::new();
        let intent = ctrl_s(&model);
        handle_board_intent_with_host(
            &temp.store,
            &mut domain,
            &mut model,
            intent,
            &mut recovery,
            false,
            false,
            &mut host,
            &mut |_| {},
        )
        .expect("start");
        assert_eq!(model.dispatch_prompt(), None);
        assert_eq!(host.ran, 0);
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );
    }

    /// F-19: palette start on an assigned, never-dispatched done task is a plain correction.
    #[test]
    fn an_assigned_done_task_starts_without_a_launch() {
        let temp = Temp::new("done-start", &["builder"]);
        let (mut domain, mut model, ids) = assigned_board(&temp, &["finished"]);
        domain.complete(ids[0]).expect("done");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::SetStatus(HumanStatus::Started),
            &mut host,
        );
        assert_eq!(host.ran, 0);
        assert_eq!(model.dispatch_prompt(), None);
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        assert!(task.dispatch.is_none());
    }

    /// F-11: the relaunch card's `n` rechecks the task; one finished meanwhile stays done.
    #[test]
    fn n_on_the_relaunch_card_leaves_a_task_finished_meanwhile() {
        let temp = Temp::new("relaunch-n-done", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["finished meanwhile"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Ready);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        host.agent = Some(false);
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        assert!(model.dispatch_prompt().is_some());
        elsewhere(&temp, |other| other.complete(ids[0]).expect("done"));
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::StartWithoutRelaunch,
            &mut host,
        );
        let saved = temp.store.load().expect("reload");
        assert_eq!(saved.get(ids[0]).expect("task").status, HumanStatus::Done);
    }

    /// F-18 and F-1/F-6: a failed reply save launches nothing and keeps the box; Retry saves
    /// the reply and then dispatches; Cancel drops the start.
    #[cfg(unix)]
    #[test]
    fn a_failed_reply_save_launches_nothing_and_retry_resumes_the_start() {
        use std::os::unix::fs::PermissionsExt;
        for retry in [true, false] {
            let temp = Temp::new("reply-save-fails", &["builder"]);
            let (mut domain, mut model, ids) = assigned_board(&temp, &["answer then go"]);
            reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
            let mut host = fake_host(&temp);
            let mut recovery = SaveRecovery::new();
            let step = |domain: &mut DomainState,
                        model: &mut BoardModel,
                        recovery: &mut SaveRecovery<DomainState>,
                        host: &mut FakeHost,
                        intent: BoardIntent| {
                handle_board_intent_with_host(
                    &temp.store,
                    domain,
                    model,
                    intent,
                    recovery,
                    false,
                    true,
                    host,
                    &mut |_| {},
                )
                .expect("intent");
            };
            std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o555))
                .expect("lock the state dir");
            step(
                &mut domain,
                &mut model,
                &mut recovery,
                &mut host,
                BoardIntent::ReplySaveUnblock,
            );
            std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o700))
                .expect("unlock");
            assert!(recovery.is_pending(), "the save failed into recovery");
            assert_eq!(host.ran, 0, "nothing launches before the reply is saved");
            assert_eq!(model.reply_draft(), Some("postgres"));

            let answer = if retry {
                BoardIntent::RetrySave
            } else {
                BoardIntent::CancelSave
            };
            step(&mut domain, &mut model, &mut recovery, &mut host, answer);
            assert!(!recovery.is_pending());
            assert_eq!(model.pending_reply_start, None);
            let saved = temp.store.load().expect("reload");
            let task = saved.get(ids[0]).expect("task");
            if retry {
                assert_eq!(host.ran, 1, "retry resumed the start");
                assert_eq!(host.replies_on_disk_at_launch, ["postgres"]);
                assert_eq!(task.status, HumanStatus::Started);
            } else {
                assert_eq!(host.ran, 0, "cancel dropped the start");
                assert_eq!(task.status, HumanStatus::Blocked);
            }
        }
    }

    #[cfg(unix)]
    fn recovering_step(
        temp: &Temp,
        domain: &mut DomainState,
        model: &mut BoardModel,
        recovery: &mut SaveRecovery<DomainState>,
        host: &mut FakeHost,
        intent: BoardIntent,
    ) {
        handle_board_intent_with_host(
            &temp.store,
            domain,
            model,
            intent,
            recovery,
            false,
            true,
            host,
            &mut |_| {},
        )
        .expect("intent");
    }

    /// Run `intent` with the state dir read-only, so its save fails into recovery.
    #[cfg(unix)]
    fn failing_step(
        temp: &Temp,
        domain: &mut DomainState,
        model: &mut BoardModel,
        recovery: &mut SaveRecovery<DomainState>,
        host: &mut FakeHost,
        intent: BoardIntent,
    ) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o555))
            .expect("lock the state dir");
        recovering_step(temp, domain, model, recovery, host, intent);
        std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o700))
            .expect("unlock");
        assert!(recovery.is_pending(), "the save failed into recovery");
    }

    /// A failed reply save to a running agent sends nothing; Retry sends it exactly once,
    /// after the save, and Cancel never.
    #[cfg(unix)]
    #[test]
    fn a_failed_reply_save_sends_nothing_until_retry_saves_it() {
        for retry in [true, false] {
            let temp = Temp::new("reply-deliver-recovery", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["agent waits"]);
            dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
            reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
            let mut host = fake_host(&temp);
            let tag = agent_running(&domain, &mut host, ids[0]);
            let mut recovery = SaveRecovery::new();
            failing_step(
                &temp,
                &mut domain,
                &mut model,
                &mut recovery,
                &mut host,
                BoardIntent::ReplySaveUnblock,
            );
            assert!(host.prompts.is_empty(), "nothing before the save lands");
            let answer = if retry {
                BoardIntent::RetrySave
            } else {
                BoardIntent::CancelSave
            };
            recovering_step(
                &temp,
                &mut domain,
                &mut model,
                &mut recovery,
                &mut host,
                answer,
            );
            assert!(!recovery.is_pending());
            // A later save must not send again.
            recovering_step(
                &temp,
                &mut domain,
                &mut model,
                &mut recovery,
                &mut host,
                BoardIntent::SelectNext,
            );
            assert_eq!(model.pending_reply_delivery, None);
            assert_eq!(model.pending_reply_text, None);
            let saved = temp.store.load().expect("reload");
            let task = saved.get(ids[0]).expect("task");
            if retry {
                assert_eq!(
                    host.prompts,
                    [("w0:p1".to_string(), format!("{tag} postgres"))]
                );
                assert_eq!(
                    host.disk_at_prompt,
                    [(HumanStatus::Started, Some("postgres".to_string()))]
                );
                assert_eq!(task.status, HumanStatus::Started);
            } else {
                assert!(host.prompts.is_empty(), "cancel sent nothing");
                assert_eq!(task.status, HumanStatus::Blocked);
            }
        }
    }

    /// The agent looked gone at `ctrl+s` (Herdr's detection lagged) and the save failed; on
    /// Retry it is found running: the resumed start sends the reply exactly once.
    #[cfg(unix)]
    #[test]
    fn a_resumed_start_delivers_to_an_agent_found_on_retry() {
        let temp = Temp::new("reply-resumed", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["detection lags"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        let tag = agent_running(&domain, &mut host, ids[0]);
        host.agent = Some(false);
        let mut recovery = SaveRecovery::new();
        failing_step(
            &temp,
            &mut domain,
            &mut model,
            &mut recovery,
            &mut host,
            BoardIntent::ReplySaveUnblock,
        );
        assert!(model.pending_reply_start.is_some());
        assert!(host.prompts.is_empty());
        host.agent = Some(true);
        recovering_step(
            &temp,
            &mut domain,
            &mut model,
            &mut recovery,
            &mut host,
            BoardIntent::RetrySave,
        );
        recovering_step(
            &temp,
            &mut domain,
            &mut model,
            &mut recovery,
            &mut host,
            BoardIntent::SelectNext,
        );
        assert_eq!(host.ran, 0, "no launch");
        assert_eq!(model.dispatch_prompt(), None);
        assert_eq!(
            host.prompts,
            [("w0:p1".to_string(), format!("{tag} postgres"))]
        );
        assert_eq!(
            host.disk_at_prompt,
            [(HumanStatus::Started, Some("postgres".to_string()))]
        );
        assert_eq!(model.message(), Some("started · reply sent to @builder"));
    }

    /// The dispatch record changed (a relaunch elsewhere) while the save waited on Retry:
    /// the old workspace's pane, now someone else's, is never asked or sent to. The changed
    /// task refuses the held save, and Cancel drops the delivery.
    #[cfg(unix)]
    #[test]
    fn a_dispatch_record_changed_before_retry_sends_nothing_to_the_old_pane() {
        let temp = Temp::new("reply-record-moved", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["relaunched meanwhile"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Started);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        agent_running(&domain, &mut host, ids[0]);
        let mut recovery = SaveRecovery::new();
        failing_step(
            &temp,
            &mut domain,
            &mut model,
            &mut recovery,
            &mut host,
            BoardIntent::ReplySaveUnblock,
        );
        elsewhere(&temp, |other| {
            let mut record = other
                .get(ids[0])
                .and_then(|task| task.dispatch.clone())
                .expect("record");
            record.herdr_workspace_id = "w5".into();
            other.record_dispatch(ids[0], record).expect("relaunch");
        });
        let name = host.names.remove("w0:p1").expect("old agent");
        host.names.insert("w0:p1".into(), "someone-else".into());
        host.names.insert("w5:p1".into(), name);
        host.root_queries.clear();
        host.agent_queries.clear();
        for answer in [BoardIntent::RetrySave, BoardIntent::CancelSave] {
            recovering_step(
                &temp,
                &mut domain,
                &mut model,
                &mut recovery,
                &mut host,
                answer,
            );
        }
        assert!(!recovery.is_pending());
        assert!(host.prompts.is_empty(), "nothing sent anywhere");
        assert!(
            !host.root_queries.iter().any(|workspace| workspace == "w0")
                && !host.agent_queries.iter().any(|pane| pane == "w0:p1"),
            "the old record was never used: {:?} {:?}",
            host.root_queries,
            host.agent_queries
        );
        assert_eq!(model.pending_reply_delivery, None);
    }

    #[test]
    fn ctrl_g_no_longer_dispatches() {
        let temp = Temp::new("no-ctrl-g", &["builder"]);
        let (_, model, _) = board(&temp, &["assigned"]);
        assert_eq!(
            map_key(
                model.input_mode(),
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)
            ),
            None
        );
    }

    #[test]
    fn ctrl_s_on_an_unassigned_task_only_starts() {
        let temp = Temp::new("start-unassigned", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["do it myself"]);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        assert!(task.dispatch.is_none());
        assert_eq!((host.launched, host.ran), (0, 0), "nothing launched");
    }

    #[test]
    fn ctrl_s_on_an_assigned_task_dispatches_it() {
        let temp = Temp::new("start-assigned", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["send it"]);
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        let mut names = Vec::new();
        let mut recovery = SaveRecovery::new();
        let intent = ctrl_s(&model);
        handle_board_intent_with_host(
            &temp.store,
            &mut domain,
            &mut model,
            intent,
            &mut recovery,
            false,
            true,
            &mut host,
            &mut |naming| names.push(naming.name),
        )
        .expect("start");
        assert_eq!((host.launched, host.ran), (1, 1), "one launch");
        let number = domain
            .get(ids[0])
            .and_then(|task| task.number)
            .expect("number");
        assert_eq!(names, [format!("t{number}-builder")]);
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        assert!(
            task.dispatch.is_some(),
            "the record is saved with the start"
        );
        assert_eq!(
            model.message().map(str::to_string),
            Some(format!("dispatched T{number} to @builder"))
        );
    }

    #[test]
    fn a_failed_dispatch_leaves_the_task_unstarted_with_the_reason() {
        let temp = Temp::new("start-fails", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["will not launch"]);
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        domain
            .set_status(ids[0], HumanStatus::Ready)
            .expect("ready");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        host.fail_launch = Some("herdr is down".into());
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        assert_eq!(model.message(), Some("herdr is down"));
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Ready);
        assert!(task.dispatch.is_none());
    }

    #[test]
    fn ctrl_s_outside_herdr_starts_an_assigned_task_and_says_why() {
        let temp = Temp::new("start-no-herdr", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["needs herdr"]);
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        let mut recovery = SaveRecovery::new();
        let intent = ctrl_s(&model);
        handle_board_intent_with_host(
            &temp.store,
            &mut domain,
            &mut model,
            intent,
            &mut recovery,
            false,
            false,
            &mut host,
            &mut |_| {},
        )
        .expect("start");
        assert_eq!(model.message(), Some("started · no launch: not in Herdr"));
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Started);
        assert!(task.dispatch.is_none());
        assert_eq!((host.ran, host.launched), (0, 0));
    }

    /// Outside Herdr a marked set starts plainly with no card and names what could not launch.
    #[test]
    fn a_marked_set_outside_herdr_starts_and_names_the_rows_that_could_not_launch() {
        let temp = Temp::new("set-no-herdr", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["assigned", "mine"]);
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None).expect("mark");
        for id in &ids {
            select(&mut domain, &mut model, *id);
            apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark");
        }
        let mut host = fake_host(&temp);
        let mut recovery = SaveRecovery::new();
        handle_board_intent_with_host(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::PrimaryVerb,
            &mut recovery,
            false,
            false,
            &mut host,
            &mut |_| {},
        )
        .expect("start");
        assert_eq!(model.dispatch_prompt(), None);
        let number = domain
            .get(ids[0])
            .and_then(|task| task.number)
            .expect("number");
        assert_eq!(
            model.message().map(str::to_string),
            Some(format!("started · no launch: not in Herdr (T{number})"))
        );
        let saved = temp.store.load().expect("reload");
        for id in &ids {
            assert_eq!(saved.get(*id).expect("task").status, HumanStatus::Started);
        }
        assert_eq!(host.ran, 0);
    }

    /// The reply box outside Herdr unblocks an assigned task to started and says why.
    #[test]
    fn reply_and_unblock_outside_herdr_starts_and_says_why() {
        let temp = Temp::new("reply-no-herdr", &["builder"]);
        let (mut domain, mut model, ids) = assigned_board(&temp, &["answer"]);
        reply_on_blocked(&temp, &mut domain, &mut model, ids[0], "postgres");
        let mut host = fake_host(&temp);
        let mut recovery = SaveRecovery::new();
        handle_board_intent_with_host(
            &temp.store,
            &mut domain,
            &mut model,
            BoardIntent::ReplySaveUnblock,
            &mut recovery,
            false,
            false,
            &mut host,
            &mut |_| {},
        )
        .expect("reply");
        assert_eq!(model.message(), Some("started · no launch: not in Herdr"));
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );
        assert_eq!(last_reply(&saved, ids[0]).as_deref(), Some("postgres"));
        assert_eq!(host.ran, 0);
    }

    /// On the desk tab, the palette start on a set with a project launch lists an assigned
    /// desk task under start only with why it cannot launch, and starts it on `y`.
    #[test]
    fn a_desk_task_in_a_launching_set_is_start_only_with_the_reason() {
        let temp = Temp::new("set-desk", &["builder"]);
        let mut domain = DomainState::new();
        let project = domain
            .create_assigned(
                "project review",
                None,
                TaskScope::Project {
                    path: PROJECT.into(),
                },
                ProvenanceOrigin::Manual,
                None,
                Some("builder".into()),
            )
            .expect("project task");
        let desk = domain
            .create_assigned(
                "desk errand",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
                Some("builder".into()),
            )
            .expect("desk task");
        temp.store.reload_merge_save(&mut domain).expect("save");
        domain
            .set_status(project, HumanStatus::Review)
            .expect("review");
        temp.store.reload_merge_save(&mut domain).expect("save");
        let mut model = BoardModel::from_domain(&domain, None);
        model.set_agent_profiles(&temp.profiles());
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None).expect("mark");
        for id in [project, desk] {
            select(&mut domain, &mut model, id);
            apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark");
        }
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::SetStatus(HumanStatus::Started),
            &mut host,
        );
        let prompt = model.dispatch_prompt().expect("card").clone();
        assert_eq!(prompt.launch.len(), 1);
        assert_eq!(prompt.no_launch, [(desk, dispatch::NO_LAUNCH_DESK)]);
        let painted = frame_text(&model, Rect::new(0, 0, 90, 30));
        assert!(
            painted.contains("started · no launch: desk task has no repository"),
            "{painted}"
        );
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmDispatch,
            &mut host,
        );
        assert_eq!(host.ran, 1, "only the project task launches");
        let saved = temp.store.load().expect("reload");
        assert_eq!(saved.get(desk).expect("task").status, HumanStatus::Started);
        assert!(saved.get(desk).expect("task").dispatch.is_none());
        assert!(saved.get(project).expect("task").dispatch.is_some());
    }

    #[test]
    fn ctrl_s_with_the_agent_still_running_is_a_plain_start() {
        let temp = Temp::new("start-running", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["agent still here"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Ready);
        select(&mut domain, &mut model, ids[0]);
        for agent in [Some(true), None] {
            let mut host = fake_host(&temp);
            host.agent = agent;
            domain
                .set_status(ids[0], HumanStatus::Ready)
                .expect("ready");
            temp.store.reload_merge_save(&mut domain).expect("save");
            model.sync_from_domain(&domain);
            let intent = ctrl_s(&model);
            handle(&temp, &mut domain, &mut model, intent, &mut host);
            assert_eq!(
                domain.get(ids[0]).expect("task").status,
                HumanStatus::Started,
                "{agent:?}"
            );
            assert_eq!((host.launched, host.ran), (0, 0), "{agent:?}: no launch");
            assert_eq!(model.dispatch_prompt(), None, "{agent:?}: no card");
        }
    }

    #[test]
    fn ctrl_s_with_the_agent_gone_asks_before_relaunching() {
        let temp = Temp::new("start-gone", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["agent left"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Ready);
        select(&mut domain, &mut model, ids[0]);

        // Esc changes nothing.
        let mut host = fake_host(&temp);
        host.agent = Some(false);
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        let prompt = model.dispatch_prompt().expect("relaunch card").clone();
        let relaunch = prompt.relaunch.expect("relaunch prompt");
        assert_eq!(relaunch.assignee, "builder");
        assert_eq!(model.input_mode(), BoardInputMode::DispatchConfirm);
        let esc = key(&model, KeyCode::Esc, KeyModifiers::NONE);
        handle(&temp, &mut domain, &mut model, esc, &mut host);
        assert_eq!(model.dispatch_prompt(), None);
        assert_eq!(domain.get(ids[0]).expect("task").status, HumanStatus::Ready);

        // `n` only starts.
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        let n = key(&model, KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(n, BoardIntent::StartWithoutRelaunch);
        handle(&temp, &mut domain, &mut model, n, &mut host);
        assert_eq!(model.dispatch_prompt(), None);
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );
        assert_eq!(host.ran, 0, "n never launches");

        // `y` relaunches in the recorded workspace.
        domain
            .set_status(ids[0], HumanStatus::Ready)
            .expect("ready");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        let y = key(&model, KeyCode::Char('y'), KeyModifiers::NONE);
        handle(&temp, &mut domain, &mut model, y, &mut host);
        assert_eq!(
            (host.launched, host.ran),
            (0, 1),
            "relaunched in the kept worktree"
        );
        let saved = temp.store.load().expect("reload");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );
    }

    #[test]
    fn a_gone_workspace_or_cleaned_record_counts_as_the_agent_gone() {
        let temp = Temp::new("start-workspace-gone", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["workspace closed"]);
        dispatched_before(&temp, &mut domain, &mut model, ids[0], HumanStatus::Review);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        host.workspace_gone = true;
        host.agent = Some(true);
        // The palette's absolute status takes the same route as ctrl+s.
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::SetStatus(HumanStatus::Started),
            &mut host,
        );
        assert!(model
            .dispatch_prompt()
            .is_some_and(|prompt| prompt.relaunch.is_some()));
        assert_eq!(
            domain.get(ids[0]).expect("task").status,
            HumanStatus::Review
        );
        // `y` opens a new workspace on the kept worktree and launches there.
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmDispatch,
            &mut host,
        );
        assert_eq!((host.reopened, host.ran, host.launched), (1, 1, 0));
        let saved = temp.store.load().expect("reload");
        let record = saved
            .get(ids[0])
            .and_then(|task| task.dispatch.clone())
            .expect("record");
        assert_eq!(record.herdr_workspace_id, "w2");
        assert_eq!(record.worktree, "/tmp/tsk-start-earlier");
        assert_eq!(record.branch, "tsk/earlier");
        assert_eq!(
            saved.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );
        let mut domain = saved;
        domain
            .set_status(ids[0], HumanStatus::Review)
            .expect("review");
        temp.store.reload_merge_save(&mut domain).expect("save");

        domain
            .record_dispatch_cleaned(ids[0], crate::domain::CleanupOutcome::Removed)
            .expect("cleaned");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        host.workspace_gone = false;
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::SetStatus(HumanStatus::Started),
            &mut host,
        );
        assert!(model
            .dispatch_prompt()
            .is_some_and(|prompt| prompt.relaunch.is_some()));
    }

    #[test]
    fn undo_after_a_start_that_dispatched_restores_the_status_and_keeps_the_agent() {
        let temp = Temp::new("start-undo", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["undo me"]);
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        domain
            .set_status(ids[0], HumanStatus::Ready)
            .expect("ready");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        let intent = ctrl_s(&model);
        handle(&temp, &mut domain, &mut model, intent, &mut host);
        assert!(matches!(domain.last_undo(), Some(UndoEntry::Start { .. })));

        let undo = key(&model, KeyCode::Char('u'), KeyModifiers::CONTROL);
        handle(&temp, &mut domain, &mut model, undo, &mut host);
        let saved = temp.store.load().expect("reload");
        let task = saved.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Ready);
        assert!(
            task.dispatch.as_ref().is_some_and(|record| !record.cleaned),
            "the record and its worktree stay"
        );
        assert_eq!(
            model.message(),
            Some("start undone · @builder kept running")
        );
    }

    #[test]
    fn undoing_a_dispatched_start_out_of_blocked_reopens_the_block() {
        let temp = Temp::new("start-undo-blocked", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["blocked then started"]);
        domain
            .assign(ids[0], Some("builder".into()))
            .expect("assign");
        temp.store.reload_merge_save(&mut domain).expect("save");
        domain
            .block(
                ids[0],
                crate::domain::BlockDraft::from_input(
                    Some("which db?"),
                    None,
                    &[],
                    Default::default(),
                )
                .expect("draft"),
                crate::domain::OWNER,
            )
            .expect("block");
        temp.store.reload_merge_save(&mut domain).expect("save");
        model.sync_from_domain(&domain);
        select(&mut domain, &mut model, ids[0]);
        let mut host = fake_host(&temp);
        handle(
            &temp,
            &mut domain,
            &mut model,
            BoardIntent::SetStatus(HumanStatus::Started),
            &mut host,
        );
        assert_eq!(
            domain.get(ids[0]).expect("task").status,
            HumanStatus::Started
        );
        assert!(domain.get(ids[0]).expect("task").block.is_none());
        handle(&temp, &mut domain, &mut model, BoardIntent::Undo, &mut host);
        let task = domain.get(ids[0]).expect("task");
        assert_eq!(task.status, HumanStatus::Blocked);
        assert_eq!(
            task.block.as_ref().and_then(|block| block.why.as_deref()),
            Some("which db?")
        );
    }

    #[test]
    fn at_opens_the_picker_and_enter_assigns_with_one_undo_entry() {
        let temp = Temp::new("at", &["builder", "reviewer"]);
        let (mut domain, mut model, ids) = board(&temp, &["pick an agent"]);
        select(&mut domain, &mut model, ids[0]);

        let open = key(&model, KeyCode::Char('@'), KeyModifiers::NONE);
        assert_eq!(open, BoardIntent::OpenAssigneePicker);
        apply_intent(&mut domain, &mut model, open, None).expect("open picker");
        assert_eq!(model.input_mode(), BoardInputMode::ListPicker);
        assert_eq!(model.list_picker_kind(), Some(ListPickerKind::Assignee));
        let labels: Vec<_> = model
            .visible_list_picker_options()
            .into_iter()
            .map(|(_, option)| option.label)
            .collect();
        assert_eq!(labels, ["@builder", "@reviewer", "none"]);
        assert_eq!(
            domain.get(ids[0]).expect("task").assignee,
            None,
            "nothing changes before Enter"
        );

        let down = key(&model, KeyCode::Down, KeyModifiers::NONE);
        apply_intent(&mut domain, &mut model, down, None).expect("move");
        let enter = key(&model, KeyCode::Enter, KeyModifiers::NONE);
        assert!(super::board_intent_may_persist(&model, &enter));
        let outcome = apply_intent(&mut domain, &mut model, enter, None).expect("assign");
        assert_eq!(outcome, crate::ui::board::IntentOutcome::Persist);
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert_eq!(
            domain.get(ids[0]).expect("task").assignee.as_deref(),
            Some("reviewer")
        );
        assert!(matches!(
            domain.last_undo(),
            Some(UndoEntry::Assign { id, previous: None, .. }) if *id == ids[0]
        ));

        // Reopening preselects the current assignee.
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenAssigneePicker,
            None,
        )
        .expect("reopen");
        assert_eq!(
            model
                .selected_list_picker_option()
                .map(|(_, option)| option.label),
            Some("@reviewer".to_string())
        );
    }

    #[test]
    fn esc_in_the_at_picker_changes_nothing() {
        let temp = Temp::new("esc", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["leave me"]);
        select(&mut domain, &mut model, ids[0]);
        let before = domain.get(ids[0]).expect("task").revision;

        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenAssigneePicker,
            None,
        )
        .expect("open");
        let esc = key(&model, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            apply_intent(&mut domain, &mut model, esc, None).expect("cancel"),
            crate::ui::board::IntentOutcome::None
        );
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert!(!model.list_picker_open());
        let task = domain.get(ids[0]).expect("task");
        assert_eq!(task.assignee, None);
        assert_eq!(task.revision, before);
    }

    #[test]
    fn the_boards_dispatch_host_writes_launchers_beside_its_own_store() {
        use crate::dispatch::DispatchHost;
        let temp = Temp::new("launchers", &["builder"]);
        let launcher = super::board_dispatch_host(&temp.store)
            .launcher_path("w1")
            .expect("launcher path");
        assert!(launcher.starts_with(&temp.dir), "{}", launcher.display());
        assert_eq!(launcher.file_name(), Some("dispatch-w1.ps1".as_ref()));
    }

    #[test]
    fn at_without_profiles_says_why() {
        let temp = Temp::new("at-no-profiles", &[]);
        let (mut domain, mut model, ids) = board(&temp, &["nobody to pick"]);
        select(&mut domain, &mut model, ids[0]);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenAssigneePicker,
            None,
        )
        .expect("at without profiles");
        assert!(!model.list_picker_open());
        assert_eq!(model.message(), Some(crate::ui::board::NO_AGENT_PROFILES));
    }

    #[test]
    fn at_on_a_marked_set_assigns_every_task_with_one_undo() {
        let temp = Temp::new("marks", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["first", "second"]);
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
            .expect("mark mode");
        for id in &ids {
            select(&mut domain, &mut model, *id);
            apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark");
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenAssigneePicker,
            None,
        )
        .expect("open");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ConfirmListPicker,
            None,
        )
        .expect("assign set");
        for id in &ids {
            assert_eq!(
                domain.get(*id).expect("task").assignee.as_deref(),
                Some("builder")
            );
        }
        assert!(matches!(domain.last_undo(), Some(UndoEntry::Batch { .. })));
        assert!(model.marked_ids().is_empty(), "marks clear after the verb");
        assert_eq!(model.message(), Some("assigned 2 tasks to @builder"));

        apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("undo");
        for id in &ids {
            assert_eq!(domain.get(*id).expect("task").assignee, None);
        }
    }

    /// A marked-set assignment whose save fails enters recovery, and Retry lands the whole set
    /// as one undo entry.
    #[test]
    fn a_failed_marked_set_assignment_retries_as_one_batch() {
        let temp = Temp::new("marks-retry", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["first", "second"]);
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
            .expect("mark mode");
        for id in &ids {
            select(&mut domain, &mut model, *id);
            apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark");
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenAssigneePicker,
            None,
        )
        .expect("open");
        let mut recovery = SaveRecovery::new();
        assert!(super::board_intent_may_persist(
            &model,
            &BoardIntent::ConfirmListPicker
        ));
        save(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardIntent::ConfirmListPicker,
            false,
        );
        assert!(recovery.is_pending());
        assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);
        save(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardIntent::RetrySave,
            true,
        );
        assert!(!recovery.is_pending());
        for id in &ids {
            assert_eq!(
                domain.get(*id).expect("task").assignee.as_deref(),
                Some("builder")
            );
        }
        assert!(matches!(domain.last_undo(), Some(UndoEntry::Batch { .. })));
    }

    fn frame_text(model: &BoardModel, area: Rect) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .expect("terminal");
        terminal
            .draw(|frame| {
                crate::ui::board::draw_board(frame, model);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buffer.cell((x, y)).expect("cell").symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn task_view_paints_plus_assign_only_when_unassigned_with_profiles_and_click_opens_picker() {
        let area = Rect::new(0, 0, 80, 24);
        let temp = Temp::new("footer", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["footer task"]);
        select(&mut domain, &mut model, ids[0]);

        // The peek keeps omitting an unset assignee.
        apply_intent(&mut domain, &mut model, BoardIntent::PeekDetail, None).expect("peek");
        assert!(!frame_text(&model, area).contains("+ assign"));
        apply_intent(&mut domain, &mut model, BoardIntent::CollapseDetail, None)
            .expect("close peek");

        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        assert!(frame_text(&model, area).contains("+ assign"));

        let hits = board_hit_map(area, &model);
        let hit = hits
            .regions
            .iter()
            .find(|hit| hit.target == QueueHitTarget::FormAssignee)
            .expect("+ assign is a click target");
        let click = map_board_mouse(&model, &hits, left_click(hit.area.x, hit.area.y));
        assert_eq!(click, Some(BoardIntent::OpenAssigneePicker));
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenAssigneePicker,
            None,
        )
        .expect("open from footer");
        assert_eq!(model.list_picker_kind(), Some(ListPickerKind::Assignee));
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ConfirmListPicker,
            None,
        )
        .expect("assign from footer");
        model.sync_from_domain(&domain);
        assert_eq!(
            model.input_mode(),
            BoardInputMode::TaskPage,
            "the page survives the picker"
        );
        let text = frame_text(&model, area);
        assert!(text.contains("@builder") && !text.contains("+ assign"));

        // `@` on the page reopens the picker; Esc returns to the page.
        let at = key(&model, KeyCode::Char('@'), KeyModifiers::NONE);
        apply_intent(&mut domain, &mut model, at, None).expect("page @");
        assert_eq!(model.list_picker_kind(), Some(ListPickerKind::Assignee));
        apply_intent(&mut domain, &mut model, BoardIntent::CancelListPicker, None).expect("cancel");
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

        // No profiles: an unassigned page paints nothing in the slot.
        let bare = Temp::new("footer-bare", &[]);
        let (mut domain, mut model, ids) = board(&bare, &["no profiles"]);
        select(&mut domain, &mut model, ids[0]);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
        assert!(!frame_text(&model, area).contains("+ assign"));
    }

    /// One intent through the real save boundary; `ok` decides whether the write lands.
    fn save(
        domain: &mut DomainState,
        model: &mut BoardModel,
        recovery: &mut SaveRecovery<DomainState>,
        intent: BoardIntent,
        ok: bool,
    ) -> IntentOutcome {
        let baseline = domain.clone();
        apply_board_intent_with_save_recovery(
            domain,
            model,
            recovery,
            BoardSaveContext {
                baseline,
                intent,
                snapshot: None,
            },
            |_| {
                if ok {
                    Ok(())
                } else {
                    Err("injected save failure".into())
                }
            },
        )
        .expect("board save")
    }

    /// Open the task page, assign through `@` (first profile) at the save boundary, then edit
    /// the title and save the whole session with Shift+Enter.
    fn assign_on_page_then_edit_title(assign_saves: bool) -> (DomainState, uuid::Uuid, Temp) {
        let temp = Temp::new("page-edit", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["page task"]);
        select(&mut domain, &mut model, ids[0]);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
        let mut recovery = SaveRecovery::new();
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenAssigneePicker,
            None,
        )
        .expect("open picker");
        save(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardIntent::ConfirmListPicker,
            assign_saves,
        );
        if !assign_saves {
            assert!(recovery.is_pending());
            save(
                &mut domain,
                &mut model,
                &mut recovery,
                BoardIntent::CancelSave,
                true,
            );
            assert_eq!(domain.get(ids[0]).expect("task").assignee, None);
        }
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
            .expect("edit title");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsertText(" edited".into()),
            None,
        )
        .expect("type");
        save(
            &mut domain,
            &mut model,
            &mut recovery,
            BoardIntent::ConfirmEditNext,
            true,
        );
        assert!(!recovery.is_pending());
        assert_eq!(
            domain.get(ids[0]).expect("task").title,
            "page task edited",
            "the title edit saved"
        );
        (domain, ids[0], temp)
    }

    #[test]
    fn a_page_assignment_survives_a_later_title_edit() {
        let (domain, id, _temp) = assign_on_page_then_edit_title(true);
        assert_eq!(
            domain.get(id).expect("task").assignee.as_deref(),
            Some("builder")
        );
    }

    #[test]
    fn a_cancelled_assignment_save_is_not_reapplied_by_a_later_edit() {
        let (domain, id, _temp) = assign_on_page_then_edit_title(false);
        assert_eq!(domain.get(id).expect("task").assignee, None);
    }

    #[test]
    fn esc_closes_the_picker_with_marks_active_and_changes_nothing() {
        let temp = Temp::new("marks-esc", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["first", "second"]);
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
            .expect("mark mode");
        for id in &ids {
            select(&mut domain, &mut model, *id);
            apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark");
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenAssigneePicker,
            None,
        )
        .expect("open");
        let esc = board_keyboard_intent(
            &model,
            model.input_mode(),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .expect("Esc maps");
        assert_eq!(esc, BoardIntent::CancelListPicker);
        apply_intent(&mut domain, &mut model, esc, None).expect("cancel");
        assert!(!model.list_picker_open());
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert_eq!(
            model.marked_ids().len(),
            2,
            "cancel leaves the set as it was"
        );
        let enter = board_keyboard_intent(
            &model,
            model.input_mode(),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_ne!(enter, Some(BoardIntent::ConfirmListPicker));
        for id in &ids {
            assert_eq!(domain.get(*id).expect("task").assignee, None);
        }
    }

    #[test]
    fn palette_set_assignee_opens_the_picker_without_changing_anything() {
        let temp = Temp::new("palette", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["palette task"]);
        select(&mut domain, &mut model, ids[0]);
        let before = domain.get(ids[0]).expect("task").clone();
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::OpenCommandPalette,
            None,
        )
        .expect("palette");
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::CommandQueryInsertText("set assignee".into()),
            None,
        )
        .expect("filter");
        assert_eq!(
            model
                .visible_commands()
                .first()
                .map(|command| command.label.as_str()),
            Some("set assignee")
        );
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmCommand, None)
            .expect("run command");
        assert_eq!(model.list_picker_kind(), Some(ListPickerKind::Assignee));
        assert_eq!(model.input_mode(), BoardInputMode::ListPicker);
        assert!(!model.board_form_open(), "no edit form opens");
        assert_eq!(domain.get(ids[0]).expect("task"), &before);
    }

    #[test]
    fn plus_assign_is_a_click_target_in_the_wide_split() {
        let area = Rect::new(0, 0, 120, 30);
        let temp = Temp::new("wide", &["builder"]);
        let (mut domain, mut model, ids) = board(&temp, &["wide task"]);
        select(&mut domain, &mut model, ids[0]);
        apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
        let text = frame_text(&model, area);
        assert!(text.contains("+ assign"), "{text}");
        let hits = board_hit_map(area, &model);
        let hit = hits
            .regions
            .iter()
            .find(|hit| hit.target == QueueHitTarget::FormAssignee)
            .expect("+ assign is a click target in the split");
        let row: String = text
            .lines()
            .nth(usize::from(hit.area.y))
            .expect("hit row")
            .chars()
            .skip(usize::from(hit.area.x))
            .take(usize::from(hit.area.width))
            .collect();
        assert_eq!(row, "+ assign", "the hit covers the painted control");
        assert_eq!(
            map_board_mouse(&model, &hits, left_click(hit.area.x, hit.area.y)),
            Some(BoardIntent::OpenAssigneePicker)
        );
    }

    /// The bulk start card (`ctrl+s` on a marked set where a start would launch).
    mod bulk_dispatch {
        use super::*;
        use crate::dispatch::{AgentNaming, EligibleDispatch, GitChecks, LaunchBatch};
        use crate::domain::Dispatch;
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        /// Mark `ids` on the board after saving them.
        fn marked(
            domain: &mut DomainState,
            model: &mut BoardModel,
            temp: &Temp,
            ids: &[uuid::Uuid],
        ) {
            temp.store.reload_merge_save(domain).expect("save");
            model.sync_from_domain(domain);
            apply_intent(domain, model, BoardIntent::ToggleMarkMode, None).expect("mark mode");
            for id in ids {
                select(domain, model, *id);
                apply_intent(domain, model, BoardIntent::MarkToggle, None).expect("mark");
            }
        }

        fn assign(domain: &mut DomainState, id: uuid::Uuid) {
            domain
                .assign(id, Some("builder".into()))
                .expect("assign builder");
        }

        fn number(domain: &DomainState, id: uuid::Uuid) -> String {
            format!(
                "T{}",
                domain.get(id).and_then(|task| task.number).expect("number")
            )
        }

        fn earlier_dispatch() -> Dispatch {
            Dispatch {
                argv: vec!["true".into()],
                worktree: "/tmp/tsk-bulk-dispatch-earlier".into(),
                branch: "tsk/earlier".into(),
                base: Some("main".into()),
                base_ref: None,
                base_commit: None,
                base_remote: None,
                herdr_workspace_id: "w0".into(),
                at: std::time::SystemTime::now(),
                cleaned: false,
            }
        }

        /// Two assigned tasks, marked, on a saved board.
        fn two_marked(label: &str) -> (Temp, DomainState, BoardModel, Vec<uuid::Uuid>) {
            let temp = Temp::new(label, &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["first job", "second job"]);
            assign(&mut domain, ids[0]);
            assign(&mut domain, ids[1]);
            marked(&mut domain, &mut model, &temp, &ids);
            (temp, domain, model, ids)
        }

        fn step(
            temp: &Temp,
            domain: &mut DomainState,
            model: &mut BoardModel,
            intent: BoardIntent,
            recovery: &mut SaveRecovery<DomainState>,
            host: &mut impl DispatchHost,
            names: &mut Vec<AgentNaming>,
        ) -> bool {
            handle_board_intent_with_host(
                &temp.store,
                domain,
                model,
                intent,
                recovery,
                false,
                true,
                host,
                &mut |naming| names.push(naming),
            )
            .expect("board intent")
        }

        fn land(
            temp: &Temp,
            domain: &mut DomainState,
            model: &mut BoardModel,
            recovery: &mut SaveRecovery<DomainState>,
            names: &mut Vec<AgentNaming>,
        ) {
            super::super::land_bulk_dispatch_with_host(
                &temp.store,
                domain,
                model,
                recovery,
                &mut |naming| names.push(naming),
            )
            .expect("landing never fails the board loop");
        }

        fn saved(temp: &Temp, id: uuid::Uuid) -> crate::domain::Task {
            temp.store
                .load()
                .expect("load")
                .get(id)
                .expect("saved task")
                .clone()
        }

        /// A host that leaves launches and git checks pending until the test lands them, like
        /// the system host's background threads. It counts git checks made on the board thread.
        struct DeferredHost {
            inner: FakeHost,
            batch: Option<(LaunchBatch, Vec<EligibleDispatch>)>,
            checks: Option<(GitChecks, Vec<std::path::PathBuf>)>,
            git_calls: usize,
        }

        fn deferred(temp: &Temp) -> DeferredHost {
            DeferredHost {
                inner: fake_host(temp),
                batch: None,
                checks: None,
                git_calls: 0,
            }
        }

        impl DispatchHost for DeferredHost {
            fn is_git_repo(&mut self, project: &Path) -> Result<bool, String> {
                self.git_calls += 1;
                self.inner.is_git_repo(project)
            }
            fn resolve_base(&mut self, project: &Path) -> Result<String, String> {
                self.inner.resolve_base(project)
            }
            fn create_worktree(
                &mut self,
                project: &Path,
                branch: &str,
                base: Option<&str>,
                label: &str,
            ) -> Result<CreatedWorktree, String> {
                self.inner.create_worktree(project, branch, base, label)
            }
            fn root_pane(
                &mut self,
                workspace: &str,
            ) -> Result<String, crate::dispatch::RootPaneError> {
                self.inner.root_pane(workspace)
            }
            fn run_in_pane(&mut self, pane: &str, command: &str) -> Result<(), String> {
                self.inner.run_in_pane(pane, command)
            }
            fn begin_launches(&mut self, jobs: Vec<EligibleDispatch>) -> LaunchBatch {
                let batch = LaunchBatch::new(jobs.len());
                self.batch = Some((batch.clone(), jobs));
                batch
            }
            fn begin_git_checks(&mut self, projects: Vec<std::path::PathBuf>) -> GitChecks {
                let checks = GitChecks::default();
                self.checks = Some((checks.clone(), projects));
                checks
            }
        }

        impl DeferredHost {
            /// Launch the next pending job through the real launch path and land its outcome.
            fn land_next(&mut self) {
                let (batch, jobs) = self.batch.as_mut().expect("launches started");
                let job = jobs.remove(0);
                let outcome = crate::dispatch::launch_bulk_job(&job, &mut self.inner);
                batch.land(job, outcome);
            }

            #[cfg(unix)]
            fn batch(&self) -> &LaunchBatch {
                &self.batch.as_ref().expect("launches started").0
            }
        }

        /// Assigned, unassigned, already dispatched, and a desk task: `ctrl+s` opens one card
        /// that launches only the first and only starts the unassigned and dispatched ones.
        /// Nothing changes and the marks stay while it is up; `y` starts and launches.
        #[test]
        fn ctrl_s_on_a_mixed_marked_set_opens_one_card_listing_launches_and_starts() {
            let temp = Temp::new("bulk-card", &["builder"]);
            let (mut domain, mut model, ids) =
                board(&temp, &["ready to go", "nobody yet", "launched before"]);
            assign(&mut domain, ids[0]);
            assign(&mut domain, ids[2]);
            temp.store.reload_merge_save(&mut domain).expect("save");
            domain
                .record_dispatch(ids[2], earlier_dispatch())
                .expect("earlier dispatch");
            temp.store.reload_merge_save(&mut domain).expect("save");
            domain
                .set_status(ids[2], HumanStatus::Ready)
                .expect("back to ready");
            let desk = domain
                .create(
                    "desk errand",
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("desk task");
            assign(&mut domain, desk);
            marked(&mut domain, &mut model, &temp, &ids);
            // The desk task is not painted in this project's lens, so it cannot be marked here;
            // the card's check skips it with the single-task refusal wherever it is marked.
            let (launch, skipped) =
                super::super::check_marked_dispatches(&domain, [desk], &temp.profiles(), true);
            assert!(launch.is_empty());
            assert_eq!(
                skipped,
                [(
                    number(&domain, desk),
                    "not a project in a git repo".to_string()
                )]
            );
            let mut host = fake_host(&temp);

            let ctrl_s = key(&model, KeyCode::Char('s'), KeyModifiers::CONTROL);
            handle(&temp, &mut domain, &mut model, ctrl_s, &mut host);

            assert_eq!(host.launched, 0, "nothing launches before y");
            assert_eq!(model.input_mode(), BoardInputMode::DispatchConfirm);
            assert_eq!(model.marked_count(), 3, "the card keeps the marks");
            let prompt = model.dispatch_prompt().expect("bulk card");
            assert_eq!(
                prompt
                    .launch
                    .iter()
                    .map(|eligible| eligible.id)
                    .collect::<Vec<_>>(),
                [ids[0]]
            );
            assert!(prompt.skipped.is_empty(), "{:?}", prompt.skipped);
            // Unassigned and already-dispatched tasks only start: relaunch stays cursor-only.
            assert_eq!(
                prompt.start_only,
                [
                    (number(&domain, ids[1]), ids[1]),
                    (number(&domain, ids[2]), ids[2]),
                ]
            );

            let first = number(&domain, ids[0]);
            let painted = frame_text(&model, Rect::new(0, 0, 80, 30));
            for text in [
                "Dispatch 1 task?".to_string(),
                format!("{first}  @builder  from default"),
                "start only".to_string(),
                "started, no launch".to_string(),
                "y dispatch 1 · esc cancel".to_string(),
            ] {
                assert!(painted.contains(&text), "missing {text:?}:\n{painted}");
            }
            assert!(!painted.contains("checking…"), "{painted}");
            let narrow = frame_text(&model, Rect::new(0, 0, 40, 30));
            for text in ["@builder", "from default", "start only", "y dispatch 1"] {
                assert!(narrow.contains(text), "40 columns lost {text:?}:\n{narrow}");
            }

            // `y` starts the start-only rows in one save and launches the rest.
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ConfirmDispatch,
                &mut host,
            );
            assert_eq!((host.launched, host.ran), (1, 1), "only the first launches");
            for id in &ids {
                assert_eq!(saved(&temp, *id).status, HumanStatus::Started);
            }
            assert!(saved(&temp, ids[0]).dispatch.is_some());
            assert!(saved(&temp, ids[1]).dispatch.is_none());
            assert_eq!(
                saved(&temp, ids[2]).dispatch,
                domain.get(ids[2]).and_then(|task| task.dispatch.clone())
            );
        }

        /// F-2/F-10/F-11: `y` rechecks the start-only rows against the refreshed board. A row
        /// assigned meanwhile is refused (its launch was never shown); a row finished meanwhile
        /// stays done.
        #[test]
        fn y_rechecks_start_only_rows_changed_since_the_card_opened() {
            let temp = Temp::new("bulk-recheck", &["builder"]);
            let (mut domain, mut model, ids) = board(
                &temp,
                &["launches", "assigned meanwhile", "finished meanwhile"],
            );
            assign(&mut domain, ids[0]);
            marked(&mut domain, &mut model, &temp, &ids);
            let mut host = fake_host(&temp);
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut host,
            );
            let prompt = model.dispatch_prompt().expect("card").clone();
            assert_eq!(prompt.start_only.len(), 2);

            let mut other = temp.store.load().expect("load");
            other
                .assign(ids[1], Some("builder".into()))
                .expect("assign elsewhere");
            other.complete(ids[2]).expect("done elsewhere");
            temp.store
                .reload_merge_save(&mut other)
                .expect("save elsewhere");

            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ConfirmDispatch,
                &mut host,
            );
            assert_eq!(host.ran, 1, "only the shown launch runs");
            assert_eq!(saved(&temp, ids[0]).status, HumanStatus::Started);
            assert_eq!(saved(&temp, ids[1]).status, HumanStatus::Open, "refused");
            assert!(saved(&temp, ids[1]).dispatch.is_none());
            assert_eq!(saved(&temp, ids[2]).status, HumanStatus::Done);
            let message = model.message().unwrap_or_default().to_string();
            assert!(
                message.contains(&number(&domain, ids[1]))
                    && message.contains("assigned since the card opened"),
                "{message}"
            );
        }

        /// F-7: a set where nothing would launch (unassigned, already dispatched) is the plain
        /// batch: no card, and a broken config.toml is never read.
        #[test]
        fn a_set_with_nothing_to_launch_never_reads_config() {
            let temp = Temp::new("bulk-no-launch", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["dispatched", "unassigned"]);
            assign(&mut domain, ids[0]);
            temp.store.reload_merge_save(&mut domain).expect("save");
            domain
                .record_dispatch(ids[0], earlier_dispatch())
                .expect("earlier dispatch");
            temp.store.reload_merge_save(&mut domain).expect("save");
            domain
                .set_status(ids[0], HumanStatus::Ready)
                .expect("ready");
            marked(&mut domain, &mut model, &temp, &ids);
            std::fs::write(temp.dir.join("config.toml"), "not toml [").expect("break config");
            let mut host = fake_host(&temp);
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut host,
            );
            assert_eq!(model.dispatch_prompt(), None);
            assert_eq!(host.ran, 0);
            for id in &ids {
                assert_eq!(saved(&temp, *id).status, HumanStatus::Started);
            }
        }

        /// The palette's absolute start on a set with a launch lists a done task as a status
        /// correction and starts it on `y`, in the same undo batch, as its plain batch would.
        #[test]
        fn palette_start_on_a_launching_set_corrects_a_done_task() {
            let temp = Temp::new("bulk-done-correction", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["launches", "was done"]);
            assign(&mut domain, ids[0]);
            temp.store.reload_merge_save(&mut domain).expect("save");
            domain.complete(ids[1]).expect("done");
            temp.store.reload_merge_save(&mut domain).expect("save");
            model.sync_from_domain(&domain);
            apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
                .expect("drawer");
            apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
                .expect("mark mode");
            for id in &ids {
                select(&mut domain, &mut model, *id);
                apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark");
            }
            assert_eq!(model.marked_count(), 2);
            let mut host = fake_host(&temp);
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::SetStatus(HumanStatus::Started),
                &mut host,
            );
            let prompt = model.dispatch_prompt().expect("card").clone();
            assert_eq!(prompt.launch.len(), 1);
            assert_eq!(prompt.start_only, [(number(&domain, ids[1]), ids[1])]);
            let painted = frame_text(&model, Rect::new(0, 0, 80, 30));
            assert!(
                painted.contains("status correction, no launch"),
                "{painted}"
            );
            let undo_before = domain.undo_len();
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ConfirmDispatch,
                &mut host,
            );
            assert_eq!(host.ran, 1, "only the assigned task launches");
            assert_eq!(saved(&temp, ids[0]).status, HumanStatus::Started);
            assert_eq!(saved(&temp, ids[1]).status, HumanStatus::Started);
            assert!(saved(&temp, ids[1]).dispatch.is_none());
            assert_eq!(domain.undo_len(), undo_before + 1);
            handle(&temp, &mut domain, &mut model, BoardIntent::Undo, &mut host);
            assert_eq!(saved(&temp, ids[1]).status, HumanStatus::Done);
        }

        /// F-15: the card's start-only rows are one undo step; launches add none.
        #[test]
        fn the_start_only_rows_undo_as_one_batch() {
            let temp = Temp::new("bulk-undo", &["builder"]);
            let (mut domain, mut model, ids) =
                board(&temp, &["launches", "start only", "start only too"]);
            assign(&mut domain, ids[0]);
            domain
                .set_status(ids[2], HumanStatus::Ready)
                .expect("ready");
            marked(&mut domain, &mut model, &temp, &ids);
            let undo_before = domain.undo_len();
            let mut host = fake_host(&temp);
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut host,
            );
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ConfirmDispatch,
                &mut host,
            );
            assert_eq!(host.ran, 1);
            assert_eq!(
                domain.undo_len(),
                undo_before + 1,
                "one entry for the whole card"
            );
            assert!(matches!(
                domain.last_undo(),
                Some(UndoEntry::Batch { entries }) if entries.len() == 2
            ));
            handle(&temp, &mut domain, &mut model, BoardIntent::Undo, &mut host);
            assert_eq!(
                saved(&temp, ids[0]).status,
                HumanStatus::Started,
                "launch stays"
            );
            assert_eq!(saved(&temp, ids[1]).status, HumanStatus::Open);
            assert_eq!(saved(&temp, ids[2]).status, HumanStatus::Ready);
        }

        /// The card opens at once without a git call on the board thread, and its rows say
        /// `checking…` until the off-loop check lands.
        #[test]
        fn the_bulk_card_checks_git_off_the_event_loop() {
            let (temp, mut domain, mut model, ids) = two_marked("bulk-git");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut recovery,
                &mut host,
                &mut Vec::new(),
            );
            assert!(model
                .dispatch_prompt()
                .expect("card opens at once")
                .checking());
            assert_eq!(host.git_calls, 0, "no git call on the board thread");
            let painted = frame_text(&model, Rect::new(0, 0, 80, 30));
            assert!(painted.contains("from default  checking…"), "{painted}");

            let (checks, projects) = host.checks.take().expect("checks started");
            assert_eq!(
                projects,
                [std::path::PathBuf::from(PROJECT)],
                "one check per repo"
            );
            checks.finish(
                [(std::path::PathBuf::from(PROJECT), true)]
                    .into_iter()
                    .collect(),
            );
            assert!(model.poll_dispatch_checks());
            let prompt = model.dispatch_prompt().expect("card stays");
            assert!(!prompt.checking());
            assert_eq!(
                prompt
                    .launch
                    .iter()
                    .map(|eligible| eligible.id)
                    .collect::<Vec<_>>(),
                ids
            );
            let painted = frame_text(&model, Rect::new(0, 0, 80, 30));
            assert!(!painted.contains("checking…"), "{painted}");
        }

        /// When the git check leaves nothing to launch, the card closes with the refusal and
        /// keeps the marks.
        #[test]
        fn a_bulk_card_whose_repos_are_not_git_closes_with_the_refusal() {
            let (temp, mut domain, mut model, _ids) = two_marked("bulk-git-none");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut recovery,
                &mut host,
                &mut Vec::new(),
            );
            let (checks, _) = host.checks.take().expect("checks started");
            checks.finish(
                [(std::path::PathBuf::from(PROJECT), false)]
                    .into_iter()
                    .collect(),
            );
            model.poll_dispatch_checks();
            assert!(model.dispatch_prompt().is_none());
            assert_eq!(model.input_mode(), BoardInputMode::Normal);
            assert_eq!(
                model.message(),
                Some("nothing to dispatch: not a project in a git repo")
            );
            assert_eq!(model.marked_count(), 2);
        }

        /// `y` launches each eligible task through the single-task path, saves each record with
        /// `started`, names each agent only after its record is saved, and clears the marks;
        /// `[x]` and the footer are clickable like the keys.
        #[test]
        fn y_on_the_bulk_card_launches_only_the_eligible_tasks() {
            let temp = Temp::new("bulk-y", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["first job", "second job", "idle"]);
            assign(&mut domain, ids[0]);
            assign(&mut domain, ids[1]);
            marked(&mut domain, &mut model, &temp, &ids);
            let mut host = fake_host(&temp);
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut host,
            );

            let hits = board_hit_map(Rect::new(0, 0, 80, 30), &model);
            let footer = |index: usize| {
                let hit = hits
                    .regions
                    .iter()
                    .find(|hit| hit.target == QueueHitTarget::CleanupOption(index))
                    .expect("footer entry is clickable");
                map_board_mouse(&model, &hits, left_click(hit.area.x, hit.area.y))
            };
            assert_eq!(footer(0), Some(BoardIntent::ConfirmDispatch));
            assert_eq!(footer(1), Some(BoardIntent::CancelDispatch));
            let close = hits
                .regions
                .iter()
                .find(|hit| hit.target == QueueHitTarget::ModalClose)
                .expect("[x]");
            assert_eq!(
                map_board_mouse(&model, &hits, left_click(close.area.x, close.area.y)),
                Some(BoardIntent::CancelDispatch)
            );

            let y = key(&model, KeyCode::Char('y'), KeyModifiers::NONE);
            assert_eq!(y, BoardIntent::ConfirmDispatch);
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            step(
                &temp,
                &mut domain,
                &mut model,
                y,
                &mut recovery,
                &mut host,
                &mut names,
            );

            assert_eq!(host.launched, 2, "only the eligible tasks launch");
            for id in &ids[..2] {
                let saved = saved(&temp, *id);
                assert!(saved.dispatch.is_some(), "each launch is recorded");
                assert_eq!(saved.status, HumanStatus::Started);
            }
            assert!(saved(&temp, ids[2]).dispatch.is_none());
            let (first, second) = (number(&domain, ids[0]), number(&domain, ids[1]));
            assert_eq!(
                names
                    .iter()
                    .map(|naming| naming.name.as_str())
                    .collect::<Vec<_>>(),
                [
                    format!("{}-builder", first.to_lowercase()),
                    format!("{}-builder", second.to_lowercase())
                ],
                "each saved launch names its agent"
            );
            assert_eq!(model.marked_count(), 0, "marks clear once the run starts");
            assert_eq!(model.input_mode(), BoardInputMode::Normal);
            assert!(!model.bulk_dispatch_running(), "the run is finished");
            assert_eq!(
                model.message(),
                Some(format!("dispatched {first} to @builder, {second} to @builder").as_str())
            );
        }

        /// A launch that fails is reported, names no agent, and does not stop the others.
        #[test]
        fn one_failed_launch_in_a_bulk_dispatch_does_not_stop_the_others() {
            let temp = Temp::new("bulk-fail", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["breaks here", "goes fine"]);
            assign(&mut domain, ids[0]);
            assign(&mut domain, ids[1]);
            marked(&mut domain, &mut model, &temp, &ids);
            let mut host = fake_host(&temp);
            host.fail_branch = Some("breaks".into());
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                step(
                    &temp,
                    &mut domain,
                    &mut model,
                    intent,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
            }

            assert_eq!(host.launched, 2);
            assert!(saved(&temp, ids[0]).dispatch.is_none());
            assert!(saved(&temp, ids[1]).dispatch.is_some());
            let (failed, launched) = (number(&domain, ids[0]), number(&domain, ids[1]));
            assert_eq!(names.len(), 1, "no name for the failed launch");
            assert_eq!(
                names[0].name,
                format!("{}-builder", launched.to_lowercase())
            );
            let message = model.message().expect("outcome");
            assert!(
                message.contains(&format!("dispatched {launched} to @builder"))
                    && message.contains(&format!("{failed} failed: herdr refused")),
                "{message}"
            );
        }

        /// Esc closes the card, launches nothing, and keeps the marked set.
        #[test]
        fn esc_on_the_bulk_dispatch_card_changes_nothing_and_keeps_marks() {
            let (temp, mut domain, mut model, _ids) = two_marked("bulk-esc");
            let before = temp.store.load().expect("load");
            let mut host = fake_host(&temp);
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut host,
            );
            let esc = super::super::board_keyboard_intent(
                &model,
                model.input_mode(),
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            );
            assert_eq!(esc, Some(BoardIntent::CancelDispatch));
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::CancelDispatch,
                &mut host,
            );

            assert_eq!(host.launched, 0);
            assert!(model.dispatch_prompt().is_none());
            assert_eq!(model.input_mode(), BoardInputMode::Normal);
            assert_eq!(model.marked_count(), 2, "Esc keeps the marked set");
            assert_eq!(temp.store.load().expect("load").tasks(), before.tasks());
        }

        /// A set where no start would launch starts as before: one batch, no card.
        #[test]
        fn a_marked_set_with_nothing_to_launch_starts_without_a_card() {
            let temp = Temp::new("bulk-none", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["nobody", "nobody either"]);
            marked(&mut domain, &mut model, &temp, &ids);
            let mut host = fake_host(&temp);
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut host,
            );

            assert_eq!((host.launched, host.ran), (0, 0));
            assert!(model.dispatch_prompt().is_none());
            assert_eq!(model.input_mode(), BoardInputMode::Normal);
            for id in &ids {
                assert_eq!(saved(&temp, *id).status, HumanStatus::Started);
            }
        }

        /// A set whose only launch is refused still starts the rest from the card; the refused
        /// task stays unstarted.
        #[test]
        fn a_refused_launch_in_a_marked_set_stays_unstarted() {
            let temp = Temp::new("bulk-refused", &["builder"]);
            let (mut domain, mut model, ids) = board(&temp, &["unknown agent", "mine"]);
            domain
                .assign(ids[0], Some("builder".into()))
                .expect("assign");
            marked(&mut domain, &mut model, &temp, &ids);
            std::fs::write(
                temp.dir.join("config.toml"),
                "[agent.other]\ncommand = [\"true\"]\n",
            )
            .expect("drop the builder profile");
            let mut host = fake_host(&temp);
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut host,
            );
            let prompt = model.dispatch_prompt().expect("card").clone();
            assert!(prompt.launch.is_empty());
            assert_eq!(prompt.start_only, [(number(&domain, ids[1]), ids[1])]);
            assert_eq!(
                prompt.skipped,
                [(number(&domain, ids[0]), "unknown agent builder".to_string())]
            );
            let painted = frame_text(&model, Rect::new(0, 0, 80, 30));
            for text in ["Start 1 task?", "not started", "y start · esc cancel"] {
                assert!(painted.contains(text), "missing {text:?}:\n{painted}");
            }
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ConfirmDispatch,
                &mut host,
            );
            assert_eq!(saved(&temp, ids[0]).status, HumanStatus::Open);
            assert_eq!(saved(&temp, ids[1]).status, HumanStatus::Started);
            assert_eq!(host.ran, 0);
        }

        /// The palette's start targets the marked set too and opens the same card; it lists no
        /// dispatch entries (dispatch again stays cursor-only).
        #[test]
        fn the_palette_start_opens_the_card_for_the_marked_set() {
            let (temp, mut domain, mut model, _ids) = two_marked("bulk-palette");
            let labels = model
                .available_commands()
                .into_iter()
                .map(|command| (command.label, command.intent))
                .collect::<Vec<_>>();
            assert!(labels.contains(&(
                "set status: started".to_string(),
                BoardIntent::SetStatus(HumanStatus::Started)
            )));
            assert!(!labels
                .iter()
                .any(|(label, _)| label.starts_with("dispatch")));
            let mut host = fake_host(&temp);
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::OpenCommandPalette,
                None,
            )
            .expect("palette");
            let index = model
                .visible_commands()
                .iter()
                .position(|command| command.label == "set status: started")
                .expect("listed");
            handle(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::SelectCommand(index),
                &mut host,
            );
            assert_eq!(model.input_mode(), BoardInputMode::DispatchConfirm);
            assert_eq!(model.dispatch_prompt().expect("card").launch.len(), 2);
        }

        /// While launches land off the event loop the status row counts them, a second dispatch
        /// and quit refuse (a quit would orphan launched agents), and each landing is saved on
        /// its own.
        #[test]
        fn a_running_bulk_dispatch_reports_progress_and_holds_quit_until_it_lands() {
            let (temp, mut domain, mut model, ids) = two_marked("bulk-deferred");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                step(
                    &temp,
                    &mut domain,
                    &mut model,
                    intent,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
            }
            assert_eq!(model.message(), Some("dispatching 1/2…"));
            assert!(model.bulk_dispatch_running());

            assert!(!step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::Quit,
                &mut recovery,
                &mut host,
                &mut names,
            ));
            assert!(model
                .message()
                .is_some_and(|message| message.contains("quit when it finishes")));
            step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut recovery,
                &mut host,
                &mut names,
            );
            assert!(model
                .message()
                .is_some_and(|message| message.contains("wait for it to finish")));

            host.land_next();
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
            let (one, two) = (number(&domain, ids[0]), number(&domain, ids[1]));
            assert_eq!(
                model.message(),
                Some(format!("dispatching 2/2… · dispatched {one} to @builder").as_str())
            );
            assert!(
                saved(&temp, ids[0]).dispatch.is_some(),
                "the first landing is saved before the second lands"
            );

            host.land_next();
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
            assert!(!model.bulk_dispatch_running());
            assert_eq!(
                model.message(),
                Some(format!("dispatched {one} to @builder, {two} to @builder").as_str())
            );
        }

        /// The production handoff: the launch runs on its own thread (`spawn_launches`, which
        /// the system host uses), `y` returns while it is held, and the board loop's background
        /// step records it once it lands, with no direct call to the landing helper.
        #[test]
        fn the_board_loop_records_launches_from_the_launch_thread() {
            struct HeldHost {
                release: std::sync::mpsc::Receiver<()>,
            }
            impl DispatchHost for HeldHost {
                fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
                    Ok(true)
                }
                fn resolve_base(&mut self, _: &Path) -> Result<String, String> {
                    Ok("main".into())
                }
                fn create_worktree(
                    &mut self,
                    _: &Path,
                    branch: &str,
                    _: Option<&str>,
                    _: &str,
                ) -> Result<CreatedWorktree, String> {
                    self.release.recv().expect("released");
                    Ok(CreatedWorktree {
                        path: format!("/tmp/{branch}").into(),
                        branch: branch.into(),
                        workspace_id: "w9".into(),
                        root_pane_id: "w9:p1".into(),
                    })
                }
                fn root_pane(&mut self, _: &str) -> Result<String, crate::dispatch::RootPaneError> {
                    Ok("w9:p1".into())
                }
                fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
                    Ok(())
                }
            }
            /// Board-thread host whose launches go to the thread like the system host's.
            struct ThreadedHost {
                release: Option<std::sync::mpsc::Receiver<()>>,
            }
            impl DispatchHost for ThreadedHost {
                fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
                    Ok(true)
                }
                fn create_worktree(
                    &mut self,
                    _: &Path,
                    _: &str,
                    _: Option<&str>,
                    _: &str,
                ) -> Result<CreatedWorktree, String> {
                    panic!("launches never run on the board thread")
                }
                fn root_pane(&mut self, _: &str) -> Result<String, crate::dispatch::RootPaneError> {
                    unreachable!()
                }
                fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
                    unreachable!()
                }
                fn begin_launches(&mut self, jobs: Vec<EligibleDispatch>) -> LaunchBatch {
                    let release = self.release.take().expect("one batch");
                    crate::dispatch::spawn_launches(jobs, move || HeldHost { release })
                }
            }

            let (temp, mut domain, mut model, ids) = two_marked("bulk-thread");
            let (release, held) = std::sync::mpsc::channel();
            let mut host = ThreadedHost {
                release: Some(held),
            };
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                step(
                    &temp,
                    &mut domain,
                    &mut model,
                    intent,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
            }
            assert_eq!(model.message(), Some("dispatching 1/2…"), "y returned");
            release.send(()).expect("release first");
            release.send(()).expect("release second");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while model.bulk_dispatch_running() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "launches never landed"
                );
                super::super::board_background_step(
                    &temp.store,
                    &mut domain,
                    &mut model,
                    &mut recovery,
                    true,
                    &mut host,
                    &mut |naming| names.push(naming),
                )
                .expect("background step");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            for id in &ids {
                assert_eq!(saved(&temp, *id).status, HumanStatus::Started);
            }
            assert_eq!(names.len(), 2);
        }

        /// The store cannot be read when a launch lands: the board stays up, the launch goes to
        /// save recovery, a later landing waits on the batch, and Retry saves both and shows the
        /// batch outcome.
        // A read-only store needs Unix permissions.
        #[cfg(unix)]
        #[test]
        fn a_store_failure_while_launches_land_goes_to_recovery_and_retry_keeps_them() {
            let (temp, mut domain, mut model, ids) = two_marked("bulk-read-fail");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                step(
                    &temp,
                    &mut domain,
                    &mut model,
                    intent,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
            }
            host.land_next();
            let file = temp.dir.join("tsk.json");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000))
                .expect("make the store unreadable");
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
            assert!(recovery.is_pending(), "the failure goes to save recovery");
            assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);
            assert!(
                names.is_empty(),
                "no agent is named before its record is saved"
            );
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
                .expect("restore the store");

            // The second landing waits on the batch while recovery is unresolved.
            host.land_next();
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
            assert_eq!(host.batch().taken(), 1, "the second outcome stays queued");

            step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::RetrySave,
                &mut recovery,
                &mut host,
                &mut names,
            );
            assert!(!recovery.is_pending());
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
            for id in &ids {
                let saved = saved(&temp, *id);
                assert!(saved.dispatch.is_some());
                assert_eq!(saved.status, HumanStatus::Started);
            }
            assert_eq!(names.len(), 2);
            let (one, two) = (number(&domain, ids[0]), number(&domain, ids[1]));
            assert!(!model.bulk_dispatch_running());
            assert_eq!(
                model.message(),
                Some(format!("dispatched {one} to @builder, {two} to @builder").as_str()),
                "the batch outcome survives Retry"
            );
        }

        /// Cancel on a failed landing save discards the record, so the outcome says the agent
        /// launched without one and where it runs, never that it was dispatched.
        // A read-only store needs Unix permissions.
        #[cfg(unix)]
        #[test]
        fn cancelling_a_failed_landing_save_reports_the_launch_as_not_recorded() {
            let (temp, mut domain, mut model, ids) = two_marked("bulk-cancel");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                step(
                    &temp,
                    &mut domain,
                    &mut model,
                    intent,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
            }
            host.land_next();
            std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o555))
                .expect("make the state dir read-only");
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
            std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o755))
                .expect("restore the state dir");
            assert!(recovery.is_pending());
            step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::CancelSave,
                &mut recovery,
                &mut host,
                &mut names,
            );
            assert!(!recovery.is_pending());

            host.land_next();
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
            let (one, two) = (number(&domain, ids[0]), number(&domain, ids[1]));
            assert!(
                saved(&temp, ids[0]).dispatch.is_none(),
                "the record was discarded"
            );
            assert!(saved(&temp, ids[1]).dispatch.is_some());
            assert_eq!(
                model.message(),
                Some(
                    format!(
                        "dispatched {two} to @builder · launched but not recorded: {one} \
                         (agent running in workspace w1)"
                    )
                    .as_str()
                )
            );
            assert_eq!(names.len(), 1, "only the recorded launch names its agent");
        }

        /// A task dispatched elsewhere between `y` and its landing keeps that record; a task
        /// dispatched elsewhere while the card was open is not launched again.
        #[test]
        fn launches_never_overwrite_a_dispatch_made_meanwhile() {
            let (temp, mut domain, mut model, ids) = two_marked("bulk-meanwhile");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut recovery,
                &mut host,
                &mut names,
            );
            // Another board dispatches the second task while the card is open.
            let mut other = temp.store.load().expect("load");
            other
                .record_dispatch(ids[1], earlier_dispatch())
                .expect("dispatch elsewhere");
            temp.store
                .reload_merge_save(&mut other)
                .expect("save elsewhere");
            step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::ConfirmDispatch,
                &mut recovery,
                &mut host,
                &mut names,
            );
            assert_eq!(
                host.batch.as_ref().expect("started").1.len(),
                1,
                "the recheck at y drops it"
            );
            // And the first one while its launch is in flight.
            let mut other = temp.store.load().expect("load");
            other
                .record_dispatch(ids[0], earlier_dispatch())
                .expect("dispatch elsewhere");
            temp.store
                .reload_merge_save(&mut other)
                .expect("save elsewhere");
            host.land_next();
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);

            for id in &ids {
                assert_eq!(
                    saved(&temp, *id).dispatch.expect("record").worktree,
                    "/tmp/tsk-bulk-dispatch-earlier",
                    "the earlier record stays"
                );
            }
            let (one, two) = (number(&domain, ids[0]), number(&domain, ids[1]));
            let message = model.message().expect("outcome");
            assert!(
                message.contains(&format!("{one} failed: launched in"))
                    && message.contains("dispatched elsewhere meanwhile")
                    && message.contains(&format!(
                        "{two} failed: already dispatched (use dispatch again)"
                    )),
                "{message}"
            );
            assert!(names.is_empty());
        }

        /// A task the human marked done while its launch was in flight gets the record but
        /// keeps done, and the outcome says so.
        #[test]
        fn a_landing_keeps_a_status_the_human_changed_meanwhile() {
            let (temp, mut domain, mut model, ids) = two_marked("bulk-done-meanwhile");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                step(
                    &temp,
                    &mut domain,
                    &mut model,
                    intent,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
            }
            let mut other = temp.store.load().expect("load");
            other
                .set_status(ids[0], HumanStatus::Done)
                .expect("done elsewhere");
            temp.store
                .reload_merge_save(&mut other)
                .expect("save elsewhere");
            host.land_next();
            host.land_next();
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);

            let first = saved(&temp, ids[0]);
            assert_eq!(first.status, HumanStatus::Done, "the human's status wins");
            assert!(
                first.dispatch.is_some(),
                "the running agent is still recorded"
            );
            assert_eq!(saved(&temp, ids[1]).status, HumanStatus::Started);
            let one = number(&domain, ids[0]);
            assert!(
                model.message().is_some_and(
                    |message| message.contains(&format!("{one} kept done (changed meanwhile)"))
                ),
                "{:?}",
                model.message()
            );
        }

        /// A Projects overview whose preview seat (on `/repos/app`) has two assigned tasks marked.
        fn preview_board(label: &str) -> (Temp, DomainState, BoardModel, [uuid::Uuid; 2]) {
            let temp = Temp::new(label, &["builder"]);
            let mut domain = DomainState::new();
            let mut create = |title: &str, path: &str| {
                let id = domain
                    .create(
                        title,
                        None,
                        TaskScope::Project { path: path.into() },
                        ProvenanceOrigin::Manual,
                        None,
                    )
                    .expect("create");
                domain.assign(id, Some("builder".into())).expect("assign");
                id
            };
            let ids = [
                create("preview one", "/repos/app"),
                create("preview two", "/repos/app"),
            ];
            create("elsewhere", "/repos/zzz");
            temp.store.reload_merge_save(&mut domain).expect("save");
            let mut model = BoardModel::from_domain(&domain, None);
            model.set_agent_profiles(&temp.profiles());
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::SelectNavTab(crate::ui::queue::NavTab::Projects),
                None,
            )
            .expect("projects");
            for _ in 0..2 {
                apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
                    .expect("stage right");
            }
            let seat = model.preview_seat_mut().expect("preview seat");
            assert_eq!(
                seat.active_project().map(|path| path.to_path_buf()),
                Some(std::path::PathBuf::from("/repos/app"))
            );
            seat.set_agent_profiles(&temp.profiles());
            marked(&mut domain, seat, &temp, &ids);
            (temp, domain, model, ids)
        }

        /// A batch started in the Projects preview outlives that preview: rebinding it to
        /// another project or dropping it leaves the batch landing on the outer board.
        #[test]
        fn a_bulk_dispatch_started_in_the_project_preview_outlives_it() {
            let (temp, mut domain, mut model, ids) = preview_board("bulk-preview");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                step(
                    &temp,
                    &mut domain,
                    model.preview_seat_mut().expect("seat"),
                    intent,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
            }
            assert!(model.bulk_dispatch_running());

            // Back to the index, onto the other project: the preview is rebound, then dropped.
            apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None)
                .expect("stage left");
            apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None)
                .expect("next project");
            model.bind_project_preview();
            model.drop_project_preview();
            assert!(model.right_seat().is_none());
            assert!(
                model.bulk_dispatch_running(),
                "the outer board keeps the batch"
            );

            host.land_next();
            host.land_next();
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
            for id in ids {
                assert!(saved(&temp, id).dispatch.is_some());
            }
            assert!(!model.bulk_dispatch_running());
            assert!(model
                .message()
                .is_some_and(|message| message.starts_with("dispatched")));
        }
        /// Wait for a background result, failing rather than hanging.
        fn eventually<T>(mut poll: impl FnMut() -> Option<T>) -> T {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                if let Some(value) = poll() {
                    return value;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "background work never landed"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        /// A landing whose save fails while the human has a task edit open (a typed title,
        /// then the assignee field) never takes that form over: Retry and Cancel both leave the
        /// form, its focus, and its draft exactly as they were.
        // A read-only store needs Unix permissions.
        #[cfg(unix)]
        #[test]
        fn a_landing_save_failure_never_closes_an_open_task_edit() {
            for retry in [true, false] {
                let (temp, mut domain, mut model, ids) = two_marked("bulk-open-form");
                let mut host = deferred(&temp);
                let mut recovery = SaveRecovery::new();
                let mut names = Vec::new();
                for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                    step(
                        &temp,
                        &mut domain,
                        &mut model,
                        intent,
                        &mut recovery,
                        &mut host,
                        &mut names,
                    );
                }
                select(&mut domain, &mut model, ids[1]);
                for intent in [
                    BoardIntent::OpenTaskPage,
                    BoardIntent::BeginEditTitle,
                    BoardIntent::EditInsertText(" draft".into()),
                    BoardIntent::FocusFormField(crate::ui::capture::CaptureField::Assignee),
                ] {
                    apply_intent(&mut domain, &mut model, intent, None).expect("edit");
                }
                let mode = model.input_mode();
                assert_eq!(
                    model.form_focus(),
                    Some(crate::ui::capture::CaptureField::Assignee)
                );
                assert!(model.task_session_dirty());

                host.land_next();
                std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o555))
                    .expect("make the state dir read-only");
                land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
                std::fs::set_permissions(&temp.dir, std::fs::Permissions::from_mode(0o755))
                    .expect("restore the state dir");
                assert!(recovery.is_pending());
                assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);
                let answer = if retry {
                    BoardIntent::RetrySave
                } else {
                    BoardIntent::CancelSave
                };
                step(
                    &temp,
                    &mut domain,
                    &mut model,
                    answer,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
                assert!(!recovery.is_pending());

                assert_eq!(
                    model.input_mode(),
                    mode,
                    "retry {retry}: the field stays open"
                );
                assert_eq!(
                    model.form_focus(),
                    Some(crate::ui::capture::CaptureField::Assignee),
                    "retry {retry}"
                );
                assert!(
                    model.task_session_dirty(),
                    "retry {retry}: the draft survives"
                );
                apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
                    .expect("back to the title");
                assert_eq!(model.edit_buffer(), "second job draft", "retry {retry}");
                assert_eq!(
                    saved(&temp, ids[0]).dispatch.is_some(),
                    retry,
                    "retry {retry}: the landing's record follows the answer"
                );
            }
        }

        /// With launches outstanding, a store that cannot be read refuses a mutating key on the
        /// status row and a second dispatch is refused before any read: the board never exits,
        /// and the launches still land once the store is back.
        // A read-only store needs Unix permissions.
        #[cfg(unix)]
        #[test]
        fn an_unreadable_store_during_a_batch_never_exits_the_board() {
            let (temp, mut domain, mut model, ids) = two_marked("bulk-unreadable");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                step(
                    &temp,
                    &mut domain,
                    &mut model,
                    intent,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
            }
            select(&mut domain, &mut model, ids[0]);
            let file = temp.dir.join("tsk.json");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000))
                .expect("make the store unreadable");
            assert!(!step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::SetStatus(HumanStatus::Blocked),
                &mut recovery,
                &mut host,
                &mut names,
            ));
            assert!(
                model
                    .message()
                    .is_some_and(|message| message.starts_with("can't read the task store")
                        && message.contains("nothing changed")),
                "{:?}",
                model.message()
            );
            assert!(!step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::DispatchAgain,
                &mut recovery,
                &mut host,
                &mut names,
            ));
            assert!(model
                .message()
                .is_some_and(|message| message.contains("wait for it to finish")));
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
                .expect("restore the store");

            host.land_next();
            host.land_next();
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);
            for id in &ids {
                assert_eq!(saved(&temp, *id).status, HumanStatus::Started);
            }
        }

        /// Blocking and unblocking a task while its launch is in flight leaves the value where
        /// it was, but it is still a newer human decision: the landing records the agent and
        /// keeps ready.
        #[test]
        fn a_status_round_trip_during_the_launch_still_keeps_the_humans_status() {
            let (temp, mut domain, mut model, ids) = two_marked("bulk-aba");
            let mut other = temp.store.load().expect("load");
            other.set_status(ids[0], HumanStatus::Ready).expect("ready");
            temp.store.reload_merge_save(&mut other).expect("save");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            let mut names = Vec::new();
            for intent in [BoardIntent::PrimaryVerb, BoardIntent::ConfirmDispatch] {
                step(
                    &temp,
                    &mut domain,
                    &mut model,
                    intent,
                    &mut recovery,
                    &mut host,
                    &mut names,
                );
            }
            for status in [HumanStatus::Blocked, HumanStatus::Ready] {
                let mut other = temp.store.load().expect("load");
                other.set_status(ids[0], status).expect("status");
                temp.store.reload_merge_save(&mut other).expect("save");
            }
            host.land_next();
            host.land_next();
            land(&temp, &mut domain, &mut model, &mut recovery, &mut names);

            let first = saved(&temp, ids[0]);
            assert_eq!(
                first.status,
                HumanStatus::Ready,
                "the round trip is respected"
            );
            assert!(first.dispatch.is_some());
            assert_eq!(saved(&temp, ids[1]).status, HumanStatus::Started);
            let one = number(&domain, ids[0]);
            assert!(
                model
                    .message()
                    .is_some_and(|message| message
                        .contains(&format!("{one} kept ready (changed meanwhile)"))),
                "{:?}",
                model.message()
            );
        }

        /// The system host's git checks and launches run on a thread of their own, never the
        /// board thread (they would freeze the board on slow git or Herdr).
        #[test]
        fn the_system_host_checks_and_launches_off_the_board_thread() {
            use crate::dispatch::{thread_probe, DispatchError, SystemDispatchHost};
            let temp = Temp::new("bulk-system", &["builder"]);
            let project = temp.dir.join("not-a-repo");
            std::fs::create_dir_all(&project).expect("project dir");
            let mut domain = DomainState::new();
            let id = domain
                .create(
                    "system host",
                    None,
                    TaskScope::Project {
                        path: project.to_string_lossy().into(),
                    },
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("task");
            domain.assign(id, Some("builder".into())).expect("assign");
            temp.store
                .reload_merge_save(&mut domain)
                .expect("number the task");
            let eligible = crate::dispatch::check_task(&domain, id, &temp.profiles(), true)
                .expect("eligible before the git check");
            let board_thread = std::thread::current().id();
            let mut host = SystemDispatchHost::in_state_dir(temp.dir.join("host-state"));

            let checks = host.begin_git_checks(vec![project.clone()]);
            let results = eventually(|| checks.take());
            assert_eq!(results.get(&project), Some(&false));

            let batch = host.begin_launches(vec![eligible]);
            let landed = eventually(|| {
                let landed = batch.take_landed();
                (!landed.is_empty()).then_some(landed)
            });
            assert!(matches!(landed[0].1, Err(DispatchError::NeedsGitProject)));

            let threads = thread_probe::threads(&project);
            assert_eq!(threads.len(), 2, "one git check and one launch ran");
            assert!(
                threads.iter().all(|thread| *thread != board_thread),
                "the system host ran bulk work on the board thread"
            );
        }

        /// The board frame delivers a landed git check to the card: a set outside any git
        /// repository closes with the refusal, with no direct poll by the test.
        #[test]
        fn the_board_frame_delivers_the_cards_git_check() {
            let (temp, mut domain, mut model, _ids) = two_marked("bulk-frame");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            step(
                &temp,
                &mut domain,
                &mut model,
                BoardIntent::PrimaryVerb,
                &mut recovery,
                &mut host,
                &mut Vec::new(),
            );
            let (checks, _) = host.checks.take().expect("checks started");
            checks.finish(
                [(std::path::PathBuf::from(PROJECT), false)]
                    .into_iter()
                    .collect(),
            );
            super::super::board_frame(&mut model, |_| Ok(()), |_| Ok(false), false).expect("frame");
            assert!(model.dispatch_prompt().is_none());
            assert_eq!(
                model.message(),
                Some("nothing to dispatch: not a project in a git repo")
            );
        }

        /// The board loop's background step delivers the git check to a card opened in the
        /// Projects preview, from the outer board.
        #[test]
        fn the_background_step_delivers_a_preview_cards_git_check() {
            let (temp, mut domain, mut model, ids) = preview_board("bulk-preview-check");
            let mut host = deferred(&temp);
            let mut recovery = SaveRecovery::new();
            step(
                &temp,
                &mut domain,
                model.preview_seat_mut().expect("seat"),
                BoardIntent::PrimaryVerb,
                &mut recovery,
                &mut host,
                &mut Vec::new(),
            );
            let seat_prompt = |model: &mut BoardModel| {
                model
                    .preview_seat_mut()
                    .expect("seat")
                    .dispatch_prompt()
                    .cloned()
                    .expect("card")
            };
            assert!(seat_prompt(&mut model).checking());
            let (checks, _) = host.checks.take().expect("checks started");
            checks.finish(
                [(std::path::PathBuf::from("/repos/app"), true)]
                    .into_iter()
                    .collect(),
            );
            super::super::board_background_step(
                &temp.store,
                &mut domain,
                &mut model,
                &mut recovery,
                true,
                &mut host,
                &mut |_| {},
            )
            .expect("background step");
            let prompt = seat_prompt(&mut model);
            assert!(!prompt.checking(), "the check reached the preview's card");
            assert_eq!(
                prompt
                    .launch
                    .iter()
                    .map(|eligible| eligible.id)
                    .collect::<Vec<_>>(),
                ids
            );
        }
    }
}

#[cfg(test)]
mod queued_cleanup_tests {
    use std::cell::Cell;
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Instant, SystemTime};

    use super::{
        finish_queued_cleanup_with_host, handle_board_intent_with_host,
        offer_cleanup_prompt_with_host, CleanupOffer,
    };
    use crate::dispatch::{
        CleanupInspection, CreatedWorktree, DispatchHost, MergeCheck, MergeVerdict,
    };
    use crate::domain::{Dispatch, DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
    use crate::save_recovery::SaveRecovery;
    use crate::store::TaskStore;
    use crate::ui::board::{apply_intent, BoardModel, CleanupRowState};
    use crate::ui::input::BoardIntent;
    use crate::ui::queue::NavTab;

    /// The board's cleanup path must never take the fetching inspection: that is the network
    /// wait the event loop may not block on.
    struct CheckHost {
        check: MergeCheck,
        cached_merged: bool,
        /// Whether a fetch counts as fresh; the snapshot may flip it (a fetch landing mid-read).
        fresh: Rc<Cell<bool>>,
        fetch_lands_during_snapshot: bool,
        removed: usize,
        deleted: usize,
    }

    impl CheckHost {
        fn new(cached_merged: bool) -> Self {
            Self {
                check: MergeCheck::default(),
                cached_merged,
                fresh: Rc::default(),
                fetch_lands_during_snapshot: false,
                removed: 0,
                deleted: 0,
            }
        }
    }

    impl DispatchHost for CheckHost {
        fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
            Ok(true)
        }
        fn create_worktree(
            &mut self,
            _: &Path,
            _: &str,
            _: Option<&str>,
            _: &str,
        ) -> Result<CreatedWorktree, String> {
            Err("not used".into())
        }
        fn inspect_cleanup(
            &mut self,
            _: &Path,
            _: &Dispatch,
            _: bool,
        ) -> Result<CleanupInspection, String> {
            panic!("the board must not fetch on its own thread")
        }
        fn inspect_cleanup_cached(
            &mut self,
            _: &Path,
            _: &Dispatch,
            _: bool,
        ) -> Result<CleanupInspection, String> {
            if self.fetch_lands_during_snapshot {
                self.fresh.set(true);
            }
            Ok(CleanupInspection {
                unreachable_remote: None,
                worktree_exists: true,
                dirty: false,
                branch_merged: self.cached_merged,
                workspace_exists: true,
                target_matches: true,
                warning: None,
                base_available: true,
            })
        }
        fn begin_merge_check(&mut self, _: &Path, _: &Dispatch) -> Option<MergeCheck> {
            (!self.fresh.get()).then(|| self.check.clone())
        }
        fn remove_herdr_worktree(&mut self, _: &str) -> Result<(), String> {
            self.removed += 1;
            Ok(())
        }
        fn delete_branch(&mut self, _: &Path, _: &str) -> Result<(), String> {
            self.deleted += 1;
            Ok(())
        }
        fn root_pane(&mut self, _: &str) -> Result<String, crate::dispatch::RootPaneError> {
            Err("not used".into())
        }
        fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
            Err("not used".into())
        }
    }

    const PROJECT: &str = "/repos/queued";

    fn setup(label: &str) -> (std::path::PathBuf, TaskStore, DomainState, uuid::Uuid) {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tsk-queued-cleanup-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("state dir");
        let store = TaskStore::new(&dir);
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "queued",
                None,
                TaskScope::Project {
                    path: PROJECT.into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
        domain.set_status(id, HumanStatus::Started).expect("start");
        domain
            .record_dispatch(
                id,
                Dispatch {
                    argv: vec!["agent".into()],
                    worktree: "/tmp/tsk-queued".into(),
                    branch: "tsk/t1-queued".into(),
                    base: Some("origin/main".into()),
                    base_commit: None,
                    base_remote: Some("origin".into()),
                    base_ref: Some("refs/remotes/origin/main".into()),
                    herdr_workspace_id: "w1".into(),
                    at: SystemTime::now(),
                    cleaned: false,
                },
            )
            .expect("dispatch");
        store.reload_merge_save(&mut domain).expect("save");
        (dir, store, domain, id)
    }

    fn press(
        store: &TaskStore,
        domain: &mut DomainState,
        model: &mut BoardModel,
        intent: BoardIntent,
        host: &mut CheckHost,
    ) {
        let mut recovery = SaveRecovery::new();
        handle_board_intent_with_host(
            store,
            domain,
            model,
            intent,
            &mut recovery,
            false,
            true,
            host,
            &mut |_| {},
        )
        .expect("intent");
        assert!(!recovery.is_pending());
    }

    /// One iteration of the board loop's cleanup step; nothing else confirms.
    fn tick(
        store: &TaskStore,
        domain: &mut DomainState,
        model: &mut BoardModel,
        host: &mut CheckHost,
    ) {
        let mut recovery = SaveRecovery::new();
        finish_queued_cleanup_with_host(store, domain, model, &mut recovery, true, host)
            .expect("loop step");
        assert!(!recovery.is_pending());
    }

    fn verdict(merged: bool) -> MergeVerdict {
        MergeVerdict {
            unreachable_remote: None,
            branch_merged: merged,
            base_available: true,
            warning: None,
            confirmed: true,
        }
    }

    #[test]
    fn y_during_the_merged_check_saves_completion_at_once_and_cleans_when_the_verdict_lands() {
        let (dir, store, mut domain, id) = setup("lands");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut host = CheckHost::new(false);
        assert_eq!(
            offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
                .expect("offer"),
            CleanupOffer::Prompted
        );
        let pressed = Instant::now();
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        assert!(pressed.elapsed() < std::time::Duration::from_millis(500));
        // Completion never waits on the network: it is durable before the check lands.
        assert_eq!(domain.get(id).unwrap().status, HumanStatus::Done);
        let disk = store.load().expect("load");
        assert_eq!(disk.get(id).unwrap().status, HumanStatus::Done);
        assert_eq!(
            model.cleanup_run().unwrap().rows[0].state,
            CleanupRowState::Checking
        );
        // A second y on the confirmed card changes nothing.
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        tick(&store, &mut domain, &mut model, &mut host);
        assert!(!model.cleanup_run().unwrap().started, "no verdict yet");
        assert_eq!(host.removed, 0);

        host.check.complete(verdict(true));
        host.cached_merged = true; // the refs the check fetched
        tick(&store, &mut domain, &mut model, &mut host);
        assert!(model.cleanup_prompt().is_none());
        assert!(model.cleanup_run().is_none());
        assert_eq!((host.removed, host.deleted), (1, 1));
        assert_eq!(model.message(), Some("done T1 · cleaned"));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_check_that_never_lands_starts_the_cleanup_at_its_bound_keeping_the_branch() {
        let (dir, store, mut domain, id) = setup("bound");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut host = CheckHost::new(true);
        offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
            .expect("offer");
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        tick(&store, &mut domain, &mut model, &mut host);
        assert_eq!(host.removed, 0);
        model.cleanup_run_mut().unwrap().start_deadline = Instant::now();
        tick(&store, &mut domain, &mut model, &mut host);
        assert_eq!(domain.get(id).unwrap().status, HumanStatus::Done);
        assert_eq!(host.removed, 1, "the clean worktree is still removed");
        assert_eq!(
            host.deleted, 0,
            "cached ancestry says merged, but no check confirmed it"
        );
        // A kept branch is not missed: the card stays on its outcome until Esc.
        assert!(model.cleanup_run().is_some_and(|run| run.finished()));
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::CancelCleanup,
            &mut host,
        );
        assert!(model
            .message()
            .is_some_and(|message| message.contains("branch kept (merge unconfirmed)")));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn esc_after_y_hides_the_card_keeps_the_completion_and_cleans_when_the_verdict_lands() {
        let (dir, store, mut domain, id) = setup("cancel");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut host = CheckHost::new(true);
        offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
            .expect("offer");
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::CancelCleanup,
            &mut host,
        );
        assert!(model.cleanup_prompt().is_none());
        assert_eq!(domain.get(id).unwrap().status, HumanStatus::Done);
        tick(&store, &mut domain, &mut model, &mut host);
        assert_eq!(model.message(), Some("cleanup waits for merge checks…"));
        host.check.complete(verdict(true));
        tick(&store, &mut domain, &mut model, &mut host);
        assert_eq!((host.removed, host.deleted), (1, 1));
        assert!(domain.get(id).unwrap().dispatch.as_ref().unwrap().cleaned);
        assert_eq!(model.message(), Some("done T1 · cleaned"));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn y_in_the_focused_project_preview_is_finished_by_the_outer_loop() {
        let (dir, store, mut domain, id) = setup("preview");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("projects overview");
        for _ in 0..2 {
            apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
                .expect("stage right");
        }
        assert!(model.project_right_seat_focused());
        let mut host = CheckHost::new(false);
        assert_eq!(
            offer_cleanup_prompt_with_host(
                &mut domain,
                model.input_target_mut(),
                id,
                true,
                &mut host
            )
            .expect("offer"),
            CleanupOffer::Prompted
        );
        press(
            &store,
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        assert!(
            model.cleanup_prompt().is_none(),
            "the card lives in the preview"
        );
        assert_eq!(domain.get(id).unwrap().status, HumanStatus::Done);
        tick(&store, &mut domain, &mut model, &mut host);
        assert!(model.input_target_mut().cleanup_card_open());

        host.check.complete(verdict(true));
        host.cached_merged = true;
        tick(&store, &mut domain, &mut model, &mut host);
        assert!(model.input_target_mut().cleanup_prompt().is_none());
        assert_eq!((host.removed, host.deleted), (1, 1));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    fn focus_project_preview(domain: &mut DomainState, model: &mut BoardModel) {
        apply_intent(
            domain,
            model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("projects overview");
        for _ in 0..2 {
            apply_intent(domain, model, BoardIntent::StageRight, None).expect("stage right");
        }
        assert!(model.project_right_seat_focused());
    }

    #[test]
    fn a_cleanup_confirmed_in_the_project_preview_outlives_dropping_and_rebinding_it() {
        let (dir, store, mut domain, id) = setup("preview-drop");
        let mut model = BoardModel::from_domain(&domain, None);
        focus_project_preview(&mut domain, &mut model);
        let mut host = CheckHost::new(false);
        offer_cleanup_prompt_with_host(&mut domain, model.input_target_mut(), id, true, &mut host)
            .expect("offer");
        press(
            &store,
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        press(
            &store,
            &mut domain,
            model.input_target_mut(),
            BoardIntent::CancelCleanup,
            &mut host,
        );
        // Leave the overview (the preview is dropped), then come back (a new preview binds).
        model.drop_project_preview();
        assert!(model.right_seat().is_none());
        assert!(
            model.cleanup_running(),
            "the run is not the preview's to lose"
        );
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Desk),
            None,
        )
        .expect("tasks");
        focus_project_preview(&mut domain, &mut model);
        assert!(model.input_target_mut().cleanup_running());
        tick(&store, &mut domain, &mut model, &mut host);
        assert_eq!(model.message(), Some("cleanup waits for merge checks…"));

        host.check.complete(verdict(true));
        host.cached_merged = true;
        tick(&store, &mut domain, &mut model, &mut host);
        assert_eq!((host.removed, host.deleted), (1, 1));
        let disk = store.load().expect("load");
        assert!(
            disk.get(id).unwrap().dispatch.as_ref().unwrap().cleaned,
            "the cleaned marker still lands"
        );
        assert!(!model.cleanup_running());
        assert_eq!(model.message(), Some("done T1 · cleaned"));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn quit_waits_for_a_cleanup_whose_preview_was_dropped() {
        let (dir, store, mut domain, id) = setup("preview-quit");
        let mut model = BoardModel::from_domain(&domain, None);
        focus_project_preview(&mut domain, &mut model);
        let mut host = CheckHost::new(false);
        offer_cleanup_prompt_with_host(&mut domain, model.input_target_mut(), id, true, &mut host)
            .expect("offer");
        press(
            &store,
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        model.drop_project_preview();
        let mut recovery = SaveRecovery::new();
        let quit = handle_board_intent_with_host(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::Quit,
            &mut recovery,
            false,
            true,
            &mut host,
            &mut |_| {},
        )
        .expect("quit");
        assert!(!quit);
        assert!(!model.quit_after_cleanup_due());
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_cleanup_whose_preview_card_parks_reports_and_releases_on_the_visible_board() {
        let (dir, store, mut domain, id) = setup("parked-kept");
        let mut model = BoardModel::from_domain(&domain, None);
        focus_project_preview(&mut domain, &mut model);
        // Cached refs say merged, but the check never lands: the branch will be kept.
        let mut host = CheckHost::new(true);
        offer_cleanup_prompt_with_host(&mut domain, model.input_target_mut(), id, true, &mut host)
            .expect("offer");
        press(
            &store,
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        // The frame narrows: the preview and its card park, unpainted and unreachable.
        super::sync_frame_presentation(ratatui::layout::Rect::new(0, 0, 60, 24), &model);
        assert!(!model.project_right_seat_focused());
        tick(&store, &mut domain, &mut model, &mut host);
        assert_eq!(
            model.message(),
            Some("cleanup waits for merge checks…"),
            "progress shows on the visible board"
        );
        model.cleanup_run_mut().unwrap().start_deadline = Instant::now();
        tick(&store, &mut domain, &mut model, &mut host);
        assert_eq!(host.removed, 1);
        assert!(
            !model.cleanup_running(),
            "a finished run never stays busy behind a parked card"
        );
        assert!(model
            .message()
            .is_some_and(|message| message.contains("branch kept (merge unconfirmed)")));
        assert!(model
            .preview_seat_mut()
            .is_some_and(|seat| seat.cleanup_prompt().is_none()));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_busy_refusal_on_the_outer_board_clears_when_the_preview_closes_the_finished_card() {
        let (dir, store, mut domain, id) = setup("outer-refusal");
        let mut model = BoardModel::from_domain(&domain, None);
        focus_project_preview(&mut domain, &mut model);
        // Cached refs say merged, but the check never lands: the branch is kept, so the
        // finished card stays open until Esc.
        let mut host = CheckHost::new(true);
        offer_cleanup_prompt_with_host(&mut domain, model.input_target_mut(), id, true, &mut host)
            .expect("offer");
        press(
            &store,
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        let narrow = ratatui::layout::Rect::new(0, 0, 60, 24);
        let wide = ratatui::layout::Rect::new(0, 0, 160, 40);
        super::sync_frame_presentation(narrow, &model);
        assert!(!model.project_right_seat_focused());
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::Complete,
            &mut host,
        );
        assert_eq!(model.message(), Some(super::CLEANUP_BUSY));

        super::sync_frame_presentation(wide, &model);
        assert!(model.project_right_seat_focused());
        model.cleanup_run_mut().unwrap().start_deadline = Instant::now();
        tick(&store, &mut domain, &mut model, &mut host);
        assert!(model.cleanup_run().is_some_and(|run| run.finished()));
        assert!(model.input_target_mut().cleanup_card_open());
        press(
            &store,
            &mut domain,
            model.input_target_mut(),
            BoardIntent::CancelCleanup,
            &mut host,
        );
        tick(&store, &mut domain, &mut model, &mut host);

        super::sync_frame_presentation(narrow, &model);
        assert_ne!(
            model.message(),
            Some(super::CLEANUP_BUSY),
            "the outer board's refusal does not outlive the run"
        );
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    /// A preview task-page edit whose save fails, then the frame narrows so the preview parks.
    /// Returns the persistent recovery, with the visible board showing it.
    #[cfg(unix)]
    fn parked_preview_with_a_failed_task_edit(
        dir: &Path,
        store: &TaskStore,
        domain: &mut DomainState,
        model: &mut BoardModel,
    ) -> SaveRecovery<DomainState> {
        use std::os::unix::fs::PermissionsExt;
        apply_intent(
            domain,
            model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("projects overview");
        for _ in 0..2 {
            apply_intent(domain, model, BoardIntent::StageRight, None).expect("stage right");
        }
        assert!(model.project_right_seat_focused());
        let seat = model.input_target_mut();
        apply_intent(domain, seat, BoardIntent::OpenTaskPage, None).expect("task page");
        apply_intent(domain, seat, BoardIntent::BeginEditTitle, None).expect("edit title");
        apply_intent(
            domain,
            seat,
            BoardIntent::EditInsertText(" edited".into()),
            None,
        )
        .expect("type");
        let mut recovery = SaveRecovery::new();
        let mut host = CheckHost::new(false);
        let permissions = std::fs::Permissions::from_mode;
        std::fs::set_permissions(dir, permissions(0o555)).expect("read-only state dir");
        let saved = handle_board_intent_with_host(
            store,
            domain,
            model.input_target_mut(),
            BoardIntent::ConfirmEdit,
            &mut recovery,
            false,
            true,
            &mut host,
            &mut |_| {},
        );
        std::fs::set_permissions(dir, permissions(0o755)).expect("writable again");
        saved.expect("confirm");
        assert!(recovery.is_pending(), "the preview's save failed");
        let seat = model.right_seat().expect("seat");
        assert!(seat.owns_save_recovery() && seat.task_edit_save_held());

        super::sync_frame_presentation(ratatui::layout::Rect::new(0, 0, 60, 24), model);
        assert!(!model.project_right_seat_focused());
        finish_queued_cleanup_with_host(store, domain, model, &mut recovery, true, &mut host)
            .expect("loop step");
        assert_eq!(
            model.input_mode(),
            crate::ui::board::BoardInputMode::SaveRecovery
        );
        recovery
    }

    /// Answer from the visible board through the event loop's own dispatch boundary.
    #[cfg(unix)]
    fn answer_recovery(
        store: &TaskStore,
        domain: &mut DomainState,
        model: &mut BoardModel,
        recovery: &mut SaveRecovery<DomainState>,
        intent: BoardIntent,
    ) {
        super::dispatch_board_intent(
            store,
            domain,
            model,
            super::BoardDispatchRoute {
                area: ratatui::layout::Rect::new(0, 0, 60, 24),
                target: super::BoardIntentTarget::Focused,
            },
            intent,
            recovery,
            false,
        )
        .expect("answer");
        let mut host = CheckHost::new(false);
        finish_queued_cleanup_with_host(store, domain, model, recovery, true, &mut host)
            .expect("loop step");
    }

    #[cfg(unix)]
    #[test]
    fn cancel_on_the_visible_board_unwinds_a_parked_previews_failed_task_edit() {
        let (dir, store, mut domain, id) = setup("parked-cancel");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery =
            parked_preview_with_a_failed_task_edit(&dir, &store, &mut domain, &mut model);
        answer_recovery(
            &store,
            &mut domain,
            &mut model,
            &mut recovery,
            BoardIntent::CancelSave,
        );
        assert!(!recovery.is_pending());
        assert_eq!(model.input_mode(), crate::ui::board::BoardInputMode::Normal);
        assert_eq!(model.message(), Some("save cancelled"));
        let seat = model.right_seat().expect("seat");
        assert!(!seat.task_edit_save_held(), "the hold is released");
        assert!(!seat.owns_save_recovery() && !seat.shows_save_recovery_proxy());
        assert_eq!(
            seat.task_form_title(),
            Some("queued"),
            "the cancelled draft is gone"
        );
        assert_eq!(store.load().unwrap().get(id).unwrap().title, "queued");
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn retry_on_the_visible_board_lands_and_releases_a_parked_previews_failed_task_edit() {
        let (dir, store, mut domain, id) = setup("parked-retry");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut recovery =
            parked_preview_with_a_failed_task_edit(&dir, &store, &mut domain, &mut model);
        answer_recovery(
            &store,
            &mut domain,
            &mut model,
            &mut recovery,
            BoardIntent::RetrySave,
        );
        assert!(!recovery.is_pending());
        assert_eq!(model.input_mode(), crate::ui::board::BoardInputMode::Normal);
        assert_eq!(
            store.load().unwrap().get(id).unwrap().title,
            "queued edited"
        );
        let seat = model.right_seat().expect("seat");
        assert!(!seat.task_edit_save_held(), "the hold is released");
        assert!(!seat.owns_save_recovery() && !seat.shows_save_recovery_proxy());
        assert_eq!(seat.task_form_title(), Some("queued edited"));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    /// The outer board's save failed while the pane was narrow, then the pane widened so the
    /// focused preview shows a proxy of the banner. Answer from that preview's palette.
    fn palette_answer_reaches_the_outer_owner(answer: BoardIntent, expected: &str) {
        let (dir, store, mut domain, _id) = setup("palette-recovery");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("projects overview");
        for _ in 0..2 {
            apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
                .expect("stage right");
        }
        let narrow = ratatui::layout::Rect::new(0, 0, 60, 24);
        let wide = ratatui::layout::Rect::new(0, 0, 160, 40);
        super::sync_frame_presentation(narrow, &model);
        assert!(!model.project_right_seat_focused());
        let mut recovery = SaveRecovery::new();
        recovery.fail(domain.clone(), domain.clone(), "disk full");
        model.begin_save_recovery("disk full");
        assert!(model.owns_save_recovery());

        super::sync_frame_presentation(wide, &model);
        assert!(model.project_right_seat_focused());
        let mut host = CheckHost::new(false);
        finish_queued_cleanup_with_host(
            &store,
            &mut domain,
            &mut model,
            &mut recovery,
            true,
            &mut host,
        )
        .expect("loop step");
        assert!(model.right_seat().unwrap().shows_save_recovery_proxy());

        let seat = model.input_target_mut();
        apply_intent(&mut domain, seat, BoardIntent::OpenCommandPalette, None).expect("palette");
        let index = seat
            .visible_commands()
            .iter()
            .position(|command| command.intent == answer)
            .expect("recovery command offered");
        let intent = if answer == BoardIntent::RetrySave {
            for _ in 0..index {
                apply_intent(&mut domain, seat, BoardIntent::CommandNext, None).expect("move");
            }
            BoardIntent::ConfirmCommand
        } else {
            BoardIntent::SelectCommand(index)
        };
        super::dispatch_board_intent(
            &store,
            &mut domain,
            &mut model,
            super::BoardDispatchRoute {
                area: wide,
                target: super::BoardIntentTarget::Focused,
            },
            intent,
            &mut recovery,
            false,
        )
        .expect("answer");
        assert!(!recovery.is_pending());
        assert!(
            !model.owns_save_recovery(),
            "the outer owner is resolved, not left for a stale-banner sweep"
        );
        assert_eq!(model.message(), Some(expected));
        assert!(!model.right_seat().unwrap().shows_save_recovery_proxy());
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn palette_retry_on_a_focused_proxy_resolves_the_outer_owner() {
        palette_answer_reaches_the_outer_owner(BoardIntent::RetrySave, "saved");
    }

    #[test]
    fn palette_cancel_on_a_focused_proxy_resolves_the_outer_owner() {
        palette_answer_reaches_the_outer_owner(BoardIntent::CancelSave, "save cancelled");
    }

    #[test]
    fn a_check_that_failed_keeps_the_branch_even_when_cached_refs_later_read_merged() {
        let (dir, store, mut domain, id) = setup("failed-check");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut host = CheckHost::new(false);
        offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
            .expect("offer");
        // The ancestry query timed out: the check lands, but it confirmed nothing.
        host.check.complete(MergeVerdict {
            unreachable_remote: None,
            branch_merged: false,
            base_available: false,
            warning: Some("ancestry check timed out".into()),
            confirmed: false,
        });
        tick(&store, &mut domain, &mut model, &mut host);
        assert!(!model.cleanup_prompt().unwrap().checking());
        // The transient failure clears: the refs on disk now read as merged.
        host.cached_merged = true;
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        assert_eq!(domain.get(id).unwrap().status, HumanStatus::Done);
        assert_eq!((host.removed, host.deleted), (1, 0), "branch kept");
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn an_offline_check_keeps_the_branch_although_cached_refs_read_merged() {
        let (dir, store, mut domain, id) = setup("offline-check");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut host = CheckHost::new(true);
        offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
            .expect("offer");
        // The fetch failed: the refs on disk read as merged, but the remote may have been
        // force-pushed since.
        host.check.complete(MergeVerdict {
            unreachable_remote: Some("origin".into()),
            branch_merged: true,
            base_available: true,
            warning: Some("fetch failed: offline; merged status not confirmed".into()),
            confirmed: true,
        });
        tick(&store, &mut domain, &mut model, &mut host);
        let row = &model.cleanup_prompt().unwrap().rows[0];
        assert!(!row.checking() && !row.branch_deletable());
        assert_eq!(row.cleanup_refs(), crate::dispatch::CleanupRefs::Offline);
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        assert_eq!(domain.get(id).unwrap().status, HumanStatus::Done);
        assert_eq!((host.removed, host.deleted), (1, 0), "branch kept");
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::CancelCleanup,
            &mut host,
        );
        assert!(model
            .message()
            .is_some_and(|message| message.contains("branch kept (offline)")));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn y_on_a_card_whose_dispatch_was_relaunched_meanwhile_leaves_the_new_one_alone() {
        let (dir, store, mut domain, id) = setup("relaunched");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut host = CheckHost::new(true);
        offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
            .expect("offer");
        host.check.complete(verdict(true));
        tick(&store, &mut domain, &mut model, &mut host);

        // The CLI cleans and relaunches the task while the card is open.
        let mut other = store.load().expect("load");
        other
            .record_dispatch_cleaned(id, crate::domain::CleanupOutcome::Removed)
            .expect("cleaned");
        store.reload_merge_save(&mut other).expect("save cleaned");
        let mut other = store.load().expect("load");
        let mut relaunched = other.get(id).unwrap().dispatch.clone().unwrap();
        relaunched.cleaned = false;
        relaunched.herdr_workspace_id = "w2".into();
        relaunched.at = SystemTime::now() + std::time::Duration::from_secs(1);
        other
            .record_dispatch(id, relaunched.clone())
            .expect("relaunch");
        store.reload_merge_save(&mut other).expect("save");

        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        assert_eq!(
            (host.removed, host.deleted),
            (0, 0),
            "the new launch is untouched"
        );
        assert_eq!(domain.get(id).unwrap().dispatch.as_ref(), Some(&relaunched));
        press(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::CancelCleanup,
            &mut host,
        );
        assert!(model
            .message()
            .is_some_and(|message| message.contains("kept (dispatch changed)")));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_queued_y_in_a_preview_parked_by_a_narrowing_frame_still_finishes_on_time() {
        let (dir, store, mut domain, id) = setup("parked");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("projects overview");
        for _ in 0..2 {
            apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
                .expect("stage right");
        }
        assert!(model.project_right_seat_focused());
        let mut host = CheckHost::new(false);
        offer_cleanup_prompt_with_host(&mut domain, model.input_target_mut(), id, true, &mut host)
            .expect("offer");
        press(
            &store,
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        // The terminal narrows before the check lands: the preview parks and input returns
        // to the outer index, but the queued `y` still belongs to the preview's card.
        super::sync_frame_presentation(ratatui::layout::Rect::new(0, 0, 60, 24), &model);
        assert!(!model.project_right_seat_focused());
        host.check.complete(verdict(true));
        host.cached_merged = true;
        tick(&store, &mut domain, &mut model, &mut host);
        assert_eq!(domain.get(id).unwrap().status, HumanStatus::Done);
        assert_eq!((host.removed, host.deleted), (1, 1));
        assert!(model
            .preview_seat_mut()
            .is_none_or(|seat| seat.cleanup_prompt().is_none()));
        assert!(
            model
                .message()
                .is_some_and(|message| message.starts_with("done T")),
            "the outcome shows on the visible board: {:?}",
            model.message()
        );
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn a_parked_preview_whose_queued_cleanup_fails_to_save_shows_recovery_on_the_visible_board() {
        use crate::ui::board::BoardInputMode;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use std::os::unix::fs::PermissionsExt;
        let (dir, store, mut domain, id) = setup("parked-save");
        let mut model = BoardModel::from_domain(&domain, None);
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .expect("projects overview");
        for _ in 0..2 {
            apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
                .expect("stage right");
        }
        let mut host = CheckHost::new(false);
        offer_cleanup_prompt_with_host(&mut domain, model.input_target_mut(), id, true, &mut host)
            .expect("offer");
        press(
            &store,
            &mut domain,
            model.input_target_mut(),
            BoardIntent::ConfirmCleanup,
            &mut host,
        );
        super::sync_frame_presentation(ratatui::layout::Rect::new(0, 0, 60, 24), &model);
        assert!(!model.project_right_seat_focused());

        // The save under the parked preview fails.
        let permissions = |mode| std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(&dir, permissions(0o555)).expect("read-only state dir");
        host.check.complete(verdict(true));
        host.cached_merged = true;
        let mut recovery = SaveRecovery::new();
        let finished = finish_queued_cleanup_with_host(
            &store,
            &mut domain,
            &mut model,
            &mut recovery,
            true,
            &mut host,
        );
        std::fs::set_permissions(&dir, permissions(0o755)).expect("writable again");
        finished.expect("loop step");
        assert!(recovery.is_pending(), "the save failed");
        assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);
        assert!(model
            .message()
            .is_some_and(|message| message.contains("save failed")));
        assert_eq!(
            super::board_keyboard_intent(
                &model,
                model.input_mode(),
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)
            ),
            Some(BoardIntent::RetrySave)
        );

        handle_board_intent_with_host(
            &store,
            &mut domain,
            &mut model,
            BoardIntent::RetrySave,
            &mut recovery,
            false,
            true,
            &mut host,
            &mut |_| {},
        )
        .expect("retry");
        assert!(!recovery.is_pending());
        finish_queued_cleanup_with_host(
            &store,
            &mut domain,
            &mut model,
            &mut recovery,
            true,
            &mut host,
        )
        .expect("loop step");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert!(model
            .right_seat()
            .is_some_and(|seat| seat.popup() != crate::ui::mouse::BoardPopup::SaveRecovery));
        let disk = store.load().expect("load");
        assert_eq!(disk.get(id).unwrap().status, HumanStatus::Done);
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_fetch_landing_during_the_card_snapshot_still_leaves_a_check_to_refresh_it() {
        let (dir, _store, mut domain, id) = setup("interleave");
        let mut model = BoardModel::from_domain(&domain, None);
        let mut host = CheckHost::new(false);
        host.fetch_lands_during_snapshot = true;
        offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
            .expect("offer");
        assert!(
            model.cleanup_prompt().unwrap().checking(),
            "pre-fetch ancestry is never shown as a final verdict"
        );
        std::fs::remove_dir_all(dir).expect("cleanup");
    }
}

#[cfg(test)]
mod bulk_cleanup_tests {
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Instant, SystemTime};

    use super::{
        finish_queued_cleanup_with_host, handle_board_intent_with_host,
        offer_bulk_cleanup_prompt_with_host, BulkCleanupOffer,
    };
    use crate::dispatch::{
        run_cleanup_job, run_cleanup_row, CleanupError, CleanupInspection, CleanupJob,
        CleanupPlanRow, CreatedWorktree, DispatchHost, MergeCheck, MergeVerdict,
    };
    use crate::domain::{
        Dispatch, DomainState, HumanStatus, ProvenanceOrigin, TaskScope, UndoEntry,
    };
    use crate::save_recovery::SaveRecovery;
    use crate::store::TaskStore;
    use crate::ui::board::{apply_intent, BoardInputMode, BoardModel, CleanupRowState};
    use crate::ui::input::BoardIntent;
    use crate::ui::mouse::BoardPopup;

    const PROJECT: &str = "/repos/bulk";

    /// Per-worktree fake: which are dirty or gone, one merged check per worktree, and a log of
    /// what was removed. The fetching inspection is never allowed on the board's thread.
    #[derive(Default)]
    struct BulkHost {
        dirty: HashSet<String>,
        missing: HashSet<String>,
        cached_merged: bool,
        checks: HashMap<String, MergeCheck>,
        removed: Vec<String>,
        deleted: Vec<String>,
        /// Workspaces whose removal fails.
        fail: HashSet<String>,
        /// A slow host: `y` hands the plan over and the test lands each row by hand.
        hold: bool,
        held: Option<(CleanupJob, Vec<CleanupPlanRow>)>,
    }

    impl DispatchHost for BulkHost {
        fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
            Ok(true)
        }
        fn create_worktree(
            &mut self,
            _: &Path,
            _: &str,
            _: Option<&str>,
            _: &str,
        ) -> Result<CreatedWorktree, String> {
            Err("not used".into())
        }
        fn inspect_cleanup(
            &mut self,
            _: &Path,
            _: &Dispatch,
            _: bool,
        ) -> Result<CleanupInspection, String> {
            panic!("the board must not fetch on its own thread")
        }
        fn inspect_cleanup_cached(
            &mut self,
            _: &Path,
            record: &Dispatch,
            _: bool,
        ) -> Result<CleanupInspection, String> {
            Ok(CleanupInspection {
                unreachable_remote: None,
                worktree_exists: !self.missing.contains(&record.worktree),
                dirty: self.dirty.contains(&record.worktree),
                branch_merged: self.cached_merged,
                workspace_exists: true,
                target_matches: true,
                warning: None,
                base_available: true,
            })
        }
        fn begin_merge_check(&mut self, _: &Path, record: &Dispatch) -> Option<MergeCheck> {
            Some(
                self.checks
                    .entry(record.worktree.clone())
                    .or_default()
                    .clone(),
            )
        }
        fn remove_herdr_worktree(&mut self, workspace_id: &str) -> Result<(), String> {
            if self.fail.contains(workspace_id) {
                return Err(format!("herdr could not remove {workspace_id}"));
            }
            self.removed.push(workspace_id.to_string());
            Ok(())
        }
        fn delete_branch(&mut self, _: &Path, branch: &str) -> Result<(), String> {
            self.deleted.push(branch.to_string());
            Ok(())
        }
        fn begin_cleanup(&mut self, job: CleanupJob, plan: Vec<CleanupPlanRow>, in_herdr: bool) {
            if self.hold {
                self.held = Some((job, plan));
            } else {
                run_cleanup_job(&job, &plan, in_herdr, self);
            }
        }
        fn root_pane(&mut self, _: &str) -> Result<String, crate::dispatch::RootPaneError> {
            Err("not used".into())
        }
        fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
            Err("not used".into())
        }
    }

    impl BulkHost {
        /// The worker picks up row `index` without finishing it.
        fn start(&self, index: usize) {
            self.held.as_ref().expect("held job").0.start(index);
        }

        /// The worker finishes row `index`; the job settles once every row is done.
        fn release(&mut self, index: usize) {
            let (job, plan) = self.held.clone().expect("held job");
            run_cleanup_row(&job, index, &plan[index], true, self);
            let (slots, _) = job.snapshot().expect("job idle");
            if slots
                .iter()
                .all(|slot| matches!(slot, crate::dispatch::CleanupSlot::Done(_)))
            {
                job.settle();
            }
        }

        fn land_all(&mut self, merged: bool) {
            for check in self.checks.values() {
                check.complete(MergeVerdict {
                    unreachable_remote: None,
                    branch_merged: merged,
                    base_available: true,
                    warning: None,
                    confirmed: true,
                });
            }
            self.cached_merged = merged;
        }
    }

    struct Board {
        dir: PathBuf,
        store: TaskStore,
        domain: DomainState,
        model: BoardModel,
        /// clean (dispatched), dirty (dispatched), plain (no dispatch)
        ids: [uuid::Uuid; 3],
    }

    impl Drop for Board {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn dispatched(label: &str) -> Dispatch {
        Dispatch {
            argv: vec!["agent".into()],
            worktree: format!("/tmp/tsk-bulk-{label}"),
            branch: format!("tsk/{label}"),
            base: Some("origin/main".into()),
            base_commit: None,
            base_remote: Some("origin".into()),
            base_ref: Some("refs/remotes/origin/main".into()),
            herdr_workspace_id: format!("w-{label}"),
            at: SystemTime::now(),
            cleaned: false,
        }
    }

    /// A marked set of three: a clean dispatch, a dirty dispatch, and an undispatched task.
    fn marked_board(label: &str) -> Board {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tsk-bulk-cleanup-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("state dir");
        let store = TaskStore::new(&dir);
        let mut domain = DomainState::new();
        let mut create = |title: &str| {
            let id = domain
                .create(
                    title,
                    None,
                    TaskScope::Project {
                        path: PROJECT.into(),
                    },
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("create");
            domain.set_status(id, HumanStatus::Started).expect("start");
            id
        };
        let ids = [create("clean"), create("dirty"), create("plain")];
        domain
            .record_dispatch(ids[0], dispatched("clean"))
            .expect("dispatch");
        domain
            .record_dispatch(ids[1], dispatched("dirty"))
            .expect("dispatch");
        store.reload_merge_save(&mut domain).expect("save");
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(PROJECT)));
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
            .expect("mark mode");
        for id in ids {
            let index = model
                .visible_ids()
                .iter()
                .position(|visible| *visible == id)
                .expect("visible");
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::SelectIndex(index),
                None,
            )
            .expect("select");
            apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark");
        }
        assert!(model.bulk_verb_active());
        Board {
            dir,
            store,
            domain,
            model,
            ids,
        }
    }

    fn host() -> BulkHost {
        BulkHost {
            dirty: HashSet::from(["/tmp/tsk-bulk-dirty".to_string()]),
            ..BulkHost::default()
        }
    }

    fn press(board: &mut Board, intent: BoardIntent, host: &mut BulkHost) {
        let mut recovery = SaveRecovery::new();
        handle_board_intent_with_host(
            &board.store,
            &mut board.domain,
            &mut board.model,
            intent,
            &mut recovery,
            false,
            true,
            host,
            &mut |_| {},
        )
        .expect("intent");
        assert!(!recovery.is_pending());
    }

    fn tick(board: &mut Board, host: &mut BulkHost) {
        let mut recovery = SaveRecovery::new();
        finish_queued_cleanup_with_host(
            &board.store,
            &mut board.domain,
            &mut board.model,
            &mut recovery,
            true,
            host,
        )
        .expect("loop step");
    }

    fn statuses(board: &Board) -> Vec<HumanStatus> {
        board
            .ids
            .iter()
            .map(|id| board.domain.get(*id).unwrap().status)
            .collect()
    }

    fn cleaned(board: &Board, index: usize) -> bool {
        board
            .domain
            .get(board.ids[index])
            .unwrap()
            .dispatch
            .as_ref()
            .unwrap()
            .cleaned
    }

    #[test]
    fn bulk_done_on_a_set_with_live_dispatches_opens_one_card_and_keeps_marks() {
        let mut board = marked_board("open");
        let mut host = host();
        press(&mut board, BoardIntent::Complete, &mut host);
        let prompt = board.model.cleanup_prompt().expect("bulk card");
        assert_eq!(prompt.rows.len(), 2, "one row per live dispatch");
        assert!(prompt.checking(), "each verdict starts as checking");
        let bulk = prompt.bulk.as_ref().expect("bulk");
        assert_eq!(bulk.targets.len(), 3);
        assert_eq!(bulk.plain.len(), 1);
        assert_eq!(board.model.input_mode(), BoardInputMode::CleanupConfirm);
        assert_eq!(
            board.model.marked_ids().len(),
            3,
            "marks survive while the card is up"
        );
        assert!(statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Started));
        assert!(host.removed.is_empty());
    }

    #[test]
    fn esc_on_the_bulk_card_changes_nothing_and_keeps_the_marked_set() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut board = marked_board("esc");
        let mut host = host();
        press(&mut board, BoardIntent::Complete, &mut host);
        let key = |code| {
            super::board_keyboard_intent(
                &board.model,
                board.model.input_mode(),
                KeyEvent::new(code, KeyModifiers::NONE),
            )
        };
        // The card owns Esc and `M`: mark mode must not clear the set out from under it.
        assert_eq!(key(KeyCode::Esc), Some(BoardIntent::CancelCleanup));
        assert_eq!(key(KeyCode::Char('M')), None);
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        assert!(board.model.cleanup_prompt().is_none());
        assert_eq!(board.model.popup(), BoardPopup::None);
        assert!(statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Started));
        assert!(host.removed.is_empty() && host.deleted.is_empty());
        assert_eq!(board.model.marked_ids().len(), 3);
        assert!(board.model.bulk_verb_active());
        let disk = board.store.load().expect("load");
        assert!(disk
            .tasks()
            .iter()
            .all(|task| task.status == HumanStatus::Started));
    }

    #[test]
    fn y_cleans_every_clean_worktree_keeps_dirty_ones_and_completes_the_set_as_one_undo() {
        let mut board = marked_board("yes");
        let mut host = host();
        press(&mut board, BoardIntent::Complete, &mut host);
        host.land_all(true);
        tick(&mut board, &mut host);
        assert!(!board.model.cleanup_prompt().unwrap().checking());
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        // The dirty row is kept, so the finished card stays on its rows until Esc.
        assert!(board.model.cleanup_run().is_some_and(|run| run.finished()));
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        assert!(board.model.cleanup_prompt().is_none());
        assert!(statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Done));
        assert_eq!(host.removed, vec!["w-clean".to_string()]);
        assert_eq!(host.deleted, vec!["tsk/clean".to_string()]);
        assert!(cleaned(&board, 0));
        assert!(!cleaned(&board, 1), "a dirty worktree is never cleaned");
        assert!(board.model.marked_ids().is_empty());
        let message = board.model.message().unwrap_or_default().to_string();
        assert!(message.contains("done 3"), "{message}");
        assert!(message.contains("cleaned 1"), "{message}");
        assert!(
            message.contains("kept T2 (uncommitted changes)"),
            "{message}"
        );

        let disk = board
            .store
            .load()
            .expect("completion and cleaned markers persisted");
        assert!(disk
            .tasks()
            .iter()
            .all(|task| task.status == HumanStatus::Done));
        assert!(matches!(
            disk.last_undo(),
            Some(UndoEntry::Batch { entries }) if entries.len() == 3
        ));
        press(&mut board, BoardIntent::Undo, &mut host);
        assert!(
            statuses(&board)
                .iter()
                .all(|status| *status == HumanStatus::Open),
            "one undo reopens the whole set"
        );
        assert!(
            cleaned(&board, 0),
            "undo reverses only completion, not cleanup"
        );
    }

    #[test]
    fn n_completes_the_whole_set_and_keeps_every_worktree() {
        let mut board = marked_board("no");
        let mut host = host();
        press(&mut board, BoardIntent::Complete, &mut host);
        press(&mut board, BoardIntent::KeepCleanup, &mut host);
        assert!(statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Done));
        assert!(host.removed.is_empty() && host.deleted.is_empty());
        assert!(!cleaned(&board, 0) && !cleaned(&board, 1));
        assert!(board.model.marked_ids().is_empty());
        assert_eq!(board.model.message(), Some("done 3 · worktrees kept"));
        assert!(
            board.model.message_is_sticky(),
            "kept worktrees are something kept: the outcome stays"
        );
        let disk = board.store.load().expect("load");
        assert!(matches!(
            disk.last_undo(),
            Some(UndoEntry::Batch { entries }) if entries.len() == 3
        ));
    }

    #[test]
    fn a_bulk_row_whose_check_failed_keeps_its_branch_on_y() {
        let mut board = marked_board("failed-check");
        let mut host = host();
        press(&mut board, BoardIntent::Complete, &mut host);
        for check in host.checks.values() {
            check.complete(MergeVerdict {
                unreachable_remote: None,
                branch_merged: false,
                base_available: false,
                warning: Some("ancestry check timed out".into()),
                confirmed: false,
            });
        }
        tick(&mut board, &mut host);
        assert!(!board.model.cleanup_prompt().unwrap().checking());
        host.cached_merged = true;
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        assert!(statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Done));
        assert_eq!(host.removed, vec!["w-clean".to_string()]);
        assert!(
            host.deleted.is_empty(),
            "a failed check never confirms a merge"
        );
    }

    #[test]
    fn a_bulk_row_whose_check_was_offline_keeps_its_branch_on_y() {
        let mut board = marked_board("offline-check");
        let mut host = host();
        host.cached_merged = true;
        press(&mut board, BoardIntent::Complete, &mut host);
        for check in host.checks.values() {
            check.complete(MergeVerdict {
                unreachable_remote: Some("origin".into()),
                branch_merged: true,
                base_available: true,
                warning: Some("fetch failed: offline; merged status not confirmed".into()),
                confirmed: true,
            });
        }
        tick(&mut board, &mut host);
        assert!(!board.model.cleanup_prompt().unwrap().checking());
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        assert!(statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Done));
        assert_eq!(host.removed, vec!["w-clean".to_string()]);
        assert!(
            host.deleted.is_empty(),
            "an unreachable remote never confirms a merge"
        );
    }

    #[test]
    fn a_queued_bulk_y_waits_for_every_check_and_keeps_unconfirmed_branches_at_its_bound() {
        let mut board = marked_board("queued");
        let mut host = host();
        host.cached_merged = true;
        press(&mut board, BoardIntent::Complete, &mut host);
        let pressed = Instant::now();
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        // No check ever lands here: a `y` that waited for them would block until their bound.
        // Half that bound still catches it, with seconds to spare for a loaded runner's save.
        assert!(pressed.elapsed() < crate::dispatch::MERGE_CHECK_TIMEOUT / 2);
        // The set is done at once; only the worker waits for the checks.
        assert!(disk_statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Done));
        tick(&mut board, &mut host);
        assert!(
            !board.model.cleanup_run().unwrap().started,
            "no verdict yet"
        );
        board.model.cleanup_run_mut().unwrap().start_deadline = Instant::now();
        tick(&mut board, &mut host);
        assert_eq!(host.removed, vec!["w-clean".to_string()]);
        assert!(
            host.deleted.is_empty(),
            "cached refs say merged, but no check confirmed it"
        );
    }

    #[test]
    fn a_set_whose_only_dispatches_lost_their_worktrees_converges_without_a_card() {
        let mut board = marked_board("missing");
        let mut host = host();
        host.missing = HashSet::from([
            "/tmp/tsk-bulk-clean".to_string(),
            "/tmp/tsk-bulk-dirty".to_string(),
        ]);
        press(&mut board, BoardIntent::Complete, &mut host);
        assert!(board.model.cleanup_prompt().is_none());
        assert!(statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Done));
        assert!(cleaned(&board, 0) && cleaned(&board, 1));
        let disk = board.store.load().expect("load");
        assert!(matches!(
            disk.last_undo(),
            Some(UndoEntry::Batch { entries }) if entries.len() == 3
        ));
    }

    #[test]
    fn a_missing_worktree_relaunched_while_the_card_is_open_is_not_marked_cleaned() {
        let mut board = marked_board("relaunched");
        let mut host = host();
        host.missing = HashSet::from(["/tmp/tsk-bulk-clean".to_string()]);
        press(&mut board, BoardIntent::Complete, &mut host);
        let bulk = board.model.cleanup_prompt().unwrap().bulk.clone().unwrap();
        assert_eq!(bulk.missing.len(), 1, "T1 opens as already gone");

        // Another board relaunches T1 before this card is answered: a fresh worktree and a
        // fresh, live dispatch record land on disk.
        let mut other = board.store.load().expect("load");
        let mut relaunched = dispatched("clean");
        relaunched.at = SystemTime::now() + std::time::Duration::from_secs(1);
        other
            .record_dispatch(board.ids[0], relaunched.clone())
            .expect("relaunch");
        board
            .store
            .reload_merge_save(&mut other)
            .expect("save relaunch");
        host.missing.clear();

        press(&mut board, BoardIntent::KeepCleanup, &mut host);
        let record = board
            .domain
            .get(board.ids[0])
            .unwrap()
            .dispatch
            .clone()
            .unwrap();
        assert_eq!(record, relaunched, "the relaunched dispatch stays live");
        assert!(!record.cleaned);
        let disk = board.store.load().expect("load");
        assert!(
            !disk
                .get(board.ids[0])
                .unwrap()
                .dispatch
                .as_ref()
                .unwrap()
                .cleaned
        );
    }

    #[test]
    fn a_bulk_row_relaunched_while_the_card_is_open_is_skipped_not_cleaned() {
        let mut board = marked_board("row-relaunched");
        let mut host = host();
        host.cached_merged = true;
        press(&mut board, BoardIntent::Complete, &mut host);
        host.land_all(true);
        tick(&mut board, &mut host);

        let mut other = board.store.load().expect("load");
        other
            .record_dispatch_cleaned(board.ids[0], crate::domain::CleanupOutcome::Removed)
            .expect("cleaned");
        board
            .store
            .reload_merge_save(&mut other)
            .expect("save cleaned");
        let mut other = board.store.load().expect("load");
        let mut relaunched = dispatched("clean");
        relaunched.herdr_workspace_id = "w-clean-2".into();
        relaunched.at = SystemTime::now() + std::time::Duration::from_secs(1);
        other
            .record_dispatch(board.ids[0], relaunched.clone())
            .expect("relaunch");
        board.store.reload_merge_save(&mut other).expect("save");

        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        assert!(
            host.removed.is_empty() && host.deleted.is_empty(),
            "{:?}",
            host.removed
        );
        let record = board
            .domain
            .get(board.ids[0])
            .unwrap()
            .dispatch
            .clone()
            .unwrap();
        assert_eq!(record, relaunched);
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        let message = board.model.message().unwrap_or_default().to_string();
        assert!(message.contains("T1 (dispatch changed)"), "{message}");
    }

    #[test]
    fn bulk_done_without_a_live_dispatch_keeps_the_plain_batch() {
        let mut board = marked_board("plain");
        for index in [0, 1] {
            board
                .domain
                .record_dispatch_cleaned(board.ids[index], crate::domain::CleanupOutcome::Removed)
                .expect("cleaned");
        }
        board
            .store
            .reload_merge_save(&mut board.domain)
            .expect("save");
        board.model.sync_from_domain(&board.domain);
        let mut host = host();
        assert_eq!(
            offer_bulk_cleanup_prompt_with_host(
                &mut board.domain,
                &mut board.model,
                true,
                &mut host
            )
            .expect("offer"),
            BulkCleanupOffer::None
        );
        press(&mut board, BoardIntent::Complete, &mut host);
        assert!(board.model.cleanup_prompt().is_none());
        assert!(statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Done));
    }

    fn slow_board(label: &str) -> (Board, BulkHost) {
        let mut board = marked_board(label);
        let mut host = BulkHost {
            hold: true,
            ..BulkHost::default()
        };
        press(&mut board, BoardIntent::Complete, &mut host);
        host.land_all(true);
        tick(&mut board, &mut host);
        (board, host)
    }

    fn disk_statuses(board: &Board) -> Vec<HumanStatus> {
        let disk = board.store.load().expect("load");
        board
            .ids
            .iter()
            .map(|id| disk.get(*id).unwrap().status)
            .collect()
    }

    /// The card's row state for `board.ids[index]`.
    fn row_state(board: &Board, index: usize) -> CleanupRowState {
        board
            .model
            .cleanup_run()
            .expect("run")
            .rows
            .iter()
            .find(|row| row.task_id == board.ids[index])
            .expect("row")
            .state
            .clone()
    }

    /// The job slot the worker handles `board.ids[index]` in.
    fn slot(board: &Board, index: usize) -> usize {
        board
            .model
            .cleanup_run()
            .expect("run")
            .rows
            .iter()
            .find(|row| row.task_id == board.ids[index])
            .and_then(|row| row.slot)
            .expect("worker row")
    }

    #[test]
    fn y_returns_before_any_removal_and_rows_land_through_the_loop() {
        let (mut board, mut host) = slow_board("slow");
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        // The board is interactive again with nothing removed: completion is durable first.
        assert!(host.removed.is_empty());
        assert!(disk_statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Done));
        assert!(board.model.cleanup_card_open(), "the card stays to report");
        assert_eq!(row_state(&board, 0), CleanupRowState::Queued);
        assert_eq!(row_state(&board, 1), CleanupRowState::Queued);
        let (first, second) = (slot(&board, 0), slot(&board, 1));
        // Idle frames never wait on the worker.
        tick(&mut board, &mut host);
        tick(&mut board, &mut host);
        host.start(first);
        tick(&mut board, &mut host);
        assert_eq!(row_state(&board, 0), CleanupRowState::Removing);
        host.release(first);
        tick(&mut board, &mut host);
        assert_eq!(
            row_state(&board, 0),
            CleanupRowState::Cleaned { branch_kept: None }
        );
        assert!(cleaned(&board, 0));
        let disk = board.store.load().expect("load");
        assert!(
            disk.get(board.ids[0])
                .unwrap()
                .dispatch
                .as_ref()
                .unwrap()
                .cleaned,
            "each landed row saves its cleaned marker"
        );
        assert!(!cleaned(&board, 1));
        // Esc hides the card; cleanup continues and the status row reports it.
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        assert!(!board.model.cleanup_card_open());
        assert!(board.model.cleanup_run().is_some());
        tick(&mut board, &mut host);
        assert_eq!(board.model.message(), Some("cleaning 2 of 2…"));
        host.release(second);
        tick(&mut board, &mut host);
        assert!(board.model.cleanup_run().is_none());
        assert!(cleaned(&board, 1));
        assert_eq!(board.model.message(), Some("done 3 · cleaned 2"));
        assert_eq!(host.removed.len(), 2);
        // Cleaned markers landed after completion, and one undo still reopens the set.
        press(&mut board, BoardIntent::Undo, &mut host);
        assert!(statuses(&board)
            .iter()
            .all(|status| *status == HumanStatus::Open));
        assert!(cleaned(&board, 0) && cleaned(&board, 1));
    }

    /// The cleanup card as painted, one string per screen row.
    fn rendered_card(board: &Board) -> String {
        let (width, height) = (120u16, 30u16);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
                .expect("terminal");
        terminal
            .draw(|frame| {
                crate::ui::board::draw_board(frame, &board.model);
            })
            .expect("draw card");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer.cell((x, y)).expect("cell").symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_row_the_worker_kept_says_kept_once_on_the_card() {
        // These refusals carry `kept:` in their CLI wording; the card writes its own lead.
        for (error, reason) in [
            (CleanupError::FilesInUse, "kept: files in use"),
            (CleanupError::PathTooLong, "kept: path too long"),
            (CleanupError::RemovalTimedOut, "kept: removal timed out"),
        ] {
            let (mut board, mut host) = slow_board("kept-once");
            press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
            let (first, second) = (slot(&board, 0), slot(&board, 1));
            let job = host.held.as_ref().expect("held job").0.clone();
            job.finish(first, Err(error));
            host.release(second);
            tick(&mut board, &mut host);
            assert!(board.model.cleanup_card_open(), "a kept row is not missed");
            let card = rendered_card(&board);
            assert!(card.contains(&format!("T1  {reason}")), "{card}");
            assert!(!card.contains("kept: kept:"), "{card}");
        }
    }

    #[test]
    fn a_failed_row_never_stops_the_others_and_the_card_shows_why() {
        let mut board = marked_board("fail");
        let mut host = BulkHost {
            fail: HashSet::from(["w-clean".to_string()]),
            ..BulkHost::default()
        };
        press(&mut board, BoardIntent::Complete, &mut host);
        host.land_all(true);
        tick(&mut board, &mut host);
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        assert_eq!(host.removed, vec!["w-dirty".to_string()]);
        assert!(!cleaned(&board, 0) && cleaned(&board, 1));
        let state = row_state(&board, 0);
        assert!(
            matches!(&state, CleanupRowState::Kept { short, full }
                if short == "removal failed" && full.contains("could not remove w-clean")),
            "{state:?}"
        );
        assert!(board.model.cleanup_card_open(), "a kept row is not missed");
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        assert_eq!(
            board.model.message(),
            Some("done 3 · cleaned 1 · kept T1 (removal failed)")
        );
        // Sticky: still there after the ephemeral window would have passed.
        assert!(board.model.message_is_sticky());
    }

    #[test]
    fn quit_mid_cleanup_waits_for_the_git_and_herdr_steps() {
        let (mut board, mut host) = slow_board("quit");
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        let mut recovery = SaveRecovery::new();
        let quit = handle_board_intent_with_host(
            &board.store,
            &mut board.domain,
            &mut board.model,
            BoardIntent::Quit,
            &mut recovery,
            false,
            true,
            &mut host,
            &mut |_| {},
        )
        .expect("quit");
        assert!(!quit, "the board stays up while cleanup finishes");
        assert!(board.model.quitting_after_cleanup());
        assert!(!board.model.cleanup_card_open());
        assert_eq!(board.model.message(), Some("finishing cleanup…"));
        assert!(!board.model.quit_after_cleanup_due());
        let (first, second) = (slot(&board, 0), slot(&board, 1));
        host.release(first);
        tick(&mut board, &mut host);
        assert!(!board.model.quit_after_cleanup_due());
        assert_eq!(board.model.message(), Some("finishing cleanup…"));
        host.release(second);
        tick(&mut board, &mut host);
        assert!(board.model.quit_after_cleanup_due());
        let disk = board.store.load().expect("load");
        assert!(board.ids[..2].iter().all(|id| disk
            .get(*id)
            .unwrap()
            .dispatch
            .as_ref()
            .unwrap()
            .cleaned));
    }

    #[test]
    fn ctrl_d_on_another_dispatch_while_cleanup_runs_is_refused() {
        let (mut board, mut host) = slow_board("busy");
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        let id = board
            .domain
            .create(
                "another",
                None,
                TaskScope::Project {
                    path: PROJECT.into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
        board
            .domain
            .record_dispatch(id, dispatched("another"))
            .expect("dispatch");
        board.model.sync_from_domain(&board.domain);
        assert_eq!(
            super::offer_cleanup_prompt_with_host(
                &mut board.domain,
                &mut board.model,
                id,
                true,
                &mut host
            )
            .expect("offer"),
            super::CleanupOffer::Busy
        );
        assert_ne!(board.domain.get(id).unwrap().status, HumanStatus::Done);
    }

    #[test]
    fn a_row_relaunched_after_y_is_skipped_right_before_its_host_work() {
        let (mut board, mut host) = slow_board("rebound");
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        let (clean, dirty) = (slot(&board, 0), slot(&board, 1));
        // Another process relaunches T1 after `y`, before the worker reaches it.
        let mut other = board.store.load().expect("load");
        let mut relaunched = dispatched("clean");
        relaunched.herdr_workspace_id = "w-clean-2".into();
        relaunched.at = SystemTime::now() + std::time::Duration::from_secs(1);
        other
            .record_dispatch(board.ids[0], relaunched.clone())
            .expect("relaunch");
        board.store.reload_merge_save(&mut other).expect("save");
        // This board relaunches T2 itself; its own domain has the new record.
        let mut again = dispatched("dirty");
        again.at = SystemTime::now() + std::time::Duration::from_secs(1);
        board
            .domain
            .record_dispatch(board.ids[1], again.clone())
            .expect("relaunch here");
        tick(&mut board, &mut host);
        host.release(clean);
        host.release(dirty);
        tick(&mut board, &mut host);
        assert!(
            host.removed.is_empty(),
            "neither relaunch loses its worktree: {:?}",
            host.removed
        );
        assert!(board
            .model
            .cleanup_run()
            .is_some_and(|run| run.rows.iter().all(
                |row| matches!(&row.state, CleanupRowState::Kept { short, .. }
                if short == "dispatch changed")
            )));
        assert!(!cleaned(&board, 1));
    }

    #[test]
    fn hidden_progress_owns_its_status_slot_and_ends_with_the_run() {
        let (mut board, mut host) = slow_board("status");
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        tick(&mut board, &mut host);
        assert_eq!(board.model.message(), Some("cleaning 1 of 2…"));
        // Another action's feedback covers it, then gives the slot back.
        board.model.set_message("marked");
        assert_eq!(board.model.message(), Some("marked"));
        board.model.clear_message();
        assert_eq!(board.model.message(), Some("cleaning 1 of 2…"));
        tick(&mut board, &mut host);
        assert_eq!(board.model.message(), Some("cleaning 1 of 2…"));
        let (first, second) = (slot(&board, 0), slot(&board, 1));
        host.release(first);
        host.release(second);
        tick(&mut board, &mut host);
        assert_eq!(board.model.message(), Some("done 3 · cleaned 2"));
        assert!(!board.model.message_is_sticky());
        // When the clean summary goes, the finished run's progress does not come back.
        board.model.clear_message();
        assert_eq!(board.model.message(), None);
    }

    #[test]
    fn ctrl_d_in_the_project_preview_respects_a_cleanup_running_on_the_outer_board() {
        let (mut board, mut host) = slow_board("outer-busy");
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        let id = board
            .domain
            .create(
                "another",
                None,
                TaskScope::Project {
                    path: PROJECT.into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
        board
            .domain
            .record_dispatch(id, dispatched("another"))
            .expect("dispatch");
        board.model.sync_from_domain(&board.domain);
        apply_intent(
            &mut board.domain,
            &mut board.model,
            BoardIntent::SelectNavTab(crate::ui::queue::NavTab::Projects),
            None,
        )
        .expect("projects overview");
        for _ in 0..2 {
            apply_intent(
                &mut board.domain,
                &mut board.model,
                BoardIntent::StageRight,
                None,
            )
            .expect("stage right");
        }
        assert!(board.model.project_right_seat_focused());
        assert_eq!(
            super::offer_cleanup_prompt_with_host(
                &mut board.domain,
                board.model.input_target_mut(),
                id,
                true,
                &mut host
            )
            .expect("offer"),
            super::CleanupOffer::Busy
        );
    }

    fn plain_task(board: &mut Board, title: &str) -> uuid::Uuid {
        let id = board
            .domain
            .create(
                title,
                None,
                TaskScope::Project {
                    path: PROJECT.into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
        board.model.sync_from_domain(&board.domain);
        id
    }

    /// Move the cursor to `id` (a second select on the cursor row would open its page).
    fn select(board: &mut Board, id: uuid::Uuid) {
        if board.model.selected_id() == Some(id) {
            return;
        }
        let index = board
            .model
            .visible_ids()
            .iter()
            .position(|visible| *visible == id)
            .expect("visible");
        apply_intent(
            &mut board.domain,
            &mut board.model,
            BoardIntent::SelectIndex(index),
            None,
        )
        .expect("select");
    }

    #[test]
    fn ctrl_d_is_refused_for_any_target_while_cleanup_runs() {
        let (mut board, mut host) = slow_board("busy-any");
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        // A task with no dispatch at all.
        let plain = plain_task(&mut board, "plain one");
        select(&mut board, plain);
        press(&mut board, BoardIntent::Complete, &mut host);
        assert_eq!(board.model.message(), Some(super::CLEANUP_BUSY));
        assert_ne!(board.domain.get(plain).unwrap().status, HumanStatus::Done);
        // A marked set of ordinary tasks.
        let other = plain_task(&mut board, "plain two");
        if !board.model.mark_mode_active() {
            apply_intent(
                &mut board.domain,
                &mut board.model,
                BoardIntent::ToggleMarkMode,
                None,
            )
            .expect("mark mode");
        }
        for id in [plain, other] {
            select(&mut board, id);
            apply_intent(
                &mut board.domain,
                &mut board.model,
                BoardIntent::MarkToggle,
                None,
            )
            .expect("mark");
        }
        assert!(board.model.bulk_verb_active());
        press(&mut board, BoardIntent::Complete, &mut host);
        assert_eq!(board.model.message(), Some(super::CLEANUP_BUSY));
        let disk = board.store.load().expect("load");
        assert!([plain, other].iter().all(|id| disk
            .get(*id)
            .is_none_or(|task| task.status != HumanStatus::Done)));
        assert!(
            board.model.bulk_verb_active(),
            "the marks wait with the refusal"
        );
    }

    #[test]
    fn a_finished_cleanup_clears_its_busy_refusal_with_the_summary() {
        let (mut board, mut host) = slow_board("busy-cleared");
        press(&mut board, BoardIntent::ConfirmCleanup, &mut host);
        press(&mut board, BoardIntent::CancelCleanup, &mut host);
        press(&mut board, BoardIntent::Complete, &mut host);
        assert_eq!(board.model.message(), Some(super::CLEANUP_BUSY));
        let (first, second) = (slot(&board, 0), slot(&board, 1));
        host.release(first);
        host.release(second);
        tick(&mut board, &mut host);
        assert_eq!(board.model.message(), Some("done 3 · cleaned 2"));
        board.model.clear_message();
        assert_eq!(
            board.model.message(),
            None,
            "the obsolete refusal does not come back"
        );
    }
}
