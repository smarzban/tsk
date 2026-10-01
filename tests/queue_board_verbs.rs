//! Verb Surface reducers — primary verbs, done/reopen/block, drawer, Esc layers.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use tsk_tui::agents::AgentProfiles;
use tsk_tui::app::{
    cleanup_and_complete_with_host, offer_cleanup_prompt_with_host, resolve_dispatch_again,
    CleanupOffer,
};
use tsk_tui::context::InvocationSnapshot;
use tsk_tui::dispatch::{CleanupError, CleanupInspection, CreatedWorktree, DispatchHost};
use tsk_tui::domain::{
    Dispatch, DomainState, HumanStatus, ProvenanceOrigin, TaskEventKind, TaskScope,
};
use tsk_tui::store::TaskStore;
use tsk_tui::ui::board::{
    apply_intent, board_hit_map, board_intent_may_persist, board_verb_items, draw_board,
    resolve_board_command, BoardInputMode, BoardModel, CleanupPrompt, CommandSurface,
    IntentOutcome, ProjectScopeOption,
};
use tsk_tui::ui::capture::CaptureField;
use tsk_tui::ui::input::{
    map_capture_key, map_key, map_task_form_key, normal_help_bindings, BoardIntent, CaptureIntent,
    MarkDirection,
};
use tsk_tui::ui::mouse::BoardPopup;
use tsk_tui::ui::queue::SectionKind;
use tsk_tui::ui::tier;

const THIS_REPO: &str = "/repos/app";

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

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

fn ctrl_alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::ALT)
}

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
        .expect("create");
    if status != HumanStatus::Open {
        domain.set_status(id, status).expect("status");
    }
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    (domain, model, id)
}

fn select_done_task(domain: &mut DomainState, model: &mut BoardModel, id: uuid::Uuid) {
    apply_intent(domain, model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    let index = model
        .visible_ids()
        .iter()
        .position(|&row| row == id)
        .expect("done task visible");
    apply_intent(domain, model, BoardIntent::SelectIndex(index), None).expect("select done");
}

fn set_agent_profiles(model: &mut BoardModel, names: &[&str]) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "tsk-board-verbs-agents-{nanos}-{}",
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create profiles dir");
    let content = names
        .iter()
        .map(|name| format!("[agent.{name}]\ncommand = [\"true\"]\n"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("agents.toml"), content).expect("write profiles");
    model.set_agent_profiles(&AgentProfiles::load(&dir).expect("load profiles"));
    std::fs::remove_dir_all(dir).expect("remove profiles dir");
}

#[derive(Default)]
struct CleanupHost {
    inspection: Option<CleanupInspection>,
    inspections: usize,
    removed: usize,
}

impl DispatchHost for CleanupHost {
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
        self.inspections += 1;
        self.inspection
            .clone()
            .ok_or_else(|| "inspection missing".into())
    }

    fn remove_herdr_worktree(&mut self, _: &str) -> Result<(), String> {
        self.removed += 1;
        Ok(())
    }

    fn root_pane(&mut self, _: &str) -> Result<String, String> {
        Err("not used".into())
    }

    fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
        Err("not used".into())
    }
}

fn mark_tasks(domain: &mut DomainState, model: &mut BoardModel, ids: &[uuid::Uuid]) {
    if !model.mark_mode_active() {
        apply_intent(domain, model, BoardIntent::ToggleMarkMode, None).expect("enter mark mode");
    }
    for &id in ids {
        let index = model
            .visible_ids()
            .iter()
            .position(|&visible| visible == id)
            .expect("marked task visible");
        apply_intent(domain, model, BoardIntent::SelectIndex(index), None).expect("select task");
        apply_intent(domain, model, BoardIntent::MarkToggle, None).expect("mark task");
    }
}

#[test]
fn dispatch_key_and_palette_route_to_the_cursor_only_verb() {
    let (mut domain, mut model, id) = board_with_task("send it", HumanStatus::Ready);
    set_agent_profiles(&mut model, &["implementer"]);
    domain
        .assign(id, Some("implementer".into()))
        .expect("assign task");
    model.sync_from_domain(&domain);

    assert_eq!(
        map_key(BoardInputMode::Normal, ctrl(KeyCode::Char('g'))),
        Some(BoardIntent::Dispatch)
    );
    assert_eq!(
        map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('g'))),
        Some(BoardIntent::Dispatch)
    );
    let labels = model
        .available_commands()
        .into_iter()
        .map(|command| command.label)
        .collect::<Vec<_>>();
    assert!(
        labels
            .iter()
            .any(|label| label == "dispatch to @implementer"),
        "{labels:?}"
    );
}

#[test]
fn cleanup_popup_maps_explicit_choices_and_paints_the_guardrail_state() {
    let (_, mut model, id) = board_with_task("clean me", HumanStatus::Started);
    model.begin_cleanup_prompt(CleanupPrompt {
        task_id: id,
        worktree: "/tmp/tsk-t1-clean-me".into(),
        branch: "tsk/t1-clean-me".into(),
        dirty: false,
        branch_merged: false,
        workspace_exists: true,
    });
    assert_eq!(model.input_mode(), BoardInputMode::CleanupConfirm);
    assert_eq!(
        map_key(BoardInputMode::CleanupConfirm, press(KeyCode::Char('y'))),
        Some(BoardIntent::ConfirmCleanup)
    );
    assert_eq!(
        map_key(BoardInputMode::CleanupConfirm, press(KeyCode::Char('n'))),
        Some(BoardIntent::KeepCleanup)
    );
    assert_eq!(
        map_key(BoardInputMode::CleanupConfirm, press(KeyCode::Esc)),
        Some(BoardIntent::CancelCleanup)
    );
    let screen = rendered_board(&model, 100, 30);
    for text in [
        "Clean dispatch?",
        "/tmp/tsk-t1-clean-me",
        "tsk/t1-clean-me",
        "unmerged, branch will be kept",
        "agent pane will close",
        "y clean + done",
        "n done only",
        "esc cancel",
    ] {
        assert!(screen.contains(text), "missing {text:?}:\n{screen}");
    }
}

#[test]
fn cleanup_prompt_is_cursor_only_and_dirty_confirmation_still_completes() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Started);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("second");
    domain
        .record_dispatch(
            first,
            Dispatch {
                argv: vec!["agent".into()],
                worktree: "/tmp/first".into(),
                branch: "tsk/t1-first".into(),
                base: None,
                herdr_workspace_id: "w1".into(),
                at: SystemTime::now(),
                cleaned: false,
            },
        )
        .expect("dispatch first");
    let dir = std::env::temp_dir().join(format!(
        "tsk-cleanup-popup-{}-{}",
        std::process::id(),
        AtomicU64::new(0).fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp store");
    let store = TaskStore::new(&dir);
    store.save(&domain).expect("number tasks");
    domain = store.load().expect("reload numbered tasks");
    model.sync_from_domain(&domain);
    mark_tasks(&mut domain, &mut model, &[first, second]);
    let mut host = CleanupHost {
        inspection: Some(CleanupInspection {
            worktree_exists: true,
            dirty: true,
            branch_merged: false,
            workspace_exists: true,
            target_matches: true,
        }),
        ..CleanupHost::default()
    };
    assert_eq!(
        offer_cleanup_prompt_with_host(&mut domain, &mut model, first, true, &mut host)
            .expect("bulk bypass"),
        CleanupOffer::None
    );
    assert_eq!(
        host.inspections, 0,
        "bulk done never inspects one cursor worktree"
    );

    let first_index = model
        .visible_ids()
        .iter()
        .position(|id| *id == first)
        .unwrap();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(first_index),
        None,
    )
    .expect("select dispatched task");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    assert_eq!(
        offer_cleanup_prompt_with_host(&mut domain, &mut model, first, true, &mut host)
            .expect("task page is cursor-only despite retained marks"),
        CleanupOffer::Prompted
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CancelCleanup, None).expect("cancel prompt");
    assert_eq!(host.inspections, 1);

    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close page");
    assert_eq!(
        offer_cleanup_prompt_with_host(&mut domain, &mut model, first, true, &mut host)
            .expect("offer"),
        CleanupOffer::Prompted
    );
    assert_eq!(model.input_mode(), BoardInputMode::CleanupDirtyConfirm);
    assert_eq!(
        map_key(
            BoardInputMode::CleanupDirtyConfirm,
            press(KeyCode::Char('y'))
        ),
        None,
        "dirty worktrees do not offer cleanup"
    );
    let screen = rendered_board(&model, 100, 30);
    assert!(!screen.contains("y clean + done"), "{screen}");
    assert!(screen.contains("n done only"), "{screen}");
    let result = cleanup_and_complete_with_host(&mut domain, &mut model, true, true, &mut host)
        .expect("completion")
        .expect("cleanup attempted");
    assert_eq!(result, Err(CleanupError::DirtyWorktree));
    assert_eq!(domain.get(first).unwrap().status, HumanStatus::Done);
    assert_eq!(domain.get(second).unwrap().status, HumanStatus::Open);
    assert_eq!(model.popup(), BoardPopup::None);
    store
        .reload_merge_save(&mut domain)
        .expect("cleanup and completion retain the original save merge base");
    std::fs::remove_dir_all(dir).expect("cleanup temp store");
}

#[test]
fn missing_worktree_converges_cleaned_and_done_in_one_board_save_without_a_popup() {
    let (mut domain, mut model, id) = board_with_task("already removed", HumanStatus::Started);
    domain
        .record_dispatch(
            id,
            Dispatch {
                argv: vec!["agent".into()],
                worktree: "/tmp/already-removed".into(),
                branch: "tsk/t1-already-removed".into(),
                base: Some("main".into()),
                herdr_workspace_id: "w1".into(),
                at: SystemTime::now(),
                cleaned: false,
            },
        )
        .expect("dispatch");
    let dir = std::env::temp_dir().join(format!(
        "tsk-cleanup-missing-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp store");
    let store = TaskStore::new(&dir);
    store.save(&domain).expect("seed");
    domain = store.load().expect("numbered state");
    model.sync_from_domain(&domain);
    let mut host = CleanupHost {
        inspection: Some(CleanupInspection {
            worktree_exists: false,
            dirty: false,
            branch_merged: false,
            workspace_exists: false,
            target_matches: true,
        }),
        ..CleanupHost::default()
    };

    assert!(matches!(
        offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
            .expect("missing worktree converges"),
        CleanupOffer::MissingConverged(_)
    ));
    assert_eq!(model.popup(), BoardPopup::None);
    assert!(domain.get(id).unwrap().dispatch.as_ref().unwrap().cleaned);
    assert_eq!(domain.get(id).unwrap().status, HumanStatus::Done);
    store
        .reload_merge_save(&mut domain)
        .expect("one save contains cleanup and completion");
    let saved = store.load().expect("reload");
    let task = saved.get(id).unwrap();
    assert_eq!(task.status, HumanStatus::Done);
    assert!(task.dispatch.as_ref().unwrap().cleaned);
    assert!(task
        .history
        .iter()
        .any(|event| event.kind == TaskEventKind::Cleaned));
    std::fs::remove_dir_all(dir).expect("cleanup temp store");
}

#[test]
fn cleanup_offer_skips_done_and_archived_tasks_without_inspection_or_mutation() {
    for archived in [false, true] {
        let (mut domain, mut model, id) =
            board_with_task("no second completion", HumanStatus::Started);
        domain
            .record_dispatch(
                id,
                Dispatch {
                    argv: vec!["agent".into()],
                    worktree: "/tmp/no-second-completion".into(),
                    branch: "tsk/t1-no-second-completion".into(),
                    base: Some("main".into()),
                    herdr_workspace_id: "w1".into(),
                    at: SystemTime::now(),
                    cleaned: false,
                },
            )
            .expect("dispatch");
        if archived {
            domain.archive_task(id).expect("archive");
        } else {
            domain.set_status(id, HumanStatus::Done).expect("done");
        }
        let dir = std::env::temp_dir().join(format!(
            "tsk-cleanup-noop-{}-{}-{archived}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp store");
        let store = TaskStore::new(&dir);
        store.save(&domain).expect("number task");
        domain = store.load().expect("reload numbered task");
        model.sync_from_domain(&domain);
        let before = serde_json::to_value(&domain).unwrap();
        let mut host = CleanupHost {
            inspection: Some(CleanupInspection {
                worktree_exists: true,
                dirty: false,
                branch_merged: true,
                workspace_exists: true,
                target_matches: true,
            }),
            ..CleanupHost::default()
        };

        assert_eq!(
            offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
                .expect("no offer"),
            CleanupOffer::None
        );
        assert_eq!(host.inspections, 0);
        assert_eq!(model.popup(), BoardPopup::None);
        assert_eq!(serde_json::to_value(&domain).unwrap(), before);
        std::fs::remove_dir_all(dir).expect("cleanup temp store");
    }
}

#[test]
fn cleanup_offer_defers_to_the_archived_project_read_only_refusal() {
    let (mut domain, mut model, id) = read_only_focus();
    domain
        .record_dispatch(
            id,
            Dispatch {
                argv: vec!["agent".into()],
                worktree: "/tmp/read-only".into(),
                branch: "tsk/t1-read-only".into(),
                base: Some("main".into()),
                herdr_workspace_id: "w1".into(),
                at: SystemTime::now(),
                cleaned: false,
            },
        )
        .expect("dispatch");
    let dir = std::env::temp_dir().join(format!(
        "tsk-cleanup-read-only-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp store");
    let store = TaskStore::new(&dir);
    store.save(&domain).expect("number task");
    domain = store.load().expect("reload numbered task");
    model.sync_from_domain(&domain);
    let before = serde_json::to_value(&domain).unwrap();
    let mut host = CleanupHost {
        inspection: Some(CleanupInspection {
            worktree_exists: true,
            dirty: false,
            branch_merged: true,
            workspace_exists: true,
            target_matches: true,
        }),
        ..CleanupHost::default()
    };

    assert_eq!(
        offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host)
            .expect("read-only skips cleanup"),
        CleanupOffer::None
    );
    assert_eq!(host.inspections, 0);
    assert_eq!(model.popup(), BoardPopup::None);
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::Complete, None).unwrap(),
        IntentOutcome::None
    );
    assert_eq!(
        model.message(),
        Some("project app is archived · ctrl+u unarchive")
    );
    assert_eq!(serde_json::to_value(&domain).unwrap(), before);
    std::fs::remove_dir_all(dir).expect("cleanup temp store");
}

#[test]
fn successful_popup_cleanup_and_completion_save_once_and_undo_only_status() {
    let (mut domain, _, id) = board_with_task("clean and done", HumanStatus::Started);
    domain
        .record_dispatch(
            id,
            Dispatch {
                argv: vec!["agent".into()],
                worktree: "/tmp/clean-and-done".into(),
                branch: "tsk/t1-clean-and-done".into(),
                base: None,
                herdr_workspace_id: "w1".into(),
                at: SystemTime::now(),
                cleaned: false,
            },
        )
        .expect("dispatch");
    let dir = std::env::temp_dir().join(format!(
        "tsk-cleanup-save-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp store");
    let store = TaskStore::new(&dir);
    store.save(&domain).expect("seed");
    domain = store.load().expect("numbered state");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let mut host = CleanupHost {
        inspection: Some(CleanupInspection {
            worktree_exists: true,
            dirty: false,
            branch_merged: false,
            workspace_exists: true,
            target_matches: true,
        }),
        ..CleanupHost::default()
    };
    assert_eq!(
        offer_cleanup_prompt_with_host(&mut domain, &mut model, id, true, &mut host).unwrap(),
        CleanupOffer::Prompted
    );
    let result = cleanup_and_complete_with_host(&mut domain, &mut model, true, true, &mut host)
        .unwrap()
        .unwrap()
        .expect("cleanup");
    assert_eq!(result.number, 1);
    assert_eq!(host.removed, 1);
    store
        .reload_merge_save(&mut domain)
        .expect("cleaned completion is one save intent");
    let task = domain.get(id).unwrap();
    assert_eq!(task.status, HumanStatus::Done);
    assert!(task.dispatch.as_ref().unwrap().cleaned);
    domain.undo().expect("undo completion");
    let task = domain.get(id).unwrap();
    assert_ne!(task.status, HumanStatus::Done);
    assert!(task.dispatch.as_ref().unwrap().cleaned);
    std::fs::remove_dir_all(dir).expect("cleanup temp store");
}

#[test]
fn dispatched_task_page_renders_the_record_and_assigned_legend() {
    let (mut domain, mut model, id) = board_with_task("send it", HumanStatus::Ready);
    assert!(
        board_verb_items(&model).iter().all(|verb| verb.key != "g"),
        "unassigned tasks do not advertise dispatch"
    );
    domain
        .assign(id, Some("implementer".into()))
        .expect("assign task");
    model.sync_from_domain(&domain);
    let verbs = board_verb_items(&model);
    assert!(
        verbs
            .windows(2)
            .any(|pair| pair[0].key == "s" && pair[1].key == "g"),
        "assigned legend should pair start and dispatch: {verbs:?}"
    );

    domain
        .record_dispatch(
            id,
            Dispatch {
                argv: vec!["runner".into()],
                worktree: "/tmp/dispatch-worktree".into(),
                branch: "tsk/t1-send-it".into(),
                base: None,
                herdr_workspace_id: "w9".into(),
                at: SystemTime::now(),
                cleaned: false,
            },
        )
        .expect("record dispatch");
    model.sync_from_domain(&domain);
    let commands = model.available_commands();
    assert!(commands.iter().any(|command| {
        command.label == "dispatch again" && command.intent == BoardIntent::DispatchAgain
    }));
    assert!(commands
        .iter()
        .all(|command| !command.label.starts_with("dispatch to @")));
    assert_eq!(resolve_dispatch_again(&domain, &mut model, id, false), None);
    assert_eq!(
        model.message(),
        Some("already dispatched at /tmp/dispatch-worktree · ctrl+g again relaunches")
    );
    assert_eq!(
        resolve_dispatch_again(&domain, &mut model, id, false),
        Some(true)
    );
    assert_eq!(
        resolve_dispatch_again(&domain, &mut model, id, true),
        Some(true),
        "the explicit palette command does not need a second confirmation"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    let screen = rendered_board(&model, 100, 30);
    assert!(
        screen.contains("worktree /tmp/dispatch-worktree"),
        "{screen}"
    );
    assert!(screen.contains("branch tsk/t1-send-it"), "{screen}");
    assert!(screen.contains("when"), "{screen}");

    domain.record_dispatch_cleaned(id).expect("mark cleaned");
    model.sync_from_domain(&domain);
    let cleaned = rendered_board(&model, 100, 30);
    assert!(cleaned.contains("dispatch · cleaned"), "{cleaned}");
    assert!(cleaned.contains("worktree removed"), "{cleaned}");
}

#[test]
fn dispatched_task_page_hides_record_during_notes_edit_and_restores_it_in_view_mode() {
    let (mut domain, mut model, id) = board_with_task("edit dispatched notes", HumanStatus::Ready);
    domain
        .assign(id, Some("implementer".into()))
        .expect("assign task");
    domain
        .record_dispatch(
            id,
            Dispatch {
                argv: vec!["runner".into()],
                worktree: "/tmp/notes-edit-dispatch-worktree".into(),
                branch: "tsk/t1-notes-edit-dispatch".into(),
                base: None,
                herdr_workspace_id: "w9".into(),
                at: SystemTime::now(),
                cleaned: false,
            },
        )
        .expect("record dispatch");
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    let view = rendered_board(&model, 100, 30);
    assert!(
        view.contains("worktree /tmp/notes-edit-dispatch-worktree"),
        "view mode should paint the dispatch record: {view}"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("edit notes");
    let editing = rendered_board(&model, 100, 30);
    assert!(
        !editing.contains("notes-edit-dispatch-worktree")
            && !editing.contains("tsk/t1-notes-edit-dispatch"),
        "notes edit must not paint uneditable dispatch rows: {editing}"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("exit notes edit");
    let restored = rendered_board(&model, 100, 30);
    assert!(
        restored.contains("worktree /tmp/notes-edit-dispatch-worktree")
            && restored.contains("branch tsk/t1-notes-edit-dispatch"),
        "view mode should restore the dispatch record: {restored}"
    );
}

#[test]
fn palette_assignment_applies_to_marked_tasks_as_one_undoable_batch() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Started);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    domain
        .set_status(second, HumanStatus::Review)
        .expect("set second status");
    model.sync_from_domain(&domain);
    set_agent_profiles(&mut model, &["reviewer"]);
    mark_tasks(&mut domain, &mut model, &[first, second]);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::BeginEditAssignee,
        None,
    )
    .expect("open picker");
    assert_eq!(model.input_mode(), BoardInputMode::EditAssignee);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Assignee),
        None,
    )
    .expect("open dropdown");
    apply_intent(&mut domain, &mut model, BoardIntent::FormDropdownNext, None)
        .expect("select reviewer");
    assert!(board_intent_may_persist(
        &model,
        &BoardIntent::ConfirmFormDropdown
    ));
    assert_eq!(
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ConfirmFormDropdown,
            None,
        )
        .expect("assign"),
        IntentOutcome::Persist
    );
    assert_eq!(
        domain.get(first).expect("first").assignee.as_deref(),
        Some("reviewer")
    );
    assert_eq!(
        domain.get(second).expect("second").assignee.as_deref(),
        Some("reviewer")
    );
    assert_eq!(
        domain.get(first).expect("first").status,
        HumanStatus::Started
    );
    assert_eq!(
        domain.get(second).expect("second").status,
        HumanStatus::Review
    );
    assert!(model.marked_ids().is_empty());

    domain.undo().expect("one undo reverses batch");
    assert_eq!(domain.get(first).expect("first").assignee, None);
    assert_eq!(domain.get(second).expect("second").assignee, None);
    assert_eq!(
        domain.get(first).expect("first").status,
        HumanStatus::Started
    );
    assert_eq!(
        domain.get(second).expect("second").status,
        HumanStatus::Review
    );
}

