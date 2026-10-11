//! The start cards (relaunch, start anyway, the bulk start card) share the cleanup card's
//! option selector: one line per option starting with its key, a `▸` cursor that `↑`/`↓` move
//! and `Enter` runs past the early-`Enter` guard, a single click that selects and a double
//! click that runs, and a dim `esc cancel` line instead of a footer key list.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tsk_tui::agents::AgentProfiles;
use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;
use tsk_tui::ui::board::{
    apply_intent, board_hit_map, draw_board, BoardInputMode, BoardModel, DispatchPrompt,
    RelaunchPrompt, StartAnywayPrompt, CARD_ENTER_GUARD, ROW_DOUBLE_CLICK_WINDOW,
};
use tsk_tui::ui::input::{map_key, BoardIntent};
use tsk_tui::ui::mouse::map_board_mouse;
use tsk_tui::ui::render::{QueueHitMap, QueueHitTarget};
use uuid::Uuid;

const WIDTHS: [u16; 3] = [40, 78, 110];

fn card(prompt: DispatchPrompt) -> BoardModel {
    let domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    model.begin_dispatch_prompt(prompt);
    model
}

fn relaunch() -> BoardModel {
    card(DispatchPrompt::relaunch(RelaunchPrompt {
        task_id: Uuid::from_u128(12),
        number: 12,
        any_status: false,
        assignee: "claude".into(),
        worktree: "~/worktrees/tsk-t12-card".into(),
    }))
}

fn start_anyway() -> BoardModel {
    card(DispatchPrompt::start_anyway(StartAnywayPrompt {
        waiting: vec!["T203 runs after T202 (started)".into()],
        intent: BoardIntent::PrimaryVerb,
    }))
}

/// A marked set that only starts (no launch needs an eligible agent here).
fn bulk() -> BoardModel {
    let mut prompt = DispatchPrompt::start_anyway(StartAnywayPrompt {
        waiting: Vec::new(),
        intent: BoardIntent::PrimaryVerb,
    });
    prompt.start_anyway = None;
    prompt.any_status = false;
    prompt.start_only = vec![
        ("T4".into(), Uuid::from_u128(4)),
        ("T5".into(), Uuid::from_u128(5)),
    ];
    prompt.skipped = vec![("T6".into(), "unknown agent builder".into())];
    card(prompt)
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("tsk-start-cards-{label}-{nanos}-{seq}"))
}

/// A marked set where T1 (assigned to `claude`) launches and T2 only starts, built through the
/// same eligibility check `ctrl+s` runs.
fn bulk_launching() -> BoardModel {
    let dir = temp_dir("launch");
    std::fs::create_dir_all(&dir).expect("state dir");
    std::fs::write(
        dir.join("config.toml"),
        "[agent.claude]\ncommand = [\"true\"]\n",
    )
    .expect("profiles");
    let profiles = AgentProfiles::load(&dir).expect("load profiles");
    let scope = TaskScope::Project {
        path: "/repos/app".into(),
    };
    let mut state = DomainState::new();
    let launch = state
        .create_assigned(
            "first job",
            None,
            scope.clone(),
            ProvenanceOrigin::Manual,
            None,
            Some("claude".into()),
        )
        .expect("assigned task");
    let start = state
        .create("second job", None, scope, ProvenanceOrigin::Manual, None)
        .expect("plain task");
    let store = TaskStore::new(&dir);
    store.save(&state).expect("save");
    let state = store.load().expect("numbered");
    let eligible =
        tsk_tui::dispatch::check_task(&state, launch, &profiles, true).expect("eligible");
    let _ = std::fs::remove_dir_all(&dir);
    let mut prompt = DispatchPrompt::start_anyway(StartAnywayPrompt {
        waiting: Vec::new(),
        intent: BoardIntent::PrimaryVerb,
    });
    prompt.start_anyway = None;
    prompt.any_status = false;
    prompt.launch = vec![eligible];
    prompt.start_only = vec![("T2".into(), start)];
    card(prompt)
}

