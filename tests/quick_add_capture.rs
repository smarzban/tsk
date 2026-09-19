//! Capture-bar regression coverage.

use std::cell::RefCell;
use std::io::{self, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use tsk_tui::agents::AgentProfiles;
use tsk_tui::app::{apply_board_intent_with_save_recovery, BoardSaveContext};
use tsk_tui::context::InvocationSnapshot;
use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskEventKind, TaskScope};
use tsk_tui::save_recovery::SaveRecovery;
use tsk_tui::ui::board::{
    apply_intent, board_hit_map, draw_board, BoardInputMode, BoardModel, IntentOutcome,
};
use tsk_tui::ui::input::{map_key, BoardIntent};

fn snapshot() -> InvocationSnapshot {
    InvocationSnapshot {
        default_scope: TaskScope::Project {
            path: "/repos/invocation".into(),
        },
        this_repo: Some(PathBuf::from("/repos/invocation")),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    }
}

fn apply(
    domain: &mut DomainState,
    model: &mut BoardModel,
    intent: BoardIntent,
    snapshot: Option<&InvocationSnapshot>,
) -> IntentOutcome {
    apply_intent(domain, model, intent, snapshot).expect("apply capture-bar intent")
}

fn open(domain: &mut DomainState, model: &mut BoardModel, snapshot: &InvocationSnapshot) {
    assert_eq!(
        apply(domain, model, BoardIntent::OpenCapture, Some(snapshot)),
        IntentOutcome::None
    );
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
}

fn type_title(domain: &mut DomainState, model: &mut BoardModel, title: &str) {
    for character in title.chars() {
        apply(domain, model, BoardIntent::QuickAddInsert(character), None);
    }
}

fn set_agent_profiles(model: &mut BoardModel, names: &[&str]) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "tsk-quick-add-agents-{nanos}-{}",
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create agents dir");
    let content = names
        .iter()
        .map(|name| format!("[agent.{name}]\ncommand = [\"true\"]\n"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("agents.toml"), content).expect("write profiles");
    let profiles = AgentProfiles::load(&dir).expect("load profiles");
    model.set_agent_profiles(&profiles);
    std::fs::remove_dir_all(dir).expect("remove agents dir");
}

#[test]
fn plus_opens_focused_bar_regardless_of_shift_and_legacy_chord_is_unbound() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/invocation")));
    let snap = snapshot();

    for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
        assert_eq!(
            map_key(
                BoardInputMode::Normal,
                KeyEvent::new(KeyCode::Char('+'), modifiers)
            ),
            Some(BoardIntent::OpenCapture)
        );
    }
    assert_eq!(
        map_key(
            BoardInputMode::Normal,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
        ),
        None
    );
    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "capture this");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    // The reducer retains the draft until the save boundary refreshes from the durable state.
    model.sync_from_domain(&domain);

    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(model.visible_tasks().len(), 1);
    let task = domain.tasks().first().expect("created task");
    assert_eq!(task.title, "capture this");
    assert_eq!(
        task.scope,
        TaskScope::Project {
            path: "/repos/invocation".into()
        }
    );
    assert_eq!(task.provenance, ProvenanceOrigin::Capture);
    assert_eq!(model.selected_id(), Some(task.id));
    assert_eq!(model.message(), None);
}

#[test]
fn expanded_capture_ctrl_a_opens_step_add() {
    assert_eq!(
        tsk_tui::ui::input::map_task_form_key(
            tsk_tui::ui::capture::CaptureField::Title,
            false,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        ),
        Some(BoardIntent::BeginAddStep)
    );
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();
    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "ctrl a step");
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    apply(&mut domain, &mut model, BoardIntent::BeginAddStep, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    apply(&mut domain, &mut model, BoardIntent::CancelEdit, None);
    assert_ne!(model.input_mode(), BoardInputMode::EditStep);
}

#[test]
fn scoped_project_quick_add_defaults_to_the_selected_project() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/repos/project-x");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/project-y")));
    model.set_selected_project(Some(PathBuf::from("/repos/project-x")));
    let mut snap = snapshot();
    snap.default_scope = TaskScope::Project {
        path: "/repos/project-y".into(),
    };

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "selected project task");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);

    assert_eq!(
        domain.tasks().last().expect("saved task").scope,
        TaskScope::Project {
            path: "/repos/project-x".into()
        }
    );
}

#[test]
fn all_projects_quick_add_keeps_the_invocation_default_scope() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/repos/project-x");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/project-y")));
    let mut snap = snapshot();
    snap.default_scope = TaskScope::Project {
        path: "/repos/project-y".into(),
    };

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "all projects task");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);

    assert_eq!(
        domain.tasks().last().expect("saved task").scope,
        TaskScope::Project {
            path: "/repos/project-y".into()
        }
    );
}