#[test]
fn scope_dropdown_preserves_marked_assignment_until_assignee_is_picked() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Started);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    model.sync_from_domain(&domain);
    set_agent_profiles(&mut model, &["reviewer"]);
    mark_tasks(&mut domain, &mut model, &[first, second]);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::BeginEditAssignee,
        None,
    )
    .expect("open marked assignment");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("move to thread");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("move to scope");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Scope),
        None,
    )
    .expect("open scope dropdown");
    assert_eq!(
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ConfirmFormDropdown,
            None,
        )
        .expect("pick scope"),
        IntentOutcome::None
    );
    assert_eq!(domain.get(first).expect("first").assignee, None);
    assert_eq!(domain.get(second).expect("second").assignee, None);
    assert_eq!(model.marked_ids().len(), 2);
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Scope),
        None,
    )
    .expect("reopen scope dropdown");
    assert_eq!(
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SelectFormDropdownOption(0),
            None,
        )
        .expect("click scope option"),
        IntentOutcome::None
    );
    assert_eq!(domain.get(first).expect("first").assignee, None);
    assert_eq!(domain.get(second).expect("second").assignee, None);
    assert_eq!(model.marked_ids().len(), 2);
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Assignee),
        None,
    )
    .expect("return to assignee");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Assignee),
        None,
    )
    .expect("open assignee dropdown");
    assert!(
        board_intent_may_persist(&model, &BoardIntent::ConfirmFormDropdown),
        "the retained marked assignment must still be pending"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::FormDropdownNext, None)
        .expect("select reviewer");
    assert_eq!(
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ConfirmFormDropdown,
            None,
        )
        .expect("assign marked tasks"),
        IntentOutcome::Persist
    );
    assert_eq!(
        domain.get(first).expect("first").assignee.as_deref(),
        Some("reviewer")
    );
    assert_eq!(
        domain.get(second).expect("second").assignee.as_deref(),
        Some("reviewer")
    );
}

#[test]
fn dropdown_persistence_classification_requires_pending_assignee_targets() {
    let (mut domain, mut model, _) = board_with_task("classify dropdown", HumanStatus::Started);
    set_agent_profiles(&mut model, &["reviewer"]);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::BeginEditAssignee,
        None,
    )
    .expect("begin assignment");
    for _ in 0..2 {
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
            .expect("move toward scope");
    }
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Scope),
        None,
    )
    .expect("open scope dropdown");
    for intent in [
        BoardIntent::ConfirmFormDropdown,
        BoardIntent::SelectFormDropdownOption(0),
    ] {
        assert!(
            !board_intent_may_persist(&model, &intent),
            "{intent:?} on Scope must remain a draft-only action"
        );
    }

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CancelFormDropdown,
        None,
    )
    .expect("close scope dropdown");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Assignee),
        None,
    )
    .expect("open assignee dropdown");
    for intent in [
        BoardIntent::ConfirmFormDropdown,
        BoardIntent::SelectFormDropdownOption(0),
    ] {
        assert!(
            board_intent_may_persist(&model, &intent),
            "{intent:?} on Assignee must load a save baseline"
        );
    }
}

#[test]
fn assignee_enter_opens_a_keyboard_dropdown_with_none_and_profiles() {
    let (mut domain, mut model, id) = board_with_task("assign from dropdown", HumanStatus::Open);
    set_agent_profiles(&mut model, &["implementer", "reviewer"]);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Assignee),
        None,
    )
    .expect("focus assignee");
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        map_task_form_key(CaptureField::Assignee, false, enter),
        Some(BoardIntent::OpenFormDropdown(CaptureField::Assignee))
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Assignee),
        None,
    )
    .expect("open dropdown");
    assert_eq!(model.input_mode(), BoardInputMode::FormDropdown);
    let rendered = rendered_board(&model, 80, 24);
    assert!(
        rendered.contains("none")
            && rendered.contains("implementer")
            && rendered.contains("reviewer"),
        "assignee options:\n{rendered}"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::FormDropdownNext, None)
        .expect("highlight implementer");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CancelFormDropdown,
        None,
    )
    .expect("cancel dropdown");
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("save cancelled");
    assert_eq!(domain.get(id).expect("task").assignee, None);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Assignee),
        None,
    )
    .expect("focus assignee again");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Assignee),
        None,
    )
    .expect("reopen dropdown");
    apply_intent(&mut domain, &mut model, BoardIntent::FormDropdownNext, None)
        .expect("highlight implementer again");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmFormDropdown,
        None,
    )
    .expect("pick implementer");
    assert_eq!(model.input_mode(), BoardInputMode::EditAssignee);
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("save");
    assert_eq!(
        domain.get(id).expect("task").assignee.as_deref(),
        Some("implementer")
    );
}

#[test]
fn task_page_assignee_save_persists_without_changing_human_status() {
    let (mut domain, mut model, id) = board_with_task("started", HumanStatus::Started);
    set_agent_profiles(&mut model, &["reviewer"]);

    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Assignee),
        None,
    )
    .expect("focus assignee");
    apply_intent(&mut domain, &mut model, BoardIntent::FormAssigneeNext, None)
        .expect("pick reviewer");
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
            .expect("save task page"),
        IntentOutcome::Persist
    );

    let task = domain.get(id).expect("started task");
    assert_eq!(task.assignee.as_deref(), Some("reviewer"));
    assert_eq!(task.status, HumanStatus::Started);
}

#[test]
fn retargeted_task_page_keeps_all_assignee_options_and_the_saved_value() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "assigned second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    domain
        .assign(second, Some("reviewer".into()))
        .expect("seed assignee");
    model.sync_from_domain(&domain);
    set_agent_profiles(&mut model, &["implementer", "reviewer"]);

    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    assert_eq!(model.selected_id(), Some(first));
    let second_index = model
        .visible_ids()
        .iter()
        .position(|id| *id == second)
        .expect("second visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusBoardAndSelectIndex(second_index),
        None,
    )
    .expect("retarget page");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::BeginEditAssignee,
        None,
    )
    .expect("edit retargeted assignee");
    for _ in 0..3 {
        apply_intent(&mut domain, &mut model, BoardIntent::FormAssigneeNext, None)
            .expect("cycle full assignee ring");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
        .expect("save unchanged assignee");

    assert_eq!(
        domain.get(second).expect("second").assignee.as_deref(),
        Some("reviewer"),
        "the retargeted form must include unassigned and both configured profiles"
    );
}

#[test]
fn confirm_edit_clears_palette_targets_before_a_later_task_assignment() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    model.sync_from_domain(&domain);
    set_agent_profiles(&mut model, &["reviewer"]);
    mark_tasks(&mut domain, &mut model, &[first, second]);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::BeginEditAssignee,
        None,
    )
    .expect("open marked assignment");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Title),
        None,
    )
    .expect("leave assignee field");
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
        .expect("confirm through ordinary task save");
    apply_intent(&mut domain, &mut model, BoardIntent::MarkClear, None).expect("clear old marks");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Assignee),
        None,
    )
    .expect("focus first assignee");
    apply_intent(&mut domain, &mut model, BoardIntent::FormAssigneeNext, None)
        .expect("pick reviewer");
    assert_eq!(
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ConfirmFormAssignee,
            None,
        )
        .expect("confirm assignee field"),
        IntentOutcome::None
    );
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("save first");

    assert_eq!(domain.get(first).expect("first").assignee, None);
    assert_eq!(
        domain.get(second).expect("second").assignee.as_deref(),
        Some("reviewer")
    );
}

#[test]
fn selection_retarget_clears_palette_targets_before_assignee_confirmation() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    let third = domain
        .create(
            "third",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create third");
    model.sync_from_domain(&domain);
    set_agent_profiles(&mut model, &["reviewer"]);
    mark_tasks(&mut domain, &mut model, &[first, second]);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::BeginEditAssignee,
        None,
    )
    .expect("open marked assignment");

    let third_index = model
        .visible_ids()
        .iter()
        .position(|id| *id == third)
        .expect("third visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusBoardAndSelectIndex(third_index),
        None,
    )
    .expect("retarget page to third");
    apply_intent(&mut domain, &mut model, BoardIntent::FormAssigneeNext, None)
        .expect("pick reviewer");
    assert_eq!(
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ConfirmFormAssignee,
            None,
        )
        .expect("confirm assignee field"),
        IntentOutcome::None
    );
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("save third");

    assert_eq!(domain.get(first).expect("first").assignee, None);
    assert_eq!(domain.get(second).expect("second").assignee, None);
    assert_eq!(
        domain.get(third).expect("third").assignee.as_deref(),
        Some("reviewer")
    );
}

#[test]
fn canceling_palette_assignment_cannot_apply_its_old_marked_targets_later() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    model.sync_from_domain(&domain);
    set_agent_profiles(&mut model, &["reviewer"]);
    mark_tasks(&mut domain, &mut model, &[first, second]);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::BeginEditAssignee,
        None,
    )
    .expect("open palette assignment");
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
        .expect("cancel palette assignment");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Assignee),
        None,
    )
    .expect("refocus task-page assignee");
    apply_intent(&mut domain, &mut model, BoardIntent::FormAssigneeNext, None)
        .expect("select reviewer in the task form");

    assert_eq!(
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ConfirmFormAssignee,
            None,
        )
        .expect("confirm task-form field"),
        IntentOutcome::None
    );
    assert_eq!(domain.get(first).expect("first").assignee, None);
    assert_eq!(domain.get(second).expect("second").assignee, None);
}

#[test]
fn shift_arrows_and_space_build_a_session_only_marked_set() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    model.sync_from_domain(&domain);
    let first_index = model
        .visible_ids()
        .iter()
        .position(|&id| id == first)
        .expect("first visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(first_index),
        None,
    )
    .expect("select first");

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::MarkExtend(MarkDirection::Down),
        None,
    )
    .expect("marking is inert before mark mode");
    assert_eq!(model.marked_count(), 0);
    assert_eq!(model.selected_id(), Some(first));

    apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
        .expect("enter mark mode");
    assert!(model.mark_mode_active());
    assert_eq!(
        map_key(BoardInputMode::Normal, shift(KeyCode::Down)),
        Some(BoardIntent::MarkExtend(MarkDirection::Down))
    );
    assert_eq!(
        map_key(BoardInputMode::Normal, press(KeyCode::Char(' '))),
        Some(BoardIntent::MarkToggle)
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::MarkExtend(MarkDirection::Down),
        None,
    )
    .expect("extend marks");
    assert!(model.marked_ids().contains(&first));
    assert_eq!(model.marked_count(), 1);
    assert_ne!(model.selected_id(), Some(first));

    apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None)
        .expect("toggle destination mark");
    assert_eq!(model.marked_count(), 2);
    assert!(model.marked_ids().contains(&second));

    apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
        .expect("leave mark mode");
    assert_eq!(model.marked_count(), 0);
    assert!(!model.mark_mode_active());

    apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
        .expect("re-enter empty mark mode");
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
        .expect("escape empty mark mode");
    assert!(!model.mark_mode_active());
}

#[test]
fn status_verbs_target_all_marks_once_then_clear_them() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    model.sync_from_domain(&domain);

    mark_tasks(&mut domain, &mut model, &[first, second]);

    let outcome = apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SetStatus(HumanStatus::Blocked),
        None,
    )
    .expect("block marked set");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert_eq!(
        domain.get(first).expect("first").status,
        HumanStatus::Blocked
    );
    assert_eq!(
        domain.get(second).expect("second").status,
        HumanStatus::Blocked
    );
    assert_eq!(model.marked_count(), 0, "a completed verb clears marks");
}

#[test]
fn bulk_start_preserves_per_task_eligibility_and_toggle_verbs_are_all_or_nothing() {
    let (mut domain, mut model, open) = board_with_task("open", HumanStatus::Open);
    let blocked = domain
        .create(
            "blocked",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create blocked");
    domain
        .set_status(blocked, HumanStatus::Blocked)
        .expect("set blocked");
    model.sync_from_domain(&domain);

    mark_tasks(&mut domain, &mut model, &[open, blocked]);
    apply_intent(&mut domain, &mut model, BoardIntent::PrimaryVerb, None)
        .expect("start eligible marks");
    assert_eq!(domain.get(open).expect("open").status, HumanStatus::Started);
    assert_eq!(
        domain.get(blocked).expect("blocked").status,
        HumanStatus::Blocked,
        "bulk start skips blocked tasks"
    );

    mark_tasks(&mut domain, &mut model, &[open, blocked]);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleBlock, None)
        .expect("mixed set goes blocked");
    assert!([open, blocked]
        .into_iter()
        .all(|id| domain.get(id).expect("task").status == HumanStatus::Blocked));
    mark_tasks(&mut domain, &mut model, &[open, blocked]);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleBlock, None)
        .expect("all-blocked set goes ready");
    assert!([open, blocked]
        .into_iter()
        .all(|id| domain.get(id).expect("task").status == HumanStatus::Ready));

    mark_tasks(&mut domain, &mut model, &[open, blocked]);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleReview, None)
        .expect("mixed set goes review");
    assert!([open, blocked]
        .into_iter()
        .all(|id| domain.get(id).expect("task").status == HumanStatus::Review));
    mark_tasks(&mut domain, &mut model, &[open, blocked]);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleReview, None)
        .expect("all-review set goes ready");
    assert!([open, blocked]
        .into_iter()
        .all(|id| domain.get(id).expect("task").status == HumanStatus::Ready));

    domain.complete(blocked).expect("complete one target");
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("open done drawer");
    mark_tasks(&mut domain, &mut model, &[open, blocked]);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleBlock, None)
        .expect("bulk block includes a done target");
    assert!([open, blocked]
        .into_iter()
        .all(|id| domain.get(id).expect("task").status == HumanStatus::Blocked));
}

#[test]
fn bulk_verbs_reach_marked_done_drawer_rows_and_archive_each_mark() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Done);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    domain.complete(second).expect("complete second");
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("open done drawer");
    mark_tasks(&mut domain, &mut model, &[first, second]);
    apply_intent(&mut domain, &mut model, BoardIntent::Reopen, None).expect("reopen done marks");
    assert!([first, second]
        .into_iter()
        .all(|id| domain.get(id).expect("task").status == HumanStatus::Open));
    assert_eq!(model.marked_count(), 0);

    mark_tasks(&mut domain, &mut model, &[first, second]);
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::File, None)
            .expect("archive marked rows"),
        IntentOutcome::Persist
    );
    assert!([first, second]
        .into_iter()
        .all(|id| domain.get(id).expect("task").archived));
    assert_eq!(model.marked_count(), 0);
}

#[test]
fn one_undo_reverses_bulk_done_and_a_stale_batch_refuses_atomically() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    let already_done = domain
        .create(
            "already done",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create done task");
    domain
        .complete(already_done)
        .expect("complete one in advance");
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("open done drawer");

    mark_tasks(&mut domain, &mut model, &[already_done]);
    let before_noop = serde_json::to_value(&domain).expect("snapshot done state");
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::Complete, None)
            .expect("repeat done status"),
        IntentOutcome::None
    );
    assert_eq!(
        serde_json::to_value(&domain).expect("snapshot repeated state"),
        before_noop,
        "an all-done marked set does not mutate or add an undo entry"
    );

    mark_tasks(&mut domain, &mut model, &[first, second, already_done]);
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::Complete, None).expect("bulk done"),
        IntentOutcome::Persist
    );
    assert!([first, second, already_done].into_iter().all(|id| domain
        .get(id)
        .expect("task")
        .status
        == HumanStatus::Done));
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("batch undo"),
        IntentOutcome::Persist
    );
    assert!([first, second]
        .into_iter()
        .all(|id| domain.get(id).expect("task").status == HumanStatus::Open));
    assert_eq!(
        domain.get(already_done).expect("already done").status,
        HumanStatus::Done,
        "undo leaves tasks unchanged by the bulk completion alone"
    );

    mark_tasks(&mut domain, &mut model, &[first, second]);
    apply_intent(&mut domain, &mut model, BoardIntent::Complete, None).expect("second bulk done");
    domain
        .set_status(second, HumanStatus::Blocked)
        .expect("make one child stale");
    model.sync_from_domain(&domain);
    for _ in 0..2 {
        assert_eq!(
            apply_intent(&mut domain, &mut model, BoardIntent::Undo, None)
                .expect("stale batch undo"),
            IntentOutcome::None
        );
        assert_eq!(domain.get(first).expect("first").status, HumanStatus::Done);
        assert_eq!(
            domain.get(second).expect("second").status,
            HumanStatus::Blocked
        );
    }
}

#[test]
fn bulk_delete_confirms_count_once_and_one_undo_restores_the_set() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    model.sync_from_domain(&domain);
    mark_tasks(&mut domain, &mut model, &[first, second]);

    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None)
            .expect("arm bulk delete"),
        IntentOutcome::None
    );
    assert_eq!(
        model.message(),
        Some("press ctrl+x again to delete 2 tasks")
    );
    assert_eq!(
        model.marked_count(),
        0,
        "the delete verb spends the marked set"
    );

    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None)
            .expect("confirm bulk delete"),
        IntentOutcome::Persist
    );
    assert!([first, second]
        .into_iter()
        .all(|id| domain.get(id).expect("task").soft_deleted));
    assert_eq!(model.marked_count(), 0);
    assert!(
        rendered_board(&model, 80, 24).contains("deleted 2 tasks · ctrl+u restores"),
        "bulk delete notice must name its one-step recovery"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("undo batch delete");
    assert!([first, second]
        .into_iter()
        .all(|id| !domain.get(id).expect("task").soft_deleted));
}

#[test]
fn cursor_only_open_and_edit_ignore_marks_and_lens_boundaries_clear_marks() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    model.sync_from_domain(&domain);
    mark_tasks(&mut domain, &mut model, &[second]);
    let first_index = model
        .visible_ids()
        .iter()
        .position(|&id| id == first)
        .expect("first visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(first_index),
        None,
    )
    .expect("put cursor on first");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
        .expect("open cursor task");
    assert_eq!(model.edit_target(), Some(first));
    assert_eq!(model.marked_count(), 1, "Enter itself does not spend marks");
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("clear marks");
    assert_eq!(model.marked_count(), 0, "Esc clears marks");
    assert_eq!(
        model.edit_target(),
        Some(first),
        "mark clearing keeps the page open"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close page");

    mark_tasks(&mut domain, &mut model, &[second]);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(first_index),
        None,
    )
    .expect("restore first cursor");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("edit cursor task");
    assert_eq!(model.edit_target(), Some(first));
    assert_eq!(model.marked_count(), 0, "ctrl+e clears the consumed set");
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("cancel edit");
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close page");

    mark_tasks(&mut domain, &mut model, &[first]);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("open drawer");
    assert_eq!(model.marked_count(), 0, "drawer changes clear marks");
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleInboxGroup, None).expect("fold inbox");
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleInboxGroup, None)
        .expect("reopen inbox on its header");
    apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None)
        .expect("select an inbox task");
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None)
        .expect("enter mark mode");
    apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).expect("mark inbox task");
    assert_eq!(model.marked_count(), 1);
    apply_intent(&mut domain, &mut model, BoardIntent::SelectPrev, None)
        .expect("return to inbox header");
    assert!(model.inbox_header_selected());
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
        .expect("fold inbox with Enter");
    assert_eq!(
        model.marked_count(),
        0,
        "Enter on a group header clears hidden marks"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
        .expect("reopen inbox with Enter");
    mark_tasks(&mut domain, &mut model, &[first]);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::Projects),
        None,
    )
    .expect("switch lens");
    assert_eq!(model.marked_count(), 0, "tab changes clear marks");
}

#[test]
fn parked_task_page_still_allows_marking_and_bulk_verbs_on_the_board() {
    let (mut domain, mut model, first) = board_with_task("first", HumanStatus::Open);
    let second = domain
        .create(
            "second",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    model.sync_from_domain(&domain);
    mark_tasks(&mut domain, &mut model, &[first, second]);

    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
        .expect("open marked cursor task");
    for _ in 0..3 {
        apply_intent(&mut domain, &mut model, BoardIntent::StageLeft, None)
            .expect("return focus to the board");
    }
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(model.marked_count(), 2);

    apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None)
        .expect("toggle a mark while the page is parked");
    assert_eq!(model.marked_count(), 1);
    apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None)
        .expect("restore the cursor mark");
    assert_eq!(model.marked_count(), 2);

    apply_intent(&mut domain, &mut model, BoardIntent::Complete, None)
        .expect("complete marked set while the page is parked");
    assert_eq!(domain.get(first).expect("first").status, HumanStatus::Done);
    assert_eq!(
        domain.get(second).expect("second").status,
        HumanStatus::Done
    );
}

#[test]
fn task_page_status_verbs_stay_cursor_only_and_clear_board_marks() {
    let (mut domain, mut model, cursor) = board_with_task("cursor", HumanStatus::Open);
    let marked = domain
        .create(
            "marked",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create marked task");
    model.sync_from_domain(&domain);
    mark_tasks(&mut domain, &mut model, &[marked]);
    let cursor_index = model
        .visible_ids()
        .iter()
        .position(|&id| id == cursor)
        .expect("cursor task visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(cursor_index),
        None,
    )
    .expect("select cursor task");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
        .expect("open cursor task");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SetStatus(HumanStatus::Blocked),
        None,
    )
    .expect("apply status from task page");
    assert_eq!(
        domain.get(cursor).expect("cursor").status,
        HumanStatus::Blocked
    );
    assert_eq!(
        domain.get(marked).expect("marked").status,
        HumanStatus::Open
    );
    assert_eq!(model.edit_target(), Some(cursor));
    assert_eq!(model.marked_count(), 0);
}

