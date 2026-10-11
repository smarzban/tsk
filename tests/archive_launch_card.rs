//! Launch card inside an archived project's directory (AC-22..AC-26).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tsk_tui::app::{apply_board_intent_with_save_recovery, BoardSaveContext};
use tsk_tui::context::InvocationSnapshot;
use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::save_recovery::SaveRecovery;
use tsk_tui::store::TaskStore;
use tsk_tui::ui::board::{apply_intent, draw_board, BoardInputMode, BoardModel};
use tsk_tui::ui::input::{map_key, BoardIntent};
use tsk_tui::ui::mouse::{left_click, map_board_mouse};
use tsk_tui::ui::render::QueueHitTarget;

const PROJ: &str = "/tmp/x/proj";
const STANDARD: (u16, u16) = (80, 24);

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("tsk-launch-card-{label}-{nanos}-{seq}"))
}

fn snapshot_for(path: &str) -> InvocationSnapshot {
    InvocationSnapshot {
        default_scope: TaskScope::Project {
            path: path.to_string(),
        },
        this_repo: Some(PathBuf::from(path)),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    }
}

/// Seed a store whose project is (optionally) archived and return the loaded board with
/// `offer_launch_card` already applied against the archived-project snapshot.
fn setup(archive: bool) -> (TaskStore, DomainState, BoardModel, PathBuf) {
    let dir = temp_dir("state");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create(
            "seed task",
            None,
            TaskScope::Project {
                path: PROJ.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create seed task");
    if archive {
        state.archive_project(PROJ).expect("archive the project");
    }
    store.save(&state).expect("seed the store");
    let state = store.load().expect("reload");
    let snapshot = snapshot_for(PROJ);
    let mut model = BoardModel::from_domain(&state, snapshot.this_repo.clone());
    model.offer_launch_card(&state, &snapshot);
    (store, state, model, dir)
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn board_rows(model: &BoardModel, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer();
    tsk_tui::ui::render::assert_buffer_mono(buffer);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

#[test]
fn launch_in_an_archived_project_paints_the_two_choice_card_before_any_key() {
    let (_store, _state, model, dir) = setup(true);
    assert_eq!(model.input_mode(), BoardInputMode::LaunchCard);

    let rows = board_rows(&model, STANDARD.0, STANDARD.1);
    let frame = rows.join("\n");
    assert!(
        frame.contains("project proj is archived"),
        "the card names the project:\n{frame}"
    );
    assert!(
        frame.contains("unarchive"),
        "the first choice paints:\n{frame}"
    );
    assert!(
        frame.contains("keep archived"),
        "the second choice paints:\n{frame}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn archived_non_git_directory_opens_desk_with_the_existing_archive_card() {
    let dir = temp_dir("non-git-archived");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create(
            "archived directory task",
            None,
            TaskScope::Project {
                path: PROJ.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    state.archive_project(PROJ).expect("archive project");
    store.save(&state).expect("save archived project");
    let state = store.load().expect("reload");
    let snapshot = InvocationSnapshot {
        default_scope: TaskScope::Global,
        this_repo: Some(PathBuf::from(PROJ)),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    };
    let mut model = BoardModel::from_domain_for_snapshot(&state, &snapshot);

    assert_eq!(model.nav_tab(), tsk_tui::ui::queue::NavTab::Desk);
    assert_eq!(
        model.selected_project(),
        None,
        "archived projects remain outside live navigation"
    );
    assert!(model.offer_launch_card(&state, &snapshot));
    assert_eq!(model.input_mode(), BoardInputMode::LaunchCard);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn no_card_when_the_default_is_desk_or_an_unarchived_project() {
    // Unarchived project default: no card.
    let (_store, _state, model, dir) = setup(false);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    let _ = std::fs::remove_dir_all(dir);

    // Desk default: no card.
    let dir = temp_dir("desk");
    let store = TaskStore::new(&dir);
    let state = DomainState::new();
    store.save(&state).expect("seed empty store");
    let state = store.load().expect("reload");
    let snapshot = InvocationSnapshot {
        default_scope: TaskScope::Global,
        this_repo: None,
        title_prefill: None,
        provenance: ProvenanceOrigin::Manual,
    };
    let mut model = BoardModel::from_domain(&state, None);
    assert!(
        !model.offer_launch_card(&state, &snapshot),
        "a desk default never raises the card"
    );
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn y_or_a_click_unarchives_durably_and_quick_add_defaults_to_the_project() {
    let (store, mut state, mut model, dir) = setup(true);
    let snapshot = snapshot_for(PROJ);
    let mut recovery = SaveRecovery::new();

    // Keyboard `y`, driven through the save-recovery boundary with the store's own
    // merge-save as the persistence closure.
    let intent = map_key(BoardInputMode::LaunchCard, press(KeyCode::Char('y')))
        .expect("y maps to the launch unarchive intent");
    assert_eq!(intent, BoardIntent::LaunchUnarchive);
    let baseline = state.clone();
    let outcome = apply_board_intent_with_save_recovery(
        &mut state,
        &mut model,
        &mut recovery,
        BoardSaveContext {
            baseline,
            intent,
            snapshot: Some(&snapshot),
        },
        |domain| {
            store
                .reload_merge_save(domain)
                .map_err(|error| error.to_string())
        },
    )
    .expect("unarchive applies");
    assert_eq!(outcome, tsk_tui::ui::board::IntentOutcome::Persisted);
    assert!(
        !store.load().expect("reload").is_project_archived(PROJ),
        "the unarchive is durable"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::Normal,
        "the card closed"
    );

    // `+` (OpenCapture) picks up the project as the quick-add scope: type and save a
    // draft, then check where it landed.
    apply_intent(
        &mut state,
        &mut model,
        BoardIntent::OpenCapture,
        Some(&snapshot),
    )
    .expect("open capture");
    apply_intent(
        &mut state,
        &mut model,
        BoardIntent::QuickAddInsertText("draft".into()),
        None,
    )
    .expect("type title");
    apply_intent(&mut state, &mut model, BoardIntent::QuickAddSave, None).expect("save the draft");
    model.sync_from_domain(&state);
    let draft = state
        .tasks()
        .iter()
        .find(|task| task.title == "draft")
        .expect("the draft was created");
    assert_eq!(
        draft.scope,
        TaskScope::Project {
            path: PROJ.to_string()
        },
        "quick-add defaults to the project after unarchive"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn n_esc_or_click_keep_archived_defaults_quick_add_to_desk_with_a_status_line() {
    let (store, mut state, mut model, dir) = setup(true);
    let snapshot = snapshot_for(PROJ);
    let mut recovery = SaveRecovery::new();

    let intent = map_key(BoardInputMode::LaunchCard, press(KeyCode::Char('n')))
        .expect("n maps to keep archived");
    assert_eq!(intent, BoardIntent::LaunchKeepArchived);
    let baseline = state.clone();
    apply_board_intent_with_save_recovery(
        &mut state,
        &mut model,
        &mut recovery,
        BoardSaveContext {
            baseline,
            intent,
            snapshot: Some(&snapshot),
        },
        |domain| {
            store
                .reload_merge_save(domain)
                .map_err(|error| error.to_string())
        },
    )
    .expect("keep archived applies");
    assert!(
        store.load().expect("reload").is_project_archived(PROJ),
        "the record stays"
    );
    assert!(
        model
            .message()
            .is_some_and(|message| message.contains("proj")),
        "the status line names the project: {:?}",
        model.message()
    );

    // `+` (OpenCapture) defaults to the desk for this session.
    apply_intent(
        &mut state,
        &mut model,
        BoardIntent::OpenCapture,
        Some(&snapshot),
    )
    .expect("open capture");
    apply_intent(
        &mut state,
        &mut model,
        BoardIntent::QuickAddInsertText("desk draft".into()),
        None,
    )
    .expect("type title");
    apply_intent(&mut state, &mut model, BoardIntent::QuickAddSave, None).expect("save the draft");
    model.sync_from_domain(&state);
    let draft = state
        .tasks()
        .iter()
        .find(|task| task.title == "desk draft")
        .expect("the draft was created");
    assert_eq!(
        draft.scope,
        TaskScope::Global,
        "quick-add defaults to the desk after keep archived"
    );

    // Esc on a fresh card also keeps archived.
    let (_store2, mut state2, mut model2, dir2) = setup(true);
    let intent = map_key(BoardInputMode::LaunchCard, press(KeyCode::Esc))
        .expect("esc maps to keep archived");
    let _ = apply_intent(&mut state2, &mut model2, intent, None).expect("esc keeps");
    assert!(
        model2.input_mode() == BoardInputMode::Normal,
        "esc closes the card"
    );

    // A double click on the `keep archived` line does the same on a fresh card; a single
    // click only selects it.
    let (_store3, mut state3, mut model3, dir3) = setup(true);
    let hits = tsk_tui::ui::board::board_hit_map(
        ratatui::layout::Rect::new(0, 0, STANDARD.0, STANDARD.1),
        &model3,
    );
    let hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::CleanupOption(1)))
        .expect("the keep-archived option paints a hit region");
    let intent = map_board_mouse(&model3, &hits, left_click(hit.area.x, hit.area.y))
        .expect("the option click maps");
    assert_eq!(intent, BoardIntent::CleanupClickOption(1));
    let now = Instant::now();
    assert_eq!(model3.resolve_card_choice(intent.clone(), now), None);
    assert_eq!(model3.input_mode(), BoardInputMode::LaunchCard);
    let intent = model3
        .resolve_card_choice(intent, now + Duration::from_millis(100))
        .expect("a double click runs it");
    assert_eq!(intent, BoardIntent::LaunchKeepArchived);
    let _ = apply_intent(&mut state3, &mut model3, intent, None).expect("click keeps");
    assert_eq!(model3.input_mode(), BoardInputMode::Normal);

    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
    let _ = std::fs::remove_dir_all(dir3);
}

#[test]
fn card_is_offered_once_per_session_whichever_choice() {
    let (_store, state, mut model, dir) = setup(true);
    let snapshot = snapshot_for(PROJ);
    // The setup already raised the card once; a second offer is refused.
    assert!(
        !model.offer_launch_card(&state, &snapshot),
        "the card is offered at most once per session"
    );

    // Whichever choice was made, a later offer attempt still refuses.
    let _ = apply_intent(
        &mut state.clone(),
        &mut model,
        BoardIntent::LaunchKeepArchived,
        None,
    )
    .expect("keep archived closes the card");
    assert!(!model.offer_launch_card(&state, &snapshot));
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn keep_archived_persists_nothing() {
    let (store, mut state, mut model, dir) = setup(true);
    let snapshot = snapshot_for(PROJ);
    let before = std::fs::read(dir.join("tsk.json")).expect("read store bytes");
    let mut recovery = SaveRecovery::new();

    let intent = map_key(BoardInputMode::LaunchCard, press(KeyCode::Char('n')))
        .expect("n maps to keep archived");
    let baseline = state.clone();
    let outcome = apply_board_intent_with_save_recovery(
        &mut state,
        &mut model,
        &mut recovery,
        BoardSaveContext {
            baseline,
            intent,
            snapshot: Some(&snapshot),
        },
        |domain| {
            store
                .reload_merge_save(domain)
                .map_err(|error| error.to_string())
        },
    )
    .expect("keep archived applies");

    assert_eq!(
        outcome,
        tsk_tui::ui::board::IntentOutcome::None,
        "keep archived must be a non-persisting outcome"
    );
    assert_eq!(
        std::fs::read(dir.join("tsk.json")).expect("read store bytes after"),
        before,
        "the store file's bytes are unchanged"
    );
    assert!(store.load().expect("reload").is_project_archived(PROJ));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn launch_card_asks_on_one_line_then_lists_its_options_with_the_cursor_on_keep() {
    let (_store, mut state, mut model, dir) = setup(true);
    assert_eq!(model.input_mode(), BoardInputMode::LaunchCard);
    for width in [40u16, 78, 110] {
        let rows = board_rows(&model, width, STANDARD.1);
        let frame = rows.join("\n");
        let flat = frame
            .replace('│', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        // The question, wrapped as it needs, and nothing in the top border.
        assert!(
            flat.contains("project proj is archived, would you like to unarchive it?"),
            "the card body asks the question:\n{frame}"
        );
        let top = rows
            .iter()
            .find(|row| row.contains("┌") && row.contains("[x]"))
            .expect("card top border");
        assert!(
            !top.trim().contains("┌─ "),
            "no title in the border:\n{top}"
        );
        // One line per option, starting with its key; the cursor on the safe `n`.
        assert!(
            flat.contains("y unarchive proj"),
            "the `y` option:\n{frame}"
        );
        assert!(
            flat.contains("▸ n keep proj archived"),
            "the cursor starts on `n`:\n{frame}"
        );
        assert!(flat.contains("esc keep archived"), "the way out:\n{frame}");
        assert!(
            !frame.contains("y unarchive · n keep archived"),
            "no footer key list:\n{frame}"
        );
    }

    // `↑` moves the cursor to `y` and stops there; `Enter` past the guard runs it.
    for _ in 0..2 {
        apply_intent(&mut state, &mut model, BoardIntent::CleanupSelectUp, None).expect("up");
    }
    let flat = board_rows(&model, 110, STANDARD.1).join(" ");
    assert!(flat.contains("▸ y"), "the cursor moved to `y`:\n{flat}");
    // Times count from the card's own open, so a slow runner cannot expire the guard.
    let opened = model.launch_card().expect("card").selector.opened_at;
    assert_eq!(
        model.resolve_card_choice(
            BoardIntent::CleanupRunSelected,
            opened + tsk_tui::ui::board::CARD_ENTER_GUARD / 2
        ),
        None,
        "an Enter right as the card opened is ignored"
    );
    assert_eq!(
        model.resolve_card_choice(
            BoardIntent::CleanupRunSelected,
            opened + tsk_tui::ui::board::CARD_ENTER_GUARD
        ),
        Some(BoardIntent::LaunchUnarchive)
    );

    // The `esc` line is the cancel target: it keeps the project archived.
    let hits = tsk_tui::ui::board::board_hit_map(
        ratatui::layout::Rect::new(0, 0, STANDARD.0, STANDARD.1),
        &model,
    );
    let esc = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::ModalClose)
        .expect("the esc line is hit-testable");
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(esc.area.x, esc.area.y)),
        Some(BoardIntent::LaunchKeepArchived)
    );
    let _ = std::fs::remove_dir_all(dir);
}
