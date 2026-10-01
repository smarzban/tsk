//! mouse hit-map parity for every the control.
//!
//! The hit-map comes from [`board_hit_map`], the same painter [`draw_board`] uses (via
//! `draw_queue_frame`), so these tests exercise the *real* renderer geometry rather than a
//! hand-maintained shadow of it. Each control is proved by dispatching both the keyboard
//! path and the mouse path and comparing what actually happened, never by asserting a
//! mapping table.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::layout::{Position, Rect};
use ratatui::Terminal;

use tsk_tui::app::{drag_content_area, tick_drag_autoscroll};
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::board::{
    apply_intent, board_hit_map, board_verb_items, draw_board, resolve_board_command,
    BoardInputMode, BoardModel, ProjectScopeOption,
};
use tsk_tui::ui::capture::CaptureField;
use tsk_tui::ui::input::{
    map_board_form_key, map_capture_key_state, map_capture_paste_state, map_key, BoardIntent,
    CaptureIntent, PRIMARY_CAPTURE_ACTIONS,
};
use tsk_tui::ui::mouse::{
    capture_layout, capture_mouse_paths_complete, left_click, map_board_mouse, map_capture_mouse,
    map_scrollbar_mouse, primary_capture_action_sample_mouse, scrollbar_intent_at, ScrollbarMouse,
};
use tsk_tui::ui::render::{QueueHit, QueueHitMap, QueueHitTarget};
use tsk_tui::ui::text_select::{
    self, AutoScrollDirection, DragAutoScrollState, DragSelectGesture, DragSelectPhase,
};

const THIS_REPO: &str = "/repos/app";
const OTHER_REPO: &str = "/repos/other";
const STANDARD: Rect = Rect {
    x: 0,
    y: 0,
    width: 80,
    height: 24,
};

fn project(path: &str) -> TaskScope {
    TaskScope::Project {
        path: path.to_string(),
    }
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn mapped_key(code: KeyCode) -> KeyEvent {
    match code {
        KeyCode::Char('s' | 'd' | 'o' | 'b' | 'a' | 'e' | 'x' | 'u' | 'f' | 'q')
        | KeyCode::Delete => ctrl(code),
        _ => press(code),
    }
}

fn wheel_up(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn wheel_down(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn left_drag(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn left_up(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// One task, this-repo scoped, at the given status, selected.
fn board_with_task(title: &str, status: HumanStatus) -> (DomainState, BoardModel, uuid::Uuid) {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            title,
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    if status != HumanStatus::Open {
        domain.set_status(id, status).expect("set status");
    }
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    (domain, model, id)
}

/// Two Todo tasks in different projects, so the project selector offers more than one
/// concrete project option (All, Global, this repo, the other repo).
fn scoped_board() -> (DomainState, BoardModel, uuid::Uuid) {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "here",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create here");
    domain
        .create(
            "elsewhere",
            None,
            project(OTHER_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create elsewhere");
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    (domain, model, id)
}

/// `n` Todo tasks in the same project, enough to overflow the standard 80x24 viewport (20
/// rows) and put the palette's command panel, and the scope dropdown's later options, over
/// real task rows underneath -- the deck size C1 needs to exist at all.
fn deck_of(n: usize) -> (DomainState, BoardModel) {
    let mut domain = DomainState::new();
    for i in 0..n {
        domain
            .create(
                format!("task {i}"),
                None,
                project(THIS_REPO),
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create task");
    }
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    (domain, model)
}

/// A hit region of `target_kind` whose row (`area.y`) coincides with a `Task` region's row
/// -- i.e. an overlay row genuinely painted over the base list, the only geometry C1's
/// z-order bug could ever surface on. `None` when the fixture is not actually long enough
/// to produce the overlap the caller wants to prove something about.
fn overlay_row_over_a_task(
    hits: &QueueHitMap,
    target_kind: impl Fn(&QueueHitTarget) -> bool,
) -> Option<&QueueHit> {
    hits.regions.iter().find(|hit| {
        target_kind(&hit.target)
            && hits.regions.iter().any(|other| {
                matches!(other.target, QueueHitTarget::Task(_)) && other.area.y == hit.area.y
            })
    })
}

fn verb_hit_for_chord<'a>(model: &BoardModel, hits: &'a QueueHitMap, chord: &str) -> &'a QueueHit {
    let entries = board_verb_items(model);
    let index = entries
        .iter()
        .position(|entry| entry.key == chord)
        .unwrap_or_else(|| panic!("no verb entry for chord {chord:?}: {entries:?}"));
    hits.regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::Verb(i) if i == index))
        .unwrap_or_else(|| panic!("no hit region for verb chord {chord:?} at index {index}"))
}

fn click(region: &QueueHit, model: &BoardModel, hits: &QueueHitMap) -> Option<BoardIntent> {
    map_board_mouse(model, hits, left_click(region.area.x, region.area.y))
}

#[test]
fn hit_map_covers_selection_rows_verbs_drawer_selector_chip_dropdown_palette_rows_help() {
    // Base list: selection row, verbs, selector chip (project focus only).
    let (_domain, mut model, id) = board_with_task("Fix flake", HumanStatus::Ready);
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    let hits = board_hit_map(STANDARD, &model);
    assert!(
        hits.regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::Task(t) if t == id)),
        "no selection row hit region: {hits:?}"
    );
    assert!(
        hits.regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::Verb(_))),
        "no verb hit region: {hits:?}"
    );
    assert!(
        hits.regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::NavTab(_))),
        "no navigation tab hit region: {hits:?}"
    );

    // Drawer: only painted while the done drawer is open.
    let (mut domain, mut model, _id) = board_with_task("archived", HumanStatus::Done);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("open drawer");
    let hits = board_hit_map(STANDARD, &model);
    assert!(
        hits.regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::Drawer)),
        "no drawer hit region while open: {hits:?}"
    );

    // Dropdown: the project-scope selector open.
    let (mut domain, mut model, _id) = scoped_board();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open selector");
    let hits = board_hit_map(STANDARD, &model);
    assert!(
        hits.regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::ProjectOption(_))),
        "no dropdown option hit region: {hits:?}"
    );

    // Palette rows.
    let (mut domain, mut model, _id) = board_with_task("palette", HumanStatus::Ready);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("open palette");
    let hits = board_hit_map(STANDARD, &model);
    assert!(
        hits.regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::Command(0))),
        "no palette row hit region: {hits:?}"
    );

    // Help card.
    let (mut domain, mut model, _id) = board_with_task("help", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("open help");
    let hits = board_hit_map(STANDARD, &model);
    assert!(
        hits.regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::HelpDismiss)),
        "no help dismiss hit region: {hits:?}"
    );
}

#[test]
fn projects_index_row_click_selects_and_a_double_click_opens_the_project() {
    let (mut domain, mut model, _id) = scoped_board();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::Desk),
        None,
    )
    .expect("start from the desk");
    assert_eq!(
        model.selected_project(),
        Some(Path::new("/repos/app")),
        "slot 2 retains its project while Desk is active"
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::Projects),
        None,
    )
    .expect("open projects index");
    let hits = board_hit_map(STANDARD, &model);
    let view = model.queue_view();
    let row = hits
        .regions
        .iter()
        .find(|hit| match hit.target {
            QueueHitTarget::ProjectRow(index) => view
                .projects
                .get(index)
                .is_some_and(|row| row.path == OTHER_REPO),
            _ => false,
        })
        .unwrap_or_else(|| panic!("no index row hit region for {OTHER_REPO}: {hits:?}"));
    let intent = click(row, &model, &hits).expect("row click maps to an intent");
    apply_intent(&mut domain, &mut model, intent.clone(), None).expect("apply row click");
    assert_eq!(
        model.selected_project(),
        Some(Path::new("/repos/app")),
        "a single index-row click only selects; slot 2 keeps its project"
    );
    assert_eq!(
        model.selected_project_row().map(|row| row.path),
        Some(OTHER_REPO.to_string()),
        "the clicked row is the index selection"
    );
    assert_eq!(
        model.nav_tab(),
        tsk_tui::ui::queue::NavTab::Projects,
        "the index stays open after a single click"
    );
    apply_intent(&mut domain, &mut model, intent, None).expect("apply second click");
    assert_eq!(
        model.selected_project(),
        Some(Path::new(OTHER_REPO)),
        "a second click on the same row inside the window opens it in slot 2"
    );
}

#[test]
fn scoped_project_named_all_projects_has_no_group_header_hit_target() {
    const COLLIDING_REPO: &str = "/repos/all projects";
    let mut domain = DomainState::new();
    domain
        .create(
            "name collision",
            None,
            project(COLLIDING_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(COLLIDING_REPO)));
    model.set_selected_project(Some(PathBuf::from(COLLIDING_REPO)));

    let hits = board_hit_map(STANDARD, &model);
    assert!(
        !hits
            .regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::ProjectRow(_))),
        "project focus is a task board: it must offer no index rows at all: {hits:?}"
    );
}

/// The selected task owns one standard form even below the list fold. Its field hits are
/// topmost, the dropdown click equals keyboard navigation plus Enter, and inactive board
/// chrome never escapes the modal form.
#[test]
fn task_form_mouse_fields_dropdown_and_verbs_match_keyboard_while_scrolled() {
    let mut domain = DomainState::new();
    let target = domain
        .create(
            "Selected form target",
            Some("first\nsecond".to_string()),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create selected task");
    domain
        .create(
            "other project",
            None,
            project(OTHER_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create scope option");
    for index in 0..30 {
        domain
            .create(
                format!("padding task {index}"),
                None,
                project(THIS_REPO),
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create padding task");
    }
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let target_index = model
        .visible_ids()
        .iter()
        .position(|id| *id == target)
        .expect("target visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(target_index),
        None,
    )
    .expect("select target");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open task form");

    let mut hits = board_hit_map(STANDARD, &model);
    let intent_for = |target| {
        let hit = hits
            .regions
            .iter()
            .find(|hit| hit.target == target)
            .unwrap_or_else(|| panic!("missing task-form hit region for {target:?}"));
        click(hit, &model, &hits)
    };
    // The Title bar is `enter next · shift+enter save · esc cancel`: each seat matches
    // the key it names.
    assert_eq!(
        intent_for(QueueHitTarget::Verb(0)),
        map_board_form_key(
            CaptureField::Title,
            false,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ),
        "task-form `enter next` verb must match Title's Enter route"
    );
    assert_eq!(
        intent_for(QueueHitTarget::Verb(0)),
        Some(BoardIntent::FormFocusNext)
    );
    assert_eq!(
        intent_for(QueueHitTarget::Verb(1)),
        map_board_form_key(
            CaptureField::Title,
            false,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        ),
        "task-form Save verb must match Title's Shift+Enter route"
    );
    assert_eq!(
        intent_for(QueueHitTarget::Verb(2)),
        map_board_form_key(
            CaptureField::Title,
            false,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        ),
        "task-form Cancel verb must match Esc"
    );

    // This form was opened with `BeginEditTitle`, so edit mode is ALREADY open: a field click
    // must move focus. The
    // separate view-state rule -- that a click must not ENTER edit mode -- is exercised in
    // `page_field_clicks_activate_after_task_editing_starts`, which asserts
    // `input_mode() == TaskPage` first. Conflating the two is what previously let this test
    // assert inertness while sitting in edit mode.
    for (target, field) in [
        (QueueHitTarget::FormTitle, CaptureField::Title),
        (QueueHitTarget::FormNotes(0), CaptureField::Notes),
        (QueueHitTarget::FormNotes(1), CaptureField::Notes),
        (QueueHitTarget::FormThread, CaptureField::Thread),
    ] {
        let hit = hits
            .regions
            .iter()
            .find(|hit| hit.target == target)
            .unwrap_or_else(|| panic!("missing task-form field hit {target:?}"));
        assert_eq!(
            click(hit, &model, &hits),
            Some(BoardIntent::FocusFormField(field)),
            "clicking task-form field {target:?} in edit mode must focus it"
        );
    }

    let scope_hit = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormScope)
        .expect("task-form scope hit");
    let open = click(scope_hit, &model, &hits).expect("scope click intent");
    assert_eq!(open, BoardIntent::OpenFormDropdown(CaptureField::Scope));
    apply_intent(&mut domain, &mut model, open, None).expect("open form dropdown");
    assert_eq!(model.input_mode(), BoardInputMode::FormDropdown);
    assert_eq!(
        map_board_mouse(&model, &board_hit_map(STANDARD, &model), left_click(0, 0)),
        None,
        "base controls remain inert while the dropdown owns input"
    );

    let mut keyboard_domain = domain.clone();
    let mut keyboard_model = model.clone();
    while keyboard_model.form_scope_dropdown_choice() != Some(&TaskScope::Global) {
        let next = map_board_form_key(
            CaptureField::Scope,
            true,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        )
        .expect("Down selects the next scope option");
        apply_intent(&mut keyboard_domain, &mut keyboard_model, next, None)
            .expect("move keyboard choice");
    }
    let confirm = map_board_form_key(
        CaptureField::Scope,
        true,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    )
    .expect("Enter confirms the highlighted scope");
    apply_intent(&mut keyboard_domain, &mut keyboard_model, confirm, None)
        .expect("confirm keyboard scope");

    let global_index = model
        .form_scope_options()
        .iter()
        .position(|scope| *scope == TaskScope::Global)
        .expect("Global form option");
    hits = board_hit_map(STANDARD, &model);
    let option = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormDropdownOption(global_index))
        .expect("Global dropdown option is painted above the scrolled task list");
    let choose = click(option, &model, &hits).expect("dropdown option click");
    assert_eq!(choose, BoardIntent::SelectFormDropdownOption(global_index));
    apply_intent(&mut domain, &mut model, choose, None).expect("choose form scope");
    assert_eq!(model.form_scope(), keyboard_model.form_scope());
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
    assert_eq!(
        domain.get(target).expect("target still exists").scope,
        project(THIS_REPO),
        "dropdown selection changes only the draft before Save"
    );

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Scope),
        None,
    )
    .expect("reopen dropdown");
    let esc = map_board_form_key(
        CaptureField::Scope,
        true,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    )
    .expect("Esc cancels only the dropdown");
    apply_intent(&mut domain, &mut model, esc, None).expect("cancel dropdown");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
    assert_eq!(model.form_scope(), Some(&TaskScope::Global));
}

