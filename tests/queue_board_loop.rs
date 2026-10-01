//! App loop wiring: the Frame Scheduler on the board's input wait, no the attention poll on
//! the board frame path, and a load+draw smoke path.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tsk_tui::app::{
    board_frame, board_poll_duration, load_board_model, take_pending_after_paint, FramePoll,
};
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::scheduler::{next_wait, DEFAULT_BASE_TICK};
use tsk_tui::ui::{apply_intent, draw_board, BoardIntent, BoardModel, IntentOutcome};
use tsk_tui::update::suppress_background_fetch;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("lock environment")
}

/// A directory this test owns alone, removed on drop even if the test panics.
struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Restores an environment variable to whatever it held before the test touched it, so one
/// test's `TSK_STATE_DIR` never leaks into the next.
struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let previous = env::var_os(key);
        env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => env::set_var(self.key, value),
            None => env::remove_var(self.key),
        }
    }
}

fn temp_state_dir(label: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = env::temp_dir().join(format!("tsk-queue-board-loop-{label}-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("create temp state dir");
    dir
}

/// The board loop's poll duration is not a second copy of the scheduler's arithmetic: it
/// calls straight through to it, idle and short-tick alike.
#[test]
fn board_frame_poll_duration_uses_scheduler_idle_and_short_ticks() {
    assert_eq!(
        board_poll_duration(false),
        next_wait(false, DEFAULT_BASE_TICK),
        "idle wait must equal the scheduler's own idle answer"
    );
    assert_eq!(
        board_poll_duration(false),
        Duration::from_millis(250),
        "M1 has no animation source, so the loop's idle wait is the 250ms base tick"
    );

    assert_eq!(
        board_poll_duration(true),
        next_wait(true, DEFAULT_BASE_TICK),
        "the animating wait must equal the scheduler's own short-tick answer"
    );
    assert!(
        board_poll_duration(true) < board_poll_duration(false),
        "an active animation must shorten the wait below the idle floor"
    );
    assert!(
        board_poll_duration(true) >= Duration::from_millis(25),
        "the short tick must never drop below the scheduler's 25ms floor"
    );
}

/// Drives [`board_frame`] the way the real board loop does -- settle, paint, wait, repeat --
/// for many idle iterations, and observes the duration `board_frame` *itself* hands the wait
/// closure, so this is a test of the product function's wiring, not a second copy of the
/// scheduler's arithmetic re-asserted through a test-owned duration.
///
/// The wait closure here only records what it is given and never computes a duration of its
/// own -- that is the point: if `board_frame` stopped calling [`board_poll_duration`] (e.g. a
/// regression back to a fixed constant, or to a value under the scheduler's 25ms/250ms
/// floors), this test would observe the wrong duration and fail, because the duration is read
/// off the call, not recomputed by the test.
#[test]
fn instrumented_loop_idle_wait_never_sustained_below_25ms_without_animation() {
    let domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    let mut recorded_waits = Vec::new();
    for _ in 0..50 {
        let poll = board_frame(
            &mut model,
            |model| {
                // `TestBackend`'s draw error is `Infallible`; `expect` collapses it to the
                // `io::Result<()>` `board_frame` requires without inventing a fake error path.
                terminal
                    .draw(|frame| {
                        let _ = draw_board(frame, model);
                    })
                    .expect("test backend draw");
                Ok(())
            },
            |duration| {
                recorded_waits.push(duration);
                Ok(false)
            },
            false,
        )
        .expect("board frame");
        assert_eq!(
            poll,
            FramePoll::Idle,
            "the wait closure always answers no-event, so every iteration is Idle"
        );
    }

    assert_eq!(
        recorded_waits.len(),
        50,
        "every iteration must record its wait"
    );
    assert!(
        recorded_waits
            .iter()
            .all(|wait| *wait >= Duration::from_millis(25)),
        "an idle loop must never sustain a sub-25ms wait: {recorded_waits:?}"
    );
    assert!(
        recorded_waits
            .iter()
            .all(|wait| *wait >= Duration::from_millis(250)),
        "with no animation ever active the wait must stay at the 250ms idle floor: {recorded_waits:?}"
    );
}

#[test]
fn autoscroll_shortens_the_board_frame_wait() {
    let domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let mut recorded = None;
    let poll = board_frame(
        &mut model,
        |model| {
            terminal
                .draw(|frame| {
                    let _ = draw_board(frame, model);
                })
                .expect("draw");
            Ok(())
        },
        |duration| {
            recorded = Some(duration);
            Ok(false)
        },
        true,
    )
    .expect("board frame");
    assert_eq!(poll, FramePoll::Idle);
    let wait = recorded.expect("wait recorded");
    assert_eq!(wait, next_wait(true, DEFAULT_BASE_TICK));
    assert!(
        wait < Duration::from_millis(250),
        "armed autoscroll must shorten the wait, got {wait:?}"
    );
    assert!(
        wait >= Duration::from_millis(25),
        "short tick must stay at the 25ms floor, got {wait:?}"
    );
}

/// [`load_board_model`] (the whole the open path: store load + context snapshot + BoardModel
/// construction, no host refresh) feeds straight into [`draw_board`] without panicking at the
/// standard tier's floor size ( smoke;/'s "board loop stops calling attention"
/// leaves this as the one open-path exercise: load, then draw).
#[test]
fn load_board_and_draw_path_smoke_at_80x24() {
    let _env = env_lock();
    suppress_background_fetch();
    let dir = temp_state_dir("smoke");
    let _dir_guard = TempDirGuard(dir.clone());
    let _env_guard = EnvVarGuard::set("TSK_STATE_DIR", &dir);

    let model = load_board_model().expect("load board model from an empty temp state dir");

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw the loaded board without panicking");
}

#[test]
fn malformed_agent_profiles_degrade_to_a_painted_board_status() {
    let _env = env_lock();
    suppress_background_fetch();
    let dir = temp_state_dir("malformed-agents");
    let _dir_guard = TempDirGuard(dir.clone());
    let _env_guard = EnvVarGuard::set("TSK_STATE_DIR", &dir);
    fs::write(
        dir.join("agents.toml"),
        "[agent.Reviewer]\ncommand = [\"true\"]\n",
    )
    .expect("write malformed profiles");

    let model = load_board_model().expect("malformed optional profiles must not block open");
    let message = model.message().expect("profile load error on status row");
    assert!(message.contains("agents.toml"), "{message}");

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw degraded board");
    let painted = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(
        painted.contains("agents.toml"),
        "the load error must be visible in the painted frame: {painted}"
    );
}

#[test]
fn pending_resize_event_paints_before_it_is_returned() {
    let key = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    let mut pending = Some(key.clone());
    let mut painted = 0usize;
    let out = take_pending_after_paint(&mut pending, || {
        painted += 1;
        Ok::<_, ()>(())
    })
    .expect("paint");
    assert_eq!(
        painted, 1,
        "settled size must paint before the deferred event"
    );
    assert_eq!(out, Some(key));
    assert!(pending.is_none());
}

#[test]
fn no_pending_event_skips_the_settled_paint() {
    let mut pending: Option<Event> = None;
    let mut painted = 0usize;
    let out = take_pending_after_paint(&mut pending, || {
        painted += 1;
        Ok::<_, ()>(())
    })
    .expect("paint");
    assert_eq!(painted, 0);
    assert!(out.is_none());
}

#[test]
fn run_board_resize_arm_drains_through_coalesce_then_paints() {
    let src = include_str!("../src/app.rs");
    assert!(
        src.contains("pending_event = scheduler::coalesce_resizes(event::poll, event::read)?"),
        "run_board Resize arm must call coalesce_resizes"
    );
    assert!(
        src.contains("take_pending_after_paint(&mut pending_event"),
        "run_board must paint the settled size before a deferred event"
    );
}

#[test]
fn repeated_threshold_resizes_keep_board_loop_live() {
    let mut domain = DomainState::new();
    domain
        .create(
            "resize survivor",
            Some("notes".to_string()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, None);
    // A two-column stage exercises the split compositor across the crossings.
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
            .expect("slide to the split stage"),
        IntentOutcome::None
    );

    for width in [109, 110].into_iter().cycle().take(40) {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).expect("test terminal");
        let poll = board_frame(
            &mut model,
            |model| {
                terminal
                    .draw(|frame| {
                        let _ = draw_board(frame, model);
                    })
                    .expect("settled responsive frame");
                Ok(())
            },
            |_| Ok(false),
            false,
        )
        .expect("responsive board frame");
        assert_eq!(poll, FramePoll::Idle);
    }
}

#[test]
fn threshold_crossings_without_task_verbs_leave_domain_unchanged() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "immutable through resize",
            Some("domain must not move".to_string()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    let before = domain.get(id).expect("task").clone();
    let mut model = BoardModel::from_domain(&domain, None);

    // Slide to the rail stage: two-column geometry with the task owning focus, reached
    // without invoking any task verb.
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
            .expect("slide to the split stage"),
        IntentOutcome::None
    );
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None)
            .expect("slide to the rail stage"),
        IntentOutcome::None
    );
    for width in [109, 110].into_iter().cycle().take(20) {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).expect("test terminal");
        let poll = board_frame(
            &mut model,
            |model| {
                terminal
                    .draw(|frame| {
                        let _ = draw_board(frame, model);
                    })
                    .expect("draw threshold frame");
                Ok(())
            },
            |_| Ok(false),
            false,
        )
        .expect("threshold frame");
        assert_eq!(poll, FramePoll::Idle);
    }

    assert_eq!(domain.get(id), Some(&before));
}

