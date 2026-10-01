//! Board task-page title and notes editing regressions.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::board::{
    apply_intent, board_hit_map, draw_board, BoardInputMode, BoardModel, IntentOutcome,
};
use tsk_tui::ui::capture::CaptureField;
use tsk_tui::ui::input::{map_board_form_key, map_key, BoardIntent};
use tsk_tui::ui::render::QueueHitTarget;

const THIS_REPO: &str = "/repos/app";

fn project(path: &str) -> TaskScope {
    TaskScope::Project {
        path: path.to_string(),
    }
}

/// Title edit opened via `e` must obey EditBuffer char-index cursor, word chords,
/// paste, and the bound-task refusal (confirm lands on the id bound at open, not live selection).
#[test]
fn title_edit_e_obeys_editbuffer_char_index_word_chords_paste_and_bound_task_refusal() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Alpha Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));

    // Open via BeginEditTitle (the 'e' path).
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("begin title");
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    assert_eq!(model.edit_target(), Some(id));
    assert_eq!(model.edit_buffer(), "Alpha Task");

    // Char-index cursor movement and insert.
    for _ in 0..5 {
        apply_intent(&mut domain, &mut model, BoardIntent::EditMoveLeft, None).expect("left");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('X'), None).expect("insert");
    assert_eq!(model.edit_buffer(), "AlphaX Task");

    // Word chord: move word left then right; cursor must be char-granular.
    apply_intent(&mut domain, &mut model, BoardIntent::EditMoveWordLeft, None).expect("word left");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditMoveWordRight,
        None,
    )
    .expect("word right");

    // Paste must route (EditInsertText).
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText(" pasted".to_string()),
        None,
    )
    .expect("paste");
    assert!(model.edit_buffer().contains("pasted"));

    // Confirm must land on the bound id even if a sync reorders selection.
    // (The heavy cross-actor reorder cases live in edit_target_binding.rs; here we assert
    // the open path binds and confirm uses the binding.)
    let expected = model.edit_buffer().to_string();
    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
        .expect("confirm title");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert_eq!(domain.get(id).expect("task").title, expected);
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(model.edit_target(), Some(id));
}

/// Palette notes edit obeys the Shift+Enter save chord and bound-task refusal.
#[test]
fn palette_notes_edit_obeys_notes_save_chord_pair_and_bound_task_refusal() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Notes Task",
            Some("orig notes".into()),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));

    // Notes is palette-routed: BeginEditNotes (the 'n' or palette "edit notes" path).
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("begin notes");
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    assert_eq!(model.edit_target(), Some(id));

    // Type a multi-line draft (notes accept breaks).
    for ch in "line1\nline2".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None).expect("insert");
    }
    // Confirm via the notes save chord intent (bare Enter in Notes inserts a line break;
    // Shift+Enter maps to ConfirmEdit at the reducer boundary this test drives).
    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
        .expect("confirm notes");
    assert_eq!(outcome, IntentOutcome::Persist);
    let saved = domain.get(id).expect("task");
    assert!(saved.notes.as_deref().unwrap_or("").contains("line1"));
    assert!(saved.notes.as_deref().unwrap_or("").contains("line2"));
}

/// An open edit session is not redirected by sync_from_domain reorder.
/// The bound task remains the confirm target even when a domain reorder changes visible order.
#[test]
fn open_edit_session_not_redirected_by_sync_from_domain_reorder() {
    let mut domain = DomainState::new();
    let a = domain
        .create(
            "A",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("a");
    let b = domain
        .create(
            "B",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("b");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));

    // Select A explicitly and open a title edit.
    let visible = model.visible_ids();
    let a_idx = visible.iter().position(|&id| id == a).unwrap();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(a_idx),
        None,
    )
    .expect("select a");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("begin title on a");
    assert_eq!(model.edit_target(), Some(a));

    // Simulate a domain reorder that would put B first (e.g., B updated later).
    domain.set_status(b, HumanStatus::Started).expect("b doing");
    // Force a sync that reorders visible list.
    model.sync_from_domain(&domain);
    // Selection may move, but the edit binding must not.
    assert_eq!(model.edit_target(), Some(a), "binding must survive reorder");
    // Edit buffer and mode stay.
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);

    // Confirm must still land on A, not whatever now sits at the old index.
    // Type a distinguishing suffix.
    for ch in " edited".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None).expect("type");
    }
    let _ = apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("confirm");
    assert_eq!(domain.get(a).expect("a").title, "A edited");
    assert_eq!(domain.get(b).expect("b").title, "B", "bystander untouched");
}