/// One verb-bar chord: dispatching the keyboard's key and clicking the matching hit region
/// must resolve to the same `BoardIntent`, and applying each to its own independent, fresh
/// board must land on the same domain/model effect.
fn assert_verb_parity(title: &str, status: HumanStatus, chord: &str, key: KeyCode) {
    let (mut domain_key, mut model_key, id_key) = board_with_task(title, status);
    let (mut domain_mouse, mut model_mouse, id_mouse) = board_with_task(title, status);
    // A Done task lives in the closed done drawer and is not selected by default; open it
    // and re-select the task if the reanchor moved off it, so the verb bar shows the
    // reopen chord for a real selection (mirrors `space_on_done_reopens` in
    // tests/queue_board_verbs.rs).
    if status == HumanStatus::Done {
        for (domain, model, id) in [
            (&mut domain_key, &mut model_key, id_key),
            (&mut domain_mouse, &mut model_mouse, id_mouse),
        ] {
            apply_intent(domain, model, BoardIntent::ToggleDoneDrawer, None).expect("open drawer");
            if model.selected_id() != Some(id) {
                let idx = model
                    .visible_ids()
                    .iter()
                    .position(|&row| row == id)
                    .expect("done row visible with drawer open");
                apply_intent(domain, model, BoardIntent::SelectIndex(idx), None)
                    .expect("select done");
            }
        }
    }

    let keyboard_intent = map_key(BoardInputMode::Normal, mapped_key(key))
        .unwrap_or_else(|| panic!("no key for {chord}"));
    apply_intent(
        &mut domain_key,
        &mut model_key,
        keyboard_intent.clone(),
        None,
    )
    .expect("keyboard apply");

    let hits = board_hit_map(STANDARD, &model_mouse);
    let region = verb_hit_for_chord(&model_mouse, &hits, chord);
    let mouse_intent =
        click(region, &model_mouse, &hits).unwrap_or_else(|| panic!("no mouse intent for {chord}"));
    assert_eq!(
        mouse_intent, keyboard_intent,
        "chord {chord:?}: mouse and key must agree"
    );
    apply_intent(&mut domain_mouse, &mut model_mouse, mouse_intent, None).expect("mouse apply");

    assert_eq!(
        domain_key.get(id_key).unwrap().status,
        domain_mouse.get(id_mouse).unwrap().status,
        "chord {chord:?}: status diverged between the two routes"
    );
    assert_eq!(
        model_key.detail_open().is_some(),
        model_mouse.detail_open().is_some(),
        "chord {chord:?}: detail-open diverged between the two routes"
    );
    assert_eq!(
        model_key.command_surface(),
        model_mouse.command_surface(),
        "chord {chord:?}: command surface diverged between the two routes"
    );
    assert_eq!(
        model_key.input_mode(),
        model_mouse.input_mode(),
        "chord {chord:?}: input mode diverged between the two routes"
    );
}

#[test]
fn dispatch_chip_clicks_route_to_dispatch_on_board_and_task_page() {
    let (mut domain, mut model, id) = board_with_task("send it", HumanStatus::Ready);
    domain
        .assign(id, Some("implementer".into()))
        .expect("assign task");
    model.sync_from_domain(&domain);

    for mode in [BoardInputMode::Normal, BoardInputMode::TaskPage] {
        if mode == BoardInputMode::TaskPage {
            apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
                .expect("open task page");
        }
        assert_eq!(model.input_mode(), mode);
        let hits = board_hit_map(STANDARD, &model);
        let dispatch = verb_hit_for_chord(&model, &hits, "g");
        assert_eq!(
            click(dispatch, &model, &hits),
            Some(BoardIntent::Dispatch),
            "dispatch chip must match ctrl+g in {mode:?}"
        );
    }
}

#[test]
fn click_and_wheel_match_keyboard_effects_for_each_control() {
    // Verb bar: every chord a ready task shows, plus a block chord from IN MOTION and the
    // Done-only inbox chord.
    assert_verb_parity("s", HumanStatus::Ready, "s", KeyCode::Char('s'));
    assert_verb_parity("enter", HumanStatus::Ready, "enter", KeyCode::Enter);
    assert_verb_parity("d", HumanStatus::Ready, "d", KeyCode::Char('d'));
    assert_verb_parity("b", HumanStatus::Started, "b", KeyCode::Char('b'));
    assert_verb_parity("question", HumanStatus::Ready, "?", KeyCode::Char('?'));
    assert_verb_parity("add", HumanStatus::Ready, "+", KeyCode::Char('+'));
    // Archive has no bar seat: it lives in `?` / `:` and on ctrl+f.
    assert_verb_parity("unblock", HumanStatus::Blocked, "b", KeyCode::Char('b'));
    assert_verb_parity("inbox", HumanStatus::Done, "o", KeyCode::Char('o'));

    // Drawer toggle: open it by keyboard on both boards first (a shared start state), then
    // close it by keyboard on one and by clicking the DONE header on the other.
    let (mut domain_key, mut model_key, _id) = board_with_task("drawer key", HumanStatus::Done);
    let (mut domain_mouse, mut model_mouse, _id2) =
        board_with_task("drawer mouse", HumanStatus::Done);
    for (domain, model) in [
        (&mut domain_key, &mut model_key),
        (&mut domain_mouse, &mut model_mouse),
    ] {
        apply_intent(domain, model, BoardIntent::ToggleDoneDrawer, None).expect("open drawer");
        assert!(model.drawer_open());
    }
    let keyboard_intent =
        map_key(BoardInputMode::Normal, press(KeyCode::Char('d'))).expect("d key");
    apply_intent(
        &mut domain_key,
        &mut model_key,
        keyboard_intent.clone(),
        None,
    )
    .expect("keyboard close drawer");
    assert!(!model_key.drawer_open());

    let hits = board_hit_map(STANDARD, &model_mouse);
    let drawer_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::Drawer))
        .expect("drawer hit region");
    let mouse_intent = click(drawer_hit, &model_mouse, &hits).expect("drawer click intent");
    assert_eq!(mouse_intent, keyboard_intent);
    apply_intent(&mut domain_mouse, &mut model_mouse, mouse_intent, None)
        .expect("mouse close drawer");
    assert!(!model_mouse.drawer_open());

    // Wheel scrolls the list viewport; it does not move selection (j/k still do).
    let (mut domain_mouse, mut model_mouse) = deck_of(40);
    let selected = model_mouse.selected_id();
    let _ = page_rows(&model_mouse);
    let hits = board_hit_map(STANDARD, &model_mouse);
    let mouse_next = map_board_mouse(&model_mouse, &hits, wheel_down(0, 0)).expect("wheel down");
    assert_eq!(mouse_next, BoardIntent::ListScrollTo(1));
    apply_intent(&mut domain_mouse, &mut model_mouse, mouse_next, None).expect("wheel scroll");
    assert_eq!(model_mouse.selected_id(), selected);
    assert_eq!(model_mouse.list_scroll(), 1);
    let hits = board_hit_map(STANDARD, &model_mouse);
    assert_eq!(
        map_board_mouse(&model_mouse, &hits, wheel_up(0, 0)),
        Some(BoardIntent::ListScrollTo(0))
    );

    // Project selector chip: no the key binds it (mouse-only, like `SelectIndex`), so its
    // effect is proved against the same `OpenProjectSelector` the reducer would apply for
    // any future key bound to it, on an independently built, otherwise-identical board.
    let (mut domain_direct, mut model_direct, _id3) = board_with_task("chip a", HumanStatus::Ready);
    let (mut domain_mouse, mut model_mouse, _id4) = board_with_task("chip b", HumanStatus::Ready);
    model_direct.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    model_mouse.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    apply_intent(
        &mut domain_direct,
        &mut model_direct,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("direct open");
    let hits = board_hit_map(STANDARD, &model_mouse);
    let tab_hit = hits
        .regions
        .iter()
        .find(|hit| {
            matches!(
                hit.target,
                QueueHitTarget::NavTab(tsk_tui::ui::queue::NavTab::ProjectBoard)
            )
        })
        .expect("slot-2 tab hit region");
    let mouse_intent = click(tab_hit, &model_mouse, &hits).expect("tab click intent");
    assert_eq!(
        mouse_intent,
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::ProjectBoard),
        "slot 2's click routes through the tab intent; the reducer opens the picker"
    );
    apply_intent(&mut domain_mouse, &mut model_mouse, mouse_intent, None).expect("mouse open");
    assert_eq!(
        model_direct.project_picker_index(),
        model_mouse.project_picker_index()
    );
    assert_eq!(model_direct.message(), model_mouse.message());

    // Dropdown option: a single click on a non-selected row must land exactly where
    // stepping the keyboard's `ProjectPickerNext` there and pressing Enter would.
    let (mut domain_key, mut model_key, _id5) = scoped_board();
    let (mut domain_mouse, mut model_mouse, _id6) = scoped_board();
    for (domain, model) in [
        (&mut domain_key, &mut model_key),
        (&mut domain_mouse, &mut model_mouse),
    ] {
        apply_intent(domain, model, BoardIntent::OpenProjectSelector, None).expect("open selector");
    }
    let target_index = model_key
        .project_options()
        .iter()
        .position(|option| {
            *option == tsk_tui::ui::board::ProjectScopeOption::Project(PathBuf::from(OTHER_REPO))
        })
        .expect("the other repo is an offered option");
    assert!(
        target_index > 0,
        "the test needs a non-selected option to jump to"
    );
    let next_key = map_key(BoardInputMode::ProjectPicker, press(KeyCode::Down))
        .expect("project picker down key");
    // The picker opens highlighting the current destination; step from there.
    let start = model_key
        .project_picker_index()
        .expect("highlighted option");
    let options_len = model_key.project_options().len();
    let steps = (target_index + options_len - start) % options_len;
    for _ in 0..steps {
        apply_intent(&mut domain_key, &mut model_key, next_key.clone(), None)
            .expect("step to option");
    }
    let confirm_key = map_key(BoardInputMode::ProjectPicker, press(KeyCode::Enter))
        .expect("project picker enter key");
    apply_intent(&mut domain_key, &mut model_key, confirm_key, None).expect("confirm choice");

    let hits = board_hit_map(STANDARD, &model_mouse);
    let option_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ProjectOption(i) if i == target_index))
        .expect("dropdown option hit region");
    let mouse_intent = click(option_hit, &model_mouse, &hits).expect("dropdown click intent");
    assert_eq!(mouse_intent, BoardIntent::SelectProjectOption(target_index));
    apply_intent(&mut domain_mouse, &mut model_mouse, mouse_intent, None).expect("mouse choice");

    assert!(
        model_key.project_picker_index().is_none(),
        "keyboard route must close the picker"
    );
    assert!(
        model_mouse.project_picker_index().is_none(),
        "mouse route must close the picker"
    );
    assert_eq!(
        model_key.message(),
        model_mouse.message(),
        "the two routes must scope the deck to the same project"
    );
    // Each board created its own tasks (different uuids), so compare the *shape* of what
    // is now visible -- one ON DECK task, the one scoped to the chosen project -- rather
    // than exact ids.
    // The selected project contains one open task plus its inbox heading.
    assert_eq!(model_key.visible_ids().len(), 2);
    assert_eq!(model_mouse.visible_ids().len(), 2);
}

