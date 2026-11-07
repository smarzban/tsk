//! The board's inline project title editor (T459): `ctrl+e` on a Projects view row or a
//! project picker row, and the palette's **set project title…** on a project tab.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tsk_tui::app::{apply_board_intent_with_save_recovery, BoardSaveContext};
use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::save_recovery::SaveRecovery;
use tsk_tui::ui::board::{
    apply_intent, draw_board, resolve_board_command, BoardInputMode, BoardModel, IntentOutcome,
    ProjectScopeOption,
};
use tsk_tui::ui::input::{map_key, BoardIntent};
use tsk_tui::ui::queue::NavTab;

const SITE_A: &str = "/w/site-a/wp-content";
const SITE_B: &str = "/w/site-b/wp-content";

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn board() -> (DomainState, BoardModel) {
    let mut domain = DomainState::new();
    for path in [SITE_A, SITE_B] {
        domain
            .create(
                "seed",
                None,
                TaskScope::Project { path: path.into() },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create");
    }
    let model = BoardModel::from_domain(&domain, None);
    (domain, model)
}

fn apply(domain: &mut DomainState, model: &mut BoardModel, intent: BoardIntent) -> IntentOutcome {
    apply_intent(domain, model, intent, None).expect("intent applies")
}

/// Press `key` in the board's current mode, as the keyboard boundary maps it.
fn key(domain: &mut DomainState, model: &mut BoardModel, key: KeyEvent) -> IntentOutcome {
    let intent = map_key(model.input_mode(), key).expect("the key maps");
    apply(domain, model, intent)
}

fn type_text(domain: &mut DomainState, model: &mut BoardModel, text: &str) {
    for character in text.chars() {
        key(domain, model, press(KeyCode::Char(character)));
    }
}

fn clear_draft(domain: &mut DomainState, model: &mut BoardModel) {
    apply(domain, model, BoardIntent::EditDeleteToLineStart);
    apply(domain, model, BoardIntent::EditDeleteToLineEnd);
}

fn rendered(model: &BoardModel, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn projects_view_on(domain: &mut DomainState, model: &mut BoardModel, path: &str) {
    apply(domain, model, BoardIntent::SelectNavTab(NavTab::Projects));
    for _ in 0..4 {
        if model
            .selected_project_row()
            .is_some_and(|row| row.path == path)
        {
            return;
        }
        apply(domain, model, BoardIntent::SelectNext);
    }
    panic!("no Projects row for {path}");
}

#[test]
fn ctrl_e_on_a_projects_row_titles_it_and_a_duplicate_keeps_the_editor_open() {
    let (mut domain, mut model) = board();
    projects_view_on(&mut domain, &mut model, SITE_A);

    assert_eq!(
        key(&mut domain, &mut model, ctrl(KeyCode::Char('e'))),
        IntentOutcome::None
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditProjectTitle);
    let frame = rendered(&model, 100, 24);
    assert!(
        frame.contains(SITE_A),
        "the footer names the full path while editing:\n{frame}"
    );

    clear_draft(&mut domain, &mut model);
    type_text(&mut domain, &mut model, "Site A");
    assert_eq!(
        key(&mut domain, &mut model, press(KeyCode::Enter)),
        IntentOutcome::Persist
    );
    assert_eq!(domain.project_title(SITE_A), Some("Site A"));
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditProjectTitle,
        "the editor outlives the save call until the confirmed sync"
    );
    model.sync_from_domain(&domain);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(
        model.selected_project_row().map(|row| row.path),
        Some(SITE_A.to_string()),
        "the cursor stays on the renamed project"
    );
    let frame = rendered(&model, 100, 24);
    assert!(
        frame.contains("Site A"),
        "the row shows the title:\n{frame}"
    );

    // The other project cannot take the title, in any case: refusal on the status row, the
    // editor and its draft stay.
    projects_view_on(&mut domain, &mut model, SITE_B);
    key(&mut domain, &mut model, ctrl(KeyCode::Char('e')));
    clear_draft(&mut domain, &mut model);
    type_text(&mut domain, &mut model, "site a");
    assert_eq!(
        key(&mut domain, &mut model, press(KeyCode::Enter)),
        IntentOutcome::None
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditProjectTitle);
    assert_eq!(
        model.message(),
        Some(format!("title site a is used by {SITE_A}").as_str())
    );
    assert_eq!(domain.project_title(SITE_B), None);
    let frame = rendered(&model, 100, 24);
    assert!(frame.contains("title site a is used by"), "{frame}");

    // Esc cancels: nothing saved, the row keeps its folder name.
    key(&mut domain, &mut model, press(KeyCode::Esc));
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(domain.project_title(SITE_B), None);

    // ctrl+u undoes the title as one entry.
    let undo = map_key(BoardInputMode::Normal, ctrl(KeyCode::Char('u'))).expect("ctrl+u");
    assert_eq!(apply(&mut domain, &mut model, undo), IntentOutcome::Persist);
    assert_eq!(domain.project_title(SITE_A), None);
    assert_eq!(
        model.message(),
        Some("project title undone \u{b7} wp-content")
    );
}

#[test]
fn an_unchanged_folder_name_saves_nothing_and_an_empty_draft_clears() {
    let (mut domain, mut model) = board();
    projects_view_on(&mut domain, &mut model, SITE_A);
    key(&mut domain, &mut model, ctrl(KeyCode::Char('e')));
    assert_eq!(
        key(&mut domain, &mut model, press(KeyCode::Enter)),
        IntentOutcome::None,
        "the prefilled folder name is not a title"
    );
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert!(domain.projects().is_empty());

    domain.set_project_title(SITE_A, "Site A").expect("title");
    model.sync_from_domain(&domain);
    key(&mut domain, &mut model, ctrl(KeyCode::Char('e')));
    clear_draft(&mut domain, &mut model);
    assert_eq!(
        key(&mut domain, &mut model, press(KeyCode::Enter)),
        IntentOutcome::Persist
    );
    assert_eq!(domain.project_title(SITE_A), None, "empty clears the title");
}

#[test]
fn ctrl_e_in_the_picker_titles_the_highlighted_project_and_ctrl_u_undoes_it_there() {
    let (mut domain, mut model) = board();
    apply(&mut domain, &mut model, BoardIntent::OpenProjectSelector);
    let target = ProjectScopeOption::Project(PathBuf::from(SITE_B));
    while model.project_options()[model.project_picker_index().expect("index")] != target {
        apply(&mut domain, &mut model, BoardIntent::ProjectPickerNext);
    }
    key(&mut domain, &mut model, ctrl(KeyCode::Char('e')));
    assert_eq!(model.input_mode(), BoardInputMode::EditProjectTitle);
    clear_draft(&mut domain, &mut model);
    type_text(&mut domain, &mut model, "Site B");
    let frame = rendered(&model, 100, 24);
    assert!(frame.contains("Site B"), "the row edits in place:\n{frame}");
    assert!(
        frame.contains(SITE_B),
        "the footer names the path:\n{frame}"
    );
    assert_eq!(
        key(&mut domain, &mut model, press(KeyCode::Enter)),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(
        model.input_mode(),
        BoardInputMode::ProjectPicker,
        "the picker stays open"
    );
    assert_eq!(
        model.project_options()[model.project_picker_index().expect("index")],
        target,
        "the highlight follows the renamed project"
    );
    let frame = rendered(&model, 100, 24);
    assert!(
        frame.contains("Site B"),
        "the picker lists the title:\n{frame}"
    );

    key(&mut domain, &mut model, ctrl(KeyCode::Char('u')));
    assert_eq!(
        domain.project_title(SITE_B),
        None,
        "ctrl+u undoes it in the picker"
    );
    assert_eq!(model.input_mode(), BoardInputMode::ProjectPicker);
}

#[test]
fn the_palette_on_a_project_tab_edits_its_title_in_the_picker_and_closes_it() {
    let (mut domain, mut model) = board();
    apply(&mut domain, &mut model, BoardIntent::OpenProjectSelector);
    let target = ProjectScopeOption::Project(PathBuf::from(SITE_A));
    while model.project_options()[model.project_picker_index().expect("index")] != target {
        apply(&mut domain, &mut model, BoardIntent::ProjectPickerNext);
    }
    apply(&mut domain, &mut model, BoardIntent::ConfirmProjectChoice);

    // Drive the real palette: `:`, type the command's name, Enter.
    apply(&mut domain, &mut model, BoardIntent::OpenCommandPalette);
    apply(
        &mut domain,
        &mut model,
        BoardIntent::CommandQueryInsertText("set project title".into()),
    );
    let enter = map_key(model.input_mode(), press(KeyCode::Enter)).expect("enter maps");
    let resolved = resolve_board_command(&mut model, enter).expect("the palette runs a command");
    apply(&mut domain, &mut model, resolved);
    assert_eq!(model.input_mode(), BoardInputMode::EditProjectTitle);
    clear_draft(&mut domain, &mut model);
    type_text(&mut domain, &mut model, "Alpha");
    key(&mut domain, &mut model, press(KeyCode::Enter));
    model.sync_from_domain(&domain);
    assert_eq!(
        model.input_mode(),
        BoardInputMode::Normal,
        "the picker closed"
    );
    assert_eq!(domain.project_title(SITE_A), Some("Alpha"));
    let frame = rendered(&model, 100, 24);
    assert!(
        frame.contains("Alpha"),
        "the project tab shows the title:\n{frame}"
    );
}

#[test]
fn the_picker_and_the_projects_view_sort_by_the_title() {
    let (mut domain, mut model) = board();
    // By folder path, site-a sorts first; titles reverse that under name A-Z.
    domain.set_project_title(SITE_A, "Zulu").expect("title A");
    domain.set_project_title(SITE_B, "Alpha").expect("title B");
    model.sync_from_domain(&domain);
    assert_eq!(
        model.project_options(),
        vec![
            ProjectScopeOption::Home,
            ProjectScopeOption::Project(PathBuf::from(SITE_B)),
            ProjectScopeOption::Project(PathBuf::from(SITE_A)),
        ],
        "the picker sorts by the shown name"
    );
    apply(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(NavTab::Projects),
    );
    let rows: Vec<String> = model
        .queue_view()
        .projects
        .into_iter()
        .map(|row| row.path)
        .collect();
    assert_eq!(rows, vec![SITE_B.to_string(), SITE_A.to_string()]);
}

#[test]
fn ctrl_e_on_the_taskless_here_row_refuses_without_opening_the_editor() {
    let mut domain = DomainState::new();
    domain
        .create(
            "seed",
            None,
            TaskScope::Project {
                path: SITE_A.into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/w/here")));
    projects_view_on(&mut domain, &mut model, "/w/here");
    key(&mut domain, &mut model, ctrl(KeyCode::Char('e')));
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(
        model.message(),
        Some("here has no tasks yet: add one, then title it")
    );
    assert!(domain.projects().is_empty());
}

#[test]
fn a_failed_save_keeps_the_editor_and_its_draft_through_cancel() {
    let (mut domain, mut model) = board();
    projects_view_on(&mut domain, &mut model, SITE_A);
    key(&mut domain, &mut model, ctrl(KeyCode::Char('e')));
    clear_draft(&mut domain, &mut model);
    type_text(&mut domain, &mut model, "Site A");
    let mut recovery = SaveRecovery::new();
    let baseline = domain.clone();
    let outcome = apply_board_intent_with_save_recovery(
        &mut domain,
        &mut model,
        &mut recovery,
        BoardSaveContext {
            baseline: baseline.clone(),
            intent: BoardIntent::ConfirmProjectTitle,
            snapshot: None,
        },
        |_| Err("disk full".to_string()),
    )
    .expect("the save fails into recovery");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);

    apply_board_intent_with_save_recovery(
        &mut domain,
        &mut model,
        &mut recovery,
        BoardSaveContext {
            baseline,
            intent: BoardIntent::CancelSave,
            snapshot: None,
        },
        |_| Err("disk full".to_string()),
    )
    .expect("cancel");
    assert_eq!(domain.project_title(SITE_A), None, "nothing saved");
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditProjectTitle,
        "the editor is still open"
    );
    let frame = rendered(&model, 100, 24);
    assert!(frame.contains("Site A"), "the draft survives:\n{frame}");
}