#[test]
fn ctrl_n_sets_ready_and_ctrl_o_sets_open_from_every_status_without_ready_open_events() {
    assert_eq!(
        map_key(BoardInputMode::Normal, ctrl(KeyCode::Char('n'))),
        Some(BoardIntent::SetStatus(HumanStatus::Ready))
    );
    assert_eq!(
        map_key(BoardInputMode::Normal, ctrl(KeyCode::Char('o'))),
        Some(BoardIntent::Reopen)
    );
    assert_eq!(
        map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('n'))),
        Some(BoardIntent::SetStatus(HumanStatus::Ready))
    );

    for from in [
        HumanStatus::Open,
        HumanStatus::Ready,
        HumanStatus::Started,
        HumanStatus::Blocked,
        HumanStatus::Review,
        HumanStatus::Done,
    ] {
        let (mut domain, mut model, id) = board_with_task("ready target", from);
        if from == HumanStatus::Done {
            select_done_task(&mut domain, &mut model, id);
        }
        let before = domain.get(id).expect("task").clone();
        let outcome = apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::SetStatus(HumanStatus::Ready),
            None,
        )
        .expect("set ready");
        let task = domain.get(id).expect("task");
        assert_eq!(task.status, HumanStatus::Ready, "{from:?} → ready");
        if from == HumanStatus::Ready {
            assert_eq!(outcome, IntentOutcome::None, "ready is a no-op");
            assert_eq!(task.revision, before.revision, "ready has no event");
            assert_eq!(
                task.history.len(),
                before.history.len(),
                "ready has no history"
            );
        } else {
            assert_eq!(outcome, IntentOutcome::Persist, "{from:?} → ready persists");
            assert_ne!(task.revision, before.revision, "{from:?} gets one event");
        }

        let (mut domain, mut model, id) = board_with_task("open target", from);
        if from == HumanStatus::Done {
            select_done_task(&mut domain, &mut model, id);
        }
        let before = domain.get(id).expect("task").clone();
        let reopen = map_key(BoardInputMode::Normal, ctrl(KeyCode::Char('o')))
            .expect("ctrl+o maps to reopen");
        let outcome = apply_intent(&mut domain, &mut model, reopen, None).expect("set open");
        let task = domain.get(id).expect("task");
        assert_eq!(task.status, HumanStatus::Open, "{from:?} → open");
        if from == HumanStatus::Open {
            assert_eq!(outcome, IntentOutcome::None, "open is a no-op");
            assert_eq!(task.revision, before.revision, "open has no event");
            assert_eq!(
                task.history.len(),
                before.history.len(),
                "open has no history"
            );
        } else {
            assert_eq!(outcome, IntentOutcome::Persist, "{from:?} → open persists");
            assert_ne!(task.revision, before.revision, "{from:?} gets one event");
            assert_eq!(
                task.history.last().expect("open event").kind,
                if from == HumanStatus::Done {
                    TaskEventKind::Reopened
                } else {
                    TaskEventKind::StatusSet
                },
                "ctrl+o event for {from:?}"
            );
        }
    }
}

#[test]
fn space_on_todo_sets_doing_via_domain() {
    let (mut domain, mut model, id) = board_with_task("start me", HumanStatus::Ready);
    assert_eq!(model.selected_id(), Some(id));

    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::PrimaryVerb, None).expect("primary");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Started);
    assert_eq!(
        model
            .visible_tasks()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.status),
        Some(HumanStatus::Started)
    );
    assert!(
        board_intent_may_persist(&model, &BoardIntent::PrimaryVerb),
        "PrimaryVerb must load a save baseline when it can mutate"
    );
}

#[test]
fn space_on_done_is_silent_noop() {
    let (mut domain, mut model, id) = board_with_task("keep done", HumanStatus::Done);
    // Done tasks live in the drawer; open it so selection can hold the done id.
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    // Re-select the done task if reanchor moved off it.
    if model.selected_id() != Some(id) {
        let idx = model
            .visible_ids()
            .iter()
            .position(|&row| row == id)
            .expect("done row visible with drawer open");
        apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None)
            .expect("select done");
    }

    let before = domain.get(id).expect("task").clone();
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::PrimaryVerb, None).expect("primary");
    let after = domain.get(id).expect("task");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(after.status, HumanStatus::Done);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.history, before.history);
}

#[test]
fn space_on_doing_is_silent_noop() {
    let (mut domain, mut model, id) = board_with_task("already going", HumanStatus::Started);
    let before = domain.get(id).expect("task").clone();

    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::PrimaryVerb, None).expect("primary");
    assert_eq!(outcome, IntentOutcome::None);
    let after = domain.get(id).expect("task");
    assert_eq!(after.status, before.status);
    assert_eq!(after.revision, before.revision);
    assert_eq!(model.message(), None, "started primary stays silent");
}

#[test]
fn d_completes_non_done_and_o_reopens_done() {
    // d completes non-done
    let (mut domain, mut model, id) = board_with_task("finish me", HumanStatus::Ready);
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::Complete, None).expect("complete");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Done);

    // o reopens done (drawer so the done row stays addressable)
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    if model.selected_id() != Some(id) {
        let idx = model
            .visible_ids()
            .iter()
            .position(|&row| row == id)
            .expect("done visible");
        apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None).expect("select");
    }
    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::Reopen, None).expect("reopen");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Open);

    assert!(board_intent_may_persist(&model, &BoardIntent::Complete));
    assert!(board_intent_may_persist(&model, &BoardIntent::Reopen));
}

#[test]
fn b_on_blocked_sets_todo_and_keeps_task_on_deck_not_in_motion() {
    let (mut domain, mut model, id) = board_with_task("unblock me", HumanStatus::Blocked);

    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleBlock, None).expect("unblock");

    assert_eq!(outcome, IntentOutcome::Persist);
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Ready);
    let view = model.queue_view();
    assert!(view
        .sections
        .iter()
        .any(|section| section.kind == SectionKind::OnDeck && section.task_ids.contains(&id)));
    assert!(!view
        .sections
        .iter()
        .any(|section| section.kind == SectionKind::InMotion && section.task_ids.contains(&id)));
    assert!(board_intent_may_persist(&model, &BoardIntent::ToggleBlock));
}

#[test]
fn b_blocks_todo_doing_and_review() {
    for status in [
        HumanStatus::Ready,
        HumanStatus::Started,
        HumanStatus::Review,
    ] {
        let (mut domain, mut model, id) = board_with_task("block me", status);

        let outcome =
            apply_intent(&mut domain, &mut model, BoardIntent::ToggleBlock, None).expect("block");

        assert_eq!(outcome, IntentOutcome::Persist, "{status:?}");
        assert_eq!(
            domain.get(id).expect("task").status,
            HumanStatus::Blocked,
            "{status:?}"
        );
    }
}

#[test]
fn enter_opens_the_task_page_and_enter_again_closes_it_without_mutating() {
    let (mut domain, mut model, id) = board_with_task("inspect me", HumanStatus::Ready);
    let rev_before = domain.get(id).expect("task").revision;
    assert_eq!(model.input_mode(), BoardInputMode::Normal);

    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
        .expect("open task page");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(model.detail_open(), None, "the page replaces any peek");
    assert_eq!(domain.get(id).expect("task").revision, rev_before);
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Ready);

    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
        .expect("close task page");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(domain.get(id).expect("task").revision, rev_before);
    assert!(!board_intent_may_persist(
        &model,
        &BoardIntent::OpenTaskPage
    ));
}

#[test]
fn right_arrow_peeks_detail_and_left_arrow_collapses_it() {
    let (mut domain, mut model, id) = board_with_task("peek me", HumanStatus::Ready);
    let rev_before = domain.get(id).expect("task").revision;
    assert_eq!(model.detail_open(), None);

    // Right opens the peek on the selected row without touching the domain.
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::PeekDetail, None).expect("peek detail");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.detail_open(), Some(id));
    assert_eq!(domain.get(id).expect("task").revision, rev_before);

    // Right again is idempotent: still open, still no mutation.
    apply_intent(&mut domain, &mut model, BoardIntent::PeekDetail, None).expect("peek again");
    assert_eq!(model.detail_open(), Some(id));

    // Left collapses the open peek.
    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::CollapseDetail, None)
        .expect("collapse detail");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.detail_open(), None);

    // Left with nothing open is a clean no-op.
    apply_intent(&mut domain, &mut model, BoardIntent::CollapseDetail, None)
        .expect("collapse no-op");
    assert_eq!(model.detail_open(), None);
    assert_eq!(domain.get(id).expect("task").revision, rev_before);

    assert!(!board_intent_may_persist(&model, &BoardIntent::PeekDetail));
    assert!(!board_intent_may_persist(
        &model,
        &BoardIntent::CollapseDetail
    ));
}

#[test]
fn detail_closes_when_its_task_leaves_the_visible_set() {
    let (mut domain, mut model, id) = board_with_task("inspect then remove", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::PeekDetail, None).expect("open peek");
    assert_eq!(model.detail_open(), Some(id));

    domain.soft_delete(id).expect("soft delete");
    model.sync_from_domain(&domain);
    assert_eq!(model.detail_open(), None);
}

#[test]
fn z_toggles_done_drawer_membership_on_list() {
    let (mut domain, mut model, done_id) = board_with_task("archived", HumanStatus::Done);
    let open_id = domain
        .create(
            "still open",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("open task");
    model.sync_from_domain(&domain);

    assert!(!model.drawer_open());
    assert!(
        !model.visible_ids().contains(&done_id),
        "done row hidden while drawer closed"
    );
    assert!(model.visible_ids().contains(&open_id));

    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("open drawer");
    assert!(model.drawer_open());
    assert!(
        model.visible_ids().contains(&done_id),
        "done row joins the list when drawer opens"
    );
    let has_done_section = model
        .queue_view()
        .sections
        .iter()
        .any(|s| s.kind == SectionKind::Done && s.task_ids.contains(&done_id));
    assert!(has_done_section);

    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("close drawer");
    assert!(!model.drawer_open());
    assert!(!model.visible_ids().contains(&done_id));
}

#[test]
fn block_on_done_and_verbs_without_selection_leave_visible_notices() {
    let (mut domain, mut model, id) = board_with_task("finished", HumanStatus::Done);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("open drawer");
    let index = model
        .visible_ids()
        .iter()
        .position(|&row| row == id)
        .expect("done task visible in drawer");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(index),
        None,
    )
    .expect("select done task");

    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleBlock, None).expect("block done");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(domain.get(id).expect("done task").status, HumanStatus::Done);
    assert!(model.message().is_some_and(|message| !message.is_empty()));

    let mut empty_domain = DomainState::new();
    let mut empty_model = BoardModel::from_domain(&empty_domain, Some(PathBuf::from(THIS_REPO)));
    for intent in [BoardIntent::PrimaryVerb, BoardIntent::ToggleBlock] {
        let outcome = apply_intent(&mut empty_domain, &mut empty_model, intent, None)
            .expect("verb without selection");
        assert_eq!(outcome, IntentOutcome::None);
        assert!(empty_model
            .message()
            .is_some_and(|message| !message.is_empty()));
    }
}

#[test]
fn esc_closes_transient_then_detail_then_quits_at_the_root() {
    let (mut domain, mut model, id) = board_with_task("layered", HumanStatus::Ready);

    // Transient: help card
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("help");
    assert_eq!(model.input_mode(), BoardInputMode::Help);
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("esc help");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);

    // Transient: palette (command surface)
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("palette");
    assert_ne!(model.command_surface(), CommandSurface::None);
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("esc palette");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.command_surface(), CommandSurface::None);

    // Peek / accordion
    apply_intent(&mut domain, &mut model, BoardIntent::PeekDetail, None).expect("peek");
    assert_eq!(model.detail_open(), Some(id));
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("esc detail");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.detail_open(), None);

    // Nothing open: the board-level Esc quits without changing navigation state.
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("esc quits");
    assert_eq!(outcome, IntentOutcome::Quit);
}

#[test]
fn t64_ctrl_q_quits_every_non_editor_mode_and_stays_inert_in_editors_and_recovery() {
    let ctrl_q = ctrl(KeyCode::Char('q'));
    for mode in [
        BoardInputMode::Normal,
        BoardInputMode::TaskPage,
        BoardInputMode::CapturePage,
        BoardInputMode::SelectThread,
        BoardInputMode::EditScope,
        BoardInputMode::FormDropdown,
        BoardInputMode::LaunchCard,
        BoardInputMode::ProjectPicker,
        BoardInputMode::ListPicker,
        BoardInputMode::Help,
    ] {
        assert_eq!(
            map_key(mode, ctrl_q),
            Some(BoardIntent::Quit),
            "{mode:?} is a non-editor surface"
        );
    }

    for mode in [
        BoardInputMode::EditTitle,
        BoardInputMode::EditNotes,
        BoardInputMode::EditStep,
        BoardInputMode::EditThread,
        BoardInputMode::Search,
        BoardInputMode::QuickAdd,
        BoardInputMode::Palette,
        BoardInputMode::SaveRecovery,
    ] {
        assert_ne!(
            map_key(mode, ctrl_q),
            Some(BoardIntent::Quit),
            "{mode:?} retains its existing editor or recovery behavior"
        );
    }

    assert_eq!(
        map_key(BoardInputMode::Normal, press(KeyCode::Char('q'))),
        None
    );
}

#[test]
fn t64_ctrl_q_quits_instead_of_dismissing_help_or_pickers() {
    let (mut domain, mut model, _) = board_with_task("quit surfaces", HumanStatus::Ready);

    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("open help");
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::Quit, None).expect("quit help"),
        IntentOutcome::Quit
    );

    apply_intent(&mut domain, &mut model, BoardIntent::CloseHelp, None).expect("close help");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open picker");
    assert_eq!(model.input_mode(), BoardInputMode::ProjectPicker);
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::Quit, None).expect("quit picker"),
        IntentOutcome::Quit
    );
}

#[test]
fn t64_root_esc_quits_on_each_navigation_tab_without_resetting_it() {
    for tab in [
        tsk_tui::ui::queue::NavTab::Desk,
        tsk_tui::ui::queue::NavTab::ProjectBoard,
        tsk_tui::ui::queue::NavTab::Projects,
    ] {
        let mut domain = DomainState::new();
        let id = domain
            .create(
                "tab task",
                None,
                project(THIS_REPO),
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("project task");
        domain
            .set_status(id, HumanStatus::Ready)
            .expect("put task on deck");
        let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
        if tab == tsk_tui::ui::queue::NavTab::ProjectBoard {
            model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
        } else {
            apply_intent(
                &mut domain,
                &mut model,
                BoardIntent::SelectNavTab(tab),
                None,
            )
            .expect("select tab");
        }

        assert_eq!(model.wide_stage(), tier::WideStage::FullBoard);
        assert_eq!(model.nav_tab(), tab);
        assert_eq!(model.input_mode(), BoardInputMode::Normal, "{tab:?}");
        assert_eq!(model.popup(), BoardPopup::None, "{tab:?}");
        assert_eq!(model.command_surface(), CommandSurface::None, "{tab:?}");
        assert_eq!(model.detail_open(), None, "{tab:?}");
        assert_eq!(model.search_query(), "", "{tab:?}");
        assert!(!model.inbox_header_selected(), "{tab:?}");
        assert!(!model.archived_header_selected(), "{tab:?}");
        assert!(!model.focus_is_archived(), "{tab:?}");
        assert_eq!(
            apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("root Esc"),
            IntentOutcome::Quit,
            "{tab:?}"
        );
        assert_eq!(model.nav_tab(), tab, "root Esc must not reset the tab");
    }
}

#[test]
fn t64_esc_collapses_split_without_quitting_and_keeps_header_exclusions() {
    let (mut domain, mut model, _) = board_with_task("split", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None).expect("open split");
    assert_eq!(model.wide_stage(), tier::WideStage::Split);
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("split Esc"),
        IntentOutcome::None
    );
    assert_eq!(model.wide_stage(), tier::WideStage::FullBoard);

    let mut domain = DomainState::new();
    let archived = domain
        .create(
            "header",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("archived task");
    domain.archive_task(archived).expect("archive task");
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("open drawer");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleArchivedGroup,
        None,
    )
    .expect("select archived header");
    assert!(model.archived_header_selected());
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("header Esc"),
        IntentOutcome::None
    );
}

#[test]
fn t64_inbox_header_esc_does_not_quit() {
    let (mut domain, mut model, _) = board_with_task("inbox", HumanStatus::Open);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleInboxGroup, None)
        .expect("select inbox header");
    assert!(model.inbox_header_selected());
    assert_eq!(model.wide_stage(), tier::WideStage::FullBoard);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
            .expect("inbox header Esc"),
        IntentOutcome::None
    );
}

#[test]
fn t64_help_has_one_ctrl_q_quit_binding() {
    for query in ["", "quit", "exit"] {
        let lines = tsk_tui::ui::input::help_card_lines_for_query(query);
        let quit_rows = lines
            .iter()
            .filter(|line| line.split_whitespace().next() == Some("ctrl+q"))
            .collect::<Vec<_>>();
        assert_eq!(quit_rows.len(), 1, "query={query:?}: {lines:?}");
        assert!(quit_rows[0].contains("quit"));
    }
}

fn rendered_board(model: &BoardModel, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
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

/// The queue status line exactly as painted (the chrome), matching f6's helper.
fn board_chrome_row(model: &BoardModel, mode: (u16, u16)) -> String {
    let geo = tier::resolve(mode.0, mode.1);
    let status_row = geo
        .status_row
        .expect("status row present at supported sizes");
    let mut terminal = Terminal::new(TestBackend::new(mode.0, mode.1)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer().clone();
    (0..mode.0)
        .map(|x| buffer[(x, status_row)].symbol())
        .collect()
}

/// the palette catalog, subsequence filter, same intents as key routes.
#[test]
fn palette_lists_exactly_m1_commands_for_selection_filters_by_subsequence_and_dispatches_same_intents_as_keys(
) {
    let (mut domain, mut model, id) = board_with_task("palette me", HumanStatus::Ready);

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("open palette");
    assert_eq!(model.command_surface(), CommandSurface::Palette);

    let labels: Vec<String> = model
        .visible_commands()
        .iter()
        .map(|c| c.label.clone())
        .collect();
    let expected = [
        "set status: ready",
        "set status: open",
        "set status: started",
        "set status: blocked",
        "set status: review",
        "edit notes",
        "change scope",
        "set assignee",
        "new task",
        "delete",
        "undo",
        "done drawer",
        "help",
        "quit",
    ];
    assert_eq!(
        labels, expected,
        "palette catalog with a selection must match exactly (no toggle-groups entry: \
         the destinations have no collapsible task groups)"
    );
    assert!(
        labels.iter().any(|label| label == "set status: open"),
        "a ready selection offers the absolute inbox status"
    );

    // Subsequence (not substring): "started" matches only that status label.
    for ch in "started".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::CommandQueryInsert(ch),
            None,
        )
        .expect("type");
    }
    let filtered: Vec<String> = model
        .visible_commands()
        .iter()
        .map(|c| c.label.clone())
        .collect();
    assert_eq!(filtered, vec!["set status: started"]);
    // Substring-only would need the contiguous run "ssg"; none of the labels contain it.
    assert!(
        model
            .visible_commands()
            .iter()
            .all(|c| !c.label.to_ascii_lowercase().contains("ssg")),
        "filter must be subsequence, not substring"
    );

    // Confirm dispatches the same intent as SetStatus(Doing) would.
    let resolved = resolve_board_command(&mut model, BoardIntent::ConfirmCommand).expect("resolve");
    assert_eq!(resolved, BoardIntent::SetStatus(HumanStatus::Started));
    apply_intent(&mut domain, &mut model, resolved, None).expect("apply");
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Started);

    // The absolute status commands remain available from every working status.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("palette while doing");
    assert!(
        model
            .visible_commands()
            .iter()
            .any(|c| c.label == "set status: open"),
        "a doing selection offers the absolute inbox status"
    );

    // Key-equivalent: `o` sets open, and the palette has the same absolute route.
    domain.set_status(id, HumanStatus::Done).expect("done");
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    if model.selected_id() != Some(id) {
        let idx = model
            .visible_ids()
            .iter()
            .position(|&row| row == id)
            .expect("done visible");
        apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None).expect("select");
    }

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("palette again");
    // the done selection's catalog offers the absolute inbox command.
    assert!(
        model
            .visible_commands()
            .iter()
            .any(|c| c.label == "set status: open"),
        "a done selection must offer set status: open"
    );
    for ch in "set status: open".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::CommandQueryInsert(ch),
            None,
        )
        .expect("type reopen");
    }
    let via_palette =
        resolve_board_command(&mut model, BoardIntent::ConfirmCommand).expect("open cmd");
    assert_eq!(via_palette, BoardIntent::SetStatus(HumanStatus::Open));
    apply_intent(&mut domain, &mut model, via_palette, None).expect("open");
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Open);

    // Direct key route lands on the same intent.
    assert_eq!(
        map_key(BoardInputMode::Normal, ctrl(KeyCode::Char('o'))),
        Some(BoardIntent::Reopen)
    );

    // Painted palette shows the catalog header / a command row.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("paint palette");
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.lines().any(|line| line.contains(" command")),
        "palette overlay must paint its header: {frame:?}"
    );
}

/// park / resume / link / dispatch never appear in the palette.
///
/// The second block builds the strongest old-store state (a selected task carrying a
/// durable dispatch attempt) so the exclusion stays load-bearing even though the
/// recovery surface is gone from this tree.
#[test]
fn palette_excludes_park_resume_link_dispatch() {
    let (mut domain, mut model, _id) = board_with_task("no classic tail", HumanStatus::Ready);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("palette");

    for cmd in model.visible_commands() {
        let lower = cmd.label.to_ascii_lowercase();
        for word in ["park", "resume", "link", "dispatch", "cleanup"] {
            assert!(
                !lower.contains(word),
                "palette label {:?} must not mention {word}",
                cmd.label
            );
        }
    }
}

/// The palette window follows a wrapped selection, so Enter never targets an invisible row.
#[test]
fn palette_window_keeps_wrapped_last_command_visible() {
    let (mut domain, mut model, _id) = board_with_task("palette window", HumanStatus::Ready);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCommandPalette,
        None,
    )
    .expect("open palette");
    apply_intent(&mut domain, &mut model, BoardIntent::CommandPrev, None)
        .expect("wrap to last command");
    assert_eq!(
        model.selected_command().expect("last command").label,
        "quit"
    );

    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.lines().any(|line| line.contains("▸ quit")),
        "the selected tail command must be painted: {frame:?}"
    );
}