/// non-regression: the standalone quick-capture popup's mouse paths are untouched by
/// this rewrite (its `CaptureLayout`/`map_capture_mouse` route is separate from the board's
/// hit-map, per the task's implementation boundary).
#[test]
fn footer_assignee_then_thread_then_scope_each_focuses_its_field() {
    let scope_path = "/repos/foo · thread";
    let mut domain = DomainState::new();
    domain
        .create(
            "Task",
            None,
            project(scope_path),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(scope_path)));
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open task form");

    let hits = board_hit_map(STANDARD, &model);
    let scope_hit = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormScope)
        .expect("scope hit");
    let assignee_hit = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormAssignee)
        .expect("empty assignee target");
    let thread_hit = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormThread)
        .expect("empty thread target");
    let expected_scope_width = "foo · thread".chars().count() as u16;
    assert_eq!(
        scope_hit.area.width, expected_scope_width,
        "the entire project basename remains the scope target, excluding number chrome"
    );
    assert_eq!(
        assignee_hit.area.x, 2,
        "the leading assignee target starts at the footer inset"
    );
    assert_eq!(
        thread_hit.area.x,
        2 + assignee_hit.area.width + 3,
        "thread follows assignee and its separator"
    );
    assert_eq!(
        scope_hit.area.x,
        thread_hit.area.x + thread_hit.area.width + 3,
        "scope follows the thread and its separator"
    );
    assert_eq!(
        scope_hit.area.width, expected_scope_width,
        "the entire project basename remains the scope target"
    );

    let assignee = click(assignee_hit, &model, &hits).expect("assignee click intent");
    assert_eq!(
        assignee,
        BoardIntent::OpenFormDropdown(CaptureField::Assignee)
    );
    apply_intent(&mut domain, &mut model, assignee, None).expect("open assignee dropdown");
    assert_eq!(model.form_focus(), Some(CaptureField::Assignee));
    assert_eq!(model.input_mode(), BoardInputMode::FormDropdown);

    let assignee_hits = board_hit_map(STANDARD, &model);
    let assignee_option = assignee_hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormDropdownOption(0))
        .expect("none option");
    assert_eq!(
        assignee_option.area.x, assignee_hit.area.x,
        "assignee dropdown must anchor at the painted assignee field"
    );
    assert_eq!(
        click(assignee_option, &model, &assignee_hits),
        Some(BoardIntent::SelectFormDropdownOption(0)),
        "clicking a dropdown entry picks that assignee"
    );

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CancelFormDropdown,
        None,
    )
    .expect("close assignee dropdown");
    let thread = click(thread_hit, &model, &hits).expect("thread click intent");
    assert_eq!(thread, BoardIntent::FocusFormField(CaptureField::Thread));
    apply_intent(&mut domain, &mut model, thread, None).expect("focus thread");
    assert_eq!(model.form_focus(), Some(CaptureField::Thread));

    let scope = click(scope_hit, &model, &hits).expect("scope click intent");
    assert_eq!(scope, BoardIntent::OpenFormDropdown(CaptureField::Scope));
    apply_intent(&mut domain, &mut model, scope, None).expect("focus scope");
    assert_eq!(model.form_focus(), Some(CaptureField::Scope));
    let scope_hits = board_hit_map(STANDARD, &model);
    let scope_option = scope_hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormDropdownOption(0))
        .expect("scope option");
    assert_eq!(
        scope_option.area.x, scope_hit.area.x,
        "scope dropdown must anchor at the painted scope field"
    );
}

#[test]
fn capture_popup_mouse_paths_unchanged() {
    let layout = capture_layout(Rect::new(0, 0, 80, 16));
    assert!(
        capture_mouse_paths_complete(&layout),
        "every primary capture action must still have a resolvable mouse path"
    );
    for &action in PRIMARY_CAPTURE_ACTIONS {
        let mouse = primary_capture_action_sample_mouse(action, &layout);
        assert!(
            map_capture_mouse(&layout, mouse).is_some(),
            "{action:?}: capture mouse route regressed"
        );
    }

    // The narrow capture width still keeps every control clickable.
    let narrow = capture_layout(Rect::new(0, 0, 40, 16));
    assert!(capture_mouse_paths_complete(&narrow));
}

#[test]
fn capture_thread_row_click_and_input_paths_focus_and_edit_the_thread_field() {
    let layout = capture_layout(Rect::new(0, 0, 80, 16));
    assert_eq!(
        map_capture_mouse(
            &layout,
            left_click(layout.thread_area.x.saturating_add(1), layout.thread_area.y),
        ),
        Some(CaptureIntent::FocusField(CaptureField::Thread)),
        "the visible capture Thread row must take mouse focus"
    );
    assert_eq!(
        map_capture_key_state(
            CaptureField::Thread,
            false,
            false,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        ),
        Some(CaptureIntent::Insert('r')),
        "a focused Thread field accepts keyboard input"
    );
    assert_eq!(
        map_capture_paste_state(CaptureField::Thread, false, "release"),
        Some(CaptureIntent::InsertText("release".into())),
        "a focused Thread field accepts paste"
    );
}

/// C1: on a deck long enough for the palette's command panel to actually
/// overlap the base list underneath it, a click on a command row must run that command --
/// not fall through to whatever `Task` region the hit-map's old paint-order search found
/// first, which was always `CloseCommandSurface`'s territory once the overlap existed.
/// Every prior parity fixture was 1-2 tasks, too short to ever reach this geometry, which
/// is why the palette's own regression net never caught it. Confirmed to fail before the
/// `hit_at` z-order fix (reverse search, topmost wins): with paint-order search, this
/// clicked a `Task` row and dispatched `SelectIndex`, never the command.
#[test]
fn a_palette_row_over_a_full_deck_still_runs_its_own_command_not_the_task_row_under_it() {
    let (mut domain, mut model) = deck_of(20);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("open palette");
    let hits = board_hit_map(STANDARD, &model);
    let command_hit = overlay_row_over_a_task(&hits, |t| matches!(t, QueueHitTarget::Command(_)))
        .unwrap_or_else(|| {
            panic!("fixture is not long enough to overlap a command row with a task row: {hits:?}")
        });
    let QueueHitTarget::Command(index) = command_hit.target else {
        unreachable!()
    };
    let expected = model
        .visible_commands()
        .get(index)
        .expect("command index in range")
        .intent
        .clone();
    assert_ne!(
        expected,
        BoardIntent::CloseCommandSurface,
        "the overlapping row must be a real command, not the dismiss case itself"
    );
    // a command row click is `SelectCommand(index)`, not the row's own intent directly
    // (the same mouse-only shape as `SelectIndex`/`SelectProjectOption`); resolve it on a
    // clone to check it still names this exact command without perturbing `model`.
    let clicked = click(command_hit, &model, &hits).expect("mouse command intent");
    let mut resolved_model = model.clone();
    assert_eq!(
        resolve_board_command(&mut resolved_model, clicked),
        Some(expected)
    );
}

/// C1: the scope dropdown's own regression net. `scoped_board` offers four
/// project options starting at the same row the task list starts on, so later options sit
/// directly over real task rows; a click on one of those rows must select that option, not
/// cancel the picker the way the region underneath it used to win. Confirmed to fail before
/// the `hit_at` fix the same way the palette case does.
#[test]
fn a_dropdown_option_over_a_task_row_still_selects_that_option_not_the_task_under_it() {
    let (mut domain, mut model, _id) = scoped_board();
    domain
        .create(
            "third repo",
            None,
            project("/repos/third"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create third project for a longer selector");
    // The project board lists only its own rows; fill it deep enough that the picker's
    // later options sit over real task rows.
    for i in 0..12 {
        domain
            .create(
                format!("filler {i}"),
                None,
                project(THIS_REPO),
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create filler rows");
    }
    model.sync_from_domain(&domain);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open selector");
    let hits = board_hit_map(STANDARD, &model);
    let option_hit =
        overlay_row_over_a_task(&hits, |t| matches!(t, QueueHitTarget::ProjectOption(_)))
            .unwrap_or_else(|| {
                panic!(
                    "fixture is not long enough to overlap an option row with a task row: {hits:?}"
                )
            });
    let QueueHitTarget::ProjectOption(index) = option_hit.target else {
        unreachable!()
    };
    assert_eq!(
        click(option_hit, &model, &hits),
        Some(BoardIntent::SelectProjectOption(index))
    );
}

/// `ViewBoard` is the one view-segment click that actually dispatches
/// something (`ViewQueue` is the already-active no-op the parity test above covers). Prove
/// the segment that does something, not just the one that does not.
///
/// `v` is the keyboard's identical route to the same intent, so the mouse and
/// keyboard paths are compared here rather than asserted separately.
#[test]
fn click_a_task_row_selects_its_index_on_a_scrolled_list() {
    let (mut domain, mut model) = deck_of(40);
    let visible = model.visible_ids();
    let last = visible.len() - 1;
    let neighbor = last - 1;

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(last),
        None,
    )
    .expect("select last task");
    apply_intent(&mut domain, &mut model, BoardIntent::PeekDetail, None)
        .expect("open peek on the last task");
    assert_eq!(model.detail_open(), Some(visible[last]));

    let hits = board_hit_map(STANDARD, &model);
    let geo = tsk_tui::ui::tier::resolve(STANDARD.width, STANDARD.height);
    assert!(
        hits.regions
            .iter()
            .all(|hit| !matches!(hit.target, QueueHitTarget::Task(id) if id == visible[1])),
        "the deck must actually be long enough to scroll task 0 out of the viewport: {hits:?} \
         (geo={geo:?})"
    );

    let region = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::Task(id) if id == visible[neighbor]))
        .expect("the task just above the open accordion must still carry its own hit region");
    assert_eq!(
        click(region, &model, &hits),
        Some(BoardIntent::SelectIndex(neighbor)),
        "a click on a scrolled-into-view row must resolve to its own index, not the \
         unscrolled row position"
    );
}

#[test]
fn plain_click_marks_only_in_mark_mode_and_ctrl_click_keeps_ordinary_behavior() {
    let (mut domain, mut model) = deck_of(4);
    let target = *model.visible_ids().last().expect("visible task");
    let hits = board_hit_map(STANDARD, &model);
    let region = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::Task(id) if id == target))
        .expect("target row hit");
    let index = model
        .visible_ids()
        .iter()
        .position(|&id| id == target)
        .expect("target index");
    let row_mouse = |modifiers| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: region.area.x,
        row: region.area.y,
        modifiers,
    };
    assert_eq!(
        map_board_mouse(&model, &hits, row_mouse(KeyModifiers::NONE)),
        Some(BoardIntent::SelectIndex(index))
    );
    assert_eq!(
        map_board_mouse(&model, &hits, row_mouse(KeyModifiers::CONTROL)),
        Some(BoardIntent::SelectIndex(index)),
        "ctrl+click no longer has marking behavior"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
        .expect("enter mark mode");
    assert_eq!(
        map_board_mouse(&model, &hits, row_mouse(KeyModifiers::NONE)),
        Some(BoardIntent::MarkToggleAt(index))
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::MarkToggleAt(index),
        None,
    )
    .expect("mark clicked row");
    assert_eq!(model.selected_id(), Some(target));
    assert!(model.marked_ids().contains(&target));
    assert_eq!(
        map_board_mouse(&model, &hits, row_mouse(KeyModifiers::CONTROL)),
        Some(BoardIntent::SelectIndex(index)),
        "ctrl+click stays an ordinary row click in mark mode"
    );

    let number_hits = QueueHitMap {
        regions: vec![QueueHit {
            target: QueueHitTarget::TaskNumber(target),
            area: Rect::new(1, 1, 1, 1),
        }],
        ..QueueHitMap::default()
    };
    let number_mouse = |modifiers| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 1,
        row: 1,
        modifiers,
    };
    assert_eq!(
        map_board_mouse(&model, &number_hits, number_mouse(KeyModifiers::NONE)),
        Some(BoardIntent::MarkToggleAt(index))
    );
    assert_eq!(
        map_board_mouse(&model, &number_hits, number_mouse(KeyModifiers::CONTROL)),
        Some(BoardIntent::CopyTaskNumber(target))
    );
}

/// G-2 (gate round 1, PR #11): the follow-selection scroll above only proves the click's
/// row->index mapping once the *accordion's* anchor has forced a scroll. This is the
/// keyboard-navigation form of the same defect the gate named: with nothing open at all
/// (no capture, no accordion), stepping the plain selection past the fold with `j`
/// (`SelectNext`) must keep it painted at every step -- the mutating verbs (`space`/`d`/`x`)
/// act on whatever is selected, so an off-screen selection would silently mutate a row the
/// user cannot see.
#[test]
fn stepping_selection_past_the_fold_with_select_next_keeps_the_selected_row_painted() {
    let (mut domain, mut model) = deck_of(40);
    let visible = model.visible_ids();
    let last = visible.len() - 1;

    // The inbox heading is visible[0], and the model initially selects task 0 at visible[1].
    for step in 0..last.saturating_sub(1) {
        apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None).expect("select next");
        let selected = model
            .selected_id()
            .expect("a task stays selected while stepping through the deck");
        let hits = board_hit_map(STANDARD, &model);
        assert!(
            hits.regions
                .iter()
                .any(|hit| matches!(hit.target, QueueHitTarget::Task(id) if id == selected)),
            "step {step}: selected task {selected} must still carry a hit region, i.e. still \
             be painted, with nothing open: {hits:?}"
        );
    }
    assert_eq!(
        model.selected_id(),
        Some(visible[last]),
        "SelectNext must have walked all the way to the last task"
    );
}

/// a click on the palette's own chrome -- the `command` header row or
/// the `:` query row -- must not dismiss it. The old code returned `None` there; this
/// rewrite's `_ => Some(CloseCommandSurface)` fallthrough turned that into a dismissal
/// (and, combined with C1, a destructive one on any overlapping deck). Bound the
/// fallthrough: only a click genuinely outside the whole surface closes it.
#[test]
fn a_click_on_the_palettes_own_chrome_does_not_dismiss_it() {
    let (mut domain, mut model, _id) = board_with_task("palette chrome", HumanStatus::Ready);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("open palette");
    let hits = board_hit_map(STANDARD, &model);
    let chrome_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::CommandChrome))
        .expect("the palette must paint chrome (header/query row) outside its command rows");
    assert_eq!(
        click(chrome_hit, &model, &hits),
        None,
        "a click on the palette's own chrome must be inert, not a dismissal"
    );
    // A click genuinely outside the whole surface still closes it -- the far corner of the
    // frame paints no palette chrome at all on a short command list.
    assert_eq!(
        map_board_mouse(
            &model,
            &hits,
            left_click(STANDARD.width - 1, STANDARD.height - 1)
        ),
        Some(BoardIntent::CloseCommandSurface)
    );
}