#[test]
fn home_board_quick_add_keeps_the_invocation_default_scope() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/project-y")));
    let mut snap = snapshot();
    snap.default_scope = TaskScope::Project {
        path: "/repos/project-y".into(),
    };

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "home board task");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);

    assert_eq!(
        domain.tasks().last().expect("saved task").scope,
        TaskScope::Project {
            path: "/repos/project-y".into()
        }
    );
}

#[test]
fn non_git_desk_quick_add_stays_on_desk_until_the_directory_project_is_opened() {
    let mut domain = DomainState::new();
    let outside = PathBuf::from("/work/outside-git");
    let snap = InvocationSnapshot {
        default_scope: TaskScope::Global,
        this_repo: Some(outside.clone()),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    };
    let mut model = BoardModel::from_domain_for_snapshot(&domain, &snap);

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "desk task");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(
        domain.tasks().last().expect("desk task").scope,
        TaskScope::Global
    );

    model.set_selected_project(Some(outside.clone()));
    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "directory task");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(
        domain.tasks().last().expect("directory task").scope,
        TaskScope::Project {
            path: outside.to_string_lossy().into_owned()
        }
    );
}

#[test]
fn quick_add_project_token_overrides_the_selected_project() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/repos/project-x");
    create_project_fixture(&mut domain, "/repos/project-z");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/project-y")));
    model.set_selected_project(Some(PathBuf::from("/repos/project-x")));
    let mut snap = snapshot();
    snap.default_scope = TaskScope::Project {
        path: "/repos/project-y".into(),
    };

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "token wins !p project-z");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);

    assert_eq!(
        domain.tasks().last().expect("saved task").scope,
        TaskScope::Project {
            path: "/repos/project-z".into()
        }
    );
}

#[test]
fn expanded_quick_add_keeps_the_selected_project_scope_through_esc_and_tab() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/repos/project-x");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/project-y")));
    model.set_selected_project(Some(PathBuf::from("/repos/project-x")));
    let mut snap = snapshot();
    snap.default_scope = TaskScope::Project {
        path: "/repos/project-y".into(),
    };

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "expanded selected project task");
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    assert_eq!(
        model.form_scope(),
        Some(&TaskScope::Project {
            path: "/repos/project-x".into()
        })
    );

    apply(&mut domain, &mut model, BoardIntent::CancelEdit, None);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    assert_eq!(
        model.form_scope(),
        Some(&TaskScope::Project {
            path: "/repos/project-x".into()
        })
    );
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::ConfirmEdit, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);

    assert_eq!(
        domain.tasks().last().expect("saved task").scope,
        TaskScope::Project {
            path: "/repos/project-x".into()
        }
    );
}

#[test]
fn quick_add_save_selects_the_new_task_and_navigation_stays_relative_to_it() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/repos/existing");
    domain
        .create(
            "desk fixture",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("desk fixture");
    let mut model = BoardModel::from_domain(&domain, None);
    let prior_selection = model.selected_id().expect("desk fixture is selected");

    let desk_snapshot = InvocationSnapshot {
        default_scope: TaskScope::Global,
        this_repo: None,
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    };
    open(&mut domain, &mut model, &desk_snapshot);
    type_title(&mut domain, &mut model, "new task");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    let saved = domain.tasks().last().expect("saved task").id;
    assert_eq!(model.selected_id(), Some(saved));
    assert_ne!(saved, prior_selection);

    let visible = model.visible_ids();
    let saved_index = visible
        .iter()
        .position(|id| *id == saved)
        .expect("saved task is visible");
    let expected_down = visible[(saved_index + 1) % visible.len()];
    apply(&mut domain, &mut model, BoardIntent::SelectNext, None);
    assert!(model.inbox_header_selected());
    assert_eq!(model.selected_id(), None);
    assert_eq!(
        expected_down, visible[0],
        "the saved row wraps to the inbox heading"
    );

    apply(&mut domain, &mut model, BoardIntent::SelectPrev, None);
    assert_eq!(model.selected_id(), Some(saved));
    apply(
        &mut domain,
        &mut model,
        BoardIntent::OpenCapture,
        Some(&snapshot()),
    );
    assert_eq!(model.selected_id(), Some(saved));
}