#[test]
fn help_card_uses_one_binding_per_row_and_opens_from_the_task_page() {
    let (mut domain, mut model, _id) = board_with_task("help structure", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("help");
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.lines().all(|line| !line.contains(" | ")),
        "help must not pair unrelated bindings on one row:\n{frame}"
    );
    assert_eq!(
        map_key(BoardInputMode::TaskPage, press(KeyCode::Char('?'))),
        Some(BoardIntent::OpenHelp),
        "question mark opens help from the non-editing task page"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close Help");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("task page");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("task Help");
    assert!(
        !model.has_unsaved_work(),
        "Help over a clean task page must not invent unsaved work"
    );
}

/// Help lists every binding, filters from its focused query, scrolls, and closes progressively.
#[test]
fn help_card_lists_every_binding_scrolls_and_closes_on_esc() {
    let (mut domain, mut model, _id) = board_with_task("help me", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("help");
    assert_eq!(model.input_mode(), BoardInputMode::Help);
    assert_eq!(model.help_query(), "");

    let lines = tsk_tui::ui::input::help_card_lines();
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.split_whitespace().next() == Some("/"))
            .count(),
        1,
        "Help must list the board search binding exactly once:\n{}",
        lines.join("\n")
    );
    assert!(
        lines.iter().all(|line| !line.contains("search projects")),
        "Help must not retain the Projects-only search label"
    );
    let bindings = normal_help_bindings();
    assert!(
        !bindings.is_empty(),
        "help bindings table must not be empty"
    );
    for (chord, label) in bindings {
        assert!(
            lines.iter().any(|line| {
                line.contains(label)
                    && chord.split(" / ").all(|part| {
                        line.to_ascii_lowercase()
                            .contains(&part.to_ascii_lowercase())
                    })
            }),
            "help content must keep {chord:?} and {label:?} on one row:\n{}",
            lines.join("\n")
        );
    }
    for group in [
        "NAVIGATION",
        "TASK ACTIONS",
        "CREATE & EDIT",
        "VIEWS & FIND",
        "SURFACE CONTROLS",
        "APP CONTROLS",
    ] {
        assert!(
            lines.iter().any(|line| line.contains(group)),
            "missing {group}"
        );
    }
    assert!(lines.iter().all(|line| !line.contains(" | ")));
    for (chord, action) in [
        ("ctrl+x / ctrl+delete", "delete"),
        ("ctrl+c", "cancel editing"),
        ("ctrl+e", "line end (text input)"),
        ("ctrl+←", "word left (text input)"),
        ("home", "line start (text input)"),
        ("backspace", "delete backward (text input)"),
        ("q", "close project picker"),
        ("y", "unarchive launch project"),
        ("r / enter", "retry failed save"),
        ("↑↓ / page keys", "scroll Help"),
    ] {
        assert!(
            lines
                .iter()
                .any(|line| line.contains(chord) && line.contains(action)),
            "missing help row for {chord:?} {action:?}"
        );
    }

    let filtered = tsk_tui::ui::input::help_card_lines_for_query("drawer");
    let filtered_text = filtered.join("\n");
    assert!(filtered_text.contains("VIEWS & FIND"));
    assert!(filtered_text.contains("done drawer"));
    assert!(!filtered_text.contains("start / reopen"));
    let aliases = tsk_tui::ui::input::help_card_lines_for_query("finish").join("\n");
    assert!(aliases.contains("TASK ACTIONS") && aliases.contains("done"));

    // The centered card never exceeds half the available height.
    let frame = rendered_board(&model, 100, 60);
    let rows: Vec<_> = frame.lines().collect();
    let top = rows
        .iter()
        .position(|row| row.contains("┌─ help"))
        .expect("top");
    let bottom = rows
        .iter()
        .position(|row| row.contains('└'))
        .expect("bottom");
    assert!(bottom - top < 30, "help exceeds half height:\n{frame}");
    assert!(frame.contains("search keys or actions"));

    let compact = rendered_board(&model, 40, 10);
    assert!(
        compact.contains("help ▼"),
        "compact help must mark overflow: {compact}"
    );

    let down = map_key(BoardInputMode::Help, press(KeyCode::Down)).expect("↓ scrolls");
    assert_eq!(down, BoardIntent::HelpScrollDown);
    assert_eq!(
        map_key(BoardInputMode::Help, press(KeyCode::Char('j'))),
        Some(BoardIntent::HelpQueryInsert('j'))
    );
    assert_eq!(
        map_key(BoardInputMode::Help, press(KeyCode::Char('?'))),
        Some(BoardIntent::HelpQueryInsert('?'))
    );

    for character in "drawer".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::HelpQueryInsert(character),
            None,
        )
        .expect("type query");
    }
    assert_eq!(model.help_query(), "drawer");
    let searched = rendered_board(&model, 80, 24);
    assert!(
        searched.contains("done drawer"),
        "matching action remains:\n{searched}"
    );
    let compact_search = rendered_board(&model, 40, 10);
    assert!(
        compact_search.contains("done drawer"),
        "compact help must show a filtered result below search:\n{compact_search}"
    );
    let compact_rows: Vec<_> = compact_search.lines().collect();
    let query_row = compact_rows
        .iter()
        .position(|row| row.contains("/ drawer"))
        .expect("focused Help query");
    assert!(
        compact_rows[query_row + 1].matches('─').count() > 10,
        "a dim divider belongs directly below search:\n{compact_search}"
    );
    let compact_top = compact_rows
        .iter()
        .position(|row| row.contains("┌─ help"))
        .expect("compact Help top");
    let compact_bottom = compact_rows
        .iter()
        .rposition(|row| row.contains('└'))
        .expect("compact Help bottom");
    assert!(
        compact_bottom - compact_top < 6,
        "short Help may grow to six rows, no more:\n{compact_search}"
    );

    let mut wrapped_model = model.clone();
    for _ in 0..wrapped_model.help_query().chars().count() {
        apply_intent(
            &mut domain,
            &mut wrapped_model,
            BoardIntent::HelpQueryBackspace,
            None,
        )
        .expect("clear cloned query");
    }
    for character in "previous step".chars() {
        apply_intent(
            &mut domain,
            &mut wrapped_model,
            BoardIntent::HelpQueryInsert(character),
            None,
        )
        .expect("type wrapping query");
    }
    let wrapped_help = rendered_board(&wrapped_model, 48, 19);
    let wrapped_rows: Vec<_> = wrapped_help.lines().collect();
    let binding_row = wrapped_rows
        .iter()
        .position(|row| row.contains("shift+tab") && row.contains("select previous step"))
        .expect("wrapped binding first row");
    let description_col = wrapped_rows[binding_row]
        .find("select previous step")
        .expect("description column");
    assert_eq!(
        wrapped_rows[binding_row + 1].find("(task page)"),
        Some(description_col),
        "continuation must align under the description:\n{wrapped_help}"
    );

    let mut scroll_model = model.clone();
    for _ in 0..scroll_model.help_query().chars().count() {
        apply_intent(
            &mut domain,
            &mut scroll_model,
            BoardIntent::HelpQueryBackspace,
            None,
        )
        .expect("clear scroll query");
    }
    let mut scrolled_help = rendered_board(&scroll_model, 40, 10);
    for _ in 0..200 {
        apply_intent(
            &mut domain,
            &mut scroll_model,
            BoardIntent::HelpScrollDown,
            None,
        )
        .expect("scroll wrapped Help");
        scrolled_help = rendered_board(&scroll_model, 40, 10);
    }
    assert!(
        scroll_model.help_scroll() > tsk_tui::ui::input::help_card_lines().len() - 1,
        "wrapped rows must extend the scroll horizon"
    );
    assert!(
        scrolled_help.contains("ctrl+q") && scrolled_help.contains("quit"),
        "the wrapped tail must remain reachable:\n{scrolled_help}"
    );
    assert!(
        !searched.contains("start / reopen"),
        "non-match remains:\n{searched}"
    );

    let close = map_key(BoardInputMode::Help, press(KeyCode::Esc)).expect("esc");
    apply_intent(&mut domain, &mut model, close.clone(), None).expect("clear query");
    assert_eq!(model.input_mode(), BoardInputMode::Help);
    assert_eq!(model.help_query(), "");
    apply_intent(&mut domain, &mut model, close, None).expect("close help");
    assert_eq!(model.input_mode(), BoardInputMode::Normal);

    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("task page");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("task help");
    assert!(
        !model.has_unsaved_work(),
        "Help over a clean task page must not invent unsaved work"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close task help");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    assert_eq!(
        map_key(BoardInputMode::ProjectPicker, press(KeyCode::Char('?'))),
        Some(BoardIntent::OpenHelp)
    );
    assert_eq!(
        map_key(BoardInputMode::LaunchCard, press(KeyCode::Char('?'))),
        Some(BoardIntent::OpenHelp)
    );
    for mode in [
        BoardInputMode::SelectThread,
        BoardInputMode::EditScope,
        BoardInputMode::FormDropdown,
    ] {
        assert_eq!(
            map_key(mode, press(KeyCode::Char('?'))),
            Some(BoardIntent::OpenHelp),
            "question mark opens Help from non-text control {mode:?}"
        );
    }
    for mode in [
        BoardInputMode::EditTitle,
        BoardInputMode::EditNotes,
        BoardInputMode::EditThread,
    ] {
        assert_eq!(
            map_key(mode, press(KeyCode::Char('?'))),
            Some(BoardIntent::EditInsert('?')),
            "question mark remains text while editing {mode:?}"
        );
    }
    for (mode, expected) in [
        (BoardInputMode::QuickAdd, BoardIntent::QuickAddInsert('?')),
        (
            BoardInputMode::Palette,
            BoardIntent::CommandQueryInsert('?'),
        ),
        (
            BoardInputMode::ListPicker,
            BoardIntent::ListPickerQueryInsert('?'),
        ),
        (BoardInputMode::Search, BoardIntent::SearchQueryInsert('?')),
    ] {
        assert_eq!(
            map_key(mode, press(KeyCode::Char('?'))),
            Some(expected),
            "question mark remains text in {mode:?}"
        );
    }
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close task page");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("project picker");
    assert_eq!(model.input_mode(), BoardInputMode::ProjectPicker);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("picker help");
    assert_eq!(model.input_mode(), BoardInputMode::Help);
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
        .expect("close picker help");
    assert_eq!(model.input_mode(), BoardInputMode::ProjectPicker);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CancelProjectPicker,
        None,
    )
    .expect("close picker");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("task page");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditScope, None)
        .expect("scope control");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("scope help");
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close scope help");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
}

/// project-scope chip/dropdown filters every visible section and count.
#[test]
fn project_scope_chip_and_dropdown_filter_all_visible_sections_matching_ac5() {
    let mut domain = DomainState::new();
    let motion_app = domain
        .create(
            "motion app",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("motion app");
    domain
        .set_status(motion_app, HumanStatus::Started)
        .expect("doing");
    let motion_other = domain
        .create(
            "motion other",
            None,
            project("/repos/other"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("motion other");
    domain
        .set_status(motion_other, HumanStatus::Started)
        .expect("doing other");
    let motion_global = domain
        .create(
            "motion global",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("motion global");
    domain
        .set_status(motion_global, HumanStatus::Started)
        .expect("doing global");
    let deck_app = domain
        .create(
            "deck app",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("deck app");
    let deck_other = domain
        .create(
            "deck other",
            None,
            project("/repos/other"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("deck other");
    let deck_global = domain
        .create(
            "deck global",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("deck global");
    let done_app = domain
        .create(
            "done app",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("done app");
    domain
        .set_status(done_app, HumanStatus::Done)
        .expect("done");
    let done_other = domain
        .create(
            "done other",
            None,
            project("/repos/other"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("done other");
    domain
        .set_status(done_other, HumanStatus::Done)
        .expect("done");
    let done_global = domain
        .create(
            "done global",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("done global");
    domain
        .set_status(done_global, HumanStatus::Done)
        .expect("done");
    // Empty-scoped project path with no open deck tasks (only a done task).
    let done_empty = domain
        .create(
            "done only",
            None,
            project("/repos/empty"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("empty project done");
    domain
        .set_status(done_empty, HumanStatus::Done)
        .expect("done");

    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("open done drawer");

    // Chip path: open selector without needing the classic Projects lens.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open scope dropdown");
    assert_eq!(model.input_mode(), BoardInputMode::ProjectPicker);
    assert_eq!(model.popup(), BoardPopup::ProjectPicker);

    let frame = rendered_board(&model, 80, 24);
    // The dropdown marks its highlighted option (the current project at open) and
    // paints the desk choice plus every project option around it.
    let marked_row = frame
        .lines()
        .position(|line| line.contains('▸'))
        .expect("scope dropdown must mark the highlighted option");
    assert!(
        frame.lines().any(|line| line.contains("desk")),
        "scope dropdown must paint the home/desk option: {frame:?}"
    );
    assert!(
        frame
            .lines()
            .skip(marked_row.saturating_sub(2))
            .take(5)
            .any(|line| line.contains("other") || line.contains("app") || line.contains("empty")),
        "scope dropdown must paint the project options: {frame:?}"
    );

    // Move to /repos/other and confirm (session-only deck scope).
    let options = model.project_options();
    let other_idx = options
        .iter()
        .position(|opt| opt == &ProjectScopeOption::Project(PathBuf::from("/repos/other")))
        .expect("/repos/other in options");
    for _ in 0..options.len() {
        if model.project_picker_index() == Some(other_idx) {
            break;
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ProjectPickerNext,
            None,
        )
        .expect("next");
    }
    assert_eq!(model.project_picker_index(), Some(other_idx));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmProjectChoice,
        None,
    )
    .expect("confirm scope");

    assert_eq!(model.selected_project(), Some(Path::new("/repos/other")));
    let other_view = model.queue_view();
    assert_eq!(other_view.counts.in_motion, 1);
    assert_eq!(other_view.counts.done, 1);
    assert_eq!(
        model.visible_ids(),
        vec![
            motion_other,
            tsk_tui::ui::queue::INBOX_HEADER_ROW_ID,
            deck_other,
            done_other,
        ],
        "a project scope leaves no task from another scope visible"
    );
    assert!(!model.visible_ids().contains(&done_app));
    assert!(!model.visible_ids().contains(&done_global));
    model.clear_message();
    let other_status = board_chrome_row(&model, (80, 24));
    assert_eq!(
        other_status.trim(),
        "other",
        "scoped status must name the project basename: {other_status:?}"
    );
    assert!(!model.visible_ids().contains(&motion_app));
    assert!(!model.visible_ids().contains(&deck_app));

    // Home returns to the desk tab with global IN MOTION and desk-scoped ON DECK.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("reopen for home");
    let home_idx = model
        .project_options()
        .iter()
        .position(|option| option == &ProjectScopeOption::Home)
        .expect("home option");
    for _ in 0..model.project_options().len() {
        if model.project_picker_index() == Some(home_idx) {
            break;
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ProjectPickerNext,
            None,
        )
        .expect("next home");
    }
    assert_eq!(model.project_picker_index(), Some(home_idx));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmProjectChoice,
        None,
    )
    .expect("scope home");
    assert!(model.at_home());
    assert_eq!(model.nav_tab(), tsk_tui::ui::queue::NavTab::Desk);
    let home_view = model.queue_view();
    assert_eq!(home_view.counts.in_motion, 3);
    assert!(model.visible_ids().contains(&motion_global));
    assert!(model.visible_ids().contains(&motion_app));
    assert!(model.visible_ids().contains(&deck_global));
    assert!(!model.visible_ids().contains(&deck_app));
    model.clear_message();
    let home_status = board_chrome_row(&model, (80, 24));
    assert_eq!(
        home_status.trim(),
        "desk",
        "home status must name the desk lens: {home_status:?}"
    );

    // Scoped project with zero open deck tasks → header + empty hint, no invented row.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("reopen");
    let empty_idx = model
        .project_options()
        .iter()
        .position(|opt| opt == &ProjectScopeOption::Project(PathBuf::from("/repos/empty")))
        .expect("empty project option (from selected history or tasks)");
    // Ensure empty is reachable: if not in options because only done tasks, select via path.
    // project_options includes paths from non-soft-deleted tasks, including done.
    for _ in 0..model.project_options().len() {
        if model.project_picker_index() == Some(empty_idx) {
            break;
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ProjectPickerNext,
            None,
        )
        .expect("next empty");
    }
    assert_eq!(model.project_picker_index(), Some(empty_idx));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmProjectChoice,
        None,
    )
    .expect("scope empty");

    let empty_sections: Vec<_> = model
        .queue_view()
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::OnDeck)
        .cloned()
        .collect();
    assert_eq!(empty_sections.len(), 1, "one scoped deck group");
    assert!(empty_sections[0].empty_hint, "empty hint required");
    assert!(
        empty_sections[0].task_ids.is_empty(),
        "no invented deck rows"
    );
    assert_eq!(model.queue_view().counts.in_motion, 0);
    assert_eq!(model.queue_view().counts.done, 1);
    assert_eq!(model.visible_ids(), vec![done_empty]);

    let scoped_frame = rendered_board(&model, 80, 24);
    assert!(
        scoped_frame.to_ascii_lowercase().contains("no open")
            || scoped_frame.contains("empty")
            || scoped_frame.contains("—"),
        "empty scoped deck should paint a hint: {scoped_frame:?}"
    );
}

#[test]
fn x_soft_deletes_and_status_line_names_task_with_undo_hint() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "delete me",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain
        .create(
            "keep me",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("keep");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    // pin the one we will delete
    if model.selected_id() != Some(id) {
        if let Some(idx) = model.visible_ids().iter().position(|&rid| rid == id) {
            let _ = apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None);
        }
    }
    assert_eq!(model.selected_id(), Some(id));

    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("arm delete");
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("soft delete");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert!(domain.get(id).expect("task").soft_deleted);
    assert!(!model.visible_ids().contains(&id));
    // Status line carries the framed delete notice when armed.
    // Read the status row explicitly.
    let row = board_chrome_row(&model, (80, 24));
    assert!(
        row.contains("Deleted \"delete me\""),
        "status line must name deleted task with framed wording: {row:?}"
    );
    assert!(
        row.contains("· ctrl+u undo"),
        "status line must include undo hint: {row:?}"
    );
    // selection reanchored to remaining task
    assert!(model.selected_id().is_some());
    assert_ne!(model.selected_id(), Some(id));
}

#[test]
fn delete_notice_prefixes_the_board_verb_bar_with_undo_until_undone() {
    let (mut domain, mut model, id) = board_with_task("undo seat", HumanStatus::Ready);
    // A neighbour keeps a truthful selection alive after the delete reanchors.
    let other = domain
        .create(
            "still here",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("neighbour");
    let _ = other;
    // The neighbour must be in the model before reanchoring, so the inbox heading cannot win
    // merely because the newly grouped open row was absent from the previous visible order.
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("arm delete");
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("delete");
    assert!(domain.get(id).expect("task").soft_deleted);

    let armed: Vec<&str> = board_verb_items(&model).iter().map(|v| v.key).collect();
    assert_eq!(
        armed,
        vec!["u", "enter", "s", "+", "?"],
        "armed prompt: undo · open · the row's first truthful status verb · add · help"
    );

    // The seat belongs to the notice, and the notice belongs to the plain board: any
    // surface that hides the status row hides the seat with it.
    apply_intent(&mut domain, &mut model, BoardIntent::OpenCapture, None).expect("quick-add");
    assert!(
        !board_verb_items(&model)
            .iter()
            .any(|entry| entry.key == "u"),
        "no ghost undo seat while the quick-add line hides the status row"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CancelQuickAdd, None).expect("close");
    assert_eq!(
        board_verb_items(&model)
            .iter()
            .map(|v| v.key)
            .collect::<Vec<_>>(),
        vec!["u", "enter", "s", "+", "?"],
        "the seat returns with the notice"
    );

    // Deleting the neighbour too empties the selection: the prompt keeps the undo and
    // drops the row seats it can no longer describe.
    let neighbour = model.selected_id().expect("reanchored onto the neighbour");
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("arm second");
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("delete second");
    assert!(domain.get(neighbour).expect("neighbour").soft_deleted);
    let armed: Vec<&str> = board_verb_items(&model).iter().map(|v| v.key).collect();
    assert_eq!(
        armed,
        vec!["u", "+", "?"],
        "no selection: undo · add · help"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("undo");
    assert!(
        !board_verb_items(&model)
            .iter()
            .any(|entry| entry.key == "u" && entry.label == "undo"),
        "undo seat leaves with the delete notice"
    );
}

#[test]
fn u_undoes_with_domain_coverage_and_stale_undo_refused_visibly() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "undo me",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("arm delete");
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("delete");
    assert!(domain.get(id).expect("task").soft_deleted);
    assert!(!model.visible_ids().contains(&id));

    // undo restores
    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("undo");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert!(!domain.get(id).expect("task").soft_deleted);
    assert!(model.visible_ids().contains(&id));
    // selection reanchored to the restored task
    assert_eq!(model.selected_id(), Some(id));

    // stale undo: delete again (records entry with current rev), then direct-restore (advances rev
    // without popping the entry). Next undo must refuse visibly.
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("arm delete2");
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("delete2");
    assert!(domain.get(id).expect("task").soft_deleted);
    let _ = domain.restore(id);
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("stale undo");
    assert_eq!(outcome, IntentOutcome::None);
    // refusal visible on status line (with notice also armed); domain state unchanged.
    // Read the painted status row, not only model.message().
    let row = board_chrome_row(&model, (80, 24));
    assert!(
        row.contains("changed since") || row.contains("since the undoable"),
        "stale undo refusal must be visible on status row: {row:?}"
    );
    // Also ensure the refusal reached the model message channel.
    let msg = model
        .message()
        .expect("stale refusal visible on status line");
    let m = msg.to_ascii_lowercase();
    assert!(
        m.contains("changed") || m.contains("since") || m.contains("stale"),
        "stale undo refusal must be visible: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Task page: Enter opens a view-first full page; `e`/`n`/Tab enter field edits;
// verbs act on the page's task; Shift+Enter saves; Esc layers back.
// ---------------------------------------------------------------------------

fn board_with_noted_task() -> (DomainState, BoardModel, uuid::Uuid) {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "paged task",
            Some("line one\nline two".into()),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    (domain, model, id)
}

#[test]
fn esc_closes_the_task_page_while_ctrl_q_quits_from_it() {
    let (mut domain, mut model, _id) = board_with_noted_task();
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    let close = map_key(BoardInputMode::TaskPage, press(KeyCode::Esc)).expect("Esc");
    assert_eq!(close, BoardIntent::CloseLayer);
    assert_eq!(
        apply_intent(&mut domain, &mut model, close, None).expect("close page"),
        IntentOutcome::None
    );
    assert_eq!(model.input_mode(), BoardInputMode::Normal);

    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("reopen page");
    let quit = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('q'))).expect("Ctrl+Q");
    assert_eq!(quit, BoardIntent::Quit);
    assert_eq!(
        apply_intent(&mut domain, &mut model, quit, None).expect("quit page"),
        IntentOutcome::Quit
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

#[test]
fn task_page_navigation_ignores_unrelated_modifier_chords() {
    for (code, mods) in [
        (KeyCode::Esc, KeyModifiers::CONTROL),
        (KeyCode::Esc, KeyModifiers::ALT),
        (KeyCode::Char('j'), KeyModifiers::ALT),
        (KeyCode::Down, KeyModifiers::CONTROL),
        (KeyCode::Char('k'), KeyModifiers::CONTROL),
        (KeyCode::Tab, KeyModifiers::ALT),
    ] {
        assert_eq!(
            map_key(BoardInputMode::TaskPage, KeyEvent::new(code, mods)),
            None,
            "{code:?} + {mods:?} must not navigate or close the page"
        );
    }
    assert_eq!(
        map_key(
            BoardInputMode::TaskPage,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        ),
        None
    );
    assert_eq!(
        map_key(
            BoardInputMode::TaskPage,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        ),
        Some(BoardIntent::PageScrollDown)
    );
}

#[test]
fn page_verbs_act_on_the_page_task_and_the_page_stays_open() {
    let (mut domain, mut model, id) = board_with_noted_task();
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    // `d` completes the page's task; the page remains its surface.
    let complete = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('d'))).expect("d");
    assert_eq!(complete, BoardIntent::Complete);
    apply_intent(&mut domain, &mut model, complete, None).expect("complete");
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Done);
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    // `o` reopens it, still from the page.
    let reopen = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('o'))).expect("o");
    apply_intent(&mut domain, &mut model, reopen, None).expect("reopen");
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Open);
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    // `b` blocks, `b` again unblocks.
    let block = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('b'))).expect("b");
    apply_intent(&mut domain, &mut model, block.clone(), None).expect("block");
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Blocked);
    apply_intent(&mut domain, &mut model, block, None).expect("unblock");
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Ready);
}

#[test]
fn deleting_from_the_page_closes_it_and_undo_restores() {
    let (mut domain, mut model, id) = board_with_noted_task();
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    let delete = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('x'))).expect("x");
    assert_eq!(delete, BoardIntent::SoftDelete);
    apply_intent(&mut domain, &mut model, delete.clone(), None).expect("arm delete");
    apply_intent(&mut domain, &mut model, delete, None).expect("delete");
    assert!(domain.get(id).expect("task").soft_deleted);
    assert_eq!(
        model.input_mode(),
        BoardInputMode::Normal,
        "the page closes"
    );
    // The undo route back works from the board.
    apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("undo");
    assert!(!domain.get(id).expect("task").soft_deleted);
}