/// The shared modal card's own `[x]` closes/cancels whichever surface painted it, its
/// border+footer chrome is inert (not a second, redundant dismiss route on top of the
/// palette's `CommandChrome`/project picker's outside-click fallthrough already proved
/// above), and a click on the board behind the card still dismisses -- for all three
/// surfaces the card now shares (Palette, `?` help, `P` project picker).
#[test]
fn the_modal_cards_close_control_and_chrome_behave_the_same_on_palette_help_and_project_picker() {
    // Palette: `[x]` closes the command surface; the card's own border is inert.
    let (mut domain, mut model, _id) = board_with_task("modal card palette", HumanStatus::Ready);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("open palette");
    let hits = board_hit_map(STANDARD, &model);
    let close_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ModalClose))
        .expect("the palette card must paint an `[x]` close control");
    assert_eq!(
        click(close_hit, &model, &hits),
        Some(BoardIntent::CloseCommandSurface)
    );
    let chrome_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ModalChrome))
        .expect("the palette card must paint its own border/footer chrome");
    assert_eq!(
        click(chrome_hit, &model, &hits),
        None,
        "a click on the card's border/footer must be inert"
    );

    // Project picker: `[x]` cancels the picker; the card's own border is inert.
    let (mut domain, mut model, _id) = board_with_task("modal card scope", HumanStatus::Ready);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open project selector");
    let hits = board_hit_map(STANDARD, &model);
    let close_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ModalClose))
        .expect("the project picker card must paint an `[x]` close control");
    assert_eq!(
        click(close_hit, &model, &hits),
        Some(BoardIntent::CancelProjectPicker)
    );
    let chrome_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ModalChrome))
        .expect("the project picker card must paint its own border/footer chrome");
    assert_eq!(
        click(chrome_hit, &model, &hits),
        None,
        "a click on the card's border/footer must be inert"
    );

    // Help: `[x]` closes the layer, while its border, focused search field, and binding
    // rows are inert. Only an explicit close or a click outside dismisses the searchable card.
    let (mut domain, mut model, _id) = board_with_task("modal card help", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("open help");
    let hits = board_hit_map(STANDARD, &model);
    let close_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ModalClose))
        .expect("the help card must paint an `[x]` close control");
    assert_eq!(
        click(close_hit, &model, &hits),
        Some(BoardIntent::CloseHelp)
    );
    let mut queried = model.clone();
    apply_intent(
        &mut domain,
        &mut queried,
        BoardIntent::HelpQueryInsertText("done".to_string()),
        None,
    )
    .expect("type help query");
    apply_intent(&mut domain, &mut queried, BoardIntent::CloseHelp, None)
        .expect("mouse close help");
    assert_eq!(
        queried.input_mode(),
        BoardInputMode::Normal,
        "mouse dismissal closes immediately instead of only clearing the query"
    );
    let chrome_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ModalChrome))
        .expect("the help card must paint its own border/footer chrome");
    assert_eq!(
        click(chrome_hit, &model, &hits),
        None,
        "a click on the card's border/footer must be inert"
    );
    let card = chrome_hit.area;
    let body = hits
        .copyable
        .iter()
        .find(|rect| {
            rect.y > card.y
                && rect.y < card.y.saturating_add(card.height).saturating_sub(1)
                && rect.x >= card.x
                && rect.x.saturating_add(rect.width) <= card.x.saturating_add(card.width)
        })
        .expect("help has a binding row");
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(body.x, body.y)),
        None,
        "clicking searchable help content must not dismiss it"
    );

    // A click on the board behind the help card still dismisses it -- the far corner of
    // the frame paints no card chrome at all. (Palette's own far-corner dismissal is
    // already proved by `a_click_on_the_palettes_own_chrome_does_not_dismiss_it` above.)
    let outside = left_click(STANDARD.width - 1, STANDARD.height - 1);
    assert_eq!(
        map_board_mouse(&model, &hits, outside),
        Some(BoardIntent::CloseHelp)
    );
}

/// R-2: the standard-tier command surface windows to 6
/// rows at 80x24 while 14 commands exist, and the painted `▲▼` marker is inert
/// `CommandChrome`, so a mouse-only user could not reach the 7 commands outside the
/// initial window (`quit`, the last one, among them). The wheel now moves the command
/// selection the same `CommandNext`/`CommandPrev` the keyboard's `j`/`k` dispatch, which
/// scrolls the painted window because it derives `scroll` from the selected index
/// (`paint_palette_overlay`) -- no new scroll state, no new hit target. Confirmed to fail
/// before the fix: `wheel_board_intent` returned `None` for any non-`Normal` mode, so the
/// wheel did nothing over an open palette.
#[test]
fn wheel_scrolls_the_open_command_surface_so_every_command_becomes_reachable() {
    let (mut domain, mut model, _id) = board_with_task("palette wheel", HumanStatus::Ready);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("open palette");

    let commands = model.visible_commands();
    assert_eq!(
        commands.len(),
        14,
        "this ready fixture must expose every palette command a ready selection has: {commands:?}"
    );
    let last = commands.len() - 1;
    assert_eq!(commands[last].label, "quit");
    assert_eq!(model.command_selected(), Some(0));

    // The initial 6-row window does not paint `quit`'s row at all: no hit region for it.
    let hits = board_hit_map(STANDARD, &model);
    assert!(
        !hits
            .regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::Command(i) if i == last)),
        "quit must start outside the painted window: {hits:?}"
    );

    // Drive ScrollDown through the real `map_board_mouse` path, one step at a time (the
    // same path the app's own event loop uses), until the selection reaches the last
    // command.
    let mut steps = 0;
    while model.command_selected() != Some(last) {
        let hits = board_hit_map(STANDARD, &model);
        let intent = map_board_mouse(&model, &hits, wheel_down(0, 0)).unwrap_or_else(|| {
            panic!(
                "wheel must keep advancing the selection (step {steps}, at {:?})",
                model.command_selected()
            )
        });
        apply_intent(&mut domain, &mut model, intent, None).expect("apply wheel step");
        steps += 1;
        assert!(
            steps <= last,
            "wheel must reach the last command within {last} steps"
        );
    }

    // Now reachable: the repainted window carries a hit region for `quit`, and the wheel
    // clamps rather than wraps past the last command (the signed-off Normal-mode wheel
    // convention at `wheel_board_intent`, `:1474-1476`, kept consistent here).
    let hits = board_hit_map(STANDARD, &model);
    let quit_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::Command(i) if i == last))
        .unwrap_or_else(|| {
            panic!("quit must be reachable once the wheel has scrolled to it: {hits:?}")
        });
    // the click is `SelectCommand(last)`, not `Quit` directly; resolve on a clone so
    // this check does not itself close the surface used by the wheel-clamp check below.
    let quit_clicked = click(quit_hit, &model, &hits).expect("mouse command intent");
    let mut resolved_model = model.clone();
    assert_eq!(
        resolve_board_command(&mut resolved_model, quit_clicked),
        Some(BoardIntent::Quit),
        "clicking the reachable `quit` region must resolve to BoardIntent::Quit"
    );
    assert_eq!(
        map_board_mouse(&model, &hits, wheel_down(0, 0)),
        None,
        "the wheel must clamp at the last command, not wrap to the first"
    );

    // And clamps at the other end too: drive ScrollUp back to the first command, then
    // confirm one more step yields no intent rather than wrapping to the last.
    while model.command_selected() != Some(0) {
        let hits = board_hit_map(STANDARD, &model);
        let intent = map_board_mouse(&model, &hits, wheel_up(0, 0))
            .expect("wheel must retreat the selection");
        apply_intent(&mut domain, &mut model, intent, None).expect("apply wheel step");
    }
    let hits = board_hit_map(STANDARD, &model);
    assert_eq!(
        map_board_mouse(&model, &hits, wheel_up(0, 0)),
        None,
        "the wheel must clamp at the first command, not wrap to the last"
    );
}

/// `DELETE_NOTICE_UNDO` ("ctrl+u undo") is painted on the status line while a
/// delete-recovery notice is armed (`draw_queue_frame`'s status row), but had no hit region
/// and no `map_board_mouse` arm -- a painted affordance with no mouse route, restoring the
/// coverage `the_delete_notice_undo_control_is_clickable_in_every_mode_that_shows_it`
/// (deleted in this rewrite) used to guard. Matches the keyboard's own `u` -> `Undo`.
#[test]
fn delete_notice_undo_control_is_clickable_and_matches_the_keyboard() {
    let (mut domain, mut model, id) = board_with_task("doomed", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("arm delete");
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("soft delete");
    assert!(
        model.delete_notice().is_some(),
        "a fresh soft delete must arm the undo notice"
    );
    let hits = board_hit_map(STANDARD, &model);
    let undo_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::DeleteNoticeUndo))
        .expect("no hit region for the painted Undo control");
    let keyboard_intent = map_key(BoardInputMode::Normal, ctrl(KeyCode::Char('u'))).expect("u key");
    assert_eq!(keyboard_intent, BoardIntent::Undo);
    assert_eq!(click(undo_hit, &model, &hits), Some(BoardIntent::Undo));

    let undo_intent = click(undo_hit, &model, &hits).unwrap();
    apply_intent(&mut domain, &mut model, undo_intent, None).expect("mouse undo");
    assert!(
        !domain.get(id).unwrap().soft_deleted,
        "the click must restore the task"
    );
}

/// Minor 1: the Undo hit region used to be located by `find`ing the
/// literal `ctrl+u undo` text over the whole composed status row (`Deleted "<title>" · ctrl+u undo`),
/// which is partly user text. A task titled with that literal steals the region: the real
/// control (painted at the end of the notice, after the *first* deleted-title occurrence)
/// goes unreachable at its own coordinates, while a click on the earlier occurrence inside
/// the title fires `Undo` it never painted there. The region must land only on the control
/// the notice actually painted, and a click on the title's own occurrence of the words must
/// not fire `Undo`.
#[test]
fn delete_notice_undo_region_survives_a_title_containing_the_literal_undo_control() {
    let (mut domain, mut model, id) = board_with_task("ctrl+u undo now", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("arm delete");
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("soft delete");
    assert!(
        model.delete_notice().is_some(),
        "a fresh soft delete must arm the undo notice"
    );

    let hits = board_hit_map(STANDARD, &model);
    let undo_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::DeleteNoticeUndo))
        .expect("no hit region for the painted Undo control");

    // The region must dispatch Undo when clicked, exactly as the plain-title case does.
    let keyboard_intent = map_key(BoardInputMode::Normal, ctrl(KeyCode::Char('u'))).expect("u key");
    assert_eq!(keyboard_intent, BoardIntent::Undo);
    assert_eq!(click(undo_hit, &model, &hits), Some(BoardIntent::Undo));

    // A click on the title's own (earlier) occurrence of the literal words must not fire
    // `Undo`: it must resolve through whatever the row actually paints there (the notice
    // text is not a control), never through the region a naive `find` would have located.
    // The status row is painted from column 0 (`paint_status_line`'s `put_line`), leading
    // with one space, then `Deleted "`: the title's own `ctrl+u undo` inside the title starts
    // right after that 10-column prefix, well left of the real control near the row's end.
    let title_occurrence_x: u16 = 10;
    assert!(
        title_occurrence_x + 11 <= undo_hit.area.x,
        "the title's own occurrence must sit left of the real control: {undo_hit:?}"
    );
    assert_eq!(
        map_board_mouse(
            &model,
            &hits,
            left_click(title_occurrence_x, undo_hit.area.y)
        ),
        None,
        "a click on the notice's own text must not fire Undo"
    );

    let undo_intent = click(undo_hit, &model, &hits).unwrap();
    apply_intent(&mut domain, &mut model, undo_intent, None).expect("mouse undo");
    assert!(
        !domain.get(id).unwrap().soft_deleted,
        "the click on the real control must still restore the task"
    );
}

/// `non_left_click_ignored`'s guard was deleted along with the rest
/// of the classic `mouse.rs` suite; the behavior it covered survives at the top of
/// `map_board_mouse` (every `mouse.kind` other than `Down(Left)`/`ScrollUp`/`ScrollDown`
/// returns `None` before any hit-map lookup) but was left unguarded. A right or middle
/// click over a live control (the first verb-bar entry) must not dispatch anything.
#[test]
fn non_left_clicks_over_a_live_control_are_ignored() {
    let (_domain, model, _id) = board_with_task("click kinds", HumanStatus::Ready);
    let hits = board_hit_map(STANDARD, &model);
    let verb_hit = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::Verb(0)))
        .expect("verb hit region");
    for kind in [
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Down(MouseButton::Middle),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Moved,
        MouseEventKind::Drag(MouseButton::Left),
    ] {
        let mouse = MouseEvent {
            kind,
            column: verb_hit.area.x,
            row: verb_hit.area.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            map_board_mouse(&model, &hits, mouse),
            None,
            "{kind:?} over a live control must not dispatch"
        );
    }
}

// ---------------------------------------------------------------------------
// Task page mouse parity: click peeks, double-click opens the page, page field
// clicks focus their fields, the scope footer opens its dropdown, wheel scrolls.
// ---------------------------------------------------------------------------

