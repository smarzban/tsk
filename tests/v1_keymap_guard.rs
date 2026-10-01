//! Guard the documented V1 queue keymap against retired UI returning.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::board::{apply_intent, BoardInputMode, BoardModel};
use tsk_tui::ui::input::{map_key, normal_mode_keymap, BoardIntent, MarkDirection};

fn normal(code: KeyCode) -> Option<BoardIntent> {
    map_key(
        BoardInputMode::Normal,
        KeyEvent::new(code, KeyModifiers::NONE),
    )
}

fn ctrl(code: KeyCode) -> Option<BoardIntent> {
    map_key(
        BoardInputMode::Normal,
        KeyEvent::new(code, KeyModifiers::CONTROL),
    )
}

#[test]
fn normal_mode_keymap_equals_the_readme_and_queue_board_v1_set() {
    // (key, intent, needs ctrl)
    let documented = [
        (KeyCode::Char('j'), BoardIntent::SelectNext, false),
        (KeyCode::Down, BoardIntent::SelectNext, false),
        (KeyCode::Char('k'), BoardIntent::SelectPrev, false),
        (KeyCode::Up, BoardIntent::SelectPrev, false),
        (KeyCode::Char('M'), BoardIntent::ToggleMarkMode, false),
        (KeyCode::Char(' '), BoardIntent::MarkToggle, false),
        (KeyCode::Enter, BoardIntent::OpenTaskPage, false),
        (KeyCode::Right, BoardIntent::PeekDetail, false),
        (KeyCode::Char('l'), BoardIntent::PeekDetail, false),
        (KeyCode::Left, BoardIntent::CollapseDetail, false),
        (KeyCode::Char('h'), BoardIntent::CollapseDetail, false),
        (KeyCode::Esc, BoardIntent::CloseLayer, false),
        (KeyCode::Char('s'), BoardIntent::PrimaryVerb, true),
        (KeyCode::Char('g'), BoardIntent::Dispatch, true),
        (KeyCode::Char('d'), BoardIntent::Complete, true),
        (
            KeyCode::Char('n'),
            BoardIntent::SetStatus(tsk_tui::domain::HumanStatus::Ready),
            true,
        ),
        (KeyCode::Char('o'), BoardIntent::Reopen, true),
        (KeyCode::Char('b'), BoardIntent::ToggleBlock, true),
        (KeyCode::Char('r'), BoardIntent::ToggleReview, true),
        (KeyCode::Char('e'), BoardIntent::BeginEditTitle, true),
        (KeyCode::Char('x'), BoardIntent::SoftDelete, true),
        (KeyCode::Delete, BoardIntent::SoftDelete, true),
        (KeyCode::Char('u'), BoardIntent::Undo, true),
        (KeyCode::Char('f'), BoardIntent::File, true),
        (KeyCode::Char('+'), BoardIntent::OpenCapture, false),
        (KeyCode::Char('d'), BoardIntent::ToggleDoneDrawer, false),
        (KeyCode::Char('g'), BoardIntent::ToggleAllGroups, false),
        (KeyCode::Char('p'), BoardIntent::OpenProjectSelector, false),
        (
            KeyCode::Char('t'),
            BoardIntent::OpenThreadFilterPicker,
            false,
        ),
        (
            KeyCode::Char('v'),
            BoardIntent::OpenProjectsViewPicker,
            false,
        ),
        (KeyCode::Char('/'), BoardIntent::FocusSearch, false),
        (KeyCode::Char(':'), BoardIntent::OpenCommandPalette, false),
        (KeyCode::Char('?'), BoardIntent::OpenHelp, false),
        (KeyCode::Char('q'), BoardIntent::Quit, true),
    ];
    let mut table: Vec<(KeyCode, BoardIntent)> = documented
        .iter()
        .map(|(key, intent, _)| (*key, intent.clone()))
        .collect();
    table.splice(
        5..5,
        [
            (KeyCode::Down, BoardIntent::MarkExtend(MarkDirection::Down)),
            (KeyCode::Up, BoardIntent::MarkExtend(MarkDirection::Up)),
        ],
    );
    assert_eq!(
        normal_mode_keymap(),
        table,
        "the normal-mode table must equal the documented queue keymap"
    );
    for (key, intent, needs_ctrl) in documented {
        if needs_ctrl {
            assert_eq!(ctrl(key), Some(intent), "ctrl+{key:?}");
            // A bare mutating letter is dead unless the same letter carries a bare route
            // of its own (`d` opens the done drawer, `g` folds groups).
            if !matches!(key, KeyCode::Char('d' | 'g')) {
                assert_eq!(normal(key), None, "bare mutating key {key:?} must be dead");
            }
        } else {
            assert_eq!(normal(key), Some(intent), "documented key {key:?}");
        }
    }
    assert_eq!(
        map_key(
            BoardInputMode::Normal,
            KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT),
        ),
        Some(BoardIntent::ToggleMarkMode)
    );
    assert_eq!(normal(KeyCode::Char('m')), None);
    assert_eq!(
        map_key(
            BoardInputMode::Normal,
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
        ),
        Some(BoardIntent::MarkExtend(MarkDirection::Down))
    );
    assert_eq!(
        map_key(
            BoardInputMode::Normal,
            KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT),
        ),
        Some(BoardIntent::MarkExtend(MarkDirection::Up))
    );
    // The bare/ctrl split on one letter resolves by modifier, never by table order.
    assert_eq!(
        normal(KeyCode::Char('d')),
        Some(BoardIntent::ToggleDoneDrawer)
    );
    assert_eq!(ctrl(KeyCode::Char('d')), Some(BoardIntent::Complete));

    for retired in ['a', 'c', 'i', 'z', 'P', '1', '2', '3', '[', ']'] {
        assert_eq!(
            normal(KeyCode::Char(retired)),
            None,
            "retired normal-mode key {retired:?} must stay unbound"
        );
    }
    assert_eq!(
        ctrl(KeyCode::Char('g')),
        Some(BoardIntent::Dispatch),
        "ctrl+g dispatch remains distinct from bare g group folding"
    );
}