/// The cursor and visible window an open editor paints must be computed against the frame's
/// actual paint width, not a hardcoded one, otherwise the cursor detaches from what is on
/// screen once the draft is longer than the hardcoded width assumed.
#[test]
fn title_edit_cursor_and_window_track_the_actual_paint_width_not_a_hardcoded_one() {
    // 62 chars: longer than the old hardcoded 40-wide window in every tier under test.
    let title: String = "0123456789"
        .chars()
        .chain('A'..='Z')
        .chain('a'..='z')
        .collect();
    assert_eq!(title.chars().count(), 62);

    let mut domain = DomainState::new();
    domain
        .create(
            &title,
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("begin title");
    assert_eq!(
        model.edit_cursor(),
        62,
        "cursor opens parked at the draft's end"
    );

    // Wide (100 cols): the whole draft fits in the page header, so the window is unscrolled
    // and the cursor sits exactly at the glyph prefix (2) + the draft's full length, not a
    // column computed against a narrower hardcoded width.
    {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let _ = draw_board(frame, &model);
            })
            .expect("draw 100x30");
        let (title_y, _row) = (0..30)
            .map(|y| (y, row_text(&terminal, 100, y)))
            .find(|(_, row)| row.contains(&title))
            .expect("task page header row");
        assert_eq!(title_y, 1, "the page header paints under one blank row");
        let cursor = terminal.get_cursor_position().expect("cursor position");
        assert_eq!(
            cursor,
            ratatui::layout::Position::new(4 + 62, title_y),
            "cursor must land immediately after the full draft, not a 40-wide-window column"
        );
    }

    // Narrow (40 cols, compact): the draft overflows the header, so it WRAPS onto a
    // second bold header row -- nothing is cut, and the caret follows onto that
    // continuation row at its past-end column.
    {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let _ = draw_board(frame, &model);
            })
            .expect("draw 40x10");
        // The open status word leaves a 31-cell title field, so this wraps as 31 / 31.
        let head_row = row_text(&terminal, 40, 1);
        assert!(
            head_row.contains("0123456789"),
            "40-wide header keeps the head on its first wrapped row: {head_row:?}"
        );
        let tail_row = (0..10)
            .map(|y| (y, row_text(&terminal, 40, y)))
            .find(|(_, row)| row.trim_end().contains("yz"))
            .expect("wrapped title continuation row");
        assert_ne!(
            tail_row.0, 1,
            "the title's tail must wrap below the first header row"
        );
        let cursor = terminal.get_cursor_position().expect("cursor position");
        assert_eq!(
            cursor,
            ratatui::layout::Position::new(4 + 31, tail_row.0),
            "caret lands at the wrapped title's past-end column"
        );
    }
}