#[test]
fn section_headers_have_no_hits_and_collapsed_task_titles_are_selectable() {
    let mut domain = DomainState::new();
    domain
        .create(
            "threaded row",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            Some("release".to_string()),
        )
        .expect("create threaded task");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    let rows = page_rows(&model);
    // Collapsed task titles carry task hits; section headers are inert.
    let task_row = rows
        .iter()
        .position(|row| row.contains("threaded row"))
        .expect("task title paints without attribution");
    let header_y = rows
        .iter()
        .position(|row| row.contains("ON DECK"))
        .expect("the ON DECK header paints");
    let hits = board_hit_map(STANDARD, &model);

    assert!(
        hits.regions.iter().all(|hit| hit.area.y != header_y as u16),
        "section header rows must register no hit target: {hits:?}"
    );
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(0, header_y as u16)),
        None,
        "clicking the decorative header must be inert"
    );
    assert!(
        hits.regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::Task(_))
                && hit.area.y == task_row as u16),
        "title row carries the task hit"
    );
}

#[test]
fn a_row_click_selects_and_peeks_and_a_second_click_opens_the_task_page() {
    let (mut domain, mut model) = deck_of(3);
    let visible = model.visible_ids();
    let target = visible[1];

    // First click selects the row and expands its peek.
    let hits = board_hit_map(STANDARD, &model);
    let area = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::Task(target))
        .expect("the target row paints a hit region")
        .area;
    let intent =
        map_board_mouse(&model, &hits, left_click(area.x + 1, area.y)).expect("first click");
    apply_intent(&mut domain, &mut model, intent, None).expect("apply first click");
    assert_eq!(model.selected_id(), Some(target));
    assert_eq!(
        model.detail_open(),
        Some(target),
        "a single row click expands that row's peek"
    );
    assert_eq!(model.input_mode(), BoardInputMode::Normal);

    // A second click on the same row inside the double-click window opens the page.
    let hits = board_hit_map(STANDARD, &model);
    let area = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::Task(target))
        .expect("the peeked row still paints a hit region")
        .area;
    let intent =
        map_board_mouse(&model, &hits, left_click(area.x + 1, area.y)).expect("second click");
    apply_intent(&mut domain, &mut model, intent, None).expect("apply second click");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(model.detail_open(), None, "the page replaces the peek");
}

#[test]
fn task_identifier_click_copies_without_falling_through_to_row_or_page_actions() {
    let (domain, _model, id) = board_with_task("Copy this identifier", HumanStatus::Ready);
    let mut task = domain.get(id).expect("task").clone();
    task.number = Some(30);
    let mut model = BoardModel::from_tasks(vec![task], Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));

    let hits = board_hit_map(STANDARD, &model);
    let identifier = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::TaskNumber(id))
        .expect("identifier hit")
        .area;
    assert_eq!(
        identifier.width, 3,
        "the T30 cells are the whole click target"
    );
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(identifier.x, identifier.y)),
        Some(BoardIntent::CopyTaskNumber(id)),
        "identifier click must not become a row selection"
    );
}

#[test]
fn quick_add_identifier_click_copies_without_discarding_the_draft() {
    let (mut domain, _model, id) = board_with_task("Copy while drafting", HumanStatus::Ready);
    let mut task = domain.get(id).expect("task").clone();
    task.number = Some(30);
    let mut model = BoardModel::from_tasks(vec![task], Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None).expect("open draft");

    let hits = board_hit_map(STANDARD, &model);
    let identifier = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::TaskNumber(id))
        .expect("identifier hit")
        .area;
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(identifier.x, identifier.y)),
        Some(BoardIntent::CopyTaskNumber(id))
    );
}

#[test]
fn unnumbered_rows_and_drafts_register_no_identifier_hit() {
    let (mut domain, mut model, _id) = board_with_task("Unsaved task", HumanStatus::Ready);
    assert!(
        !board_hit_map(STANDARD, &model)
            .regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::TaskNumber(_))),
        "a task without a persisted number must not paint an identifier"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None).expect("open draft");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::QuickAddInsertText("Draft title".to_string()),
        None,
    )
    .expect("type draft");
    apply_intent(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None)
        .expect("expand draft onto the task page");
    assert!(
        !board_hit_map(STANDARD, &model)
            .regions
            .iter()
            .any(|hit| matches!(hit.target, QueueHitTarget::TaskNumber(_))),
        "an unsaved quick-add task page must not synthesize an identifier"
    );
}

#[test]
fn task_page_identifier_click_copies_instead_of_hitting_the_title_region() {
    let (mut domain, _model, id) = board_with_task("Copy this page identifier", HumanStatus::Ready);
    let mut task = domain.get(id).expect("task").clone();
    task.number = Some(31);
    let mut model = BoardModel::from_tasks(vec![task], Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    let hits = board_hit_map(STANDARD, &model);
    let identifier = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::TaskNumber(id))
        .expect("page identifier hit")
        .area;
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(identifier.x, identifier.y)),
        Some(BoardIntent::CopyTaskNumber(id)),
        "the identifier must outrank the broad title hit"
    );
}

#[test]
fn page_field_clicks_activate_after_task_editing_starts() {
    let (mut domain, mut model) = deck_of(1);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open the page");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    // Title and notes regions are inert: only `e`/`n`/Tab enter edit mode on the page.
    let hits = board_hit_map(STANDARD, &model);
    let title_area = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormTitle)
        .expect("the page header paints a title hit")
        .area;
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(title_area.x + 3, title_area.y)),
        None,
        "clicking the title must not enter edit mode"
    );
    let notes_area = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::FormNotes(_)))
        .expect("the page body paints notes hits")
        .area;
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(notes_area.x + 3, notes_area.y)),
        None,
        "clicking the notes must not enter edit mode"
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    // The meta footer is inert in view mode. Scope changes only after an edit starts.
    let scope_area = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormScope)
        .expect("the page footer paints a scope hit")
        .area;
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(scope_area.x + 3, scope_area.y)),
        None,
        "clicking the scope footer must not open the dropdown in view mode"
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("start task edit session");
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("return page");
    let hits = board_hit_map(STANDARD, &model);
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(title_area.x + 3, title_area.y)),
        Some(BoardIntent::FocusFormField(CaptureField::Title)),
        "an active task session lets Title clicks edit"
    );
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(notes_area.x + 3, notes_area.y)),
        Some(BoardIntent::FocusFormField(CaptureField::Notes)),
        "an active task session lets Notes clicks edit"
    );
    let active_scope_hit = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormScope)
        .expect("the active page footer paints a scope hit");
    assert_eq!(
        click(active_scope_hit, &model, &hits),
        Some(BoardIntent::OpenFormDropdown(CaptureField::Scope)),
        "an active task session lets Scope clicks edit"
    );
    let thread_hit = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormThread)
        .expect("the active page paints the empty Thread target");
    assert_eq!(
        click(thread_hit, &model, &hits),
        Some(BoardIntent::FocusFormField(CaptureField::Thread)),
        "the first Thread click selects it"
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Thread),
        None,
    )
    .expect("select Thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    let hits = board_hit_map(STANDARD, &model);
    assert_eq!(
        click(thread_hit, &model, &hits),
        Some(BoardIntent::ToggleThreadEditing),
        "the second Thread click opens its text editor"
    );
}

#[test]
fn the_wheel_scrolls_the_page_notes_not_the_board_list() {
    let (mut domain, mut model) = deck_of(3);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open the page");
    let selection_before = model.selected_id();

    let hits = board_hit_map(STANDARD, &model);
    let down = map_board_mouse(&model, &hits, wheel_down(5, 5)).expect("wheel down");
    assert_eq!(down, BoardIntent::PageWheelScrollDown);
    apply_intent(&mut domain, &mut model, down, None).expect("scroll down");
    let up = map_board_mouse(&model, &hits, wheel_up(5, 5)).expect("wheel up");
    assert_eq!(up, BoardIntent::PageWheelScrollUp);
    apply_intent(&mut domain, &mut model, up, None).expect("scroll up");

    assert_eq!(
        model.selected_id(),
        selection_before,
        "the page's wheel never moves the board's selection"
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

#[test]
fn the_wheel_keeps_scrolling_while_a_step_add_is_open() {
    let (mut domain, mut model, id) = board_with_task("Scrollable step add", HumanStatus::Ready);
    for index in 0..30 {
        domain
            .add_step(id, format!("step {index:02}"))
            .expect("add overflowing step");
    }
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    let _ = page_rows(&model);
    for _ in 0..64 {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::PageWheelScrollDown,
            None,
        )
        .expect("reach page bottom");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None)
        .expect("open inline step add");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("draft survives wheel".into()),
        None,
    )
    .expect("type draft");
    let before = page_rows(&model);
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(before
        .iter()
        .any(|row| row.contains("draft survives wheel")));

    let hits = board_hit_map(STANDARD, &model);
    let up = map_board_mouse(&model, &hits, wheel_up(5, 5))
        .expect("wheel remains routed while the step add is open");
    assert_eq!(up, BoardIntent::PageWheelScrollUp);
    apply_intent(&mut domain, &mut model, up, None).expect("scroll with open draft");

    let after = page_rows(&model);
    assert_ne!(
        before, after,
        "the open step add must not lock page scrolling"
    );

    let hits = board_hit_map(STANDARD, &model);
    let down = map_board_mouse(&model, &hits, wheel_down(5, 5))
        .expect("wheel down remains routed while the step add is open");
    assert_eq!(down, BoardIntent::PageWheelScrollDown);
    apply_intent(&mut domain, &mut model, down, None).expect("scroll back to draft");
    let returned = page_rows(&model);
    assert!(returned
        .iter()
        .any(|row| row.contains("draft survives wheel")));
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
}

#[test]
fn page_verb_clicks_resolve_through_the_page_legend() {
    let (mut domain, mut model) = deck_of(1);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open the page");
    let id = model.selected_id().expect("one task");

    let hits = board_hit_map(STANDARD, &model);
    // The page legend's `d done` entry: find its verb index from the painted legend.
    let verbs = board_verb_items(&model);
    let done_index = verbs
        .iter()
        .position(|entry| entry.key == "d")
        .expect("the page legend shows d done");
    let verb_area = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::Verb(done_index))
        .expect("the d verb paints a hit region")
        .area;
    let intent = map_board_mouse(&model, &hits, left_click(verb_area.x + 1, verb_area.y))
        .expect("verb click");
    apply_intent(&mut domain, &mut model, intent, None).expect("complete via click");
    assert_eq!(
        domain.get(id).expect("task").status,
        HumanStatus::Done,
        "clicking the page's done verb completes its task"
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    // Every seat of the fixed page bar dispatches what its label says: the painted legend
    // is `ctrl+e edit · <status verbs> · esc close`, indexed exactly as painted.
    let click_verb = |model: &BoardModel, key: &str| -> BoardIntent {
        let verbs = board_verb_items(model);
        let index = verbs
            .iter()
            .position(|entry| entry.key == key)
            .unwrap_or_else(|| panic!("no {key:?} seat in {verbs:?}"));
        let hits = board_hit_map(STANDARD, model);
        let area = hits
            .regions
            .iter()
            .find(|hit| hit.target == QueueHitTarget::Verb(index))
            .unwrap_or_else(|| panic!("no hit for seat {index} ({key})"))
            .area;
        map_board_mouse(model, &hits, left_click(area.x + 1, area.y)).expect("verb click")
    };
    let verbs: Vec<&str> = board_verb_items(&model).iter().map(|v| v.key).collect();
    assert_eq!(
        verbs,
        vec!["e", "n", "o", "u", "esc"],
        "done page bar: edit · ready · inbox · undo · close"
    );
    assert_eq!(click_verb(&model, "o"), BoardIntent::Reopen);
    apply_intent(&mut domain, &mut model, BoardIntent::Reopen, None).expect("open");
    let verbs: Vec<&str> = board_verb_items(&model).iter().map(|v| v.key).collect();
    assert_eq!(verbs, vec!["e", "s", "n", "d", "esc"], "open page bar");
    assert_eq!(click_verb(&model, "e"), BoardIntent::BeginEditTitle);
    assert_eq!(click_verb(&model, "s"), BoardIntent::PrimaryVerb);
    assert_eq!(
        click_verb(&model, "n"),
        BoardIntent::SetStatus(HumanStatus::Ready)
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SetStatus(HumanStatus::Ready),
        None,
    )
    .expect("pick ready");
    let verbs: Vec<&str> = board_verb_items(&model).iter().map(|v| v.key).collect();
    assert_eq!(verbs, vec!["e", "s", "o", "d", "esc"], "ready page bar");
    assert_eq!(click_verb(&model, "o"), BoardIntent::Reopen);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SetStatus(HumanStatus::Open),
        None,
    )
    .expect("send to inbox");
    assert_eq!(click_verb(&model, "s"), BoardIntent::PrimaryVerb);
    apply_intent(&mut domain, &mut model, BoardIntent::PrimaryVerb, None).expect("start");
    assert_eq!(click_verb(&model, "b"), BoardIntent::ToggleBlock);
    let close = click_verb(&model, "esc");
    assert_eq!(close, BoardIntent::CloseLayer);
    apply_intent(&mut domain, &mut model, close, None).expect("close via click");
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
}