#[test]
fn idle_merge_hides_a_task_archived_by_another_process_without_moving_selection() {
    use tsk_tui::app::{revalidate_board_from_store, StoreWatch};
    use tsk_tui::save_recovery::SaveRecovery;
    use tsk_tui::store::TaskStore;

    let dir = temp_state_dir("idle-archive");
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut seed = DomainState::new();
    seed.create(
        "Merge archived X",
        None,
        TaskScope::Global,
        ProvenanceOrigin::Manual,
        None,
    )
    .expect("create X");
    let y = seed
        .create(
            "Stay selected Y",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create Y");
    // Y is the only started task, so IN MOTION (above ON DECK) carries the seeded
    // selection regardless of the deck's FIFO order.
    seed.set_status(y, HumanStatus::Started).expect("start Y");
    store.save(&seed).expect("seed two tasks");

    // Process A: the seeded selection rests on started Y.
    let mut domain = store.load().expect("load A");
    let mut model = BoardModel::from_domain(&domain, None);
    assert_eq!(model.selected_id(), Some(y), "selection starts on Y");
    let mut watch = StoreWatch::seeded(&store);

    // Process B archives X behind A's back.
    store
        .locked_transition(|state| {
            state
                .archive_task(x_id(&seed))
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("B archives X");

    let changed = revalidate_board_from_store(
        &store,
        &mut domain,
        &mut model,
        &mut watch,
        &SaveRecovery::new(),
    );
    assert!(changed, "the sibling write must be picked up");

    let visible = model.visible_ids();
    assert!(
        !visible.contains(&x_id(&seed)),
        "archived X must leave the visible rows: {visible:?}"
    );
    assert!(visible.contains(&y), "Y stays visible");
    assert_eq!(
        model.selected_id(),
        Some(y),
        "the selection must not move for a task it never held"
    );
}

fn x_id(seed: &DomainState) -> uuid::Uuid {
    seed.tasks()
        .iter()
        .find(|task| task.title == "Merge archived X")
        .expect("seeded task X")
        .id
}

#[test]
fn idle_merge_leaves_a_project_focus_archived_by_another_process() {
    use tsk_tui::app::{revalidate_board_from_store, StoreWatch};
    use tsk_tui::save_recovery::SaveRecovery;
    use tsk_tui::store::TaskStore;

    let focus_path = "/repos/focus";
    let dir = temp_state_dir("idle-focus-archived");
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut seed = DomainState::new();
    seed.create(
        "focus task",
        None,
        TaskScope::Project {
            path: focus_path.to_string(),
        },
        ProvenanceOrigin::Manual,
        None,
    )
    .expect("create focus task");
    seed.create(
        "desk task",
        None,
        TaskScope::Global,
        ProvenanceOrigin::Manual,
        None,
    )
    .expect("create desk task");
    store.save(&seed).expect("seed");

    // Process A focuses the project.
    let mut domain = store.load().expect("load A");
    let mut model = BoardModel::from_domain(&domain, None);
    model.set_selected_project(Some(std::path::PathBuf::from(focus_path)));
    assert_eq!(
        model.selected_project(),
        Some(std::path::Path::new(focus_path))
    );
    let mut watch = StoreWatch::seeded(&store);

    // Process B archives the focused project on disk.
    store
        .locked_transition(|state| {
            state
                .archive_project(focus_path)
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("B archives the focus project");

    let changed = revalidate_board_from_store(
        &store,
        &mut domain,
        &mut model,
        &mut watch,
        &SaveRecovery::new(),
    );
    assert!(changed, "the sibling write must be picked up");

    // The focus reset: home desk, a dim status line naming the project, and the
    // archived task stays hidden.
    assert_eq!(
        model.selected_project(),
        None,
        "project focus must reset to home desk"
    );
    let message = model.message().expect("a status line names the project");
    assert!(
        message.contains("focus"),
        "message names the project: {message:?}"
    );
    assert!(
        !model.visible_ids().iter().any(|id| {
            domain
                .get(*id)
                .is_some_and(|task| task.title == "focus task")
        }),
        "the archived project's task stays hidden"
    );

    // The quick-add default never resolves to the archived project after the merge.
    let snapshot = tsk_tui::context::InvocationSnapshot {
        default_scope: TaskScope::Project {
            path: focus_path.to_string(),
        },
        this_repo: Some(std::path::PathBuf::from(focus_path)),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    };
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCapture,
        Some(&snapshot),
    )
    .expect("open capture");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::QuickAddInsertText("probe".into()),
        None,
    )
    .expect("type");
    apply_intent(&mut domain, &mut model, BoardIntent::QuickAddSave, None).expect("save");
    model.sync_from_domain(&domain);
    let probe = domain
        .tasks()
        .iter()
        .find(|task| task.title == "probe")
        .expect("probe saved");
    assert_eq!(
        probe.scope,
        TaskScope::Global,
        "quick-add must not resolve to the archived project after the merge"
    );
}

#[test]
fn idle_merge_converts_a_read_only_focus_whose_project_was_unarchived() {
    use tsk_tui::app::{revalidate_board_from_store, StoreWatch};
    use tsk_tui::save_recovery::SaveRecovery;
    use tsk_tui::store::TaskStore;

    let focus_path = "/repos/refiled";
    let dir = temp_state_dir("idle-readonly-unarchived");
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut seed = DomainState::new();
    let inside = seed
        .create(
            "filed task",
            None,
            TaskScope::Project {
                path: focus_path.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    seed.archive_project(focus_path).expect("archive");
    store.save(&seed).expect("seed");

    // Process A opens the read-only focus from the picker's archived tab.
    let mut domain = store.load().expect("load A");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("picker");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ProjectPickerSwitchTab,
        None,
    )
    .expect("archived tab");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmProjectChoice,
        None,
    )
    .expect("read-only focus");
    assert!(model.focus_is_archived());
    let mut watch = StoreWatch::seeded(&store);

    // Process B unarchives it on disk.
    store
        .locked_transition(|state| {
            state
                .unarchive_project(focus_path)
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("B unarchives");

    let changed = revalidate_board_from_store(
        &store,
        &mut domain,
        &mut model,
        &mut watch,
        &SaveRecovery::new(),
    );
    assert!(changed, "the sibling write must be picked up");

    assert!(
        !model.focus_is_archived(),
        "a read-only focus whose project came back is an ordinary project focus"
    );
    assert_eq!(
        model.selected_project(),
        Some(std::path::Path::new(focus_path)),
        "on the same project"
    );
    assert!(
        model.visible_ids().contains(&inside),
        "its tasks stay on the board"
    );
}