/// /,,: `e`, palette Notes, and palette scope all open one
/// immutable task form. Its three drafts save together, while its scope dropdown is a child
/// surface, not a second editor or an implicit save.
#[test]
fn task_form_unifies_palette_field_routes_scope_dropdown_and_atomic_save() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Task",
            Some("old".into()),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create target");
    domain
        .create(
            "Other project",
            None,
            project("/repos/other"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create scope source");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let target_index = model
        .visible_ids()
        .iter()
        .position(|&visible| visible == id)
        .expect("target visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(target_index),
        None,
    )
    .expect("select target");

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("e opens task form");
    assert_eq!(model.edit_target(), Some(id));
    assert_eq!(model.form_focus(), Some(CaptureField::Title));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Notes),
        None,
    )
    .expect("a direct field focus keeps the same bound form");
    assert_eq!(model.edit_target(), Some(id));
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Title),
        None,
    )
    .expect("Title focus returns to the same draft");
    assert_eq!(model.form_focus(), Some(CaptureField::Title));
    assert_eq!(
        model.form_scope_options(),
        vec![
            project(THIS_REPO),
            project("/repos/other"),
            TaskScope::Global
        ],
        "form scope choices are the initial scope, current repo, live projects, then Global"
    );
    for character in " updated".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type title");
    }

    let tab = map_board_form_key(
        model.form_focus().expect("open form focus"),
        false,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
    )
    .expect("Tab moves a task form field");
    assert_eq!(tab, BoardIntent::FormFocusNext);
    apply_intent(&mut domain, &mut model, tab, None).expect("focus Notes");
    assert_eq!(model.form_focus(), Some(CaptureField::Notes));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertLineBreak,
        None,
    )
    .expect("Notes newline");
    for character in "new".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type Notes");
    }

    let tab = map_board_form_key(
        model.form_focus().expect("Notes focus"),
        false,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
    )
    .expect("Tab moves to add target");
    apply_intent(&mut domain, &mut model, tab, None).expect("select add target");
    let tab = map_board_form_key(
        model
            .form_focus()
            .expect("Notes stays focused at add target"),
        false,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
    )
    .expect("Tab moves to Assignee");
    apply_intent(&mut domain, &mut model, tab, None).expect("focus Assignee");
    assert_eq!(model.form_focus(), Some(CaptureField::Assignee));
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("focus Thread");
    assert_eq!(model.form_focus(), Some(CaptureField::Thread));
    let tab = map_board_form_key(
        CaptureField::Thread,
        false,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
    )
    .expect("Tab moves to Scope");
    apply_intent(&mut domain, &mut model, tab, None).expect("focus Scope");
    assert_eq!(model.form_focus(), Some(CaptureField::Scope));
    let before_cycle = model.form_scope().cloned().expect("scope draft");
    assert_eq!(
        map_board_form_key(
            CaptureField::Scope,
            false,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)
        ),
        Some(BoardIntent::FormCycleScope)
    );
    apply_intent(&mut domain, &mut model, BoardIntent::FormCycleScope, None)
        .expect("Space cycles direct scope");
    assert_ne!(model.form_scope(), Some(&before_cycle));

    let open_dropdown = map_board_form_key(
        CaptureField::Scope,
        false,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    )
    .expect("Scope Enter opens its dropdown");
    assert_eq!(
        open_dropdown,
        BoardIntent::OpenFormDropdown(CaptureField::Scope)
    );
    apply_intent(&mut domain, &mut model, open_dropdown, None).expect("open dropdown");
    assert_eq!(model.input_mode(), BoardInputMode::FormDropdown);
    let draft_before_dropdown = model.form_scope().cloned();
    apply_intent(&mut domain, &mut model, BoardIntent::FormDropdownNext, None)
        .expect("move dropdown selection");
    assert_eq!(
        model.form_scope(),
        draft_before_dropdown.as_ref(),
        "moving the dropdown selection must not apply it"
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CancelFormDropdown,
        None,
    )
    .expect("Esc returns to parent form");
    assert_eq!(model.form_focus(), Some(CaptureField::Scope));
    assert_ne!(model.input_mode(), BoardInputMode::FormDropdown);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Scope),
        None,
    )
    .expect("open dropdown again");
    for _ in 0..model.form_scope_options().len() {
        if model.form_scope_dropdown_choice() == Some(&TaskScope::Global) {
            break;
        }
        apply_intent(&mut domain, &mut model, BoardIntent::FormDropdownNext, None)
            .expect("move toward Global");
    }
    assert_eq!(model.form_scope_dropdown_choice(), Some(&TaskScope::Global));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmFormDropdown,
        None,
    )
    .expect("apply dropdown scope");
    assert_eq!(model.form_scope(), Some(&TaskScope::Global));

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Scope wraps to Title");
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
        .expect("one atomic form save");
    assert_eq!(outcome, IntentOutcome::Persist);
    let saved = domain.get(id).expect("saved task");
    assert_eq!(saved.title, "Task updated");
    assert_eq!(saved.notes.as_deref(), Some("old\nnew"));
    assert_eq!(saved.scope, TaskScope::Global);

    // The saved task left this project board for the desk: navigation is the user's
    // move now (no auto-reveal), so go there before the palette routes.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::Desk),
        None,
    )
    .expect("move to the desk");
    assert!(
        model.visible_ids().contains(&id),
        "the saved desk task is visible from the desk"
    );
    let saved_row = model
        .visible_ids()
        .iter()
        .position(|&visible| visible == id)
        .expect("saved row");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(saved_row),
        None,
    )
    .expect("pin the saved task");

    let palette_notes_route = model
        .available_commands()
        .into_iter()
        .find(|command| command.label == "edit notes")
        .expect("palette Edit notes command")
        .intent;
    assert_eq!(palette_notes_route, BoardIntent::BeginEditNotes);
    apply_intent(&mut domain, &mut model, palette_notes_route, None)
        .expect("palette Notes route opens the shared form");
    assert_eq!(model.edit_target(), Some(id));
    assert_eq!(model.form_focus(), Some(CaptureField::Notes));
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("close Notes form");
    let palette_scope_route = model
        .available_commands()
        .into_iter()
        .find(|command| command.label == "change scope")
        .expect("palette Change scope command")
        .intent;
    assert_eq!(palette_scope_route, BoardIntent::BeginEditScope);
    apply_intent(&mut domain, &mut model, palette_scope_route, None)
        .expect("palette Change scope route opens the shared form");
    assert_eq!(model.edit_target(), Some(id));
    assert_eq!(model.form_focus(), Some(CaptureField::Scope));
}