#[test]
fn help_card_wheel_scrolls_and_the_offset_clamps_to_the_last_page() {
    let (mut domain, mut model) = deck_of(1);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("help");
    let small = Rect::new(0, 0, 40, 10);
    let hits = board_hit_map(small, &model);
    let down = map_board_mouse(&model, &hits, wheel_down(20, 5)).expect("wheel down in help");
    assert_eq!(down, BoardIntent::HelpScrollDown);
    let up = map_board_mouse(&model, &hits, wheel_up(20, 5)).expect("wheel up in help");
    assert_eq!(up, BoardIntent::HelpScrollUp);

    // Scroll far past the end: the painter records the last page and the reducer stops
    // there, so one wheel up immediately moves the window back.
    for _ in 0..200 {
        apply_intent(&mut domain, &mut model, down.clone(), None).expect("scroll");
        let _ = board_hit_map(small, &model);
    }
    let lines = tsk_tui::ui::input::help_card_lines();
    assert!(
        model.help_scroll() > lines.len(),
        "wrapped screen rows, not source rows, define the compact scroll horizon: {}",
        model.help_scroll()
    );
    let paint = |model: &BoardModel| -> String {
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).expect("test terminal");
        terminal
            .draw(|frame| {
                let _ = draw_board(frame, model);
            })
            .expect("draw help");
        let buffer = terminal.backend().buffer();
        (0..10u16)
            .map(|y| {
                (0..40u16)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let bottom = paint(&model);
    assert!(
        !bottom.contains("help ▼"),
        "the last page has nothing below: {bottom}"
    );
    let at_bottom = model.help_scroll();
    apply_intent(&mut domain, &mut model, up, None).expect("scroll up");
    assert_eq!(model.help_scroll(), at_bottom - 1);
    let one_up = paint(&model);
    assert_ne!(
        one_up, bottom,
        "one step up from the clamped bottom moves the window at once (no dead zone)"
    );
    assert!(one_up.contains("help ▲"), "rows above are marked: {one_up}");
    assert!(
        one_up.contains("help ▲▼") || one_up.contains("▼"),
        "rows below too: {one_up}"
    );
}

#[test]
fn page_step_add_footer_chip_routes_to_begin_add_step() {
    let (mut domain, mut model) = deck_of(1);
    let id = model.selected_id().expect("task");
    domain.add_step(id, "existing step").expect("add step");
    model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("enter task edit mode");
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("return to page");
    let verbs = board_verb_items(&model);
    let step_index = verbs
        .iter()
        .position(|entry| entry.key == "a")
        .expect("visible a step chip");
    let hits = board_hit_map(STANDARD, &model);
    let area = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::Verb(step_index))
        .expect("step chip hit")
        .area;
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(area.x + 1, area.y)),
        Some(BoardIntent::BeginAddStep)
    );
}

/// The painted trailing add control is a direct pointer route from task view and every
/// task-edit state, including the selected and active Thread states plus an inline editor.
#[test]
fn clicking_trailing_step_add_works_from_every_task_page_edit_mode() {
    let (mut domain, mut model, id) = board_with_task("step add click", HumanStatus::Ready);
    domain.add_step(id, "stored step").expect("add stored step");
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    let assert_add_click = |model: &BoardModel| {
        let hits = board_hit_map(STANDARD, model);
        let area = hits
            .regions
            .iter()
            .find(|hit| hit.target == QueueHitTarget::StepAdd)
            .expect("trailing add hit")
            .area;
        assert_eq!(
            map_board_mouse(model, &hits, left_click(area.x, area.y)),
            Some(BoardIntent::BeginAddStep),
            "StepAdd must remain clickable in {:?}",
            model.input_mode()
        );
    };

    // Task view is the independent-add path.
    assert_add_click(&model);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("title");
    assert_add_click(&model);

    for field in [
        CaptureField::Notes,
        CaptureField::Scope,
        CaptureField::Thread,
    ] {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::FocusFormField(field),
            None,
        )
        .expect("focus task-edit field");
        assert_add_click(&model);
    }
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleThreadEditing,
        None,
    )
    .expect("open thread editor");
    assert_eq!(model.input_mode(), BoardInputMode::EditThread);
    assert_add_click(&model);

    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None).expect("inline add");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert_add_click(&model);
}

#[test]
fn step_editor_verb_chips_follow_their_keyboard_intents() {
    let (mut domain, mut model) = deck_of(1);
    let id = model.selected_id().expect("task");
    domain.add_step(id, "existing step").expect("add step");
    model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("enter edit session");
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("return page");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None).expect("add step");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);

    let hits = board_hit_map(STANDARD, &model);
    let verbs = board_verb_items(&model);
    let key = "shift+enter";
    let index = verbs
        .iter()
        .position(|entry| entry.key == key)
        .expect("painted step-editor verb");
    let area = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::Verb(index))
        .expect("step-editor verb hit")
        .area;
    assert_eq!(
        map_board_mouse(&model, &hits, left_click(area.x + 1, area.y)),
        Some(BoardIntent::ConfirmEditNext),
        "{key} click matches the keyboard route"
    );

    let cancel_index = verbs
        .iter()
        .position(|entry| entry.key == "esc")
        .expect("painted step-editor cancel verb");
    let cancel_area = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::Verb(cancel_index))
        .expect("step-editor cancel hit")
        .area;
    let cancel = map_board_mouse(&model, &hits, left_click(cancel_area.x + 1, cancel_area.y));
    assert_eq!(
        cancel,
        Some(BoardIntent::CancelEdit),
        "the mouse cancel chip must preserve the enclosing task edit session like keyboard Esc"
    );
    apply_intent(
        &mut domain,
        &mut model,
        cancel.expect("cancel intent"),
        None,
    )
    .expect("cancel only the step field");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(
        model.task_editing(),
        "mouse Esc must leave the enclosing task edit session active"
    );
}

/// The task page painted row by row at the standard board size, so a test can
/// click the coordinates a row actually painted at.
fn page_rows(model: &BoardModel) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(STANDARD.width, STANDARD.height)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw page");
    let buffer = terminal.backend().buffer();
    (0..STANDARD.height)
        .map(|y| {
            (0..STANDARD.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

/// A click on a step in an active task edit session opens that row in place. It neither
/// persists nor toggles, and it never falls back to the retired footer editor.
#[test]
fn clicking_a_step_row_opens_its_inline_editor() {
    let (mut domain, mut model, id) = board_with_task("Click target", HumanStatus::Ready);
    for text in ["alpha step", "bravo step", "charlie step"] {
        domain.add_step(id, text).expect("add step");
    }
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open the page");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("enter task edit mode");
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);

    // The click lands on step 3's painted row, located from the same frame the
    // hit map was recorded beside.
    let hits = board_hit_map(STANDARD, &model);
    let rows = page_rows(&model);
    let step_y = rows
        .iter()
        .position(|row| row.contains("charlie step"))
        .expect("step 3 paints a row");
    let intent = map_board_mouse(&model, &hits, left_click(3, step_y as u16))
        .expect("a step-row click must dispatch a select intent");
    let revision_before = domain.get(id).expect("task").revision;
    apply_intent(&mut domain, &mut model, intent, None).expect("apply the click");

    let selected = page_rows(&model);
    assert!(
        selected.iter().any(|row| row.contains("▸ ▪ charlie step")),
        "the cursor must sit on the clicked step:\n{}",
        selected.join("\n")
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditStep,
        "a click opens the selected row's inline editor"
    );
    assert!(
        !selected.iter().any(|row| row.contains("▎")),
        "the step editor no longer occupies the footer:\n{}",
        selected.join("\n")
    );
    let task = domain.get(id).expect("task");
    assert_eq!(
        task.steps.iter().map(|step| step.done).collect::<Vec<_>>(),
        vec![false, false, false],
        "a click never toggles a step"
    );
    assert_eq!(task.revision, revision_before, "a click persists nothing");
    assert!(
        !selected.iter().any(|row| row.contains("✗")),
        "a click never arms a delete mark:\n{}",
        selected.join("\n")
    );

    // Inline step editing is part of the task form, not a modal: page field clicks move focus
    // without discarding the displayed row draft.
    let title_hit = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::FormTitle)
        .expect("task page paints a title hit");
    assert_eq!(
        click(title_hit, &model, &hits),
        Some(BoardIntent::FocusFormField(CaptureField::Title))
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Title),
        None,
    )
    .expect("focus title from inline step editor");
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);

    // A clean inline draft also permits switching straight to another step editor.
    let hits = board_hit_map(STANDARD, &model);
    let bravo_hit = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::Step(1))
        .expect("second step hit");
    assert_eq!(
        click(bravo_hit, &model, &hits),
        Some(BoardIntent::SelectStep(1))
    );
    apply_intent(&mut domain, &mut model, BoardIntent::SelectStep(1), None)
        .expect("switch inline editor to second step");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(
        page_rows(&model)
            .iter()
            .any(|row| row.contains("▸ ▪ bravo step")),
        "the second click switches the inline editor"
    );
}