fn create_project_fixture(domain: &mut DomainState, path: &str) {
    domain
        .create(
            "known project",
            None,
            TaskScope::Project { path: path.into() },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("fixture project task");
}

fn save_quick_add(domain: &mut DomainState, model: &mut BoardModel, title: &str) {
    open(domain, model, &snapshot());
    type_title(domain, model, title);
    assert_eq!(
        apply(domain, model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(domain);
}

#[test]
fn unique_project_basename_resolves_for_expansion_without_a_saved_status_message() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/work/tsk");
    let mut model = BoardModel::from_domain(&domain, None);

    open(&mut domain, &mut model, &snapshot());
    type_title(&mut domain, &mut model, "expanded task !p tsk");
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    assert_eq!(
        model.form_scope(),
        Some(&TaskScope::Project {
            path: "/work/tsk".into()
        })
    );
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::ConfirmEdit, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(
        domain.tasks().last().expect("saved task").scope,
        TaskScope::Project {
            path: "/work/tsk".into()
        }
    );
    assert_eq!(model.message(), None);
}

#[test]
fn invocation_default_scope_and_this_repo_are_project_candidates() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let mut snap = snapshot();
    snap.default_scope = TaskScope::Project {
        path: "/repos/default-project".into(),
    };
    snap.this_repo = Some(PathBuf::from("/repos/current-project"));

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "from default !p default-project");
    apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None);
    model.sync_from_domain(&domain);
    assert_eq!(
        domain.tasks().last().expect("default task").scope,
        TaskScope::Project {
            path: "/repos/default-project".into()
        }
    );

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "from repo !p current-project");
    apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None);
    model.sync_from_domain(&domain);
    assert_eq!(
        domain.tasks().last().expect("repo task").scope,
        TaskScope::Project {
            path: "/repos/current-project".into()
        }
    );
}

#[test]
fn unmatched_project_basename_refuses_and_keeps_the_last_good_destination() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    apply(
        &mut domain,
        &mut model,
        BoardIntent::QuickAddInsertText("unmatched task !p missing".into()),
        None,
    );
    let rows = render_rows(&model, 80, 24).join("\n");
    assert!(rows.contains("add to invocation"), "{rows}");
    assert!(!rows.contains("add to missing"), "{rows}");

    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(model.quick_add_title_value(), "unmatched task !p missing");
    assert_eq!(model.message(), Some("project missing is not on the board"));
    assert!(domain.tasks().is_empty());
}

#[test]
fn ambiguous_project_basename_refuses_and_names_sorted_candidates() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/work/one/shared");
    create_project_fixture(&mut domain, "/work/two/shared");
    let mut model = BoardModel::from_domain(&domain, None);

    open(&mut domain, &mut model, &snapshot());
    type_title(&mut domain, &mut model, "ambiguous task !p shared");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    assert_eq!(
        model.message(),
        Some("project shared is ambiguous: /work/one/shared, /work/two/shared")
    );
    assert_eq!(domain.tasks().len(), 2, "the draft was not saved");
}

#[test]
fn relative_project_path_refuses_without_saving() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);

    open(&mut domain, &mut model, &snapshot());
    type_title(&mut domain, &mut model, "explicit task !p elsewhere/tsk");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    assert_eq!(model.message(), Some("no directory at elsewhere/tsk"));
    assert!(domain.tasks().is_empty());
}

#[test]
fn shift_enter_uses_the_same_project_basename_resolution() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/work/ctrl-target");
    let mut model = BoardModel::from_domain(&domain, None);

    open(&mut domain, &mut model, &snapshot());
    type_title(&mut domain, &mut model, "keep open !p ctrl-target");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSaveNext, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    let task = domain.tasks().last().expect("saved task");
    assert_eq!(
        task.scope,
        TaskScope::Project {
            path: "/work/ctrl-target".into()
        }
    );
    assert_eq!(
        model.selected_id(),
        None,
        "saved project task is outside the desk lens"
    );
    assert_eq!(model.message(), None);
}

#[test]
fn capture_bar_strips_bare_project_token_as_global_and_project_scope_tokens() {
    let snap = snapshot();
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "global task !p");
    apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None);
    model.sync_from_domain(&domain);
    assert_eq!(domain.tasks()[0].title, "global task");
    assert_eq!(domain.tasks()[0].scope, TaskScope::Global);

    let project_path = env!("CARGO_MANIFEST_DIR");
    open(&mut domain, &mut model, &snap);
    type_title(
        &mut domain,
        &mut model,
        &format!("project task !p {project_path}"),
    );
    apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None);
    model.sync_from_domain(&domain);
    assert_eq!(domain.tasks()[1].title, "project task");
    assert_eq!(
        domain.tasks()[1].scope,
        TaskScope::Project {
            path: project_path.into()
        }
    );

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "next task");
    apply(&mut domain, &mut model, BoardIntent::QuickAddSaveNext, None);
    model.sync_from_domain(&domain);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(model.quick_add_title_value(), "");
    apply(&mut domain, &mut model, BoardIntent::CancelQuickAdd, None);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(model.message(), None);

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "legacy !g");
    apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None);
    model.sync_from_domain(&domain);
    let task = domain.tasks().last().expect("saved legacy token task");
    assert_eq!(task.title, "legacy !g");
    assert_eq!(
        task.scope,
        TaskScope::Project {
            path: "/repos/invocation".into()
        }
    );
}