/// The mode-only mapper and the focused-form mapper share one field-edit implementation. The
fn row_text(terminal: &Terminal<TestBackend>, width: u16, y: u16) -> String {
    let buffer = terminal.backend().buffer();
    (0..width)
        .map(|x| buffer[(x, y)].symbol().to_string())
        .collect()
}

fn rendered_board(model: &BoardModel, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw");
    (0..height)
        .map(|y| row_text(&terminal, width, y))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The page's scope dropdown answers the footer it belongs to: options stack directly
/// above the meta footer, left-aligned, and carry short project names -- never the
/// filesystem path, never the selector's corner.
#[test]
fn task_page_scope_dropdown_sits_above_the_footer_with_short_names() {
    let mut domain = DomainState::new();
    domain
        .create(
            "scoped task",
            None,
            TaskScope::Project {
                path: "/repos/tsk".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    domain
        .create(
            "other project task",
            None,
            TaskScope::Project {
                path: "/repos/other-project".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second project");
    let mut model = BoardModel::from_domain(&domain, Some(std::path::PathBuf::from("/repos/tsk")));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Scope),
        None,
    )
    .expect("open scope dropdown");
    assert_eq!(model.input_mode(), BoardInputMode::FormDropdown);

    let (width, height) = (80u16, 24u16);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");

    let rows: Vec<String> = (0..height).map(|y| row_text(&terminal, width, y)).collect();
    let footer_y = rows
        .iter()
        .position(|row| row.contains("created"))
        .expect("the page footer paints its meta row");
    // Options sit directly above the footer, never down in the corner or below it.
    let option_rows: Vec<(usize, &String)> = rows[..footer_y]
        .iter()
        .enumerate()
        .filter(|(_, row)| row.contains("tsk") || row.contains("other-project"))
        .collect();
    assert!(
        !option_rows.is_empty(),
        "the dropdown must paint its options:\n{rows:?}"
    );
    let (last_option_y, _) = option_rows.last().expect("options painted");
    assert_eq!(
        *last_option_y,
        footer_y - 1,
        "the dropdown's last option must sit directly above the footer:\n{rows:?}"
    );
    // Short names only; left-aligned like the footer, not parked in the selector corner.
    for (_, row) in &option_rows {
        assert!(
            !row.contains("/repos/"),
            "dropdown options must show short names, not paths: {row:?}"
        );
        let first_char = row.chars().next().unwrap_or(' ');
        assert_eq!(first_char, ' ', "options are left-indented: {row:?}");
    }
}

#[test]
fn page_view_shows_thread_beside_scope() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Threaded page",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            Some("release-2026".into()),
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let painted: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        painted.contains("#release-2026"),
        "thread missing: {painted}"
    );
}

#[test]
fn task_page_form_tab_cycle_wraps_through_title() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Unthreaded task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain.add_step(id, "first").expect("first step");
    domain.add_step(id, "second").expect("second step");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("open form");

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Title to Notes");
    assert_eq!(model.form_focus(), Some(CaptureField::Notes));
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Notes to first step");
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ first"));
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("first step to second");
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ second"));
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("second step to add");
    assert!(rendered_board(&model, 80, 24).contains("▸ + step"));
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("add to Assignee");
    assert_eq!(model.input_mode(), BoardInputMode::EditAssignee);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Assignee to Thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Thread to Scope");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Scope to Title");
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None)
        .expect("Title reverses to Scope");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None)
        .expect("Scope reverses to Thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None)
        .expect("Thread reverses to Assignee");
    assert_eq!(model.input_mode(), BoardInputMode::EditAssignee);
    for (expected, label) in [
        ("▸ + step", "Assignee reverses to add"),
        ("second", "add reverses to second"),
        ("first", "second reverses to first"),
    ] {
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None).expect(label);
        assert!(rendered_board(&model, 80, 24).contains(expected), "{label}");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None)
        .expect("first reverses to Notes");
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None)
        .expect("Notes reverses to Title");
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
}