fn buffer(model: &BoardModel, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| {
            draw_board(frame, model);
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn rows(model: &BoardModel, width: u16, height: u16) -> Vec<String> {
    let buffer = buffer(model, width, height);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

/// The card's body, rows inside the borders, each trimmed.
fn card_rows(model: &BoardModel, width: u16) -> Vec<String> {
    rows(model, width, 30)
        .iter()
        .filter_map(|row| {
            let start = row.find('│')?;
            let end = row.rfind('│')?;
            (end > start).then(|| row[start + '│'.len_utf8()..end].trim_end().to_string())
        })
        .collect()
}

/// The body as one line, so a wrapped sentence reads whole whatever the width.
fn body(model: &BoardModel, width: u16) -> String {
    card_rows(model, width)
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn assert_says(model: &BoardModel, texts: &[&str]) {
    for width in WIDTHS {
        let said = body(model, width);
        for text in texts {
            assert!(said.contains(text), "{width} lost {text:?}:\n{said}");
        }
        let painted = rows(model, width, 30).join("\n");
        assert!(
            !painted.contains("y relaunch · n just start")
                && !painted.contains("y start anyway · esc")
                && !painted.contains("· esc cancel"),
            "{width}: no footer key list:\n{painted}"
        );
    }
}

/// Whether the painted row holding `text` is bold.
fn bold_row(model: &BoardModel, width: u16, text: &str) -> bool {
    let buffer = buffer(model, width, 30);
    let lines = rows(model, width, 30);
    let y = lines
        .iter()
        .position(|row| row.contains(text))
        .unwrap_or_else(|| panic!("{text:?} painted:\n{}", lines.join("\n")));
    let x = lines[y]
        .find(text)
        .map(|byte| lines[y][..byte].chars().count())
        .unwrap();
    buffer[(x as u16, y as u16)]
        .modifier
        .contains(Modifier::BOLD)
}

fn key(code: KeyCode) -> BoardIntent {
    map_key(
        BoardInputMode::DispatchConfirm,
        KeyEvent::new(code, KeyModifiers::NONE),
    )
    .unwrap_or_else(|| panic!("{code:?} maps on a start card"))
}

fn selected(model: &BoardModel) -> Option<usize> {
    let prompt = model.dispatch_prompt().expect("card");
    prompt.selector.selected_index(&prompt.options())
}

fn click(model: &BoardModel, hits: &QueueHitMap, target: QueueHitTarget) -> Option<BoardIntent> {
    let area = hits
        .regions
        .iter()
        .rev()
        .find(|hit| hit.target == target)
        .unwrap_or_else(|| panic!("{target:?} painted"))
        .area;
    map_board_mouse(
        model,
        hits,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + area.width / 2,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        },
    )
}

#[test]
fn the_relaunch_card_lists_both_options_with_the_cursor_on_relaunch() {
    let model = relaunch();
    assert_says(
        &model,
        &[
            "@claude is no longer running.",
            "▸ y relaunch @claude in the same worktree · start T12",
            "n start T12 · don't relaunch @claude",
            "worktree ~/worktrees/tsk-t12-card",
            "esc cancel",
        ],
    );
    for width in WIDTHS {
        assert!(bold_row(&model, width, "y  relaunch"), "{width}: y is bold");
        assert!(!bold_row(&model, width, "n  start"), "{width}: n is plain");
    }
    // One blank row between sections, none doubled.
    let card = card_rows(&model, 110);
    let options = card.iter().position(|row| row.contains("▸ y")).unwrap();
    assert!(card[options - 1].trim().is_empty(), "{card:#?}");
    assert!(!card[options - 2].trim().is_empty(), "{card:#?}");
    let worktree = card
        .iter()
        .position(|row| row.trim_start().starts_with("worktree"))
        .unwrap();
    assert!(card[worktree - 1].trim().is_empty(), "{card:#?}");
    assert!(card[worktree + 1].trim().is_empty(), "{card:#?}");
    assert!(card[worktree + 2].contains("esc cancel"), "{card:#?}");
}

#[test]
fn the_start_anyway_and_bulk_cards_say_what_y_does() {
    let model = start_anyway();
    assert_says(
        &model,
        &[
            "T203 runs after T202 (started).",
            "▸ y start now, without waiting",
            "esc cancel",
        ],
    );
    let model = bulk();
    assert_says(
        &model,
        &[
            "start only T4 started, no launch",
            "not started T6 unknown agent builder",
            "▸ y start T4 and T5 · T6 not started",
            "esc cancel",
        ],
    );
}

#[test]
fn the_bulk_card_with_a_launch_names_it_and_runs_it_like_every_card() {
    let mut model = bulk_launching();
    assert_says(
        &model,
        &[
            "T1 @claude from default",
            "start only T2 started, no launch",
            "▸ y dispatch T1 · start T2 without launching",
            "esc cancel",
        ],
    );
    for width in WIDTHS {
        assert!(
            rows(&model, width, 30).join("\n").contains("Dispatch"),
            "{width}: the title says it dispatches"
        );
        assert!(bold_row(&model, width, "y  dispatch"), "{width}: y is bold");
    }
    assert_eq!(selected(&model), Some(0));
    let opened = model.dispatch_prompt().unwrap().selector.opened_at;
    assert_eq!(
        model.resolve_card_choice(
            BoardIntent::CleanupRunSelected,
            opened + CARD_ENTER_GUARD / 2
        ),
        None,
        "an Enter right after the card opened is ignored"
    );
    assert_eq!(
        model.resolve_card_choice(BoardIntent::CleanupRunSelected, opened + CARD_ENTER_GUARD),
        Some(BoardIntent::ConfirmDispatch)
    );
    let hits = board_hit_map(Rect::new(0, 0, 110, 30), &model);
    let click_intent = click(&model, &hits, QueueHitTarget::CleanupOption(0)).expect("click");
    assert_eq!(
        model.resolve_card_choice(click_intent.clone(), opened),
        None
    );
    assert_eq!(
        model.resolve_card_choice(click_intent, opened + Duration::from_millis(100)),
        Some(BoardIntent::ConfirmDispatch),
        "a double click runs it"
    );
    assert_eq!(
        model.resolve_card_choice(key(KeyCode::Char('n')), opened),
        None,
        "`n` is the relaunch card's alone"
    );
}

#[test]
fn arrows_move_the_relaunch_cursor_and_stop_at_the_ends() {
    let mut domain = DomainState::new();
    let mut model = relaunch();
    assert_eq!(selected(&model), Some(0), "starts on the bold `y`");
    let mut step = |model: &mut BoardModel, code| {
        apply_intent(&mut domain, model, key(code), None).expect("step");
    };
    step(&mut model, KeyCode::Up);
    assert_eq!(selected(&model), Some(0), "no wrap at the top");
    step(&mut model, KeyCode::Down);
    assert_eq!(selected(&model), Some(1));
    step(&mut model, KeyCode::Down);
    assert_eq!(selected(&model), Some(1), "no wrap at the bottom");
    assert!(
        body(&model, 78).contains("▸ n start T12"),
        "the cursor moved"
    );

    // `Enter` runs the selected option, but not as the card appears.
    let opened = model.dispatch_prompt().unwrap().selector.opened_at;
    assert_eq!(key(KeyCode::Enter), BoardIntent::CleanupRunSelected);
    assert_eq!(
        model.resolve_card_choice(
            BoardIntent::CleanupRunSelected,
            opened + CARD_ENTER_GUARD / 2
        ),
        None,
        "an Enter right after the card opened is ignored"
    );
    assert_eq!(
        model.resolve_card_choice(BoardIntent::CleanupRunSelected, opened + CARD_ENTER_GUARD),
        Some(BoardIntent::StartWithoutRelaunch)
    );
}

#[test]
fn letter_keys_run_only_the_options_a_card_offers() {
    let now = std::time::Instant::now();
    let mut model = relaunch();
    assert_eq!(
        model.resolve_card_choice(key(KeyCode::Char('y')), now),
        Some(BoardIntent::ConfirmDispatch)
    );
    assert_eq!(
        model.resolve_card_choice(key(KeyCode::Char('n')), now),
        Some(BoardIntent::StartWithoutRelaunch)
    );
    for mut model in [start_anyway(), bulk()] {
        assert_eq!(
            model.resolve_card_choice(key(KeyCode::Char('y')), now),
            Some(BoardIntent::ConfirmDispatch)
        );
        assert_eq!(
            model.resolve_card_choice(key(KeyCode::Char('n')), now),
            None,
            "`n` is the relaunch card's alone"
        );
        // One option: the arrows keep the cursor on it.
        let mut domain = DomainState::new();
        apply_intent(&mut domain, &mut model, key(KeyCode::Down), None).expect("down");
        assert_eq!(selected(&model), Some(0));
    }
    assert_eq!(key(KeyCode::Esc), BoardIntent::CancelDispatch);
}

#[test]
fn a_single_click_selects_a_double_click_runs_and_the_esc_line_cancels() {
    for width in WIDTHS {
        let mut model = relaunch();
        let area = Rect::new(0, 0, width, 30);
        let hits = board_hit_map(area, &model);
        let intent = click(&model, &hits, QueueHitTarget::CleanupOption(1));
        assert_eq!(intent, Some(BoardIntent::CleanupClickOption(1)));
        let now = model.dispatch_prompt().unwrap().selector.opened_at;
        let intent = intent.unwrap();
        assert_eq!(
            model.resolve_card_choice(intent.clone(), now),
            None,
            "{width}: selected, not run"
        );
        assert_eq!(selected(&model), Some(1));
        assert_eq!(
            model.resolve_card_choice(intent.clone(), now + ROW_DOUBLE_CLICK_WINDOW * 3),
            None,
            "{width}: too slow is two single clicks"
        );
        assert_eq!(
            model.resolve_card_choice(
                intent,
                now + ROW_DOUBLE_CLICK_WINDOW * 3 + Duration::from_millis(100)
            ),
            Some(BoardIntent::StartWithoutRelaunch),
            "{width}: a double click runs it"
        );
        // A double click across two lines runs neither.
        let mut model = relaunch();
        let first = model.resolve_card_choice(BoardIntent::CleanupClickOption(0), now);
        let second = model.resolve_card_choice(
            BoardIntent::CleanupClickOption(1),
            now + Duration::from_millis(100),
        );
        assert_eq!((first, second), (None, None));
        // The esc line (the last close target; `[x]` is the first) cancels.
        assert_eq!(
            click(&model, &hits, QueueHitTarget::ModalClose),
            Some(BoardIntent::CancelDispatch)
        );
    }
}

#[test]
fn every_start_card_resolves_through_the_same_selector_after_a_resize() {
    // A narrow frame wraps the options; the cursor and its targets follow.
    let model = bulk();
    let hits = board_hit_map(Rect::new(0, 0, 40, 30), &model);
    let wrapped = hits
        .regions
        .iter()
        .filter(|hit| hit.target == QueueHitTarget::CleanupOption(0))
        .count();
    assert!(wrapped >= 2, "the option wraps at 40 columns");
    assert!(!hits
        .regions
        .iter()
        .any(|hit| hit.target == QueueHitTarget::CleanupOption(1)));
}