#[test]
fn bare_project_token_requires_a_title_and_preselects_global_when_expanded() {
    let snap = snapshot();
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "!p");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    assert_eq!(model.message(), Some("Title required"));
    assert!(domain.tasks().is_empty());

    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    assert_eq!(model.form_scope(), Some(&TaskScope::Global));
}

#[test]
fn empty_enter_stays_open_esc_discards_and_tab_expands_the_seeded_task_page() {
    let snap = snapshot();
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);

    open(&mut domain, &mut model, &snap);
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    assert_eq!(model.message(), Some("Title required"));
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert!(domain.tasks().is_empty());

    apply(&mut domain, &mut model, BoardIntent::CancelQuickAdd, None);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert!(domain.tasks().is_empty());

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "needs notes");
    assert_eq!(
        map_key(
            BoardInputMode::QuickAdd,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
        ),
        Some(BoardIntent::ExpandQuickAdd)
    );
    assert_eq!(
        map_key(
            BoardInputMode::QuickAdd,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)
        ),
        None,
        "Alt+Enter is ignored, it does not expand quick add"
    );
    assert_eq!(
        map_key(
            BoardInputMode::QuickAdd,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
        ),
        Some(BoardIntent::QuickAddSaveNext),
        "Shift+Enter is the only save-and-stay quick-add chord"
    );
    assert_eq!(
        map_key(
            BoardInputMode::QuickAdd,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)
        ),
        None,
        "Ctrl+Enter no longer saves-and-stays on quick add"
    );
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    assert_eq!(model.edit_buffer(), "");
    assert_eq!(
        model.form_focus(),
        Some(tsk_tui::ui::capture::CaptureField::Notes)
    );
    assert_eq!(
        model.form_scope(),
        Some(&TaskScope::Project {
            path: "/repos/invocation".into()
        })
    );
    apply(&mut domain, &mut model, BoardIntent::CancelEdit, None);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(model.quick_add_title_value(), "needs notes");
}

#[test]
fn expanded_page_stashes_notes_and_scope_across_esc_and_saves_like_quick_add() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();
    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "draft with details !p");

    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    assert_eq!(
        model.form_focus(),
        Some(tsk_tui::ui::capture::CaptureField::Notes)
    );
    let page = render_text(&model, 80, 24);
    assert!(page.contains("draft with details"));
    assert!(page.contains("desk"));
    assert!(
        !page.contains("title…"),
        "expanded draft uses the task page, not the quick-add row"
    );
    assert!(render_rows(&model, 40, 10)
        .iter()
        .all(|row| row.chars().count() == 40));
    for character in "preserved note".chars() {
        apply(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        );
    }
    assert_eq!(
        tsk_tui::ui::input::map_board_form_key(
            tsk_tui::ui::capture::CaptureField::Notes,
            false,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        ),
        Some(BoardIntent::FormFocusNext),
        "Tab in the page advances the form rather than re-expanding quick add"
    );
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert_eq!(model.input_mode(), BoardInputMode::CapturePage);
    assert!(
        render_text(&model, 80, 24).contains("▸ + step"),
        "Tab from Notes selects the trailing step target"
    );
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditThread);
    assert_eq!(
        model.form_focus(),
        Some(tsk_tui::ui::capture::CaptureField::Thread)
    );
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
    assert_eq!(model.form_scope(), Some(&TaskScope::Global));
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditAssignee);
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);

    apply(&mut domain, &mut model, BoardIntent::CancelEdit, None);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(model.quick_add_title_value(), "draft with details !p");
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    assert_eq!(model.edit_buffer(), "preserved note");
    assert_eq!(model.form_scope(), Some(&TaskScope::Global));

    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::ConfirmEdit, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert!(model.has_saved_task(), "saved task remains emphasized");
    assert_eq!(model.message(), None);
    let task = domain.tasks().last().expect("saved task");
    assert_eq!(model.selected_id(), Some(task.id));
    assert_eq!(task.title, "draft with details");
    assert_eq!(task.notes.as_deref(), Some("preserved note"));
    assert_eq!(task.scope, TaskScope::Global);
}

#[test]
fn t_token_threads_while_hash_words_stay_title_text() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "ship #urgent !t Release-2026");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);

    let task = domain.tasks().last().expect("saved task");
    assert_eq!(task.title, "ship #urgent");
    assert_eq!(task.thread.as_deref(), Some("release-2026"));
}