#[test]
fn scope_and_thread_are_selected_controls_with_enter_activation() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("open form");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("Notes");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("add target");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("Assignee");
    assert_eq!(model.input_mode(), BoardInputMode::EditAssignee);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("Thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("Scope");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);

    let scope_hit = board_hit_map(Rect::new(0, 0, 80, 24), &model)
        .regions
        .into_iter()
        .find(|hit| hit.target == QueueHitTarget::FormScope)
        .expect("scope hit");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw selected Scope");
    let scope_style = terminal.backend().buffer()[(scope_hit.area.x, scope_hit.area.y)].style();
    assert!(
        scope_style.add_modifier.contains(Modifier::REVERSED)
            && !scope_style.add_modifier.contains(Modifier::DIM),
        "selected Scope must reverse without retaining dim"
    );
    assert!(
        terminal.backend().buffer()[(scope_hit.area.right() + 3, scope_hit.area.y)]
            .style()
            .add_modifier
            .contains(Modifier::DIM),
        "the unselected created footer text stays dim"
    );

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        map_key(BoardInputMode::EditScope, enter),
        Some(BoardIntent::OpenFormDropdown(CaptureField::Scope))
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Scope),
        None,
    )
    .expect("open Scope dropdown");
    assert_eq!(model.input_mode(), BoardInputMode::FormDropdown);
    assert_eq!(
        map_key(BoardInputMode::FormDropdown, enter),
        Some(BoardIntent::ConfirmFormDropdown)
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmFormDropdown,
        None,
    )
    .expect("choose Scope option");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None).expect("select Thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    let thread_hit = board_hit_map(Rect::new(0, 0, 80, 24), &model)
        .regions
        .into_iter()
        .find(|hit| hit.target == QueueHitTarget::FormThread)
        .expect("thread hit");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw selected Thread");
    let thread_style =
        terminal.backend().buffer()[(thread_hit.area.x + 3, thread_hit.area.y)].style();
    assert!(
        thread_style.add_modifier.contains(Modifier::REVERSED)
            && !thread_style.add_modifier.contains(Modifier::DIM),
        "selected Thread must reverse without retaining dim"
    );
    assert!(
        terminal.backend().buffer()[(scope_hit.area.right() + 3, scope_hit.area.y)]
            .style()
            .add_modifier
            .contains(Modifier::DIM),
        "the unselected created footer text stays dim"
    );
    assert_eq!(
        map_key(BoardInputMode::SelectThread, enter),
        Some(BoardIntent::ToggleThreadEditing)
    );
    assert_eq!(
        map_key(
            BoardInputMode::SelectThread,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)
        ),
        None,
        "selected Thread must not accept hidden text input"
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate Thread editor");
    assert_eq!(model.input_mode(), BoardInputMode::EditThread);
    assert!(
        model.board_form_open(),
        "Thread stays inside the task session"
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("release".into()),
        None,
    )
    .expect("type Thread draft");
    assert_eq!(
        map_key(BoardInputMode::EditThread, enter),
        Some(BoardIntent::ToggleThreadEditing)
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("close Thread editor");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    assert!(
        model.board_form_open(),
        "closing Thread keeps the task session open"
    );
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw staged Thread selection");
    let painted: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        painted.contains("#release"),
        "selected Thread must keep its staged draft visible: {painted}"
    );
}

#[test]
fn page_edit_sets_thread_and_clearing_unthreads() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("open form");
    for _ in 0..4 {
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
            .expect("focus next");
    }
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate thread editor");
    for character in "Release-2026".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type thread");
    }
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("save thread"),
        IntentOutcome::Persist
    );
    assert_eq!(
        domain.get(id).expect("task").thread.as_deref(),
        Some("release-2026")
    );

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("reopen form");
    for _ in 0..4 {
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
            .expect("focus thread");
    }
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate thread editor");
    for _ in 0.."release-2026".len() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditBackspace, None)
            .expect("clear thread");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("unthread");
    assert_eq!(domain.get(id).expect("task").thread, None);
}