#[test]
fn tab_in_task_view_stays_on_the_step_target_not_task_fields() {
    let (mut domain, mut model, _id) = board_with_noted_task();
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    let tab = map_key(BoardInputMode::TaskPage, press(KeyCode::Tab)).expect("tab");
    apply_intent(&mut domain, &mut model, tab, None).expect("select add target");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(!model.task_editing());
}

#[test]
fn field_entry_intents_refocus_an_open_page_without_recreating_its_drafts() {
    let (mut domain, mut model, _id) = board_with_noted_task();
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("edit title");
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None).expect("type");
    // Moving into Notes must keep the title draft alive (no form recreation).
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None).expect("edit notes");
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    assert_eq!(model.edit_buffer(), "line one\nline two");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("back to title");
    assert_eq!(model.edit_buffer(), "paged task!");
}

#[test]
fn shift_enter_from_the_page_saves_and_keeps_the_task_page_open() {
    let (mut domain, mut model, id) = board_with_noted_task();
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("edit title");
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None).expect("type");
    let save = map_key(BoardInputMode::EditTitle, shift(KeyCode::Enter)).expect("shift+enter");
    assert_eq!(save, BoardIntent::ConfirmEdit);
    let outcome = apply_intent(&mut domain, &mut model, save, None).expect("save");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert_eq!(domain.get(id).expect("task").title, "paged task!");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

#[test]
fn page_scroll_intents_are_session_only_and_bounded() {
    let (mut domain, mut model, id) = board_with_noted_task();
    let rev_before = domain.get(id).expect("task").revision;
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    for intent in [BoardIntent::PageScrollUp, BoardIntent::PageScrollDown] {
        let outcome = apply_intent(&mut domain, &mut model, intent.clone(), None).expect("scroll");
        assert_eq!(outcome, IntentOutcome::None);
        assert!(!board_intent_may_persist(&model, &intent));
    }
    // Far more downs than the notes have lines must stay bounded and mutation-free.
    for _ in 0..50 {
        apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
            .expect("scroll down");
    }
    assert_eq!(domain.get(id).expect("task").revision, rev_before);
}

/// The bottom of a WRAPPING note must be reachable by scrolling.
///
/// The sibling test above uses `"line one\nline two"`, two short lines that never wrap at any
/// real page width, so its "stays bounded" assertion passed trivially and could not observe the
/// bound being wrong. The scroll horizon was counted in logical lines while the page rendered
/// wrapped rows, and wrapping only ever adds rows, so the tail of a long note was unreachable
/// while the divider still advertised the hidden rows.
#[test]
fn page_scroll_reaches_the_bottom_of_a_wrapping_note() {
    let mut domain = DomainState::new();
    // 10 logical lines that each wrap ~4x: ~40 wrapped rows against a ~15-row body. The gap
    // between the two counts is the bug's whole surface, so it must exceed the viewport or a
    // buggy bound still happens to reach the end.
    let long = (0..10)
        .map(|i| format!("{i}{}", "w".repeat(300)))
        .collect::<Vec<_>>()
        .join("\n");
    domain
        .create(
            "wrapping note",
            Some(long),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");

    // One painted frame establishes the wrap geometry the scroll bound is derived from.
    let paint = |model: &BoardModel| {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let _ = draw_board(frame, model);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..24)
            .map(|y| {
                (0..80)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<String>>()
    };
    paint(&model);

    // Scroll far past any plausible row count; the bound must clamp, not strand content.
    for _ in 0..400 {
        apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
            .expect("scroll down");
    }
    let rows = paint(&model);

    // At the bottom the divider must not still be promising rows below.
    let more = rows.iter().find(|row| row.contains("more"));
    assert!(
        more.is_none(),
        "scrolled to the bottom but the divider still advertises hidden rows: {more:?}"
    );
}

#[test]
fn opening_the_page_without_a_selection_is_a_no_op() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("no-op open");
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
}

/// Closing the page after completing its task must not leave selection on an invisible row.
///
/// While the page owns input its verbs act on the bound task, so selection is pinned to that
/// id even after completing it removes it from the open deck. Closing the page ends that
/// contract: if the pin survives the close, the board has no highlighted visible row while
/// `selected_id` still names the hidden done task, so the next verb mutates a task the user
/// cannot see.
#[test]
fn closing_the_page_after_completing_its_task_reanchors_to_a_visible_row() {
    let mut domain = DomainState::new();
    let target = domain
        .create(
            "page target",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create target");
    let other = domain
        .create(
            "still open",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create other");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let idx = model
        .visible_ids()
        .iter()
        .position(|id| *id == target)
        .expect("target visible");
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None)
        .expect("select target");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    apply_intent(&mut domain, &mut model, BoardIntent::Complete, None)
        .expect("complete from the page");
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close page");

    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    let visible = model.visible_ids();
    assert!(
        !visible.contains(&target),
        "the completed task should have left the open deck"
    );
    let selected = model.selected_id().expect("a visible row stays selected");
    assert!(
        visible.contains(&selected),
        "selection stayed on the hidden completed task after the page closed"
    );
    assert_eq!(
        selected, other,
        "selection should reanchor to the open task"
    );
}

#[test]
fn task_edit_save_uses_shift_enter_only_and_title_enter_advances_to_notes() {
    assert_eq!(
        map_key(BoardInputMode::EditTitle, ctrl(KeyCode::Enter)),
        None,
        "Ctrl+Enter must not save a task field"
    );
    assert_eq!(
        map_key(BoardInputMode::EditTitle, press(KeyCode::Enter)),
        None,
        "plain Enter must not be a hidden task-field save chord"
    );
    // The real task form (form navigation) moves Title → Notes on Enter.
    assert_eq!(
        map_task_form_key(
            tsk_tui::ui::capture::CaptureField::Title,
            false,
            press(KeyCode::Enter)
        ),
        Some(BoardIntent::FormFocusNext),
        "Enter in the Title editor advances to Notes"
    );
    assert_eq!(
        map_task_form_key(
            tsk_tui::ui::capture::CaptureField::Title,
            false,
            shift(KeyCode::Enter)
        ),
        Some(BoardIntent::ConfirmEdit),
        "Shift+Enter still saves from Title"
    );
    assert_eq!(
        map_task_form_key(
            tsk_tui::ui::capture::CaptureField::Title,
            false,
            alt(KeyCode::Enter)
        ),
        None,
        "Alt+Enter is no longer a task-edit save route"
    );
    assert_eq!(
        map_key(BoardInputMode::EditStep, press(KeyCode::Enter)),
        Some(BoardIntent::ConfirmEdit),
        "plain Enter saves a new step and opens the next empty row"
    );
    assert_eq!(
        map_key(BoardInputMode::EditStep, alt(KeyCode::Enter)),
        None,
        "Alt+Enter is not a step save route either"
    );
    assert_eq!(
        map_key(BoardInputMode::EditStep, shift(KeyCode::Enter)),
        Some(BoardIntent::ConfirmEditNext),
        "Shift+Enter is the step editor's session save"
    );
    assert_eq!(
        map_key(BoardInputMode::EditNotes, shift(KeyCode::Enter)),
        Some(BoardIntent::ConfirmEdit),
        "Shift+Enter saves task notes without inserting a line break"
    );
    assert_eq!(
        map_key(BoardInputMode::EditNotes, alt(KeyCode::Enter)),
        None,
        "Alt+Enter no longer saves task notes"
    );
    assert_eq!(
        map_capture_key(
            tsk_tui::ui::capture::CaptureField::Notes,
            alt(KeyCode::Enter)
        ),
        Some(CaptureIntent::Save),
        "Alt+Enter remains the legacy capture save fallback"
    );
    assert_eq!(
        map_key(BoardInputMode::EditTitle, ctrl_alt(KeyCode::Enter)),
        None,
        "Ctrl+Alt+Enter must not be a hidden task-form save chord"
    );
    assert_eq!(
        map_key(BoardInputMode::EditNotes, ctrl_alt(KeyCode::Enter)),
        None,
        "Ctrl+Alt+Enter must not be a hidden Notes save chord"
    );
    assert_eq!(
        map_capture_key(
            tsk_tui::ui::capture::CaptureField::Notes,
            ctrl_alt(KeyCode::Enter)
        ),
        None,
        "Ctrl+Alt+Enter must not be a hidden capture save chord"
    );

    let (mut domain, mut model, _) = board_with_task("Shift save label", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open task title");
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.contains("shift+enter save"),
        "the visible task-title legend names the save chord:\n{frame}"
    );
}

#[test]
fn ctrl_s_replaces_ctrl_space_as_the_primary_verb() {
    for mode in [BoardInputMode::Normal, BoardInputMode::TaskPage] {
        assert_eq!(
            map_key(mode, ctrl(KeyCode::Char('s'))),
            Some(BoardIntent::PrimaryVerb),
            "Ctrl+S must own the primary verb in {mode:?}"
        );
        assert_eq!(
            map_key(mode, ctrl(KeyCode::Char(' '))),
            None,
            "Ctrl+Space must be retired in {mode:?}"
        );
    }
    assert_eq!(
        map_key(BoardInputMode::TaskPage, press(KeyCode::Null)),
        None,
        "the legacy NUL encoding of Ctrl+Space must also be retired"
    );

    for from in [HumanStatus::Open, HumanStatus::Ready] {
        let (mut domain, mut model, id) = board_with_task("startable", from);
        let outcome =
            apply_intent(&mut domain, &mut model, BoardIntent::PrimaryVerb, None).expect("start");
        assert_eq!(outcome, IntentOutcome::Persist);
        assert_eq!(domain.get(id).expect("task").status, HumanStatus::Started);
    }
}

// ---------------------------------------------------------------------------
// T-3: page step cursor and modifier-protected steps verbs. Bare arrows own
// the cursor lifecycle (first Down activates, Up from the first step
// deactivates); Ctrl+space toggles the highlighted step, Ctrl+x marks then
// removes, Ctrl+e renames the highlighted step or the title by cursor state,
// Ctrl+a opens the one-line add editor.
// ---------------------------------------------------------------------------

/// A task page opened on a task carrying `steps`, painted at the standard
/// 80x24 board size. Returns the opened page.
fn board_with_steps(
    title: &str,
    notes: Option<&str>,
    steps: &[&str],
) -> (DomainState, BoardModel, uuid::Uuid) {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            title,
            notes.map(str::to_string),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    for step in steps {
        domain.add_step(id, step).expect("add step");
    }
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open task page");
    // The page paints before a user can press Tab, recording the shared-content viewport used
    // to bring the first selected step into view.
    let _ = rendered_board(&model, 80, 24);
    if !steps.is_empty() {
        // Tab first selects in view mode. Open and cancel Notes to establish the active task
        // edit session used by these step-editor fixtures while preserving that selection.
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
            .expect("tab into first step");
        apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None)
            .expect("start task edit session");
        apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
            .expect("return to task page");
        // Notes entry resets the shared stream origin. Restore the first selected step's
        // anchor when a second row exists, without using Tab because Tab now cycles task
        // fields during an active edit session.
        if steps.len() > 1 {
            apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
                .expect("advance selected step");
            apply_intent(&mut domain, &mut model, BoardIntent::PageScrollUp, None)
                .expect("return to first selected step");
        }
    }
    (domain, model, id)
}

#[test]
fn view_mode_add_starts_the_normal_inline_step_flow() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "View-only steps",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain.add_step(id, "first step").expect("step");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("select stored step");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("select add target");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("view add");
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditStep,
        "view-mode add opens the independent empty inline row"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
        .expect("close empty add row");

    let frame = rendered_board(&model, 80, 24);
    let step_y = frame
        .lines()
        .position(|line| line.contains("first step"))
        .expect("painted step") as u16;
    let hits = board_hit_map(Rect::new(0, 0, 80, 24), &model);
    let view_click = tsk_tui::ui::mouse::map_board_mouse(
        &model,
        &hits,
        tsk_tui::ui::mouse::left_click(5, step_y),
    );
    assert_eq!(
        view_click,
        Some(BoardIntent::SelectStep(0)),
        "a view-mode step click selects the row without opening its editor"
    );
    apply_intent(
        &mut domain,
        &mut model,
        view_click.expect("view click"),
        None,
    )
    .expect("select view step");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open selected step in task edit");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(model.task_editing());
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
        .expect("close step editor");
    let hits = board_hit_map(Rect::new(0, 0, 80, 24), &model);
    assert_eq!(
        tsk_tui::ui::mouse::map_board_mouse(
            &model,
            &hits,
            tsk_tui::ui::mouse::left_click(5, step_y)
        ),
        Some(BoardIntent::SelectStep(0)),
        "the same row becomes selectable once editing has started"
    );
}

#[test]
fn view_tab_selection_wraps_without_starting_task_edit_and_ctrl_e_opens_inline_step_edit() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "View tab selection",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain.add_step(id, "first step").expect("first");
    domain.add_step(id, "second step").expect("second");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
    let _ = rendered_board(&model, 80, 24);

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab selects first");
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ first step"));
    assert!(!model.task_editing(), "Tab must not start task editing");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab selects second");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab selects add target");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab wraps to first");
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ first step"));
    assert!(!model.task_editing(), "wrapping remains task view mode");

    let primary =
        map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('s'))).expect("Ctrl+S primary verb");
    apply_intent(&mut domain, &mut model, primary, None).expect("start the task");
    assert!(
        !domain.get(id).expect("task").steps[0].done,
        "ctrl+s never touches the selected step"
    );
    assert_eq!(
        domain.get(id).expect("task").status,
        HumanStatus::Started,
        "ctrl+s is the task's primary verb even with a step selected"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::TaskPage,
        "the task verb keeps task view open"
    );

    // Enter on the stored step toggles it (resolved to ToggleStep at the key boundary);
    // the raw OpenTaskPage intent stays inert on a step.
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleStep, None).expect("Enter toggles");
    assert!(domain.get(id).expect("task").steps[0].done);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None)
        .expect("OpenTaskPage on a step selection is inert");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(!model.task_editing());

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("Ctrl+E starts editing the selected step");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(
        model.task_editing(),
        "Ctrl+E starts the whole task edit session"
    );
    assert!(
        rendered_board(&model, 80, 24).contains("▸ ✓ first step"),
        "the selected completed step is the inline editor"
    );
    let tab = map_key(BoardInputMode::EditStep, press(KeyCode::Tab))
        .expect("Tab leaves inline editing for the next selected step");
    apply_intent(&mut domain, &mut model, tab, None).expect("select second step");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ second step"));
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab reaches the trailing add target after the final step");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab leaves the add target for Assignee");
    assert_eq!(model.input_mode(), BoardInputMode::EditAssignee);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab leaves Assignee for Thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab leaves Thread for Scope");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
}

#[test]
fn plain_enter_parks_an_existing_step_rename_without_saving_the_task_session() {
    let (mut domain, mut model, id) = board_with_steps("Park rename", None, &["first"]);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open selected step");
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None)
        .expect("edit step draft");

    let enter = map_key(BoardInputMode::EditStep, press(KeyCode::Enter)).expect("plain enter");
    assert_eq!(
        apply_intent(&mut domain, &mut model, enter, None).expect("park rename"),
        IntentOutcome::None
    );
    assert_eq!(domain.get(id).expect("task").steps[0].text, "first");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(model.task_editing());
    assert!(rendered_board(&model, 80, 24).contains("first!"));
}

#[test]
fn task_edit_tab_cycles_every_step_before_assignee_thread_and_scope() {
    let (mut domain, mut model, _) = board_with_steps("Tab fields", None, &["first", "second"]);
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ first"));

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab reaches the second selected step");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ second"));
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab reaches the add target after the final step");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab leaves the add target for Assignee");
    assert_eq!(model.input_mode(), BoardInputMode::EditAssignee);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab leaves Assignee for Thread");
    assert_eq!(model.input_mode(), BoardInputMode::SelectThread);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab reaches Scope");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab wraps Scope to Title");
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab advances Title to Notes");
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab returns to the first selected step after Notes");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ first"));

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab reaches the second step again");
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None)
        .expect("edit the second step");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None)
        .expect("Shift+Tab returns to the first step");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    let first_again = rendered_board(&model, 80, 24);
    assert!(first_again.contains("▸ ▪ first"));
    assert!(
        first_again.contains("second!"),
        "Shift+Tab must park the second step draft: {first_again}"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusPrev, None)
        .expect("Shift+Tab leaves the first step for Notes");
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
}

#[test]
fn long_step_text_wraps_and_cursor_navigation_uses_its_painted_rows() {
    let long = format!("{} step-tail", "wrap ".repeat(48));
    let (mut domain, mut model, _) = board_with_steps("Wrapped steps", None, &[&long, "next step"]);
    let first = rendered_board(&model, 80, 24);
    assert!(
        first.contains("step-tail"),
        "the tail of a wrapped step is painted rather than truncated:\n{first}"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("move to second step");
    let second = rendered_board(&model, 80, 24);
    assert!(
        second.contains("▸ ▪ next step"),
        "cursor navigation reaches the next step after the wrapped rows:\n{second}"
    );
}

/// Ten note lines that each wrap ~5x at the 80-column page width (~50 wrapped
/// rows against a ~14-row notes window), each carrying a unique `L{n}` marker on
/// its first wrapped row so scroll position is paint-observable.
fn wrapping_notes() -> String {
    (0..10)
        .map(|i| format!("L{i} {}", "w".repeat(300)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keyboard arrows walk the active cursor through the shared stream, keeping the
/// selected step visible while the cursor advances.
#[test]
fn arrow_keys_move_the_cursor_through_shared_steps() {
    let steps: Vec<String> = (1..=30).map(|i| format!("step {i:02}")).collect();
    let step_refs: Vec<&str> = steps.iter().map(String::as_str).collect();
    let (mut domain, mut model, _) =
        board_with_steps("Window walker", Some("the notes body"), &step_refs);

    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.contains('▌'),
        "overflow must show the page scrollbar:\n{frame}"
    );
    assert!(
        !frame.contains("+23 more"),
        "there is no separate steps window:\n{frame}"
    );
    assert!(frame.contains("step 01"), "first step visible:\n{frame}");

    for _ in 0..30 {
        apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
            .expect("scroll down");
    }
    let bottom = rendered_board(&model, 80, 24);
    assert!(
        bottom.contains("▸ ▪ step 30"),
        "the cursor reaches the final step:\n{bottom}"
    );
    assert!(
        !bottom.contains("step 01"),
        "the first step leaves the viewport:\n{bottom}"
    );
}

/// Ctrl+D completes exactly the highlighted step through the real apply/save path, without
/// changing the task's human status.
#[test]
fn ctrl_d_with_a_step_selected_completes_the_task_and_enter_toggles_the_step() {
    let (mut domain, mut model, id) = board_with_steps(
        "Toggle witness",
        None,
        &["alpha step", "bravo step", "charlie step"],
    );
    let revision_before = domain.get(id).expect("task").revision;

    // The active task-edit fixture starts on the first step, then Down moves to "bravo step".
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None).expect("cursor down");
    assert!(
        model.stored_step_selected(),
        "bravo is a stored step under the cursor"
    );

    // Enter owns the step: it flips bravo and rides the persist path.
    let outcome =
        apply_intent(&mut domain, &mut model, BoardIntent::ToggleStep, None).expect("toggle step");
    assert_eq!(
        outcome,
        IntentOutcome::Persist,
        "the step toggle must ride the real persist path"
    );
    let task = domain.get(id).expect("task");
    let dones: Vec<bool> = task.steps.iter().map(|step| step.done).collect();
    assert_eq!(
        dones,
        vec![false, true, false],
        "exactly the highlighted step toggles"
    );
    assert_eq!(
        task.status,
        HumanStatus::Open,
        "toggling a step never changes human status"
    );
    assert_ne!(
        task.revision, revision_before,
        "the toggle must bump the revision"
    );
    assert_eq!(
        task.history.last().expect("event").kind,
        TaskEventKind::StepChecked,
        "the toggle must journal its typed event"
    );
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.contains("steps 1/3"),
        "done/total counts must follow the toggle:\n{frame}"
    );
    assert!(
        frame.contains("▸ ✓ bravo step"),
        "the toggled step stays under the cursor:\n{frame}"
    );
    // Enter again flips it back.
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleStep, None).expect("toggle back");
    assert!(!domain.get(id).expect("task").steps[1].done);

    // Ctrl+D with the step still selected completes the TASK, and leaves the steps alone.
    let complete = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('d'))).expect("Ctrl+D");
    assert_eq!(complete, BoardIntent::Complete);
    apply_intent(&mut domain, &mut model, complete, None).expect("complete task");
    let task = domain.get(id).expect("task");
    assert_eq!(task.status, HumanStatus::Done, "ctrl+d is the task verb");
    let dones: Vec<bool> = task.steps.iter().map(|step| step.done).collect();
    assert_eq!(dones, vec![false, false, false], "no step flipped");
}