#[test]
fn p_token_consumes_one_argument_and_leaves_later_words_in_the_title() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/repos/one");
    let mut model = BoardModel::from_domain(&domain, None);

    save_quick_add(&mut domain, &mut model, "ship !p one now");

    let task = domain.tasks().last().expect("saved task");
    assert_eq!(task.title, "ship now");
    assert_eq!(
        task.scope,
        TaskScope::Project {
            path: "/repos/one".into()
        }
    );
}

#[test]
fn bare_t_token_saves_unthreaded_and_strips() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "unthread this !t");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);

    let task = domain.tasks().last().expect("saved task");
    assert_eq!(task.title, "unthread this");
    assert_eq!(task.thread, None);
}

#[test]
fn t_and_p_tokens_combine_in_either_order() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/repos/one");
    create_project_fixture(&mut domain, "/repos/two");
    let mut model = BoardModel::from_domain(&domain, None);

    save_quick_add(&mut domain, &mut model, "first !p one !t Release-2026");
    save_quick_add(&mut domain, &mut model, "second !t Other !p two");

    for (task, title, path, thread) in [
        (&domain.tasks()[2], "first", "/repos/one", "release-2026"),
        (&domain.tasks()[3], "second", "/repos/two", "other"),
    ] {
        assert_eq!(task.title, title);
        assert_eq!(task.scope, TaskScope::Project { path: path.into() });
        assert_eq!(task.thread.as_deref(), Some(thread));
    }
}

#[test]
fn malformed_t_token_refuses_on_open_line_and_clears_on_close() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    type_title(
        &mut domain,
        &mut model,
        "bad !t release_name !p /repos/other",
    );
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert!(model
        .message()
        .is_some_and(|message| message.contains("thread")));
    assert!(
        domain.tasks().is_empty(),
        "a malformed token persists nothing"
    );

    apply(&mut domain, &mut model, BoardIntent::CancelQuickAdd, None);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(model.message(), None);
}

#[test]
fn quick_add_assignee_token_requires_an_exact_profile_and_saves_the_assignee() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/invocation")));
    set_agent_profiles(&mut model, &["reviewer", "researcher"]);
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "inspect change !a Reviewer");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    assert_eq!(domain.tasks()[0].title, "inspect change");
    assert_eq!(domain.tasks()[0].assignee.as_deref(), Some("reviewer"));
}

#[test]
fn unknown_quick_add_assignee_keeps_the_draft_open() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/invocation")));
    set_agent_profiles(&mut model, &["reviewer"]);
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "inspect change !a missing");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    assert!(domain.tasks().is_empty());
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(model.quick_add_title_value(), "inspect change !a missing");
}

#[test]
fn bare_quick_add_assignee_token_explicitly_clears_a_stashed_assignment() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/invocation")));
    set_agent_profiles(&mut model, &["reviewer"]);
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "first !a reviewer");
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    apply(&mut domain, &mut model, BoardIntent::CancelEdit, None);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    apply(
        &mut domain,
        &mut model,
        BoardIntent::QuickAddInsertText(" !a".into()),
        None,
    );
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    assert_eq!(domain.tasks()[0].assignee, None);
}

#[test]
fn quick_add_without_t_does_not_inherit_a_prior_thread() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);

    save_quick_add(&mut domain, &mut model, "threaded !t release-2026");
    save_quick_add(&mut domain, &mut model, "unthreaded follow-up");

    assert_eq!(domain.tasks()[0].thread.as_deref(), Some("release-2026"));
    assert_eq!(domain.tasks()[1].thread, None);
}

#[test]
fn over_length_t_token_refuses_without_applying_scope() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();
    let too_long = "a".repeat(33);

    open(&mut domain, &mut model, &snap);
    type_title(
        &mut domain,
        &mut model,
        &format!("bad !p /repos/other !t {too_long}"),
    );
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert!(model.message().is_some());
    assert!(domain.tasks().is_empty());
}

#[test]
fn malformed_t_token_keeps_quick_add_open_when_expanding() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "bad !t release_name");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None),
        IntentOutcome::None
    );
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(model.quick_add_title_value(), "bad !t release_name");
    assert!(model.message().is_some());
    assert!(domain.tasks().is_empty());
}