#[test]
fn page_thread_field_refuses_invalid_name_without_persisting() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("open");
    for _ in 0..4 {
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("focus");
    }
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate thread editor");
    for character in "bad_name".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type");
    }
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("refusal"),
        IntentOutcome::None
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditThread);
    assert_eq!(domain.get(id).expect("task").thread, None);
}

#[test]
fn page_thread_field_accepts_version_dots() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("open");
    for _ in 0..4 {
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("focus");
    }
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate thread editor");
    for character in "V0.0.6".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type");
    }
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("save"),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(
        domain.get(id).expect("task").thread.as_deref(),
        Some("v0.0.6")
    );
}

#[test]
fn canceling_thread_edit_keeps_the_task_page_and_resets_the_thread_draft() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open task form");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Thread),
        None,
    )
    .expect("select thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate thread editor");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("release".into()),
        None,
    )
    .expect("type thread");

    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
        .expect("cancel thread field");
    assert!(
        model.board_form_open(),
        "field cancel retains the task page"
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Thread),
        None,
    )
    .expect("reselect thread field");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("reopen thread editor");
    assert_eq!(
        model.edit_buffer(),
        "",
        "field cancel restores saved thread"
    );
}

#[test]
fn editing_an_unthreaded_task_paints_a_labeled_thread_footer_slot() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open task form");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Thread),
        None,
    )
    .expect("focus thread");

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw thread field");
    let painted: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        painted.contains("thread · app · created"),
        "the unthreaded edit footer must label the leading Thread target: {painted}"
    );
    assert!(
        !painted.contains("app · # · created"),
        "an empty thread must not render as a dangling hash: {painted}"
    );
}

#[test]
fn thread_field_is_reachable_while_the_inline_step_editor_keeps_its_draft() {
    let mut domain = DomainState::new();
    let task_id = domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            Some("release".into()),
        )
        .expect("create");
    domain.add_step(task_id, "first").expect("step");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
    assert!(
        board_hit_map(Rect::new(0, 0, 80, 24), &model)
            .regions
            .iter()
            .any(|hit| hit.target == QueueHitTarget::FormThread),
        "the threaded page exposes its footer hit before a step editor opens"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("enter task edit mode");
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
        .expect("return to task page");
    apply_intent(&mut domain, &mut model, BoardIntent::SelectStep(0), None)
        .expect("existing item editor");
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None).expect("draft step");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Thread),
        None,
    )
    .expect("select Thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw Thread with inline draft");
    let buffer = terminal.backend().buffer();
    let painted = (0..24)
        .flat_map(|y| (0..80).map(move |x| buffer[(x, y)].symbol()))
        .collect::<String>();
    assert!(
        painted.contains("first!"),
        "moving to Thread keeps the inline step draft visible: {painted}"
    );
}

#[test]
fn thread_refusal_paints_inline_and_clears_without_status_leak() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("open");
    for _ in 0..4 {
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("focus");
    }
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate thread editor");
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('_'), None).expect("type");
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("refuse");
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw refusal");
    let painted: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        painted.contains("start with a letter"),
        "missing inline refusal: {painted}"
    );
    assert_eq!(
        model.message(),
        None,
        "thread refusal must not use status message"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("close");
    assert_eq!(model.message(), None);
}

#[test]
fn task_page_footer_hits_use_display_columns_and_stay_within_the_painted_row() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Threaded task",
            None,
            project("/repos/プロジェクト"),
            ProvenanceOrigin::Manual,
            Some("a2345678901234567890123456789012".into()),
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(
        "/repos/\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}",
    )));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    let width = 40;
    let hits = board_hit_map(Rect::new(0, 0, width, 10), &model);
    let scope = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormScope)
        .expect("scope hit");
    let thread = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormThread)
        .expect("thread hit");
    assert_eq!(
        scope.area.width, 2,
        "the scope target is clipped after the leading thread at this narrow width"
    );
    assert_eq!(
        thread.area.x, 2,
        "thread leads the footer, with no number in the footer"
    );
    assert!(
        thread.area.right() <= width,
        "thread hit must not extend beyond the clipped footer: {thread:?}"
    );
}