/// `ctrl+r` walks review ↔ ready and rides the persist path; a done task refuses.
#[test]
fn ctrl_r_toggles_review_and_ready_and_refuses_on_done() {
    let (mut domain, mut model, id) = board_with_task("Review me", HumanStatus::Ready);
    let review = map_key(BoardInputMode::Normal, ctrl(KeyCode::Char('r'))).expect("ctrl+r");
    assert_eq!(review, BoardIntent::ToggleReview);
    assert!(
        board_intent_may_persist(&model, &review),
        "review is a durable status change"
    );

    let outcome = apply_intent(&mut domain, &mut model, review.clone(), None).expect("to review");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Review);

    // Review lives in NEEDS YOU, so the row stays selected and the bar keeps done/block.
    let verbs: Vec<&str> = board_verb_items(&model).iter().map(|v| v.key).collect();
    assert_eq!(verbs, vec!["enter", "d", "b", "o", "+", "?"]);

    apply_intent(&mut domain, &mut model, review.clone(), None).expect("back to ready");
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Ready);

    for from in [HumanStatus::Started, HumanStatus::Blocked] {
        domain.set_status(id, from).expect("seed status");
        model.sync_from_domain(&domain);
        apply_intent(&mut domain, &mut model, review.clone(), None).expect("to review");
        assert_eq!(
            domain.get(id).expect("task").status,
            HumanStatus::Review,
            "{from:?} → review"
        );
    }

    domain.set_status(id, HumanStatus::Done).expect("done");
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    let idx = model
        .visible_ids()
        .iter()
        .position(|&row| row == id)
        .expect("done row visible");
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None).expect("select");
    let outcome = apply_intent(&mut domain, &mut model, review.clone(), None).expect("refused");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Done);
    assert_eq!(model.message(), Some("completed tasks cannot go to review"));

    // The task page maps the same chord.
    assert_eq!(
        map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('r'))),
        Some(BoardIntent::ToggleReview)
    );

    // No selection: the shared refusal.
    let mut empty = BoardModel::from_domain(&DomainState::new(), None);
    let mut empty_domain = DomainState::new();
    apply_intent(&mut empty_domain, &mut empty, review, None).expect("no selection");
    assert_eq!(empty.message(), Some("select a task first"));
}

/// Enter keeps a selected view step read-only. Ctrl+e remains contextual, opening its inline
/// editor while an inactive cursor still routes it to task-title editing.
#[test]
fn enter_stays_in_view_and_edit_verb_targets_step_or_title_by_cursor() {
    let (mut domain, mut model, id) =
        board_with_steps("Rename target", None, &["alpha step", "bravo step"]);
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("leave task edit");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab selects first step");

    // Tab selected the first step, but bare Enter leaves task view untouched.
    let enter = map_key(BoardInputMode::TaskPage, press(KeyCode::Enter)).expect("enter");
    assert_eq!(enter, BoardIntent::OpenTaskPage);
    apply_intent(&mut domain, &mut model, enter, None).expect("keep task page open");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    let edit_selected =
        map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('e'))).expect("ctrl+e");
    apply_intent(&mut domain, &mut model, edit_selected, None).expect("open inline step editor");
    assert_ne!(
        model.input_mode(),
        BoardInputMode::EditTitle,
        "with the cursor active the edit verb must not edit the title"
    );
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.lines().any(|row| row.contains("▸ ▪ alpha step")),
        "the selected step itself must paint the inline editor:\n{frame}"
    );
    assert!(
        !frame.lines().any(|row| row.contains("▎")),
        "the editor must not fall back to the footer:\n{frame}"
    );

    // Edit the draft, then Shift+Enter applies the rename to the step, not the title.
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None)
        .expect("type into step editor");
    let save = map_key(model.input_mode(), shift(KeyCode::Enter)).expect("shift+enter");
    assert_eq!(save, BoardIntent::ConfirmEditNext);
    let outcome = apply_intent(&mut domain, &mut model, save, None).expect("rename step");
    assert_eq!(outcome, IntentOutcome::Persist);
    // The app boundary persisted; its confirmed sync closes the held line (T-4).
    model.sync_from_domain(&domain);

    let task = domain.get(id).expect("task");
    assert_eq!(
        task.steps[0].text, "alpha step!",
        "Shift+Enter must rename the highlighted step"
    );
    assert_eq!(task.title, "Rename target", "the title is untouched");
    assert_eq!(
        task.history.last().expect("event").kind,
        TaskEventKind::StepRenamed
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    // Cursor inactive (fresh page session): Ctrl+e is the existing title edit.
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close page");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("reopen page");
    let rename = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('e'))).expect("ctrl+e");
    apply_intent(&mut domain, &mut model, rename, None).expect("ctrl+e inactive cursor");
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditTitle,
        "with the cursor inactive Ctrl+e edits the title, unchanged"
    );
    assert_eq!(model.edit_buffer(), "Rename target");
}

/// The trailing add target is part of the view-mode Tab ring. Activating it opens an
/// independent editor, so a plain Enter persists the one step and returns to the target
/// without starting or saving the enclosing task form.
#[test]
fn view_step_target_tabs_to_an_independent_plain_enter_editor() {
    let (mut domain, mut model, id) = board_with_steps("Add target", None, &["first"]);
    // This fixture establishes task editing for older step coverage. Reopen the page to prove
    // the target itself works from the required view-first state.
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("leave task edit");
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.contains("   + step"),
        "missing trailing target:\n{frame}"
    );
    assert!(
        !frame.contains("\n\n   + step"),
        "the target follows the final stored row without a spacer:\n{frame}"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("first step");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("add target");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open add");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(
        !model.task_editing(),
        "view add must not start task editing"
    );
    for character in "second".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type");
    }
    let enter = map_key(BoardInputMode::EditStep, press(KeyCode::Enter)).expect("plain Enter");
    assert_eq!(enter, BoardIntent::ConfirmEdit);
    assert_eq!(
        apply_intent(&mut domain, &mut model, enter, None).expect("save step"),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(
        !model.task_editing(),
        "saving an add must not save a task session"
    );
    assert_eq!(
        domain
            .get(id)
            .expect("task")
            .steps
            .iter()
            .map(|step| step.text.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

/// Task-edit deletion stays in the form until Shift+Enter. Esc abandons the staged removal.
#[test]
fn task_edit_delete_stages_until_save_and_esc_restores() {
    let (mut domain, mut model, id) = board_with_steps("Staged delete", None, &["first", "second"]);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open step editor");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    let delete = map_key(BoardInputMode::EditStep, ctrl(KeyCode::Char('x'))).expect("Ctrl+X");
    assert_eq!(
        apply_intent(&mut domain, &mut model, delete, None).expect("stage delete"),
        IntentOutcome::None
    );
    assert_eq!(
        domain.get(id).expect("task").steps.len(),
        2,
        "not durable yet"
    );
    let staged = rendered_board(&model, 80, 24);
    assert!(
        !staged.contains("first"),
        "a staged deletion must disappear from the task-edit list immediately:\n{staged}"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("Esc task edit");
    assert!(
        rendered_board(&model, 80, 24).contains("▪ first"),
        "Esc restores the staged row"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("reopen title");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None).expect("Notes");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("reopen step editor");
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("stage again");
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("save task"),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(
        domain
            .get(id)
            .expect("task")
            .steps
            .iter()
            .map(|step| step.text.as_str())
            .collect::<Vec<_>>(),
        vec!["second"]
    );
}

/// Ctrl+A reaches the independent step editor from task view and every task-edit focus,
/// including Scope, Thread, and an already-open inline rename.
#[test]
fn ctrl_a_opens_step_add_from_task_view_and_every_task_edit_state() {
    let (mut domain, mut model, id) = board_with_steps("Ctrl+A routes", None, &["first", "second"]);
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("leave task edit");

    let add = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('a'))).expect("view Ctrl+A");
    assert_eq!(add, BoardIntent::BeginAddStep);
    apply_intent(&mut domain, &mut model, add, None).expect("open view add");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    for character in "view add".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None).expect("save view add");
    model.sync_from_domain(&domain);
    assert!(
        rendered_board(&model, 80, 24).contains("▪ view add"),
        "plain Enter saves the step and opens the next empty row"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None).expect("title");
    for field in [
        CaptureField::Title,
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
        let add = map_task_form_key(field, false, ctrl(KeyCode::Char('a')))
            .expect("Ctrl+A maps from each task-edit field");
        assert_eq!(add, BoardIntent::BeginAddStep);
        apply_intent(&mut domain, &mut model, add, None).expect("open add");
        assert_eq!(model.input_mode(), BoardInputMode::EditStep);
        apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None).expect("close add");
    }

    apply_intent(&mut domain, &mut model, BoardIntent::SelectStep(0), None).expect("rename first");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    let add = map_key(BoardInputMode::EditStep, ctrl(KeyCode::Char('a'))).expect("inline Ctrl+A");
    assert_eq!(add, BoardIntent::BeginAddStep);
    apply_intent(&mut domain, &mut model, add, None).expect("replace rename with add");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert_eq!(domain.get(id).expect("task").steps.len(), 3);
}

/// Arrow keys leave an inline editor usable: existing rows navigate to adjacent steps, while
/// an add row scrolls its shared content instead of swallowing the key.
#[test]
fn inline_step_arrow_keys_navigate_or_scroll_without_trapping_the_editor() {
    let (mut domain, mut model, _) = board_with_steps(
        "Arrow routes",
        Some(&wrapping_notes()),
        &["first", "second"],
    );
    apply_intent(&mut domain, &mut model, BoardIntent::SelectStep(0), None).expect("edit first");
    let down = map_key(BoardInputMode::EditStep, press(KeyCode::Down)).expect("down maps");
    apply_intent(&mut domain, &mut model, down.clone(), None).expect("down navigates");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ second"));

    let add = map_key(BoardInputMode::EditStep, ctrl(KeyCode::Char('a'))).expect("add maps");
    apply_intent(&mut domain, &mut model, add, None).expect("open add");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("draft".into()),
        None,
    )
    .expect("type add draft");
    let before = rendered_board(&model, 80, 24);
    apply_intent(&mut domain, &mut model, down, None).expect("add down scrolls");
    let after = rendered_board(&model, 80, 24);
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert_ne!(
        before, after,
        "Down must scroll rather than trap the inline add editor"
    );
}

/// A fresh independent add opened from task view must retain its cursor and draft while its
/// arrows scroll the shared page body. The body must overflow, otherwise a locked route is
/// indistinguishable from an already-bottom viewport.
#[test]
fn fresh_step_add_down_scrolls_overflowing_page_without_losing_its_draft() {
    let (mut domain, mut model, _) =
        board_with_steps("Fresh add scroll", Some(&wrapping_notes()), &[]);
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(!model.task_editing(), "the add begins from task view");

    let add = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('a'))).expect("Ctrl+A maps");
    apply_intent(&mut domain, &mut model, add, None).expect("open independent add");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("draft survives scroll".into()),
        None,
    )
    .expect("type draft");
    let before = rendered_board(&model, 80, 24);
    let down = map_key(model.input_mode(), press(KeyCode::Down)).expect("Down maps in add");
    assert_eq!(down, BoardIntent::PageScrollDown);

    apply_intent(&mut domain, &mut model, down.clone(), None).expect("scroll down");
    let after_one = rendered_board(&model, 80, 24);
    assert_ne!(
        before, after_one,
        "Down must move the overflowing page body"
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(
        !model.task_editing(),
        "scrolling must not start a task edit session"
    );

    // Continue until the trailing inline row enters the viewport, proving the same draft and
    // cursor survived while the body moved underneath them.
    for _ in 0..64 {
        apply_intent(&mut domain, &mut model, down.clone(), None).expect("continue scrolling");
    }
    let bottom = rendered_board(&model, 80, 24);
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(
        bottom.contains("draft survives scroll"),
        "the fresh inline draft remains after scrolling:\n{bottom}"
    );
}

/// Ctrl+x is mark-then-confirm: the first press visibly marks the cursor's step,
/// any intervening key clears the mark without removing, and a second Ctrl+x
/// with nothing between removes the step through the domain command.
#[test]
fn delete_verb_marks_then_removes_on_second_press() {
    let (mut domain, mut model, id) = board_with_steps(
        "Delete witness",
        None,
        &["alpha step", "bravo step", "charlie step"],
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
        .expect("leave task edit for view delete");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("select first view step");
    let revision_before = domain.get(id).expect("task").revision;

    let delete = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('x'))).expect("ctrl+x");
    assert_eq!(delete, BoardIntent::SoftDelete);

    // First press: visible mark, no mutation.
    let outcome = apply_intent(&mut domain, &mut model, delete.clone(), None).expect("mark step");
    assert_eq!(
        outcome,
        IntentOutcome::None,
        "the marking press persists nothing"
    );
    let task = domain.get(id).expect("task");
    assert_eq!(task.steps.len(), 3, "nothing removed yet");
    assert!(!task.soft_deleted, "the task itself is not deleted");
    assert_eq!(task.revision, revision_before, "no journaled mutation yet");
    let marked = rendered_board(&model, 80, 24);
    assert!(
        marked.contains("✗ alpha step"),
        "the marked step must paint visibly:\n{marked}"
    );

    // An intervening key (a cursor move) clears the mark without removing.
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("intervening key");
    let cleared = rendered_board(&model, 80, 24);
    assert!(
        !cleared.contains("✗"),
        "the intervening key must clear the mark:\n{cleared}"
    );
    assert_eq!(
        domain.get(id).expect("task").steps.len(),
        3,
        "clearing the mark removes nothing"
    );

    // Mark again, then confirm with a second Ctrl+x and nothing between.
    apply_intent(&mut domain, &mut model, delete.clone(), None).expect("mark again");
    let removed = rendered_board(&model, 80, 24);
    assert!(
        removed.contains("✗ bravo step"),
        "the mark follows the cursor's step:\n{removed}"
    );
    let outcome = apply_intent(&mut domain, &mut model, delete, None).expect("remove step");
    assert_eq!(outcome, IntentOutcome::Persist);
    let task = domain.get(id).expect("task");
    let texts: Vec<&str> = task.steps.iter().map(|step| step.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["alpha step", "charlie step"],
        "the second press removes the marked step"
    );
    assert_eq!(
        task.history.last().expect("event").kind,
        TaskEventKind::StepRemoved
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::TaskPage,
        "the page stays open"
    );
}

/// With overflowing notes and steps, a step selected through Tab owns the next Down.
#[test]
fn a_tab_selected_step_receives_bare_down_before_shared_content() {
    let notes = wrapping_notes();
    let (mut domain, mut model, _) = board_with_steps(
        "Scroll witness",
        Some(&notes),
        &["alpha step", "bravo step"],
    );
    let before = rendered_board(&model, 80, 24);
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("move selected step");
    let after = rendered_board(&model, 80, 24);
    assert!(
        after.contains("▸ ▪ bravo step"),
        "Down must move the Tab-selected step cursor"
    );
    assert!(
        before.contains("▸ ▪ alpha step"),
        "Tab must scroll the first selected step into view"
    );
}

/// Up reverses the shared content scroll while the page chrome remains fixed.
#[test]
fn page_scroll_up_reverses_the_shared_content_region() {
    let notes = wrapping_notes();
    let (mut domain, mut model, _) =
        board_with_steps("Deactivate witness", Some(&notes), &["alpha step"]);
    rendered_board(&model, 80, 24);
    for _ in 0..4 {
        apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
            .expect("scroll down");
    }
    let down = rendered_board(&model, 80, 24);
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollUp, None).expect("scroll up");
    let up = rendered_board(&model, 80, 24);
    assert_ne!(down, up, "Up must move the shared body back");
    assert!(up.contains("Deactivate witness") && up.contains("created"));
}

/// T-10 (AC-17, AC-18, AC-26): keys own the active cursor before they move the
/// stream, while wheel input remains stream-only.
#[test]
fn active_cursor_up_precedes_shared_scroll() {
    let (mut domain, mut model, _) = board_with_steps(
        "Cursor priority",
        Some(&wrapping_notes()),
        &["first step", "second step"],
    );
    rendered_board(&model, 80, 24);
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("Down to second step");
    let selected = rendered_board(&model, 80, 24);
    assert!(
        selected.contains("▸ ▪ second step"),
        "second step selected:\n{selected}"
    );

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::PageWheelScrollDown,
        None,
    )
    .expect("wheel scroll");
    let after_wheel = rendered_board(&model, 80, 24);
    assert!(
        after_wheel.contains("▸ ▪ second step"),
        "wheel scrolling must not move the step cursor:\n{after_wheel}"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollUp, None).expect("key up");
    let after_key = rendered_board(&model, 80, 24);
    assert!(
        after_key.contains("▸ ▪ first step"),
        "Up must move the active cursor before scrolling the stream:\n{after_key}"
    );
}

/// Up from the active first step deactivates without moving the shared body. A subsequent
/// inactive Up scrolls, and Tab selects a step again from the task edit session.
#[test]
fn first_step_up_deactivates_before_inactive_up_scrolls_then_down_reactivates() {
    let (mut domain, mut model, _) = board_with_steps(
        "Cursor lifecycle",
        Some(&wrapping_notes()),
        &["first step", "second step"],
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("leave task edit");
    let _ = rendered_board(&model, 80, 24);
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("select first step");
    rendered_board(&model, 80, 24);
    let active = rendered_board(&model, 80, 24);
    assert!(
        active.contains("▸ ▪ first step"),
        "fixture must have an active cursor on the first live step:\n{active}"
    );
    let active_verbs = board_verb_items(&model);
    assert!(
        active_verbs
            .iter()
            .any(|entry| entry.key == "s" && entry.label == "start"),
        "the bar keeps the task verb with a step selected (Enter owns the step): {active_verbs:?}"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollUp, None)
        .expect("deactivate first step");
    let deactivated = rendered_board(&model, 80, 24);
    assert!(
        !deactivated.contains("▸"),
        "Up at step zero must clear the active cursor:\n{deactivated}"
    );
    // Page chrome changes from the active step verb to the task verb. Compare only
    // the shared body, normalizing its sole cursor glyph, so this fails if Up scrolls.
    let shared_body = |frame: &str| {
        frame
            .lines()
            .skip(3)
            .take(17)
            .collect::<Vec<_>>()
            .join("\n")
            .replace("▸ ", "  ")
    };
    let deactivated_body = shared_body(&deactivated);
    assert_eq!(
        shared_body(&active),
        deactivated_body,
        "the deactivating Up must not also scroll shared content"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollUp, None)
        .expect("inactive up scroll");
    let scrolled = rendered_board(&model, 80, 24);
    assert!(
        !scrolled.contains("▸"),
        "inactive Up must leave the cursor inactive:\n{scrolled}"
    );
    assert_ne!(
        deactivated_body,
        shared_body(&scrolled),
        "inactive Up must scroll the shared content"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditScope, None)
        .expect("enter Scope on the task edit traversal");
    for _ in 0..3 {
        apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
            .expect("Tab completes Scope → Title → Notes → first step");
    }
    let reactivated = rendered_board(&model, 80, 24);
    assert!(
        reactivated.contains("▸ ▪ first step"),
        "Tab from Scope must re-select the first cursor:\n{reactivated}"
    );
}

/// A background refresh can leave the page cursor past an externally shortened step list.
/// The footer must then describe the same task-status verb `PrimaryVerb` will apply.
#[test]
fn stale_step_cursor_falls_back_to_the_live_task_status_verb() {
    let (mut domain, mut model, id) =
        board_with_steps("Stale cursor", None, &["first step", "second step"]);
    rendered_board(&model, 80, 24);
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("activate first step");
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("select second step");
    let steps: Vec<_> = domain
        .get(id)
        .expect("task")
        .steps
        .iter()
        .map(|step| step.id)
        .collect();
    for step in steps {
        domain.remove_step(id, step).expect("external removal");
    }
    model.sync_from_domain(&domain);

    let verbs = board_verb_items(&model);
    assert!(
        !verbs
            .iter()
            .any(|entry| entry.label.contains("step") && entry.key == "s"),
        "a stale cursor must not advertise toggle step: {verbs:?}"
    );
    assert!(
        !model.stored_step_selected(),
        "a cursor past the live step list is not a stored-step selection"
    );
    assert!(
        !verbs
            .iter()
            .any(|entry| entry.key == "s" && entry.label == "toggle step"),
        "a stale cursor must not advertise a dead step: {verbs:?}"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::PrimaryVerb, None).expect("primary verb");
    assert_eq!(domain.get(id).expect("task").status, HumanStatus::Started);
}

/// Down activates the first step rather than behaving as plain page navigation, even when notes overflow.
#[test]
fn down_activates_the_cursor_when_content_overflows() {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Reactivation witness",
            Some(wrapping_notes()),
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("task");
    domain.add_step(id, "alpha step").expect("first");
    domain.add_step(id, "bravo step").expect("second");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open");
    let initial = rendered_board(&model, 80, 24);
    assert!(
        !initial.contains("▸ ▪"),
        "cursor starts inactive: {initial}"
    );
    assert!(initial.contains("L0"), "notes start at the top: {initial}");

    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("activate first step");
    let activated = rendered_board(&model, 80, 24);
    assert!(activated.contains("▸ ▪ alpha step"));
}

/// AC-19: a task with no steps steps never activates a cursor; bare
/// arrows scroll the page exactly as before this feature.
#[test]
fn no_step_task_arrows_scroll_notes_unchanged() {
    let notes = wrapping_notes();
    let (mut domain, mut model, _id) = board_with_steps("Notes-only task", Some(&notes), &[]);

    let start = rendered_board(&model, 80, 24);
    assert!(start.contains("L0"), "notes painted from the top:\n{start}");
    assert!(
        !start.contains("steps") && !start.contains("▸"),
        "no section, no cursor, on an empty steps:\n{start}"
    );

    // Exactly pre-feature behavior: every arrow press scrolls the notes by one row.
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("scroll down one");
    let down = rendered_board(&model, 80, 24);
    assert!(
        !down.contains("L0") && down.contains("L1"),
        "a bare Down must scroll the notes exactly one row:\n{down}"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollUp, None).expect("scroll up one");
    let up = rendered_board(&model, 80, 24);
    assert!(
        up.contains("L0"),
        "a bare Up must scroll the notes back:\n{up}"
    );
    assert!(
        !up.contains("▸"),
        "an empty steps collection never activates a cursor:\n{up}"
    );
}

/// Ctrl+a opens the section's one-line editor empty; Shift+Enter applies the add and
/// opens the next row, Esc cancels and closes.
#[test]
fn add_verb_opens_editor_shift_enter_applies_esc_cancels() {
    let (mut domain, mut model, id) = board_with_steps("Add witness", None, &["alpha step"]);

    let add = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('a')))
        .expect("ctrl+a must open the step editor");
    apply_intent(&mut domain, &mut model, add, None).expect("open add editor");
    assert_ne!(
        model.input_mode(),
        BoardInputMode::TaskPage,
        "the editor owns input"
    );
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.lines().any(|row| row.contains("▪ step…")),
        "the empty editor must paint inline in the steps section:\n{frame}"
    );

    for ch in "zed step".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None).expect("type");
    }
    let save = map_key(model.input_mode(), press(KeyCode::Enter)).expect("enter");
    assert_eq!(save, BoardIntent::ConfirmEdit);
    let outcome = apply_intent(&mut domain, &mut model, save, None).expect("add step");
    assert_eq!(outcome, IntentOutcome::Persist);
    // The app boundary persisted; add mode keeps an empty next inline row open (T-4).
    model.sync_from_domain(&domain);
    let task = domain.get(id).expect("task");
    let texts: Vec<&str> = task.steps.iter().map(|step| step.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["alpha step", "zed step"],
        "Enter must append the typed step"
    );
    assert_eq!(
        task.history.last().expect("event").kind,
        TaskEventKind::StepAdded
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);

    // Esc cancels the reopened add row: the draft is discarded, nothing is appended.
    for ch in "junk".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None)
            .expect("type junk");
    }
    let cancel = map_key(model.input_mode(), press(KeyCode::Esc)).expect("esc");
    assert_eq!(cancel, BoardIntent::CancelEdit);
    let outcome = apply_intent(&mut domain, &mut model, cancel, None).expect("cancel");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(
        domain.get(id).expect("task").steps.len(),
        2,
        "Esc appends nothing"
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    let closed = rendered_board(&model, 80, 24);
    assert!(
        !closed.lines().any(|row| row.contains("step…")),
        "the inline input row is gone on close:\n{closed}"
    );
}