#[test]
fn draft_stash_round_trips_thread_through_tab_and_esc() {
    let mut domain = DomainState::new();
    create_project_fixture(&mut domain, "/repos/draft");
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    type_title(
        &mut domain,
        &mut model,
        "threaded details !p draft !t Release-2026",
    );
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    for character in "preserved note".chars() {
        apply(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        );
    }
    apply(&mut domain, &mut model, BoardIntent::CancelEdit, None);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(
        model.quick_add_title_value(),
        "threaded details !p draft !t Release-2026"
    );

    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    assert_eq!(model.edit_buffer(), "preserved note");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::ConfirmEdit, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);

    let task = domain.tasks().last().expect("saved task");
    assert_eq!(task.title, "threaded details");
    assert_eq!(task.notes.as_deref(), Some("preserved note"));
    assert_eq!(
        task.scope,
        TaskScope::Project {
            path: "/repos/draft".into()
        }
    );
    assert_eq!(task.thread.as_deref(), Some("release-2026"));
}

#[test]
fn threaded_quick_add_produces_only_created_event_and_no_edited_event() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);

    save_quick_add(&mut domain, &mut model, "ship it !t Release-2026");

    let task = domain.tasks().last().expect("threaded task");
    assert_eq!(task.thread.as_deref(), Some("release-2026"));
    assert_eq!(
        task.history
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![TaskEventKind::Created]
    );
}

#[test]
fn bare_t_before_hash_word_unthreads_and_keeps_hash_word_in_title() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);

    save_quick_add(&mut domain, &mut model, "ship !t #urgent");

    let task = domain.tasks().last().expect("unthreaded task");
    assert_eq!(task.title, "ship #urgent");
    assert_eq!(task.thread, None);
}

#[test]
fn failed_save_keeps_the_draft_for_retry_or_cancel() {
    let snap = snapshot();
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let mut recovery = SaveRecovery::new();
    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "recover me");

    let failed = apply_board_intent_with_save_recovery(
        &mut domain,
        &mut model,
        &mut recovery,
        BoardSaveContext {
            baseline: DomainState::new(),
            intent: BoardIntent::QuickAddSave,
            snapshot: None,
        },
        |_| Err("injected failure".into()),
    )
    .expect("save failure remains in bar");
    assert_eq!(failed, IntentOutcome::None);
    assert_eq!(model.input_mode(), BoardInputMode::SaveRecovery);
    assert_eq!(model.quick_add_title_value(), "recover me");
    assert!(domain.tasks().is_empty());

    let retried = apply_board_intent_with_save_recovery(
        &mut domain,
        &mut model,
        &mut recovery,
        BoardSaveContext {
            baseline: DomainState::new(),
            intent: BoardIntent::RetrySave,
            snapshot: None,
        },
        |_| Ok(()),
    )
    .expect("retry");
    assert_eq!(retried, IntentOutcome::Persisted);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(domain.tasks()[0].title, "recover me");

    let mut cancelled_domain = DomainState::new();
    let mut cancelled_model = BoardModel::from_domain(&cancelled_domain, None);
    let mut cancelled_recovery = SaveRecovery::new();
    open(&mut cancelled_domain, &mut cancelled_model, &snap);
    type_title(
        &mut cancelled_domain,
        &mut cancelled_model,
        "keep this draft",
    );
    apply_board_intent_with_save_recovery(
        &mut cancelled_domain,
        &mut cancelled_model,
        &mut cancelled_recovery,
        BoardSaveContext {
            baseline: DomainState::new(),
            intent: BoardIntent::QuickAddSave,
            snapshot: None,
        },
        |_| Err("injected failure".into()),
    )
    .expect("save failure");
    apply_board_intent_with_save_recovery(
        &mut cancelled_domain,
        &mut cancelled_model,
        &mut cancelled_recovery,
        BoardSaveContext {
            baseline: DomainState::new(),
            intent: BoardIntent::CancelSave,
            snapshot: None,
        },
        |_| Ok(()),
    )
    .expect("cancel recovery");
    assert_eq!(cancelled_model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(cancelled_model.quick_add_title_value(), "keep this draft");
    assert!(cancelled_domain.tasks().is_empty());
}