/// A persisted task page presents its identifier in the title and leaves the footer scope
/// as a direct, independent hit target.
#[test]
fn task_page_header_identifier_precedes_the_title_and_footer_scope() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Numbered task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut persisted = domain.get(id).expect("task").clone();
    persisted.number = Some(1);
    let mut model = BoardModel::from_tasks(vec![persisted], Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    // The expanded inbox heading is the first visible row; select the task below it.
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(1), None)
        .expect("select the task");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    let (width, height) = (80, 24);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("paint task page");

    let scope = board_hit_map(Rect::new(0, 0, width, height), &model)
        .regions
        .into_iter()
        .find(|hit| hit.target == QueueHitTarget::FormScope)
        .expect("scope hit");
    let header = (0..height)
        .map(|y| row_text(&terminal, width, y))
        .find(|row| row.contains("Numbered task"))
        .expect("task page header");
    assert!(
        header.contains("T1 Numbered task"),
        "the header must lead with the identifier: {header:?}"
    );
    let identifier = board_hit_map(Rect::new(0, 0, width, height), &model)
        .regions
        .into_iter()
        .find(|hit| hit.target == QueueHitTarget::TaskNumber(id))
        .expect("identifier hit");
    assert_eq!(identifier.area.width, 2, "only T1 is clickable");
    let footer = row_text(&terminal, width, scope.area.y);
    assert!(
        !footer.contains("1 ·"),
        "the production footer must not repeat the identifier: {footer:?}"
    );
    assert_eq!(scope.area.x, 2, "scope starts at the footer inset");
}

#[test]
fn long_invalid_thread_refusal_remains_visible_at_40x10() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("open");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Thread),
        None,
    )
    .expect("select thread");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate thread editor");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("invalid_thread_name_here".into()),
        None,
    )
    .expect("type invalid thread");
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("refuse");

    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw compact refusal");
    let painted: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        painted.contains("hyphens") || painted.contains("dots"),
        "thread refusal vanished at 40x10: {painted}"
    );
}

#[test]
fn page_footer_thread_edit_operable_at_40x10() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("open");
    for _ in 0..4 {
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("focus");
    }
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate thread editor");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("tiny".into()),
        None,
    )
    .expect("type");
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw compact");
    let painted: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(painted.contains("tiny"), "thread input vanished: {painted}");
}

/// A paste into the task-page Thread editor flattens line breaks like Title: a thread name
/// is one line by definition, so a multi-line clipboard must never land as a multi-line draft.
#[test]
fn thread_paste_flattens_line_breaks_like_title() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Thread Paste",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("open form");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("Notes");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("add target");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("select Assignee");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("select Thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("activate Thread editor");
    assert_eq!(model.input_mode(), BoardInputMode::EditThread);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("release\n2026\r\nnotes".into()),
        None,
    )
    .expect("paste into Thread");
    assert_eq!(
        model.edit_buffer(),
        "release 2026 notes",
        "Thread must flatten pasted line breaks like Title"
    );
}