/// Add mode: Shift+Enter is the only save chord and reopens the inline row empty, proven by
/// two steps added back-to-back. Every save here drives the reducer then the confirmed sync
/// (`sync_from_domain`), which is exactly the pair the app save boundary runs on a successful
/// persist.
#[test]
fn step_editor_shift_enter_reopens_and_retains_task_editing() {
    let (mut domain, mut model, id) = board_with_steps("Loop witness", None, &["alpha step"]);

    // Ctrl+a opens the inline add row; type, then Shift+Enter saves and reopens it empty.
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None)
        .expect("open add editor");
    for ch in "bravo step".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None)
            .expect("type first step");
    }
    let enter =
        map_key(model.input_mode(), press(KeyCode::Enter)).expect("enter maps in the step editor");
    let outcome =
        apply_intent(&mut domain, &mut model, enter, None).expect("save and reopen the inline row");
    assert_eq!(outcome, IntentOutcome::Persist);
    model.sync_from_domain(&domain);
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditStep,
        "Enter in add mode reopens the inline row for the next step"
    );
    let reopened = rendered_board(&model, 80, 24);
    assert!(
        reopened.lines().any(|row| row.contains("▪ step…")),
        "the reopened line paints inline:\n{reopened}"
    );

    // The reopened line is empty: the second step is exactly what is typed next.
    for ch in "charlie step".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None)
            .expect("type second step");
    }
    let enter =
        map_key(model.input_mode(), press(KeyCode::Enter)).expect("enter saves the second step");
    let outcome = apply_intent(&mut domain, &mut model, enter, None).expect("save and reopen");
    assert_eq!(outcome, IntentOutcome::Persist);
    model.sync_from_domain(&domain);
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditStep,
        "Enter keeps the next inline add row open"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
        .expect("close the empty next row");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(
        model.task_editing(),
        "saving a step must retain the whole task edit session"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("Tab reaches the next existing step after a step save");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    assert!(rendered_board(&model, 80, 24).contains("▸ ▪ bravo step"));
    let texts: Vec<&str> = domain
        .get(id)
        .expect("task")
        .steps
        .iter()
        .map(|step| step.text.as_str())
        .collect();
    assert_eq!(
        texts,
        vec!["alpha step", "bravo step", "charlie step"],
        "the rapid-capture loop adds two steps back-to-back"
    );

    // Esc discards: reopen, type junk, Esc — nothing is appended.
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None)
        .expect("reopen add editor");
    for ch in "junk".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None)
            .expect("type junk");
    }
    let esc = map_key(model.input_mode(), press(KeyCode::Esc)).expect("esc");
    assert_eq!(esc, BoardIntent::CancelEdit);
    let outcome = apply_intent(&mut domain, &mut model, esc, None).expect("cancel");
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(
        domain.get(id).expect("task").steps.len(),
        3,
        "Esc discards the draft"
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

/// Existing step rename shares the enclosing task session's Shift+Enter save and close.
#[test]
fn rename_mode_shift_enter_saves_the_task_session_and_closes() {
    let (mut domain, mut model, id) =
        board_with_steps("Rename ctrl witness", None, &["alpha step", "bravo step"]);

    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("activate cursor");
    let rename = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('e'))).expect("ctrl+e");
    apply_intent(&mut domain, &mut model, rename, None).expect("open rename editor");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    for ch in " twice".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None)
            .expect("extend first step text");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::SelectStep(0), None)
        .expect("park first draft and open second");
    for ch in " again".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None)
            .expect("extend second step text");
    }
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Title),
        None,
    )
    .expect("park second draft and focus title");
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None)
        .expect("edit title in the same session");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Scope),
        None,
    )
    .expect("scope has focus without a text cursor");
    assert_eq!(model.input_mode(), BoardInputMode::EditScope);
    let staged = rendered_board(&model, 80, 24);
    assert!(staged.contains("alpha step again"), "first draft: {staged}");
    assert!(
        staged.contains("bravo step twice"),
        "second draft: {staged}"
    );
    let shift_enter = map_key(model.input_mode(), shift(KeyCode::Enter))
        .expect("shift+enter maps in the scope field");
    let outcome =
        apply_intent(&mut domain, &mut model, shift_enter, None).expect("save task session");
    assert_eq!(outcome, IntentOutcome::Persist);
    assert_eq!(
        model.input_mode(),
        BoardInputMode::TaskPage,
        "Shift+Enter in rename mode saves the whole task session and exits editing"
    );
    assert!(
        !model.task_editing(),
        "the confirmed task session returns to its read-only page"
    );
    let task = domain.get(id).expect("task");
    assert_eq!(task.title, "Rename ctrl witness!");
    assert_eq!(task.steps[0].text, "alpha step again");
    assert_eq!(task.steps[1].text, "bravo step twice");
    let closed = rendered_board(&model, 80, 24);
    assert!(
        !closed.lines().any(|row| row.contains("step…")),
        "no reopened inline line in rename mode:\n{closed}"
    );
}

/// T-4 (AC-13): empty or whitespace-only step text is refused by a message painted
/// on the editor line itself — never the board status message — and the refusal
/// clears when the line closes (Esc, or a successful save).
#[test]
fn empty_step_text_refusal_paints_on_line_and_clears_on_close() {
    let (mut domain, mut model, id) = board_with_steps("Refusal witness", None, &["alpha step"]);

    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None)
        .expect("open add editor");
    let shift_enter = map_key(model.input_mode(), shift(KeyCode::Enter)).expect("shift+enter");
    let outcome = apply_intent(&mut domain, &mut model, shift_enter.clone(), None)
        .expect("an empty draft is refused on the line, not by a propagated error");
    assert_eq!(
        outcome,
        IntentOutcome::None,
        "an empty draft persists nothing"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditStep,
        "the line stays open on refusal"
    );
    assert_eq!(
        model.message(),
        None,
        "the refusal must not leak onto the board status message"
    );
    let refused = rendered_board(&model, 80, 24);
    assert!(
        refused
            .lines()
            .any(|row| row.contains("step…") && row.contains("text required")),
        "the refusal paints on the inline editor row itself:\n{refused}"
    );
    assert_eq!(
        domain.get(id).expect("task").steps.len(),
        1,
        "the refused Enter appends nothing"
    );

    // Whitespace-only text is refused the same way.
    for ch in "   ".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None)
            .expect("type whitespace");
    }
    let outcome = apply_intent(&mut domain, &mut model, shift_enter.clone(), None)
        .expect("whitespace is refused on the line too");
    assert_eq!(outcome, IntentOutcome::None);
    let whitespace = rendered_board(&model, 80, 24);
    assert!(
        whitespace
            .lines()
            .any(|row| row.contains("step…") && row.contains("text required")),
        "whitespace-only text paints the same inline refusal:\n{whitespace}"
    );

    // Closing the line with Esc clears the refusal; nothing leaks to the board.
    let esc = map_key(model.input_mode(), press(KeyCode::Esc)).expect("esc");
    apply_intent(&mut domain, &mut model, esc, None).expect("cancel the line");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(
        model.message(),
        None,
        "closing the line leaves no message behind"
    );
    let closed = rendered_board(&model, 80, 24);
    assert!(
        !closed.contains("text required"),
        "the refusal clears when the line closes:\n{closed}"
    );

    // A successful save clears it too: refuse once, type valid text, Enter saves.
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None)
        .expect("reopen add editor");
    apply_intent(&mut domain, &mut model, shift_enter.clone(), None)
        .expect("refuse the empty line again");
    for ch in "zed step".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None)
            .expect("type valid text");
    }
    let enter = map_key(model.input_mode(), press(KeyCode::Enter)).expect("enter");
    let outcome = apply_intent(&mut domain, &mut model, enter, None).expect("save the valid text");
    assert_eq!(outcome, IntentOutcome::Persist);
    model.sync_from_domain(&domain);
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    let saved = rendered_board(&model, 80, 24);
    assert!(
        !saved.contains("text required"),
        "a successful save clears the refusal:\n{saved}"
    );
    assert_eq!(
        domain.get(id).expect("task").steps.len(),
        2,
        "the valid text after a refusal still saves"
    );
}

/// Add and rename drafts paint in the steps section, never in the shared footer slot.
#[test]
fn step_input_is_inline_and_shift_enter_reopens_an_empty_next_row() {
    let (mut domain, mut model, id) = board_with_steps("Inline witness", None, &["alpha step"]);

    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None)
        .expect("open inline add editor");
    for ch in "bravo step".chars() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditInsert(ch), None).expect("type");
    }
    let open = rendered_board(&model, 80, 24);
    assert!(
        open.contains("steps 0/1") && open.contains("▪ bravo step"),
        "{open}"
    );
    assert!(
        !open.contains("▎"),
        "the editor must not occupy the footer:\n{open}"
    );

    let save_next = map_key(model.input_mode(), press(KeyCode::Enter))
        .expect("Enter saves the current step and starts the next");
    assert_eq!(save_next, BoardIntent::ConfirmEdit);
    assert_eq!(
        apply_intent(&mut domain, &mut model, save_next, None).expect("save and continue"),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    let reopened = rendered_board(&model, 80, 24);
    assert!(
        reopened.contains("steps 0/2") && reopened.contains("step…"),
        "{reopened}"
    );
    assert!(!reopened.contains("▎"), "{reopened}");

    let cancel = map_key(model.input_mode(), press(KeyCode::Esc)).expect("Esc");
    apply_intent(&mut domain, &mut model, cancel, None).expect("cancel empty next row");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(domain.get(id).expect("task").steps.len(), 2);
}

/// T-7 (AC-23): the first delete-verb press arms the visible mark AND paints the
/// press-again hint in the footer's message slot; any intervening key clears both;
/// the second press with nothing between removes the step and the hint goes with
/// the mark.
#[test]
fn delete_mark_shows_press_again_footer_message() {
    let (mut domain, mut model, id) =
        board_with_steps("Hint witness", None, &["alpha step", "bravo step"]);
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
        .expect("leave task edit for view delete");
    apply_intent(&mut domain, &mut model, BoardIntent::FormFocusNext, None)
        .expect("select first view step");

    let delete = map_key(BoardInputMode::TaskPage, ctrl(KeyCode::Char('x'))).expect("ctrl+x");
    assert_eq!(delete, BoardIntent::SoftDelete);

    // First press: the mark is visible AND the footer hint is painted.
    let outcome = apply_intent(&mut domain, &mut model, delete.clone(), None).expect("mark step");
    assert_eq!(outcome, IntentOutcome::None);
    let marked = rendered_board(&model, 80, 24);
    assert!(
        marked.contains("✗ alpha step"),
        "the marked step must stay visible:\n{marked}"
    );
    assert!(
        marked.contains("press ctrl+x again to remove"),
        "the footer must prompt the second press while the mark is armed:\n{marked}"
    );
    assert_eq!(
        model.message().expect("hint message"),
        "press ctrl+x again to remove"
    );

    // An intervening key clears the mark AND the hint.
    apply_intent(&mut domain, &mut model, BoardIntent::PageScrollDown, None)
        .expect("intervening key");
    let cleared = rendered_board(&model, 80, 24);
    assert!(!cleared.contains("✗"), "the mark clears:\n{cleared}");
    assert!(
        !cleared.contains("again to remove"),
        "the hint clears with the mark:\n{cleared}"
    );
    assert_eq!(model.message(), None);
    assert_eq!(
        domain.get(id).expect("task").steps.len(),
        2,
        "clearing the mark removes nothing"
    );

    // Second press with nothing between: the step is removed, the hint is gone.
    apply_intent(&mut domain, &mut model, delete.clone(), None).expect("mark again");
    let outcome = apply_intent(&mut domain, &mut model, delete, None).expect("remove step");
    assert_eq!(outcome, IntentOutcome::Persist);
    let texts: Vec<&str> = domain
        .get(id)
        .expect("task")
        .steps
        .iter()
        .map(|step| step.text.as_str())
        .collect();
    assert_eq!(
        texts,
        vec!["alpha step"],
        "the second press removes the marked step (the intervening key moved the \
         cursor, and the re-mark follows it)"
    );
    let removed = rendered_board(&model, 80, 24);
    assert!(
        !removed.contains("✗"),
        "no mark survives the removal:\n{removed}"
    );
    assert!(
        !removed.contains("again to remove"),
        "the hint goes with the mark:\n{removed}"
    );
    assert_eq!(model.message(), None);
}

#[test]
fn t_token_capture_threads_while_item_text_stays_literal() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let snapshot = InvocationSnapshot {
        default_scope: project(THIS_REPO),
        this_repo: Some(PathBuf::from(THIS_REPO)),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    };

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenCapture,
        Some(&snapshot),
    )
    .expect("open quick add");
    for character in "capture !t Release-2026".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::QuickAddInsert(character),
            None,
        )
        .expect("type capture");
    }
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::QuickAddSave, None,)
            .expect("save quick add"),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    let task_id = domain.tasks()[0].id;
    assert_eq!(
        domain.get(task_id).expect("capture").thread.as_deref(),
        Some("release-2026")
    );

    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open task page");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("enter task edit mode");
    apply_intent(&mut domain, &mut model, BoardIntent::CancelEdit, None)
        .expect("return to task page");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None)
        .expect("open item editor");
    for character in "literal !t release-2026 #word".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type item");
    }
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None,).expect("save item"),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(
        domain.get(task_id).expect("capture").steps[0].text,
        "literal !t release-2026 #word"
    );
    assert_eq!(
        domain.get(task_id).expect("capture").thread.as_deref(),
        Some("release-2026")
    );
}

#[test]
fn archived_group_is_collapsed_on_a_fresh_model_and_enter_or_click_on_the_header_toggles_it() {
    let mut domain = DomainState::new();
    let live = domain
        .create(
            "live task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create live");
    let archived = domain
        .create(
            "archived task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create archived");
    domain.archive_task(archived).expect("archive it");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");

    // A fresh model starts collapsed: the header row is in the visible set, the
    // archived task's row is not.
    assert!(
        model
            .visible_ids()
            .contains(&tsk_tui::ui::queue::ARCHIVED_HEADER_ROW_ID),
        "the archived header row must be selectable in the visible set"
    );
    assert!(
        !model.visible_ids().contains(&archived),
        "a fresh model must start with the archived group collapsed"
    );

    // Toggle (the mouse route's intent): selects the header and expands.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleArchivedGroup,
        None,
    )
    .expect("toggle archived group");
    assert!(
        model.visible_ids().contains(&archived),
        "after the toggle the archived row is visible"
    );
    assert_eq!(
        model.selected_id(),
        None,
        "the header row itself is never reported as a selected task"
    );

    // Enter with the header selected collapses again.
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("enter");
    assert!(
        !model.visible_ids().contains(&archived),
        "enter on the selected header must collapse the group"
    );

    // A fresh model is collapsed again (session-only state, not persisted).
    let fresh = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert!(
        !fresh.visible_ids().contains(&archived),
        "a fresh model must start collapsed"
    );
    assert!(
        fresh.visible_ids().contains(&live),
        "the live task is unaffected"
    );
}

#[test]
fn ctrl_f_archives_the_selected_task_keeping_status_and_pushing_no_undo() {
    let mut domain = DomainState::new();
    let b = domain
        .create(
            "second row",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create b");
    let a = domain
        .create(
            "archive target",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create a");
    domain.set_status(a, HumanStatus::Review).expect("review");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    // Newest (A) sorts first in ON DECK, so the seeded selection rests on it.
    assert_eq!(model.selected_id(), Some(a));

    apply_intent(&mut domain, &mut model, BoardIntent::File, None).expect("file verb");
    let task_a = domain.get(a).expect("task a");
    assert!(task_a.archived, "ctrl+f archives the selected task");
    assert_eq!(task_a.status, HumanStatus::Review, "human status is kept");
    assert!(
        !model.visible_ids().contains(&a),
        "the archived row leaves the working lens"
    );

    // Select the remaining row and undo: nothing about A may change (AC-7).
    let b_index = model
        .visible_ids()
        .iter()
        .position(|&visible| visible == b)
        .expect("remaining row visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(b_index),
        None,
    )
    .expect("select b");
    assert_eq!(model.selected_id(), Some(b));
    apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("undo");
    let task_a = domain.get(a).expect("task a");
    assert!(
        task_a.archived,
        "archive pushes no undo entry, so undo cannot touch A"
    );
    assert_eq!(task_a.status, HumanStatus::Review);
    assert_eq!(
        domain.get(b).expect("task b").status,
        HumanStatus::Open,
        "the empty-stack undo is a no-op"
    );
}

#[test]
fn ctrl_f_in_the_archived_group_unarchives_and_the_row_returns_to_the_deck() {
    let (mut domain, mut model, id) = board_with_task("filed away", HumanStatus::Ready);
    domain.archive_task(id).expect("archive");
    model.sync_from_domain(&domain);
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
        .expect("archived row visible after expanding the group");
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None)
        .expect("select the archived row");
    apply_intent(&mut domain, &mut model, BoardIntent::File, None).expect("file verb");
    assert!(
        !domain.get(id).expect("task").archived,
        "ctrl+f in the archived group unarchives"
    );
    assert!(
        model.visible_ids().contains(&id),
        "the row returns to the deck"
    );
}

#[test]
fn ctrl_u_on_an_archived_selection_unarchives_without_popping_the_undo_stack() {
    let mut domain = DomainState::new();
    let a = domain
        .create(
            "archived undo target",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create a");
    let k = domain
        .create(
            "undoable done",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create k");
    domain.complete(k).expect("complete seeds one undo entry");
    domain.archive_task(a).expect("archive a");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
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
        .position(|&visible| visible == a)
        .expect("archived row visible");
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None)
        .expect("select the archived row");
    apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("ctrl+u");
    assert!(
        !domain.get(a).expect("a").archived,
        "ctrl+u on an archived selection unarchives"
    );

    // The undo entry seeded before the unarchive is still on top of the stack:
    // ctrl+u on the non-archived task pops exactly that entry.
    let idx = model
        .visible_ids()
        .iter()
        .position(|&visible| visible == k)
        .expect("done row visible in the open drawer");
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None).expect("select k");
    apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("undo k");
    assert_eq!(
        domain.get(k).expect("k").status,
        HumanStatus::Open,
        "the seeded undo entry survived the unarchive (stack length unchanged)"
    );
}

#[test]
fn help_card_lists_ctrl_f_and_the_verb_bar_keeps_archive_out_of_its_seats() {
    let (mut domain, mut model, _id) = board_with_task("help me", HumanStatus::Ready);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenHelp, None).expect("help");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::HelpQueryInsertText("archive".to_string()),
        None,
    )
    .expect("search help");
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.contains("ctrl+f"),
        "help card must list ctrl+f:\n{frame}"
    );
    assert!(
        frame.to_ascii_lowercase().contains("archive"),
        "help card must label the archive verb:\n{frame}"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None)
        .expect("clear help search");
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close help");

    // Archive has no bar seat: the row's bar is the shared shape, and `?` carries ctrl+f.
    let verbs = board_verb_items(&model);
    assert!(
        !verbs.iter().any(|entry| entry.key == "f"),
        "archive is not a verb-bar seat: {verbs:?}"
    );
    assert_eq!(
        verbs.iter().map(|entry| entry.key).collect::<Vec<_>>(),
        vec!["enter", "s", "o", "d", "+", "?"],
        "ready row bar: open · start · inbox · done · add · help"
    );

    // The expanded archived group: header shows its toggle, a row shows `f unarchive`.
    let archived = domain
        .create(
            "filed row",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create archived");
    domain.archive_task(archived).expect("archive it");
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleArchivedGroup,
        None,
    )
    .expect("expand");

    let verbs = board_verb_items(&model);
    assert!(
        verbs
            .iter()
            .any(|entry| entry.key == "enter" && entry.label == "collapse"),
        "header selected: verb bar shows `enter collapse`: {verbs:?}"
    );

    let idx = model
        .visible_ids()
        .iter()
        .position(|&visible| visible == archived)
        .expect("archived row visible");
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(idx), None)
        .expect("select the archived row");
    let verbs = board_verb_items(&model);
    assert!(
        !verbs.iter().any(|entry| entry.key == "f"),
        "unarchive is not a verb-bar seat either: {verbs:?}"
    );
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.contains("ctrl+u undo") || !frame.contains("u Undo"),
        "no legacy undo label survives"
    );
}