/// Painters declare copyable content rects beside the paint; a drag selection
/// over those cells yields the painted text (chrome columns outside `copyable`
/// stay out of the clipboard string).
#[test]
fn drag_selection_copies_painted_task_title_not_undeclared_chrome() {
    use ratatui::layout::Position;
    use tsk_tui::ui::text_select::{selection_text, TextSelection};

    let (_domain, mut model, _id) = board_with_task("copyable title text", HumanStatus::Ready);
    let hits = board_hit_map(STANDARD, &model);
    assert!(
        !hits.copyable.is_empty(),
        "task rows must push copyable content rects"
    );

    let mut terminal =
        Terminal::new(TestBackend::new(STANDARD.width, STANDARD.height)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..STANDARD.height)
        .map(|y| {
            (0..STANDARD.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect();

    // Anchor on the title content, drag across it.
    let area = hits.copyable[0];
    model.begin_mouse_press(Position::new(area.x, area.y));
    model.drag_text_selection(Position::new(
        area.x.saturating_add(area.width.saturating_sub(1)),
        area.y,
    ));
    model.end_mouse_press();
    let selection = model.text_selection().expect("drag left a selection");
    let text = selection_text(&rows, &hits.copyable, &selection).expect("copyable text");
    assert!(
        text.contains("copyable title text"),
        "selection should include the task title, got {text:?}"
    );
    assert!(
        !text.contains('○') && !text.contains('●'),
        "status glyph is chrome and must stay out of the copy, got {text:?}"
    );

    // A bare click (no drag) leaves no copyable text.
    model.begin_mouse_press(Position::new(area.x + 2, area.y));
    model.end_mouse_press();
    assert!(model.text_selection().is_none());
    let bare = TextSelection::new(
        Position::new(area.x + 2, area.y),
        Position::new(area.x + 2, area.y),
    );
    assert_eq!(selection_text(&rows, &hits.copyable, &bare), None);
}

/// Peek accordion body paints a `│` gutter; that pipe is chrome. A drag across the
/// open peek on a project-scoped board must copy the notes text without it — the
/// same surface that felt broken when copyable rects spanned the full row.
#[test]
fn peek_on_project_board_copy_excludes_pipe_gutter() {
    use ratatui::layout::Position;
    use tsk_tui::ui::text_select::{selection_text, TextSelection};

    let mut domain = DomainState::new();
    let notes = "Fixed. It wasn't a bug in the terminal-selection sense.";
    let id = domain
        .create(
            "hi",
            Some(notes.to_string()),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain.set_status(id, HumanStatus::Started).expect("start");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    // Focus the project board (the "project page" surface), then open peek.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open project picker");
    let project_idx = model
        .project_options()
        .iter()
        .position(|opt| matches!(opt, ProjectScopeOption::Project(p) if p == Path::new(THIS_REPO)))
        .expect("this repo is offered");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectProjectOption(project_idx),
        None,
    )
    .expect("select project");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmProjectChoice,
        None,
    )
    .expect("confirm project focus");
    assert_eq!(
        model.selected_project(),
        Some(Path::new(THIS_REPO)),
        "board must be project-focused"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::PeekDetail, None).expect("peek");
    assert_eq!(model.detail_open(), Some(id));

    let hits = board_hit_map(STANDARD, &model);
    let mut terminal =
        Terminal::new(TestBackend::new(STANDARD.width, STANDARD.height)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..STANDARD.height)
        .map(|y| {
            (0..STANDARD.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect();

    let note_y = rows
        .iter()
        .position(|row| row.contains("Fixed. It wasn't"))
        .expect("peek paints the notes preview") as u16;
    assert!(
        rows[note_y as usize].contains('│'),
        "sanity: the peek gutter is on the painted row"
    );
    let note_copyable = hits
        .copyable
        .iter()
        .find(|rect| rect.y == note_y)
        .expect("peek notes row must declare a copyable rect");
    assert!(
        note_copyable.x >= 6,
        "copyable must start past `    │ ` (6 cells), got x={}",
        note_copyable.x
    );

    let selection = TextSelection::new(
        Position::new(0, note_y),
        Position::new(STANDARD.width - 1, note_y),
    );
    let text = selection_text(&rows, &hits.copyable, &selection).expect("peek notes copy");
    assert!(
        text.contains("Fixed. It wasn't a bug"),
        "notes text must copy, got {text:?}"
    );
    assert!(
        !text.contains('│'),
        "peek pipe gutter must stay out of the clipboard, got {text:?}"
    );
}

/// Task-page header copyable rects cover title words only — not the status glyph or
/// the right-aligned status word.
#[test]
fn task_page_title_copy_excludes_glyph_and_status_word() {
    use ratatui::layout::Position;
    use tsk_tui::ui::text_select::{selection_text, TextSelection};

    let (mut domain, mut model, _id) = board_with_task("title only please", HumanStatus::Started);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    let hits = board_hit_map(STANDARD, &model);
    let mut terminal =
        Terminal::new(TestBackend::new(STANDARD.width, STANDARD.height)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..STANDARD.height)
        .map(|y| {
            (0..STANDARD.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect();

    let title_y = rows
        .iter()
        .position(|row| row.contains("title only please"))
        .expect("page paints the title") as u16;
    assert!(
        rows[title_y as usize].contains('●'),
        "sanity: status glyph is on the title row"
    );
    assert!(
        rows[title_y as usize].contains("started"),
        "sanity: status word is on the title row"
    );

    let selection = TextSelection::new(
        Position::new(0, title_y),
        Position::new(STANDARD.width - 1, title_y),
    );
    let text = selection_text(&rows, &hits.copyable, &selection).expect("title copy");
    assert_eq!(
        text, "title only please",
        "copy must be the title alone, got {text:?}"
    );
}

/// The shared modal card only declares its own body rows copyable -- never the
/// border, title, or footer rows around them -- so a selection dragged across the
/// card's border/footer paints no clipboard text, only its actual binding lines do.
#[test]
fn the_modal_cards_copyable_rects_exclude_its_own_border_and_footer() {
    use ratatui::layout::Position;
    use tsk_tui::ui::text_select::{selection_text, TextSelection};

    let (mut domain, mut model, _id) = board_with_task("modal card copyable", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("open help");
    let hits = board_hit_map(STANDARD, &model);

    let mut terminal =
        Terminal::new(TestBackend::new(STANDARD.width, STANDARD.height)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..STANDARD.height)
        .map(|y| {
            (0..STANDARD.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect();

    // The top border row (painted with the card's title) declares no copyable rect.
    let top_border_y = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ModalClose))
        .expect("the help card paints an `[x]` close control")
        .area
        .y;
    assert!(
        !hits
            .copyable
            .iter()
            .any(|rect| rect.y == top_border_y && rect.height == 1),
        "the card's own title/border row must not be copyable: {:?}",
        rows[top_border_y as usize]
    );
    let border = TextSelection::new(
        Position::new(0, top_border_y),
        Position::new(STANDARD.width - 1, top_border_y),
    );
    assert_eq!(
        selection_text(&rows, &hits.copyable, &border),
        None,
        "a drag across the card's border must yield no copyable text"
    );

    // A real binding row is copyable. The focused search row above it is input, not text
    // selection content, and the card body itself stays inert instead of dismissing help.
    let card = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ModalChrome))
        .expect("the help card paints modal chrome")
        .area;
    let body_hit = hits
        .copyable
        .iter()
        .copied()
        .find(|rect| {
            rect.y > card.y
                && rect.y < card.y.saturating_add(card.height).saturating_sub(1)
                && rect.x >= card.x
                && rect.x.saturating_add(rect.width) <= card.x.saturating_add(card.width)
        })
        .expect("the help card paints a copyable binding row");
    let body_y = body_hit.y;
    let body = TextSelection::new(
        Position::new(body_hit.x, body_y),
        Position::new(
            body_hit.x.saturating_add(body_hit.width.saturating_sub(1)),
            body_y,
        ),
    );
    let text = selection_text(&rows, &hits.copyable, &body).expect("copyable body text");
    assert!(
        !text.trim().is_empty(),
        "the card's first body row should yield its painted binding text, got {text:?}"
    );
}

#[test]
fn list_scrollbar_click_jumps_viewport_without_changing_selection() {
    let (mut domain, mut model) = deck_of(40);
    let first = model.selected_id().expect("the first task is selected");
    assert_eq!(model.visible_ids()[1], first);
    assert_eq!(model.detail_open(), None);

    let hits = board_hit_map(STANDARD, &model);
    let bottom = hits
        .regions
        .iter()
        .filter(|hit| matches!(hit.target, QueueHitTarget::ListScroll(_)))
        .max_by_key(|hit| hit.area.y)
        .expect("scrollbar track cells");
    let intent = map_board_mouse(&model, &hits, left_click(bottom.area.x, bottom.area.y))
        .expect("scrollbar click");
    let QueueHitTarget::ListScroll(bottom_offset) = bottom.target else {
        panic!("bottom cell must be ListScroll, got {:?}", bottom.target);
    };
    assert!(
        matches!(intent, BoardIntent::ListScrollTo(offset) if offset == bottom_offset && offset > 0),
        "bottom track cell must jump to its mapped offset {bottom_offset}, got {intent:?}"
    );
    apply_intent(&mut domain, &mut model, intent, None).expect("apply jump");
    let _ = page_rows(&model);
    assert_eq!(
        model.list_scroll(),
        bottom_offset,
        "bottom click must land on the mapped offset, not a sticky-clipped one"
    );
    assert_eq!(
        model.selected_id(),
        Some(first),
        "track click must leave selection alone"
    );
    assert_eq!(model.detail_open(), None, "scrollbar click must not peek");
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
}

#[test]
fn row_click_selects_without_jumping_the_viewport() {
    let (mut domain, mut model) = deck_of(40);
    let _ = page_rows(&model);
    let before = page_rows(&model);
    assert!(
        before.iter().any(|row| row.contains("task 0")),
        "top of the deck should be on screen:\n{}",
        before.join("\n")
    );
    let ids = model.visible_ids();
    let mid = 8;
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(mid), None)
        .expect("select middle row");
    assert_eq!(model.selected_id(), Some(ids[mid]));
    assert_eq!(model.detail_open(), Some(ids[mid]));
    let after = page_rows(&model);
    assert!(
        after.iter().any(|row| row.contains("task 0")),
        "clicking a visible row must not park the peek at the bottom:\n{}",
        after.join("\n")
    );
}

#[test]
fn list_scrollbar_still_moves_the_viewport_while_peek_is_open() {
    let (mut domain, mut model) = deck_of(40);
    let _ = page_rows(&model);
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(1), None)
        .expect("peek first task row");
    assert!(model.detail_open().is_some());
    let before = page_rows(&model);
    let hits = board_hit_map(STANDARD, &model);
    let bottom = hits
        .regions
        .iter()
        .filter(|hit| matches!(hit.target, QueueHitTarget::ListScroll(_)))
        .max_by_key(|hit| hit.area.y)
        .expect("peek must not remove the list scrollbar");
    let intent = map_board_mouse(&model, &hits, left_click(bottom.area.x, bottom.area.y))
        .expect("scrollbar click with peek open");
    apply_intent(&mut domain, &mut model, intent, None).expect("scroll with peek open");
    let after = page_rows(&model);
    assert_ne!(
        after,
        before,
        "scrollbar must move the list while peek is open:\n{}",
        after.join("\n")
    );
}

#[test]
fn mouse_wheel_scrolls_the_list_while_peek_is_open() {
    let (mut domain, mut model) = deck_of(40);
    let _ = page_rows(&model);
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(0), None)
        .expect("peek first row");
    let selected = model.selected_id();
    let before = page_rows(&model);
    let hits = board_hit_map(STANDARD, &model);
    for _ in 0..8 {
        let intent = map_board_mouse(&model, &hits, wheel_down(0, 0)).expect("wheel down");
        apply_intent(&mut domain, &mut model, intent, None).expect("apply wheel");
    }
    assert_eq!(
        model.selected_id(),
        selected,
        "wheel must not move selection"
    );
    assert_eq!(model.detail_open(), selected, "wheel must leave peek open");
    assert!(model.list_scroll() > 0, "wheel must advance list_scroll");
    let after = page_rows(&model);
    assert_ne!(
        after,
        before,
        "wheel must move the list while peek is open:\n{}",
        after.join("\n")
    );
}

#[test]
fn scrollbar_pointer_state_machine_drags_and_ignores_overlays() {
    let (mut domain, mut model) = deck_of(40);
    let _ = page_rows(&model);
    let hits = board_hit_map(STANDARD, &model);
    let cell = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ListScroll(_)))
        .expect("list scrollbar");
    let mut dragging = false;
    let down = map_scrollbar_mouse(
        BoardInputMode::Normal,
        &hits,
        left_click(cell.area.x, cell.area.y),
        &mut dragging,
    );
    assert!(matches!(
        down,
        ScrollbarMouse::Intent(BoardIntent::ListScrollTo(_))
    ));
    assert!(dragging);

    let below = cell.area.y.saturating_add(20);
    let drag = map_scrollbar_mouse(
        BoardInputMode::Normal,
        &hits,
        left_drag(cell.area.x, below),
        &mut dragging,
    );
    assert!(matches!(
        drag,
        ScrollbarMouse::Intent(BoardIntent::ListScrollTo(_))
    ));
    assert!(dragging);

    let up = map_scrollbar_mouse(
        BoardInputMode::Normal,
        &hits,
        left_up(cell.area.x, below),
        &mut dragging,
    );
    assert_eq!(up, ScrollbarMouse::Consumed);
    assert!(!dragging);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("open palette");
    let overlay_hits = board_hit_map(STANDARD, &model);
    let mut overlay_drag = false;
    let miss = map_scrollbar_mouse(
        model.input_mode(),
        &overlay_hits,
        left_click(cell.area.x, cell.area.y),
        &mut overlay_drag,
    );
    assert_eq!(miss, ScrollbarMouse::Miss);
    assert!(!overlay_drag);
}

#[test]
fn scrollbar_intent_at_clamps_off_track_and_ignores_the_other_surface() {
    let (_, model) = deck_of(40);
    let _ = page_rows(&model);
    let hits = board_hit_map(STANDARD, &model);
    let mut cells: Vec<_> = hits
        .regions
        .iter()
        .filter(|hit| matches!(hit.target, QueueHitTarget::ListScroll(_)))
        .collect();
    cells.sort_by_key(|hit| hit.area.y);
    let first = cells.first().expect("track");
    let last = cells.last().expect("track");
    let QueueHitTarget::ListScroll(first_off) = first.target else {
        panic!("first cell");
    };
    let QueueHitTarget::ListScroll(last_off) = last.target else {
        panic!("last cell");
    };
    assert_eq!(
        scrollbar_intent_at(&hits, first.area.y.saturating_sub(5), false),
        Some(BoardIntent::ListScrollTo(first_off))
    );
    assert_eq!(
        scrollbar_intent_at(&hits, last.area.y.saturating_add(20), false),
        Some(BoardIntent::ListScrollTo(last_off))
    );
    let mid = cells[cells.len() / 2];
    let QueueHitTarget::ListScroll(mid_off) = mid.target else {
        panic!("mid cell");
    };
    assert_eq!(
        scrollbar_intent_at(&hits, mid.area.y, false),
        Some(BoardIntent::ListScrollTo(mid_off))
    );
    assert_eq!(
        scrollbar_intent_at(&hits, mid.area.y, true),
        None,
        "list hits must not drive the page scrollbar"
    );
}

#[test]
fn task_page_scrollbar_click_jumps_notes_without_changing_selection() {
    let notes = (0..30)
        .map(|i| format!("page-scroll line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Page scrollbar",
            Some(notes),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    let before = page_rows(&model);
    let hits = board_hit_map(STANDARD, &model);
    let bottom = hits
        .regions
        .iter()
        .filter(|hit| matches!(hit.target, QueueHitTarget::PageScroll(_)))
        .max_by_key(|hit| hit.area.y)
        .expect("page scrollbar hits");
    let intent = map_board_mouse(&model, &hits, left_click(bottom.area.x, bottom.area.y))
        .expect("page scrollbar click");
    assert!(matches!(intent, BoardIntent::PageScrollTo(offset) if offset > 0));
    apply_intent(&mut domain, &mut model, intent, None).expect("jump notes");
    assert_eq!(model.selected_id(), Some(id));
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    let after = page_rows(&model);
    assert_ne!(
        after,
        before,
        "page scrollbar must move notes:\n{}",
        after.join("\n")
    );
}

#[test]
fn painting_clamps_an_oversize_list_scroll_back_into_the_model() {
    let (mut domain, mut model) = deck_of(40);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ListScrollTo(10_000),
        None,
    )
    .expect("oversize scroll");
    let _ = page_rows(&model);
    assert!(
        model.list_scroll() < 10_000,
        "paint must write the clamped offset back, got {}",
        model.list_scroll()
    );
}

#[test]
fn downward_autoscroll_copy_excludes_titles_above_the_press_row() {
    let (_, mut model) = deck_of(40);
    let _ = page_rows(&model);
    let hits = board_hit_map(STANDARD, &model);
    let rows = page_rows(&model);
    let start_y = rows
        .iter()
        .position(|row| row.contains("task 9"))
        .expect("task 9 visible") as u16;
    let origin = text_select::copyable_line_at(&rows, &hits.copyable, start_y)
        .expect("press row is copyable");
    assert!(
        origin.contains("task 9"),
        "origin must be the pressed title, got {origin:?}"
    );

    model.begin_mouse_press(Position::new(10, start_y));
    model.drag_text_selection(Position::new(10, 20));
    let mut gesture = DragSelectGesture::new();
    let _ = gesture.handle(
        DragSelectPhase::Move,
        Position::new(10, 20),
        model.text_selection(),
    );
    gesture.ensure_copy_origin(origin.clone());

    let content = drag_content_area(&model, STANDARD);
    let auto = DragAutoScrollState {
        direction: AutoScrollDirection::Down,
        speed: 1,
    };
    for _ in 0..12 {
        let rows = page_rows(&model);
        let hits = board_hit_map(STANDARD, &model);
        let before = model.list_scroll();
        tick_drag_autoscroll(
            &mut model,
            &mut gesture,
            auto,
            &rows,
            &hits.copyable,
            content,
        );
        if model.list_scroll() == before {
            break;
        }
    }

    let rows = page_rows(&model);
    let hits = board_hit_map(STANDARD, &model);
    let live = model
        .text_selection()
        .and_then(|sel| text_select::selection_text(&rows, &hits.copyable, &sel));
    let from_origin = model
        .text_selection()
        .is_some_and(|sel| sel.anchor.y <= sel.head.y);
    let text = text_select::compose_selection_copy(
        gesture.captured_before(),
        live,
        gesture.captured_after(),
        gesture.copy_origin(),
        from_origin,
    )
    .expect("copy");
    assert!(
        !text.contains("task 0"),
        "copy must not include titles above the press row:\n{text}"
    );
    assert!(
        text.contains("task 9"),
        "copy must include the press row:\n{text}"
    );
}

#[test]
fn wide_task_drag_uses_task_column_content_edges_for_autoscroll() {
    let mut domain = DomainState::new();
    domain
        .create(
            "Wide task drag",
            Some("drag notes ".repeat(80)),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None).expect("stage A");
    apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None).expect("stage G");

    // Rail 32 + rule 1 + pad 1: the page body runs from the row under the header rule down
    // to the shared footer's rule.
    let content = drag_content_area(&model, Rect::new(0, 0, 110, 24));
    assert_eq!(content, Rect::new(34, 3, 76, 18));

    let mut gesture = DragSelectGesture::new();
    gesture.update_autoscroll(3, content, true);
    assert_eq!(
        gesture.autoscroll().map(|state| state.direction),
        Some(AutoScrollDirection::Up)
    );
    gesture.update_autoscroll(20, content, true);
    assert_eq!(
        gesture.autoscroll().map(|state| state.direction),
        Some(AutoScrollDirection::Down)
    );
}

#[test]
fn task_page_autoscroll_tick_moves_notes() {
    let notes = (0..30)
        .map(|i| format!("page-scroll line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut domain = DomainState::new();
    domain
        .create(
            "Page autoscroll",
            Some(notes),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    let _ = page_rows(&model);
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    let content = drag_content_area(&model, STANDARD);
    assert!(
        content.y > 0 && content.height > 0,
        "task-page content must sit below the header, got {content:?}"
    );
    model.begin_mouse_press(Position::new(10, content.y.saturating_add(2)));
    model.drag_text_selection(Position::new(10, content.y.saturating_add(content.height)));
    let mut gesture = DragSelectGesture::new();
    let head = Position::new(
        10,
        content.y.saturating_add(content.height.saturating_sub(1)),
    );
    let _ = gesture.handle(DragSelectPhase::Move, head, model.text_selection());
    let before = page_rows(&model);
    let hits = board_hit_map(STANDARD, &model);
    tick_drag_autoscroll(
        &mut model,
        &mut gesture,
        DragAutoScrollState {
            direction: AutoScrollDirection::Down,
            speed: 2,
        },
        &before,
        &hits.copyable,
        content,
    );
    let after = page_rows(&model);
    assert_ne!(
        after,
        before,
        "task-page autoscroll tick must move notes:\n{}",
        after.join("\n")
    );
}

#[test]
fn archived_group_is_collapsed_on_a_fresh_model_and_enter_or_click_on_the_header_toggles_it() {
    let mut domain = DomainState::new();
    let archived = domain
        .create(
            "archived click me",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create archived");
    domain.archive_task(archived).expect("archive it");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");

    let hits = board_hit_map(STANDARD, &model);
    let header = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ArchivedHeader))
        .expect("the archived header paints a hit region");
    let intent = map_board_mouse(&model, &hits, left_click(header.area.x, header.area.y))
        .expect("a header click maps to an intent");
    assert_eq!(intent, BoardIntent::ToggleArchivedGroup);
    apply_intent(&mut domain, &mut model, intent, None).expect("toggle");

    assert!(
        model.visible_ids().contains(&archived),
        "the click expanded the group: archived row visible"
    );

    // Click again: collapses.
    let hits = board_hit_map(STANDARD, &model);
    let header = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ArchivedHeader))
        .expect("header hit after expand");
    let intent = map_board_mouse(&model, &hits, left_click(header.area.x, header.area.y))
        .expect("header click maps after expand");
    apply_intent(&mut domain, &mut model, intent, None).expect("toggle closed");
    assert!(
        !model.visible_ids().contains(&archived),
        "the second click collapsed the group"
    );
}

#[test]
fn the_project_picker_tab_row_click_selects_the_tab() {
    let (mut domain, mut model, _id) = board_with_task("tab click", HumanStatus::Ready);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open picker");
    let hits = board_hit_map(STANDARD, &model);
    let tab_hit = hits
        .regions
        .iter()
        .find(|hit| {
            matches!(hit.target, QueueHitTarget::PickerTab(t) if t == tsk_tui::ui::board::PickerTab::Archived)
        })
        .expect("the picker paints tab hit regions");
    let intent = map_board_mouse(&model, &hits, left_click(tab_hit.area.x, tab_hit.area.y))
        .expect("a tab click maps to an intent");
    assert_eq!(
        intent,
        BoardIntent::SelectPickerTab(tsk_tui::ui::board::PickerTab::Archived)
    );
    apply_intent(&mut domain, &mut model, intent, None).expect("switch via click");
    assert_eq!(
        model.picker_tab(),
        Some(tsk_tui::ui::board::PickerTab::Archived),
        "the click switched to the archived tab"
    );
}

#[test]
fn expanded_capture_long_notes_can_scroll_to_add_step_while_editing() {
    let (mut domain, mut model) = deck_of(1);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None).unwrap();
    apply_intent(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None).unwrap();
    let notes = (0..40)
        .map(|i| format!("note line {i}\n"))
        .collect::<String>();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText(notes),
        None,
    )
    .unwrap();
    let area = Rect::new(0, 0, 78, 13);
    for _ in 0..80 {
        let hits = board_hit_map(area, &model);
        let intent = map_board_mouse(&model, &hits, wheel_down(5, 5))
            .expect("wheel works while editing notes");
        apply_intent(&mut domain, &mut model, intent, None).unwrap();
    }
    let hits = board_hit_map(area, &model);
    assert!(
        hits.regions
            .iter()
            .any(|hit| hit.target == QueueHitTarget::StepAdd),
        "scroll exposes add step"
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    let mut dragging = false;
    let track = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::PageScroll(0)))
        .expect("top of scrollbar");
    let mouse = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: track.area.x,
        row: track.area.y,
        modifiers: KeyModifiers::NONE,
    };
    let ScrollbarMouse::Intent(intent) =
        map_scrollbar_mouse(model.input_mode(), &hits, mouse, &mut dragging)
    else {
        panic!("scrollbar works during notes edit")
    };
    apply_intent(&mut domain, &mut model, intent, None).unwrap();
    assert_eq!(model.page_scroll(), 0);
    let hits = board_hit_map(area, &model);
    assert!(!hits
        .regions
        .iter()
        .any(|hit| hit.target == QueueHitTarget::StepAdd));
}