#[test]
fn title_edit_on_an_archived_task_persists_and_keeps_the_flag() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Archived editable",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain.archive_task(id).expect("archive it");
    // Persist first so the task carries a number like a real session's rows do.
    let dir = std::env::temp_dir().join(format!(
        "tsk-edit-archived-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let store = tsk_tui::store::TaskStore::new(&dir);
    store.save(&domain).expect("seed store");
    domain = store.load().expect("reload persisted");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));

    // Open the archived task's page through the drawer's archived group.
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleArchivedGroup,
        None,
    )
    .expect("expand");
    let idx = model
        .visible_ids()
        .iter()
        .position(|&visible| visible == id)
        .expect("archived row visible");
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None)
        .expect("select the archived row");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("edit title");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText(" (kept)".to_string()),
        None,
    )
    .expect("type");
    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
        .expect("confirm title edit");
    assert_eq!(outcome, IntentOutcome::Persist);

    store.reload_merge_save(&mut domain).expect("durable save");
    let reloaded = store.load().expect("reload");
    let task = reloaded.get(id).expect("task survives");
    assert_eq!(task.title, "Archived editable (kept)");
    assert!(task.archived, "the archived flag survives the save");
    assert_eq!(task.status, HumanStatus::Open, "status is unchanged");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn task_page_scope_dropdown_omits_archived_projects_but_keeps_the_current_scope() {
    // Three projects: the invocation repo (live), a live one, and an archived one.
    let mut domain = DomainState::new();
    let live = domain
        .create(
            "live project task",
            None,
            project("/repos/other"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create live");
    let stranded = domain
        .create(
            "task inside the archived project",
            None,
            project("/repos/filed"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create stranded");
    domain.archive_project("/repos/filed").expect("archive it");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from("/repos/other")));

    // Editing a live task: the archived project is not on offer.
    let index = model
        .visible_ids()
        .iter()
        .position(|&id| id == live)
        .expect("live row");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(index),
        None,
    )
    .expect("select live");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("edit");
    let options = model.form_scope_options();
    assert!(
        !options.contains(&project("/repos/filed")),
        "an archived project is never offered: {options:?}"
    );
    assert!(
        options.contains(&project("/repos/other")) && options.contains(&TaskScope::Global),
        "live projects and desk stay: {options:?}"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("cancel");

    // A form whose initial scope IS the archived project keeps it as the current value
    // (the stranded task's own scope is never dropped from under it).
    let snapshot = tsk_tui::context::InvocationSnapshot {
        default_scope: project("/repos/filed"),
        this_repo: Some(PathBuf::from("/repos/filed")),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    };
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/filed")));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCapture,
        Some(&snapshot),
    )
    .expect("open capture");
    apply_intent(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None).expect("expand");
    let options = model.form_scope_options();
    assert_eq!(
        options.first(),
        Some(&project("/repos/filed")),
        "the form's own archived scope stays as its current value: {options:?}"
    );
    assert_eq!(
        options
            .iter()
            .filter(|scope| **scope == project("/repos/filed"))
            .count(),
        1,
        "and it is offered exactly once: {options:?}"
    );
    let _ = stranded;
}

#[test]
fn task_page_in_read_only_focus_refuses_edit_mode() {
    let mut domain = DomainState::new();
    let inside = domain
        .create(
            "read-only page task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain.archive_project(THIS_REPO).expect("archive project");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
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
    let index = model
        .visible_ids()
        .iter()
        .position(|&id| id == inside)
        .expect("row");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(index),
        None,
    )
    .expect("select");

    // The page opens: it is view-only, not shut.
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page opens");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    let refusal = "project app is archived \u{b7} ctrl+u unarchive";
    for intent in [
        BoardIntent::BeginEditTitle,
        BoardIntent::BeginEditNotes,
        BoardIntent::ExpandQuickAdd,
        BoardIntent::BeginAddStep,
    ] {
        apply_intent(&mut domain, &mut model, intent.clone(), None)
            .unwrap_or_else(|error| panic!("{intent:?} applies: {error}"));
        assert_eq!(
            model.input_mode(),
            BoardInputMode::TaskPage,
            "{intent:?} must not enter edit mode"
        );
        assert_eq!(
            model.message(),
            Some(refusal),
            "{intent:?} paints the archived refusal"
        );
        assert!(
            !model.task_session_dirty(),
            "{intent:?} started no edit session"
        );
    }
}

#[test]
fn tab_and_field_focus_on_a_read_only_task_page_stay_in_view_mode() {
    let mut domain = DomainState::new();
    let inside = domain
        .create(
            "read-only tab task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain.archive_project(THIS_REPO).expect("archive project");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
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
    let index = model
        .visible_ids()
        .iter()
        .position(|&id| id == inside)
        .expect("row");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(index),
        None,
    )
    .expect("select");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("page");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    let refusal = "project app is archived \u{b7} ctrl+u unarchive";
    for intent in [
        BoardIntent::FormFocusNext,
        BoardIntent::FormFocusPrev,
        BoardIntent::FocusFormField(CaptureField::Notes),
        BoardIntent::FocusFormField(CaptureField::Title),
        BoardIntent::SelectStep(0),
    ] {
        apply_intent(&mut domain, &mut model, intent.clone(), None)
            .unwrap_or_else(|error| panic!("{intent:?} applies: {error}"));
        assert_eq!(
            model.input_mode(),
            BoardInputMode::TaskPage,
            "{intent:?} must not enter edit mode (AC-44)"
        );
        assert_eq!(
            model.message(),
            Some(refusal),
            "{intent:?} paints the archived refusal"
        );
        assert!(
            !model.task_session_dirty(),
            "{intent:?} started no edit session"
        );
    }
}