fn render_rows(model: &BoardModel, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
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

fn render_text(model: &BoardModel, width: u16, height: u16) -> String {
    render_rows(model, width, height).concat()
}

#[derive(Clone, Default)]
struct AnsiWriter(Rc<RefCell<Vec<u8>>>);

impl Write for AnsiWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn capture_bar_renders_spaced_three_row_block_and_stays_bounded_without_color_sgr() {
    let mut domain = DomainState::new();
    domain
        .create(
            "visible task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("fixture task");
    let mut model = BoardModel::from_domain(&domain, None);
    open(&mut domain, &mut model, &snapshot());

    let standard = render_text(&model, 80, 24);
    for text in [
        "visible task",
        "title…   !p project · !t thread · !a assignee",
        "add to invocation",
        "enter save · tab details · esc close",
    ] {
        assert!(standard.contains(text), "missing {text:?}: {standard}");
    }
    let standard_rows = render_rows(&model, 80, 24);
    assert_eq!(
        standard_rows[20].trim(),
        "add to invocation",
        "the row above the input names the destination"
    );
    assert!(standard_rows[21].contains("title…"), "input row");
    assert!(standard_rows[22].trim().is_empty(), "blank row below input");
    assert!(
        standard_rows[23].contains("enter save"),
        "keys live on the verb row: {}",
        standard_rows[23]
    );
    let hits = board_hit_map(Rect::new(0, 0, 80, 24), &model);
    assert!(
        hits.regions
            .iter()
            .all(|hit| !matches!(hit.area.y, 20 | 22)),
        "blank quick-add rows must have no mouse hit targets: {hits:?}"
    );

    let compact_rows = render_rows(&model, 40, 10);
    let compact = compact_rows.concat();
    for text in ["visible task", "title…", "save"] {
        assert!(compact.contains(text), "missing {text:?}: {compact}");
    }
    assert!(compact_rows.iter().all(|row| row.chars().count() == 40));

    let writer = AnsiWriter::default();
    let bytes = Rc::clone(&writer.0);
    let backend = CrosstermBackend::new(writer);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
        },
    )
    .expect("ANSI terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("ANSI draw");
    drop(terminal);
    let output = String::from_utf8(bytes.borrow().clone()).expect("ANSI output");
    tsk_tui::ui::render::assert_no_color_sgr(&output);
}

#[test]
fn p_token_naming_an_archived_project_refuses_on_the_open_line_and_clears_on_close() {
    let archived_path = env!("CARGO_MANIFEST_DIR");
    let archived_name = PathBuf::from(archived_path)
        .file_name()
        .expect("project basename")
        .to_string_lossy()
        .into_owned();
    let mut domain = DomainState::new();
    domain
        .create(
            "anchor",
            None,
            TaskScope::Project {
                path: archived_path.into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create anchor");
    domain.archive_project(archived_path).expect("archive");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/invocation")));
    let snap = snapshot();

    open(&mut domain, &mut model, &snap);
    type_title(
        &mut domain,
        &mut model,
        &format!("ship it !p {archived_name}"),
    );
    let tasks_before = domain.tasks().len();

    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None,
        "the save is refused"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::QuickAdd,
        "the line stays open"
    );
    let message = model.message().expect("a refusal paints");
    assert!(
        message.contains(&archived_name),
        "the refusal names the project: {message:?}"
    );
    assert!(
        message.contains("archived"),
        "the refusal says archived: {message:?}"
    );
    assert_eq!(domain.tasks().len(), tasks_before, "nothing was saved");

    // The refusal clears when the line closes.
    apply(&mut domain, &mut model, BoardIntent::CancelQuickAdd, None);
    assert_eq!(model.message(), None, "cancel clears the status slot");

    // An absolute path to the archived project behaves the same.
    open(&mut domain, &mut model, &snap);
    type_title(
        &mut domain,
        &mut model,
        &format!("ship it !p {archived_path}"),
    );
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    let message = model
        .message()
        .expect("a refusal paints for the verbatim path");
    assert!(
        message.contains(&archived_name) && message.contains("archived"),
        "{message:?}"
    );
    apply(&mut domain, &mut model, BoardIntent::CancelQuickAdd, None);

    // Bare `!p` (desk) and an unarchived name still save.
    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "desk task !p");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "live task !p invocation");
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::Persist
    );
    assert_eq!(domain.tasks().len(), tasks_before + 2);
}

#[test]
fn expanded_quick_add_scope_omits_archived_projects() {
    let mut domain = DomainState::new();
    domain
        .create(
            "live elsewhere",
            None,
            TaskScope::Project {
                path: "/repos/other".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create live");
    domain
        .create(
            "filed away",
            None,
            TaskScope::Project {
                path: "/repos/filed".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create filed");
    domain.archive_project("/repos/filed").expect("archive");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/invocation")));
    let snap = snapshot();
    open(&mut domain, &mut model, &snap);
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);

    let options = model.form_scope_options();
    assert!(
        !options.contains(&TaskScope::Project {
            path: "/repos/filed".into()
        }),
        "the expanded draft's scope list omits archived projects: {options:?}"
    );
    assert!(
        options.contains(&TaskScope::Project {
            path: "/repos/other".into()
        }),
        "live projects stay on offer: {options:?}"
    );
    assert!(
        options.contains(&TaskScope::Global),
        "desk stays on offer: {options:?}"
    );
}

#[test]
fn expanded_quick_add_sets_thread_and_steps() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();
    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "capture with extras");
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);
    let page = render_text(&model, 80, 24);
    assert!(
        page.contains("thread"),
        "expanded capture paints a thread slot:\n{page}"
    );
    assert!(
        page.contains("+ step"),
        "expanded capture paints + step:\n{page}"
    );
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert!(
        render_text(&model, 80, 24).contains("▸ + step"),
        "Tab from Notes selects the trailing step target"
    );
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditThread);
    for character in "V0.0.6".chars() {
        apply(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        );
    }
    apply(&mut domain, &mut model, BoardIntent::BeginAddStep, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    for character in "first step".chars() {
        apply(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        );
    }
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::ConfirmEditNext, None),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    let task = domain.tasks().last().expect("created");
    assert_eq!(task.title, "capture with extras");
    assert_eq!(task.thread.as_deref(), Some("v0.0.6"));
    assert_eq!(task.steps.len(), 1);
    assert_eq!(task.steps[0].text, "first step");
}