#[test]
fn ctrl_f_in_the_picker_archives_the_selected_project_and_keeps_the_picker_open() {
    let mut domain = DomainState::new();
    domain
        .create(
            "in app",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let other = "/repos/other";
    domain
        .create(
            "in other",
            None,
            project(other),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open picker");
    assert_eq!(model.input_mode(), BoardInputMode::ProjectPicker);

    // Walk the main list onto the other project's option.
    let target = tsk_tui::ui::board::ProjectScopeOption::Project(PathBuf::from(other));
    for _ in 0..model.project_options().len() {
        let index = model.project_picker_index().expect("picker index");
        if model.project_options()[index] == target {
            break;
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ProjectPickerNext,
            None,
        )
        .expect("next");
    }
    let index = model.project_picker_index().expect("picker index");
    assert_eq!(
        model.project_options()[index],
        target,
        "walked onto the project"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::File, None).expect("ctrl+f files");

    assert!(
        domain.is_project_archived(other),
        "ctrl+f archives the selected project"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::ProjectPicker,
        "the picker stays open"
    );
    assert!(
        !model.project_options().contains(&target),
        "the project leaves the main list"
    );
    assert!(
        model
            .archived_project_options()
            .contains(&PathBuf::from(other)),
        "the project is listed on the archived tab"
    );
}

#[test]
fn ctrl_f_and_ctrl_u_on_the_archived_tab_unarchive_and_every_task_keeps_its_status() {
    let mut domain = DomainState::new();
    let a = domain
        .create(
            "a ready",
            None,
            project("/repos/a"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create a");
    let b = domain
        .create(
            "b started",
            None,
            project("/repos/b"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create b");
    domain.set_status(b, HumanStatus::Started).expect("started");
    let statuses_before: Vec<(uuid::Uuid, HumanStatus)> = domain
        .tasks()
        .iter()
        .map(|task| (task.id, task.status))
        .collect();
    domain.archive_project("/repos/a").expect("archive a");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/a")));

    // Switch to the archived tab and ctrl+f the entry: the project unarchives.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open picker");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ProjectPickerSwitchTab,
        None,
    )
    .expect("switch tab");
    apply_intent(&mut domain, &mut model, BoardIntent::File, None).expect("ctrl+f unarchives");
    assert!(
        !domain.is_project_archived("/repos/a"),
        "ctrl+f on the archived tab unarchives"
    );
    assert_eq!(
        model.archived_project_options(),
        Vec::<PathBuf>::new(),
        "no archived projects remain"
    );
    assert!(
        model
            .project_options()
            .contains(&tsk_tui::ui::board::ProjectScopeOption::Project(
                PathBuf::from("/repos/a")
            )),
        "the project returns to the main list (and the projects tab)"
    );

    // Re-archive, switch, and ctrl+u: same unarchive route, statuses untouched.
    domain.archive_project("/repos/a").expect("archive again");
    model.sync_from_domain(&domain);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open picker");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ProjectPickerSwitchTab,
        None,
    )
    .expect("switch tab");
    apply_intent(&mut domain, &mut model, BoardIntent::Undo, None).expect("ctrl+u unarchives");
    assert!(
        !domain.is_project_archived("/repos/a"),
        "ctrl+u on the archived tab unarchives"
    );

    let statuses_after: Vec<(uuid::Uuid, HumanStatus)> = domain
        .tasks()
        .iter()
        .map(|task| (task.id, task.status))
        .collect();
    assert_eq!(
        statuses_before, statuses_after,
        "project archive round-trips leave every task's status alone"
    );
    let _ = a;
}

#[test]
fn file_on_a_taskless_invocation_repo_refuses_on_the_status_line() {
    let mut domain = DomainState::new();
    domain
        .create(
            "desk only",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create desk task");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open picker");

    // Select the invocation repo option: it has no tasks, so there is nothing to archive.
    let target = tsk_tui::ui::board::ProjectScopeOption::Project(PathBuf::from(THIS_REPO));
    for _ in 0..model.project_options().len() {
        let index = model.project_picker_index().expect("picker index");
        if model.project_options()[index] == target {
            break;
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ProjectPickerNext,
            None,
        )
        .expect("next");
    }
    apply_intent(&mut domain, &mut model, BoardIntent::File, None).expect("file applies");

    assert_eq!(
        model.input_mode(),
        BoardInputMode::ProjectPicker,
        "the picker stays open"
    );
    let message = model
        .message()
        .expect("a refusal paints on the status slot");
    assert!(
        message.contains("nothing to archive") && message.contains("app"),
        "the refusal names the empty project: {message:?}"
    );
    assert!(
        !domain.is_project_archived(THIS_REPO),
        "no record was written"
    );
}

#[test]
fn picker_archive_of_the_focused_project_keeps_an_unarchived_cwd_default_for_quick_add() {
    // Focus /repos/other, archive it from the picker. The board goes home, but the
    // invocation default (THIS_REPO, unarchived) must still drive quick-add: archiving one
    // project is not a session-wide "everything goes to the desk".
    let mut domain = DomainState::new();
    domain
        .create(
            "in app",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let other = "/repos/other";
    domain
        .create(
            "in other",
            None,
            project(other),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let target = tsk_tui::ui::board::ProjectScopeOption::Project(PathBuf::from(other));
    let walk_to_other = |domain: &mut DomainState, model: &mut BoardModel| {
        apply_intent(domain, model, BoardIntent::OpenProjectSelector, None).expect("open picker");
        for _ in 0..model.project_options().len() {
            let index = model.project_picker_index().expect("picker index");
            if model.project_options()[index] == target {
                break;
            }
            apply_intent(domain, model, BoardIntent::ProjectPickerNext, None).expect("next");
        }
    };
    walk_to_other(&mut domain, &mut model);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmProjectChoice,
        None,
    )
    .expect("focus other");
    assert_eq!(model.selected_project(), Some(Path::new(other)));

    walk_to_other(&mut domain, &mut model);
    apply_intent(&mut domain, &mut model, BoardIntent::File, None).expect("ctrl+f files");
    assert!(domain.is_project_archived(other));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CancelProjectPicker,
        None,
    )
    .expect("close picker");
    assert_eq!(
        model.selected_project(),
        None,
        "focus on the archived project resets to home"
    );

    let snapshot = InvocationSnapshot {
        default_scope: project(THIS_REPO),
        this_repo: Some(PathBuf::from(THIS_REPO)),
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
    let probe = domain
        .tasks()
        .iter()
        .find(|task| task.title == "probe")
        .expect("probe saved");
    assert_eq!(
        probe.scope,
        project(THIS_REPO),
        "the unarchived invocation default still drives quick-add"
    );
}

/// A board focused (read-only) on an archived project holding one Ready task.
fn read_only_focus() -> (DomainState, BoardModel, uuid::Uuid) {
    let mut domain = DomainState::new();
    let inside = domain
        .create(
            "filed away task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create inside");
    domain
        .create(
            "desk task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create desk");
    domain.archive_project(THIS_REPO).expect("archive project");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open picker");
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
    .expect("enter opens read-only focus");
    (domain, model, inside)
}

#[test]
fn enter_on_the_archived_tab_opens_a_read_only_focus_that_persists_nothing() {
    let mut domain = DomainState::new();
    let inside = domain
        .create(
            "filed away task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create inside");
    domain
        .create(
            "second filed task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    domain.archive_project(THIS_REPO).expect("archive project");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert!(
        !model.visible_ids().contains(&inside),
        "no working lens paints an archived project's tasks"
    );

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("open picker");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ProjectPickerSwitchTab,
        None,
    )
    .expect("archived tab");
    let outcome = apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmProjectChoice,
        None,
    )
    .expect("enter opens the focus");
    assert_eq!(
        outcome,
        IntentOutcome::None,
        "entering a read-only focus persists nothing"
    );
    assert!(
        domain.is_project_archived(THIS_REPO),
        "the project is still archived"
    );
    assert_eq!(model.input_mode(), BoardInputMode::Normal, "picker closed");
    assert!(model.focus_is_archived(), "the focus is the read-only one");
    assert!(
        model.visible_ids().contains(&inside),
        "the archived project's tasks paint in this focus"
    );

    // The idle status row names the read-only lens, and the chip says the same.
    let status = board_chrome_row(&model, (80, 24));
    assert_eq!(status.trim(), "app · archived");
    let frame = rendered_board(&model, 80, 24);
    assert!(
        frame.contains("app \u{b7} archived"),
        "the chip reads `<name> · archived`:\n{frame}"
    );
    assert!(
        frame.contains("ctrl+u unarchive \u{b7} enter open \u{b7} esc close \u{b7} ? help"),
        "the focus verb bar offers only what works here:\n{frame}"
    );
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, &model);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let selected = model.selected_id();
    let unselected_title = if selected == Some(inside) {
        "second filed task"
    } else {
        "filed away task"
    };
    let row_y = (0..24u16)
        .find(|y| {
            (0..80u16)
                .map(|x| buffer[(x, *y)].symbol())
                .collect::<String>()
                .contains(unselected_title)
        })
        .expect("the task row paints");
    let row: String = (0..80u16).map(|x| buffer[(x, row_y)].symbol()).collect();
    let title_x = row.find(unselected_title).expect("title on the row") as u16;
    for x in title_x..title_x + 5 {
        assert!(
            buffer[(x, row_y)]
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::DIM),
            "the archived project's task titles paint dim (cell {x}): {row:?}"
        );
    }
}

#[test]
fn every_mutating_verb_in_read_only_focus_refuses_with_the_archived_message() {
    let refusal = "project app is archived \u{b7} ctrl+u unarchive";
    let chords = [
        BoardIntent::PrimaryVerb,
        BoardIntent::Complete,
        BoardIntent::Reopen,
        BoardIntent::ToggleBlock,
        BoardIntent::ToggleReview,
        BoardIntent::ToggleStep,
        BoardIntent::BeginEditTitle,
        BoardIntent::BeginEditNotes,
        BoardIntent::SoftDelete,
        BoardIntent::File,
        BoardIntent::OpenCapture,
    ];
    for intent in chords {
        let (mut domain, mut model, inside) = read_only_focus();
        let before = domain
            .tasks()
            .iter()
            .find(|task| task.id == inside)
            .cloned()
            .expect("task before");
        let outcome = apply_intent(&mut domain, &mut model, intent.clone(), None)
            .unwrap_or_else(|error| panic!("{intent:?} applies: {error}"));
        assert_eq!(
            outcome,
            IntentOutcome::None,
            "{intent:?} changes nothing durable"
        );
        assert_eq!(
            model.message(),
            Some(refusal),
            "{intent:?} refuses with the archived message"
        );
        assert_eq!(
            model.input_mode(),
            BoardInputMode::Normal,
            "{intent:?} opens no surface"
        );
        let after = domain
            .tasks()
            .iter()
            .find(|task| task.id == inside)
            .cloned()
            .expect("task after");
        assert_eq!(after.status, before.status, "{intent:?} changed the status");
        assert_eq!(
            after.soft_deleted, before.soft_deleted,
            "{intent:?} deleted the task"
        );
        assert_eq!(after.archived, before.archived, "{intent:?} archived it");
        assert!(
            model.focus_is_archived(),
            "{intent:?} left the read-only focus"
        );
    }
}

#[test]
fn ctrl_u_in_read_only_focus_unarchives_in_place() {
    let (mut domain, mut model, inside) = read_only_focus();
    assert!(domain.is_project_archived(THIS_REPO));

    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::Undo, None)
        .expect("ctrl+u unarchives in place");
    assert_eq!(outcome, IntentOutcome::Persist, "the unarchive is durable");
    assert!(
        !domain.is_project_archived(THIS_REPO),
        "the project is live again"
    );
    assert!(
        !model.focus_is_archived(),
        "the focus became a normal project focus"
    );
    assert_eq!(
        model.selected_project(),
        Some(Path::new(THIS_REPO)),
        "on the same project"
    );
    assert!(
        model.visible_ids().contains(&inside),
        "its tasks stay on the board"
    );

    let frame = rendered_board(&model, 80, 24);
    assert!(
        !frame.contains("app \u{b7} archived"),
        "the chip drops the archived suffix:\n{frame}"
    );

    // Verbs work again.
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
    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::Complete, None)
        .expect("ctrl+d works again");
    assert_eq!(outcome, IntentOutcome::Persist);
}

#[test]
fn leaving_read_only_focus_hides_the_archived_projects_tasks_again() {
    for leave in [
        BoardIntent::CancelEdit,
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::Desk),
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::Projects),
    ] {
        let (mut domain, mut model, inside) = read_only_focus();
        assert!(model.visible_ids().contains(&inside));
        apply_intent(&mut domain, &mut model, leave.clone(), None)
            .unwrap_or_else(|error| panic!("{leave:?} applies: {error}"));
        assert!(
            !model.focus_is_archived(),
            "{leave:?} leaves the read-only focus"
        );
        assert!(
            !model.visible_ids().contains(&inside),
            "{leave:?} hides the archived project's tasks again"
        );
    }
}

#[test]
fn esc_leaves_read_only_focus_and_never_quits() {
    let (mut domain, mut model, inside) = read_only_focus();
    assert!(model.focus_is_archived());
    let esc = map_key(BoardInputMode::Normal, press(KeyCode::Esc)).expect("Esc maps");
    let outcome = apply_intent(&mut domain, &mut model, esc, None).expect("esc applies");
    assert_eq!(
        outcome,
        IntentOutcome::None,
        "Esc in read-only focus never quits the board"
    );
    assert!(
        !model.focus_is_archived(),
        "Esc leaves the read-only focus (AC-45)"
    );
    assert!(model.at_home(), "and lands on the desk");
    assert!(
        !model.visible_ids().contains(&inside),
        "the archived project's tasks are hidden again"
    );
}

#[test]
fn opening_the_picker_from_read_only_focus_lands_home_on_cancel() {
    let (mut domain, mut model, inside) = read_only_focus();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("picker opens");
    assert_eq!(model.input_mode(), BoardInputMode::ProjectPicker);
    assert!(
        !model.focus_is_archived(),
        "P leaves the read-only lens before the picker paints (AC-45)"
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CancelProjectPicker,
        None,
    )
    .expect("cancel");
    assert!(model.at_home(), "cancelling the picker lands on the desk");
    assert!(
        !model.visible_ids().contains(&inside),
        "the archived project's tasks are hidden again"
    );
}

#[test]
fn unarchiving_from_the_picker_converts_a_read_only_focus_in_place() {
    let (mut domain, mut model, inside) = read_only_focus();
    // The picker leaves the lens (D3), but the focus is restored to prove the conversion
    // path rather than the exit path: unarchive through the picker's archived tab.
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
    .expect("back into the read-only focus");
    assert!(model.focus_is_archived());

    // Unarchive it by any other route: the stale read-only lens must convert.
    domain.unarchive_project(THIS_REPO).expect("unarchive");
    model.sync_from_domain(&domain);

    assert!(
        !model.focus_is_archived(),
        "the read-only lens converts once its project is live again"
    );
    assert_eq!(
        model.selected_project(),
        Some(Path::new(THIS_REPO)),
        "on the same project"
    );
    assert!(
        model.visible_ids().contains(&inside),
        "its tasks stay on the board"
    );
    let frame = rendered_board(&model, 80, 24);
    assert!(
        !frame.contains("app \u{b7} archived"),
        "the chip drops the archived suffix:\n{frame}"
    );
}

#[test]
fn ctrl_f_on_the_archived_tab_still_unarchives_from_read_only_focus() {
    let (mut domain, mut model, inside) = read_only_focus();
    // `P` leaves the lens (D3); step back into it so the picker is reached from the
    // read-only focus exactly as a user would.
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

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .expect("picker again");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ProjectPickerSwitchTab,
        None,
    )
    .expect("archived tab");
    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::File, None)
        .expect("ctrl+f on the archived entry");
    assert_eq!(
        outcome,
        IntentOutcome::Persist,
        "the picker owns ctrl+f while it is open"
    );
    assert!(
        !domain.is_project_archived(THIS_REPO),
        "the entry was unarchived"
    );
    assert_ne!(
        model.message(),
        Some("project app is archived \u{b7} ctrl+u unarchive"),
        "the read-only refusal must not fire for a picker verb"
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CancelProjectPicker,
        None,
    )
    .expect("close");
    assert!(!model.focus_is_archived());
    let _ = inside;
}

#[test]
fn toggle_all_groups_folds_and_unfolds_the_archived_group_with_the_others_when_the_drawer_is_open()
{
    let mut domain = DomainState::new();
    let live = domain
        .create(
            "live row",
            None,
            project("/repos/app"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create live");
    let filed = domain
        .create(
            "filed row",
            None,
            project("/repos/app"),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create filed");
    domain.archive_task(filed).expect("archive");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from("/repos/app")));
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).expect("drawer");
    // Expand the archived group on its own, the way Enter on the header does.
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleArchivedGroup,
        None,
    )
    .expect("expand archived");
    assert!(model.visible_ids().contains(&live), "live rows open");
    assert!(model.visible_ids().contains(&filed), "archived group open");

    let toggle_all =
        map_key(BoardInputMode::Normal, press(KeyCode::Char('g'))).expect("g is bound");
    assert_eq!(
        toggle_all,
        BoardIntent::ToggleAllGroups,
        "bare g folds the groups"
    );

    apply_intent(&mut domain, &mut model, toggle_all.clone(), None).expect("fold all");
    let visible = model.visible_ids();
    assert!(
        !visible.contains(&filed),
        "the archived group folded with the drawer open"
    );
    assert!(
        visible.contains(&live),
        "live rows never fold; they are not a group"
    );

    apply_intent(&mut domain, &mut model, toggle_all.clone(), None).expect("unfold all");
    assert!(
        model.visible_ids().contains(&filed),
        "and the archived group unfolded again"
    );

    // Drawer closed: no group answers the chord, and the archived group's own state
    // is left alone (one press, so a regression cannot cancel itself out).
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("close drawer");
    apply_intent(&mut domain, &mut model, toggle_all, None).expect("inert with the drawer shut");
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None)
        .expect("reopen drawer");
    assert!(
        model.visible_ids().contains(&filed),
        "the archived group is still expanded, untouched while the drawer was shut"
    );
}

#[test]
fn first_delete_asks_and_second_deletes() {
    let (mut domain, mut model, id) = board_with_noted_task();
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("arm");
    assert!(!domain.get(id).expect("task").soft_deleted);
    assert_eq!(model.message(), Some("press ctrl+x again to delete"));
    apply_intent(&mut domain, &mut model, BoardIntent::SoftDelete, None).expect("confirm");
    assert!(domain.get(id).expect("task").soft_deleted);
}

#[test]
fn empty_add_click_on_title_discards_the_row() {
    let (mut domain, mut model, _) = board_with_steps("Click away", None, &["alpha"]);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None).expect("open add");
    assert_eq!(model.input_mode(), BoardInputMode::EditStep);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Title),
        None,
    )
    .expect("click title");
    assert_ne!(model.input_mode(), BoardInputMode::EditStep);
    assert!(!rendered_board(&model, 80, 24).contains("step…"));
}

#[test]
fn shift_enter_on_a_typed_add_saves_and_exits_task_editing() {
    let (mut domain, mut model, id) = board_with_steps("Save exit", None, &["alpha"]);
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None).expect("open add");
    for character in "bravo".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type");
    }
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEditNext, None).expect("save"),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(!model.task_editing());
    let texts: Vec<&str> = domain
        .get(id)
        .expect("task")
        .steps
        .iter()
        .map(|step| step.text.as_str())
        .collect();
    assert_eq!(texts, vec!["alpha", "bravo"]);
}

#[test]
fn shift_enter_on_add_keeps_a_dirty_title() {
    let (mut domain, mut model, id) = board_with_steps("Dirty title", None, &["alpha"]);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Title),
        None,
    )
    .expect("title");
    apply_intent(&mut domain, &mut model, BoardIntent::EditInsert('!'), None).expect("type");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginAddStep, None).expect("add");
    for character in "bravo".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type step");
    }
    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEditNext, None).expect("save"),
        IntentOutcome::Persist
    );
    model.sync_from_domain(&domain);
    let task = domain.get(id).expect("task");
    assert_eq!(task.title, "Dirty title!");
    let texts: Vec<&str> = task.steps.iter().map(|step| step.text.as_str()).collect();
    assert_eq!(texts, vec!["alpha", "bravo"]);
    assert!(!model.task_editing());
}