#[test]
fn draft_text_click_places_title_and_notes_cursor() {
    let (mut domain, mut model) = deck_of(1);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None).unwrap();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::QuickAddInsertText("abcdef".into()),
        None,
    )
    .unwrap();
    apply_intent(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None).unwrap();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("abcdef\nsecond".into()),
        None,
    )
    .unwrap();
    let area = Rect::new(0, 0, 78, 13);
    let hits = board_hit_map(area, &model);
    let note = hits
        .regions
        .iter()
        .find(|h| h.target == QueueHitTarget::FormNotes(0))
        .unwrap();
    let intent = map_board_mouse(&model, &hits, left_click(note.area.x + 4, note.area.y)).unwrap();
    apply_intent(&mut domain, &mut model, intent, None).unwrap();
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('X'), None).unwrap();
    let rows = page_rows(&model).join("\n");
    assert!(
        rows.contains("abXcdef"),
        "click inserts at note character, not end: {rows}"
    );
    let hits = board_hit_map(area, &model);
    let title = hits
        .regions
        .iter()
        .find(|h| h.target == QueueHitTarget::FormTitle)
        .unwrap();
    let intent =
        map_board_mouse(&model, &hits, left_click(title.area.x + 6, title.area.y)).unwrap();
    apply_intent(&mut domain, &mut model, intent, None).unwrap();
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('Y'), None).unwrap();
    assert!(page_rows(&model).join("\n").contains("abYcdef"));
}

#[test]
fn popup_step_draft_scrollbar_and_refused_field_click_preserve_editor() {
    let (mut domain, mut model) = deck_of(1);
    for intent in [
        BoardIntent::OpenCapture,
        BoardIntent::QuickAddInsertText("title".into()),
        BoardIntent::ExpandQuickAdd,
        BoardIntent::EditInsertText("long note\n".repeat(40)),
        BoardIntent::BeginAddStep,
        BoardIntent::EditInsertText("step draft".into()),
    ] {
        apply_intent(&mut domain, &mut model, intent, None).unwrap();
    }
    let area = Rect::new(0, 0, 78, 13);
    let hits = board_hit_map(area, &model);
    let track = hits
        .regions
        .iter()
        .find(|h| h.target == QueueHitTarget::PageScroll(0))
        .unwrap();
    let mut dragging = false;
    let ScrollbarMouse::Intent(intent) = map_scrollbar_mouse(
        model.input_mode(),
        &hits,
        left_click(track.area.x, track.area.y),
        &mut dragging,
    ) else {
        panic!("step draft scrollbar must work")
    };
    apply_intent(&mut domain, &mut model, intent, None).unwrap();
    assert_eq!(model.page_scroll(), 0);
    let hits = board_hit_map(area, &model);
    let title = hits
        .regions
        .iter()
        .find(|h| h.target == QueueHitTarget::FormTitle)
        .unwrap();
    let intent =
        map_board_mouse(&model, &hits, left_click(title.area.x + 4, title.area.y)).unwrap();
    apply_intent(&mut domain, &mut model, intent, None).unwrap();
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('X'), None).unwrap();
    for _ in 0..100 {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::PageWheelScrollDown,
            None,
        )
        .unwrap();
    }
    assert!(
        page_rows(&model).join("\n").contains("step draftX"),
        "refused field click must not move the step cursor"
    );
}

#[test]
fn popup_click_uses_scrolled_wrapped_unicode_cells() {
    let (mut domain, mut model) = deck_of(1);
    let snapshot = tsk_tui::context::InvocationSnapshot {
        default_scope: TaskScope::Global,
        this_repo: None,
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    };
    for intent in [
        BoardIntent::OpenCapture,
        BoardIntent::QuickAddInsertText("界a界b".into()),
        BoardIntent::ExpandQuickAdd,
        BoardIntent::EditInsertText(format!("{}界a界b\n{}", "a".repeat(72), "tail\n".repeat(30))),
    ] {
        apply_intent(&mut domain, &mut model, intent, Some(&snapshot)).unwrap();
    }
    let area = Rect::new(0, 0, 78, 13);
    board_hit_map(area, &model);
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollTo(1), None).unwrap();
    let hits = board_hit_map(area, &model);
    let note = hits
        .regions
        .iter()
        .find(|h| h.target == QueueHitTarget::FormNotes(1))
        .unwrap();
    let intent = map_board_mouse(&model, &hits, left_click(note.area.x + 5, note.area.y)).unwrap();
    apply_intent(&mut domain, &mut model, intent, Some(&snapshot)).unwrap();
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('X'), None).unwrap();

    let hits = board_hit_map(area, &model);
    let title = hits
        .regions
        .iter()
        .find(|h| h.target == QueueHitTarget::FormTitle)
        .unwrap();
    let intent =
        map_board_mouse(&model, &hits, left_click(title.area.x + 7, title.area.y)).unwrap();
    apply_intent(&mut domain, &mut model, intent, Some(&snapshot)).unwrap();
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('Y'), None).unwrap();
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).unwrap();
    let task = domain
        .tasks()
        .iter()
        .find(|task| task.title == "界aY界b")
        .expect("title Unicode click")
        .clone();
    assert_eq!(
        task.notes.as_deref(),
        Some(format!("{}界aX界b\n{}", "a".repeat(72), "tail\n".repeat(30)).as_str())
    );
}

#[test]
fn popup_typing_and_cursor_movement_restore_notes_after_manual_scroll() {
    for intent in [BoardIntent::EditInsert('X'), BoardIntent::EditMoveLeft] {
        let (mut domain, mut model) = deck_of(1);
        for intent in [
            BoardIntent::OpenCapture,
            BoardIntent::ExpandQuickAdd,
            BoardIntent::EditInsertText(format!("{}LASTLINE", "start\n".repeat(40))),
        ] {
            apply_intent(&mut domain, &mut model, intent, None).unwrap();
        }
        let area = Rect::new(0, 0, 78, 13);
        board_hit_map(area, &model);
        apply_intent(&mut domain, &mut model, BoardIntent::PageScrollTo(0), None).unwrap();
        assert!(!page_rows(&model).join("\n").contains("LASTLINE"));
        apply_intent(&mut domain, &mut model, intent, None).unwrap();
        assert!(page_rows(&model).join("\n").contains("LASTLINE"));
    }
}

#[test]
fn peek_attribution_is_copyable_but_not_a_task_click_target() {
    let mut domain = DomainState::new();
    domain
        .create(
            "peek target",
            Some("note body".into()),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            Some("release".into()),
        )
        .unwrap();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::PeekDetail, None).unwrap();
    let rows = page_rows(&model);
    let y = rows
        .iter()
        .position(|row| row.contains("└─ #release"))
        .unwrap() as u16;
    let hits = board_hit_map(STANDARD, &model);
    assert!(!hits.regions.iter().any(|hit| hit.area.y == y));
    assert!(hits
        .copyable
        .iter()
        .any(|area| area.y == y && area.x == 7 && area.width == 14));
    assert_eq!(map_board_mouse(&model, &hits, left_click(8, y)), None);
}