#[test]
fn expanded_quick_add_tabs_through_staged_steps_and_the_add_target() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    let snap = snapshot();
    open(&mut domain, &mut model, &snap);
    type_title(&mut domain, &mut model, "capture ring");
    apply(&mut domain, &mut model, BoardIntent::ExpandQuickAdd, None);

    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    apply(&mut domain, &mut model, BoardIntent::BeginAddStep, None);
    for character in "first staged step".chars() {
        apply(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        );
    }
    apply(&mut domain, &mut model, BoardIntent::ConfirmEdit, None);
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditStep,
        "Enter opens the next step"
    );
    apply(&mut domain, &mut model, BoardIntent::CancelEdit, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);

    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert!(
        render_text(&model, 80, 24).contains("▸ ▪ first staged step"),
        "Tab from Notes selects the first staged step"
    );
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert!(
        render_text(&model, 80, 24).contains("▸ + step"),
        "Tab reaches the trailing add target after staged steps"
    );
    apply(&mut domain, &mut model, BoardIntent::FormFocusNext, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditThread);
    apply(&mut domain, &mut model, BoardIntent::FormFocusPrev, None);
    assert!(
        render_text(&model, 80, 24).contains("▸ + step"),
        "Shift+Tab from Thread returns to the add target"
    );
    apply(&mut domain, &mut model, BoardIntent::FormFocusPrev, None);
    assert!(
        render_text(&model, 80, 24).contains("▸ ▪ first staged step"),
        "Shift+Tab from the add target returns to the last staged step"
    );
    apply(&mut domain, &mut model, BoardIntent::FormFocusPrev, None);
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
}

/// A wrapped draft owns the reserved row above the input, so its refusal moves down to the
/// verb row instead of vanishing, and a click on that notice is inert (it must not read as
/// an outside click that discards the draft).
#[test]
fn wrapped_draft_refusal_paints_on_the_verb_row_and_a_click_there_keeps_the_draft() {
    use ratatui::layout::Position;
    use tsk_tui::ui::render::QueueHitTarget;

    let mut domain = DomainState::new();
    domain
        .create(
            "anchor",
            None,
            TaskScope::Project {
                path: "/repos/other".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create anchor");
    domain.archive_project("/repos/other").expect("archive");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/invocation")));
    let snap = snapshot();
    open(&mut domain, &mut model, &snap);
    // Long enough to wrap at 80 columns, and naming an archived project so the save refuses.
    let long: String = (0..30).map(|index| format!("w{index} ")).collect();
    type_title(&mut domain, &mut model, &format!("{long}!p other"));
    assert_eq!(
        apply(&mut domain, &mut model, BoardIntent::QuickAddSave, None),
        IntentOutcome::None
    );
    let message = model.message().expect("refusal").to_string();

    let rows = render_rows(&model, 80, 24);
    let prompt_rows = rows.iter().filter(|row| row.contains('▎')).count();
    assert_eq!(
        prompt_rows,
        2,
        "the draft wrapped onto the reserved row:\n{}",
        rows.join("\n")
    );
    assert!(
        rows[23].contains(&message),
        "the refusal moved to the verb row:\n{}",
        rows.join("\n")
    );

    let area = Rect::new(0, 0, 80, 24);
    let hits = board_hit_map(area, &model);
    let notice = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::ModalChrome && hit.area.y == 23)
        .expect("the notice row is registered as inert chrome");
    assert!(notice.area.contains(Position { x: 10, y: 23 }));
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 10,
        row: 23,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(
        tsk_tui::ui::mouse::map_board_mouse(&model, &hits, click),
        None,
        "a click on the notice is inert"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::QuickAdd,
        "the draft survives"
    );
}