#[test]
fn ctrl_c_quits_from_normal_and_task_page_modes() {
    assert_eq!(
        map_key(
            BoardInputMode::Normal,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ),
        Some(BoardIntent::Quit)
    );
    assert_eq!(
        map_key(
            BoardInputMode::TaskPage,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ),
        Some(BoardIntent::Quit)
    );
}

/// Bare `a`/`space`/`x`/`e` on the task page view produce no steps mutation. The page is
/// view-only until a field edit begins, so a bare key must never enter that edit session.
#[test]
fn bare_page_keys_never_mutate_steps() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Guarded page task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain.add_step(id, "alpha step").expect("step 1");
    domain.add_step(id, "bravo step").expect("step 2");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    let before = domain.get(id).expect("task").clone();
    for key in [
        KeyCode::Char('a'),
        KeyCode::Char('s'),
        KeyCode::Char(' '),
        KeyCode::Char('x'),
        KeyCode::Char('e'),
    ] {
        assert_eq!(
            map_key(
                BoardInputMode::TaskPage,
                KeyEvent::new(key, KeyModifiers::NONE)
            ),
            None,
            "bare {key:?} must be dead on the page view"
        );
    }
    let after = domain.get(id).expect("task");
    assert_eq!(
        after.steps, before.steps,
        "bare page keys must not mutate the steps"
    );
    assert_eq!(after.status, before.status, "status untouched");
    assert_eq!(after.revision, before.revision, "no journaled mutation");
    assert_eq!(
        model.input_mode(),
        BoardInputMode::TaskPage,
        "bare page keys must not open an edit"
    );
}

/// The shared modal card's body is at most 58 columns at the standard tier, so every help
/// line must fit or the card silently clips a binding at every terminal size.
#[test]
fn help_card_lines_fit_the_modal_body_at_every_size() {
    for line in tsk_tui::ui::input::help_card_lines() {
        let width = line.chars().count();
        assert!(
            width <= 58,
            "help line is {width} cells, over the 58-cell card body: {line:?}"
        );
    }
}

#[test]
fn vim_horizontal_keys_are_navigation_only_outside_text_entry() {
    assert_eq!(
        normal(KeyCode::Char('h')),
        Some(BoardIntent::CollapseDetail)
    );
    assert_eq!(normal(KeyCode::Char('l')), Some(BoardIntent::PeekDetail));

    for character in ['h', 'l'] {
        for mode in [
            BoardInputMode::EditTitle,
            BoardInputMode::EditNotes,
            BoardInputMode::EditThread,
            BoardInputMode::EditStep,
        ] {
            assert_eq!(
                map_key(
                    mode,
                    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
                ),
                Some(BoardIntent::EditInsert(character)),
                "{character} remains text while editing {mode:?}"
            );
        }
        for (mode, expected) in [
            (
                BoardInputMode::QuickAdd,
                BoardIntent::QuickAddInsert(character),
            ),
            (
                BoardInputMode::ListPicker,
                BoardIntent::ListPickerQueryInsert(character),
            ),
            (
                BoardInputMode::Search,
                BoardIntent::SearchQueryInsert(character),
            ),
            (
                BoardInputMode::Palette,
                BoardIntent::CommandQueryInsert(character),
            ),
            (
                BoardInputMode::Help,
                BoardIntent::HelpQueryInsert(character),
            ),
        ] {
            assert_eq!(
                map_key(
                    mode,
                    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
                ),
                Some(expected),
                "{character} remains query text in {mode:?}"
            );
        }
    }
}
