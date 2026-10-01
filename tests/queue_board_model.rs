//! BoardModel cutover onto queue query, selection anchor, and session-only state.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::BTreeSet;
use tsk_tui::context::InvocationSnapshot;
use tsk_tui::domain::{
    DomainState, HumanStatus, ProvenanceOrigin, Task, TaskEvent, TaskEventKind, TaskScope,
};
use tsk_tui::scope::paths_equivalent;
use tsk_tui::store::TaskStore;
use tsk_tui::ui::board::{apply_intent, draw_board, BoardModel, ProjectScopeOption};
use tsk_tui::ui::input::BoardIntent;
use tsk_tui::ui::queue::{
    query_board, query_lens, BoardLens, ProjectRow, QueueView, SectionKind, ThreadFilter,
    INBOX_HEADER_ROW_ID,
};
use uuid::Uuid;

const THIS_REPO: &str = "/repos/app";

fn epoch_plus(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn task(id: u128, title: &str, status: HumanStatus, scope: TaskScope, updated_secs: u64) -> Task {
    let at = epoch_plus(updated_secs);
    Task {
        id: Uuid::from_u128(id),
        number: None,
        notice: None,
        revision: Uuid::from_u128(id),
        merge_base_revision: None,
        title: title.into(),
        notes: None,
        thread: None,
        assignee: None,
        dispatch: None,
        status,
        scope,
        provenance: ProvenanceOrigin::Manual,
        history: vec![TaskEvent {
            kind: TaskEventKind::Created,
            at,
        }],
        steps: Vec::new(),
        soft_deleted: false,
        archived: false,
        created_at: at,
        updated_at: at,
    }
}

fn project(path: &str) -> TaskScope {
    TaskScope::Project {
        path: path.to_string(),
    }
}

fn domain_with_tasks(tasks: Vec<Task>) -> DomainState {
    let mut document = serde_json::to_value(DomainState::new()).expect("serialize empty domain");
    document["tasks"] = serde_json::to_value(tasks).expect("serialize tasks");
    serde_json::from_value(document).expect("domain with fixture tasks")
}

fn threaded_task(
    id: u128,
    title: &str,
    status: HumanStatus,
    scope: TaskScope,
    updated_secs: u64,
    thread: &str,
) -> Task {
    let mut task = task(id, title, status, scope, updated_secs);
    task.thread = Some(thread.to_string());
    task
}

fn on_deck(view: &tsk_tui::ui::queue::QueueView) -> &tsk_tui::ui::queue::QueueSection {
    view.sections
        .iter()
        .find(|section| section.kind == SectionKind::OnDeck)
        .expect("ON DECK section")
}

fn section_ids(view: &QueueView, kind: SectionKind) -> Vec<Uuid> {
    view.sections
        .iter()
        .filter(|section| section.kind == kind)
        .flat_map(|section| section.task_ids.iter().copied())
        .collect()
}

#[allow(unused)]
fn project_row(view: &QueueView, path: &str) -> ProjectRow {
    view.projects
        .iter()
        .find(|row| paths_equivalent(&row.path, path))
        .expect("index row")
        .clone()
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-queue-board-model-{tag}-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

struct TempGuard(PathBuf);
impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn list_files_recursive(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// First open pins the first NEEDS YOU / IN MOTION / ON DECK row of the startup
/// destination: the invocation project board, or desk with no project context.
#[test]
fn from_domain_seeds_selection_on_first_needs_you_else_motion_else_deck_row() {
    // Case A: startup in a repo opens its project board and seeds by lane order.
    let motion_newer = task(
        1,
        "motion-new",
        HumanStatus::Started,
        project(THIS_REPO),
        100,
    );
    let motion_older = task(
        2,
        "motion-old",
        HumanStatus::Started,
        project(THIS_REPO),
        50,
    );
    let deck = task(3, "deck", HumanStatus::Ready, project(THIS_REPO), 200);
    let tasks = vec![motion_newer.clone(), motion_older.clone(), deck.clone()];
    let model = BoardModel::from_tasks(tasks.clone(), Some(PathBuf::from(THIS_REPO)));
    let view = query_lens(&tasks, Some(Path::new(THIS_REPO)), BoardLens::Desk, false);
    let first_motion = view
        .sections
        .iter()
        .find(|s| s.kind == SectionKind::InMotion)
        .and_then(|s| s.task_ids.first().copied());
    assert_eq!(first_motion, Some(Uuid::from_u128(1)));
    assert_eq!(
        model.selected_id(),
        first_motion,
        "open seeds the first IN MOTION row"
    );

    // from_domain shares the same seed rule.
    let mut domain = DomainState::new();
    let id_new = domain
        .create(
            "motion-new",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    domain.set_status(id_new, HumanStatus::Started).unwrap();
    let id_old = domain
        .create(
            "motion-old",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    domain.set_status(id_old, HumanStatus::Started).unwrap();
    let _deck = domain
        .create(
            "deck",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    // Domain create stamps wall-clock times; seed still lands on some IN MOTION id.
    let from_domain = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let selected = from_domain.selected_id().expect("selection");
    assert_eq!(
        domain.get(selected).unwrap().status,
        HumanStatus::Started,
        "from_domain seeds an IN MOTION task when any exist"
    );

    // Case B: a ready-only project opens its own board and seeds the first ON DECK row.
    let deck_a = task(10, "a", HumanStatus::Ready, project("/repos/a"), 30);
    let deck_b = task(11, "b", HumanStatus::Ready, project(THIS_REPO), 40);
    let deck_tasks = vec![deck_a.clone(), deck_b.clone()];
    let view = query_lens(
        &deck_tasks,
        Some(Path::new(THIS_REPO)),
        BoardLens::Project(Path::new(THIS_REPO)),
        false,
    );
    let first_deck = view
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::OnDeck)
        .flat_map(|s| s.task_ids.iter().copied())
        .next();
    assert_eq!(first_deck, Some(Uuid::from_u128(11)));

    let mut domain = DomainState::new();
    let deck_id = domain
        .create(
            "b",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(model.nav_tab(), tsk_tui::ui::queue::NavTab::ProjectBoard);
    assert_eq!(
        model.selected_id(),
        Some(deck_id),
        "startup opens the invocation project's board and seeds its first row"
    );

    // Case C: empty → no selection.
    let empty = BoardModel::from_tasks(vec![], Some(PathBuf::from(THIS_REPO)));
    assert_eq!(empty.selected_id(), None);
}

/// Directory-aware startup: a live project opens its board, an archived project
/// stays on the desk behind the launch card, absent project context means desk.
#[test]
fn from_domain_opens_the_invocation_project_unless_archived() {
    let mut domain = DomainState::new();
    let live_id = domain
        .create(
            "live",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    domain
        .create(
            "elsewhere",
            None,
            project("/repos/other"),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();

    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(
        model.selected_project(),
        Some(Path::new(THIS_REPO)),
        "launch inside a repo opens that project's board"
    );
    assert_eq!(model.visible_ids(), vec![INBOX_HEADER_ROW_ID, live_id]);
    assert_eq!(model.nav_tab(), tsk_tui::ui::queue::NavTab::ProjectBoard);

    // Empty project: same destination, useful empty state (no fallback lens).
    let empty_repo = format!("{THIS_REPO}-empty");
    let model = BoardModel::from_domain(&DomainState::new(), Some(PathBuf::from(&empty_repo)));
    assert_eq!(
        model.selected_project(),
        Some(Path::new(empty_repo.as_str()))
    );
    assert!(model.visible_ids().is_empty());
    let view = model.queue_view();
    assert!(
        view.sections
            .iter()
            .any(|section| section.kind == SectionKind::OnDeck && section.empty_hint),
        "an empty project board keeps a hinted deck section"
    );

    // Archived invocation repo: the launch card owns it; the board stays on the desk.
    let mut domain = DomainState::new();
    let archived_id = domain
        .create(
            "archived repo task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    domain.archive_project(THIS_REPO).expect("archive project");
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(model.selected_project(), None);
    assert_eq!(model.nav_tab(), tsk_tui::ui::queue::NavTab::Desk);
    assert!(!model.visible_ids().contains(&archived_id));

    // A non-Git invocation keeps its directory in project slot 2 but opens Desk.
    let mut domain = DomainState::new();
    let desk_id = domain
        .create(
            "desk",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    let outside = PathBuf::from("/work/outside-git");
    let snapshot = InvocationSnapshot {
        default_scope: TaskScope::Global,
        this_repo: Some(outside.clone()),
        title_prefill: None,
        provenance: ProvenanceOrigin::Manual,
    };
    let model = BoardModel::from_domain_for_snapshot(&domain, &snapshot);
    assert_eq!(model.nav_tab(), tsk_tui::ui::queue::NavTab::Desk);
    assert_eq!(model.selected_project(), Some(outside.as_path()));
    assert_eq!(model.selected_id(), Some(desk_id));
}

/// Confirming a project selector choice narrows ON DECK to that project.
#[test]
fn confirming_project_choice_changes_visible_queue_sections() {
    let mut domain = DomainState::new();
    let app_id = domain
        .create(
            "app task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    let other_id = domain
        .create(
            "other task",
            None,
            project("/repos/other"),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(
        model.visible_ids(),
        vec![INBOX_HEADER_ROW_ID, app_id],
        "startup sits on the invocation project's board"
    );

    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenProjectSelector,
        None,
    )
    .unwrap();
    // Picker highlights the current deck scope (All at open); step to /repos/other.
    let other_idx = model
        .project_options()
        .iter()
        .position(|opt| opt == &ProjectScopeOption::Project(PathBuf::from("/repos/other")))
        .expect("/repos/other option");
    for _ in 0..model.project_options().len() {
        if model.project_picker_index() == Some(other_idx) {
            break;
        }
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::ProjectPickerNext,
            None,
        )
        .unwrap();
    }
    assert_eq!(model.project_picker_index(), Some(other_idx));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmProjectChoice,
        None,
    )
    .unwrap();

    assert_eq!(model.selected_project(), Some(Path::new("/repos/other")));
    assert_eq!(model.visible_ids(), vec![INBOX_HEADER_ROW_ID, other_id]);
}

/// After a domain sync, selection stays on the same id when it remains visible.
#[test]
fn sync_from_domain_reanchors_by_id() {
    let mut domain = DomainState::new();
    let id_doing = domain
        .create(
            "doing",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    domain.set_status(id_doing, HumanStatus::Started).unwrap();
    let id_todo = domain
        .create(
            "todo",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();

    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let visible = model.visible_ids();
    assert!(visible.contains(&id_doing));
    assert!(visible.contains(&id_todo));
    let todo_idx = visible.iter().position(|&id| id == id_todo).unwrap();
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(todo_idx),
        None,
    )
    .unwrap();
    assert_eq!(model.selected_id(), Some(id_todo));

    // Sync with the same snapshot: selection stays on todo by id.
    model.sync_from_domain(&domain);
    assert_eq!(
        model.selected_id(),
        Some(id_todo),
        "reanchor keeps the same id when still visible"
    );

    // Soft-delete the selected task; reanchor must leave todo and land on a survivor.
    domain.soft_delete(id_todo).unwrap();
    model.sync_from_domain(&domain);
    assert_eq!(
        model.selected_id(),
        Some(id_doing),
        "when the pinned id leaves the visible set, reanchor lands on a survivor"
    );
    assert!(!model.visible_ids().contains(&id_todo));
}

/// Completing the last ready row must not reanchor onto the inbox heading that follows it.
#[test]
fn ready_task_leaving_view_reanchors_to_neighbor_not_inbox_header() {
    let mut domain = DomainState::new();
    let first_ready = domain
        .create(
            "first ready",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    domain.set_status(first_ready, HumanStatus::Ready).unwrap();
    let last_ready = domain
        .create(
            "last ready",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    domain.set_status(last_ready, HumanStatus::Ready).unwrap();
    let open = domain
        .create(
            "inbox neighbor",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();

    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let visible = model.visible_ids();
    let last_ready_index = visible
        .iter()
        .position(|&id| id == last_ready)
        .expect("last ready row");
    let inbox_index = visible
        .iter()
        .position(|&id| id == INBOX_HEADER_ROW_ID)
        .expect("inbox heading");
    assert_eq!(last_ready_index + 1, inbox_index);
    assert!(visible.contains(&open));
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(last_ready_index),
        None,
    )
    .unwrap();

    apply_intent(&mut domain, &mut model, BoardIntent::Complete, None).unwrap();

    assert_eq!(model.selected_id(), Some(first_ready));
    assert_ne!(model.selected_id(), None);
    assert_ne!(model.selected_id(), Some(open));
    assert!(!model.visible_ids().contains(&last_ready));
}

#[test]
fn project_deck_lists_tasks_flat_with_thread_filter_across_statuses() {
    let tasks = vec![
        threaded_task(
            1,
            "open",
            HumanStatus::Ready,
            project(THIS_REPO),
            20,
            "Release",
        ),
        threaded_task(
            2,
            "done",
            HumanStatus::Done,
            project(THIS_REPO),
            30,
            "release",
        ),
        threaded_task(
            3,
            "doing",
            HumanStatus::Started,
            project(THIS_REPO),
            40,
            "release",
        ),
    ];

    // The unfiltered project deck is flat: threads are row labels, never headers.
    let view = query_board(
        &tasks,
        &BTreeSet::new(),
        Some(Path::new(THIS_REPO)),
        BoardLens::Project(Path::new(THIS_REPO)),
        true,
        &ThreadFilter::All,
    );
    let deck = on_deck(&view);
    assert_eq!(
        deck.task_ids,
        vec![Uuid::from_u128(1)],
        "only the ready task is on deck; threads do not group rows"
    );

    // The filter narrows every status section, drawer included.
    let filtered = query_board(
        &tasks,
        &BTreeSet::new(),
        Some(Path::new(THIS_REPO)),
        BoardLens::Project(Path::new(THIS_REPO)),
        true,
        &ThreadFilter::Named("release".to_string()),
    );
    assert_eq!(
        section_ids(&filtered, SectionKind::InMotion),
        vec![Uuid::from_u128(3)]
    );
    assert_eq!(
        section_ids(&filtered, SectionKind::Done),
        vec![Uuid::from_u128(2)],
        "the done drawer respects the active thread filter"
    );
}

#[test]
fn project_deck_orders_ready_tasks_oldest_first_across_threads() {
    let tasks = vec![
        threaded_task(
            1,
            "alpha older",
            HumanStatus::Ready,
            project(THIS_REPO),
            20,
            "alpha",
        ),
        task(3, "loose", HumanStatus::Ready, project(THIS_REPO), 40),
        threaded_task(
            2,
            "alpha newer",
            HumanStatus::Ready,
            project(THIS_REPO),
            50,
            "alpha",
        ),
    ];

    let view = query_board(
        &tasks,
        &BTreeSet::new(),
        Some(Path::new(THIS_REPO)),
        BoardLens::Project(Path::new(THIS_REPO)),
        false,
        &ThreadFilter::All,
    );
    let deck = on_deck(&view);
    assert_eq!(
        deck.task_ids,
        vec![Uuid::from_u128(1), Uuid::from_u128(3), Uuid::from_u128(2)],
        "one flat oldest-first backlog order; no thread blocks, no loose lane"
    );
}

#[test]
fn without_a_thread_filter_keeps_only_unthreaded_tasks() {
    let tasks = vec![
        task(
            1,
            "loose newest",
            HumanStatus::Ready,
            project(THIS_REPO),
            30,
        ),
        threaded_task(
            2,
            "threaded",
            HumanStatus::Ready,
            project(THIS_REPO),
            20,
            "release",
        ),
    ];

    let view = query_board(
        &tasks,
        &BTreeSet::new(),
        Some(Path::new(THIS_REPO)),
        BoardLens::Project(Path::new(THIS_REPO)),
        false,
        &ThreadFilter::Without,
    );
    assert_eq!(
        section_ids(&view, SectionKind::OnDeck),
        vec![Uuid::from_u128(1)]
    );
}

#[test]
fn same_thread_name_joins_only_in_the_global_view_not_the_local_filter() {
    let tasks = vec![
        threaded_task(
            1,
            "project",
            HumanStatus::Ready,
            project(THIS_REPO),
            20,
            "release",
        ),
        threaded_task(
            2,
            "global",
            HumanStatus::Ready,
            TaskScope::Global,
            30,
            "release",
        ),
    ];

    // Local filter: only the project's own match.
    let local = query_board(
        &tasks,
        &BTreeSet::new(),
        Some(Path::new(THIS_REPO)),
        BoardLens::Project(Path::new(THIS_REPO)),
        false,
        &ThreadFilter::Named("release".to_string()),
    );
    assert_eq!(
        section_ids(&local, SectionKind::OnDeck),
        vec![Uuid::from_u128(1)]
    );

    // Global View: both scopes join under one thread name.
    let view = query_board(
        &tasks,
        &BTreeSet::new(),
        None,
        BoardLens::ThreadView("release"),
        false,
        &ThreadFilter::All,
    );
    assert_eq!(
        section_ids(&view, SectionKind::OnDeck),
        vec![Uuid::from_u128(1), Uuid::from_u128(2)],
        "both scopes join under one thread name, oldest created first"
    );
}

#[test]
fn thread_view_covers_needs_you_motion_deck_and_drawer() {
    let tasks = vec![
        threaded_task(
            1,
            "blocked",
            HumanStatus::Blocked,
            project(THIS_REPO),
            10,
            "release",
        ),
        threaded_task(
            2,
            "review",
            HumanStatus::Review,
            project(THIS_REPO),
            15,
            "release",
        ),
        threaded_task(
            3,
            "motion",
            HumanStatus::Started,
            project(THIS_REPO),
            20,
            "release",
        ),
        threaded_task(
            4,
            "deck",
            HumanStatus::Ready,
            project(THIS_REPO),
            25,
            "release",
        ),
        threaded_task(
            5,
            "done",
            HumanStatus::Done,
            project(THIS_REPO),
            30,
            "release",
        ),
    ];

    let view = query_board(
        &tasks,
        &BTreeSet::new(),
        None,
        BoardLens::ThreadView("release"),
        true,
        &ThreadFilter::All,
    );
    assert_eq!(
        section_ids(&view, SectionKind::NeedsYou),
        vec![Uuid::from_u128(2), Uuid::from_u128(1)]
    );
    assert_eq!(
        section_ids(&view, SectionKind::InMotion),
        vec![Uuid::from_u128(3)]
    );
    assert_eq!(
        section_ids(&view, SectionKind::OnDeck),
        vec![Uuid::from_u128(4)]
    );
    assert_eq!(
        section_ids(&view, SectionKind::Done),
        vec![Uuid::from_u128(5)]
    );
}

#[test]
fn projects_index_rows_carry_open_work_counts() {
    let tasks = vec![
        threaded_task(
            1,
            "blocked",
            HumanStatus::Blocked,
            project(THIS_REPO),
            10,
            "release",
        ),
        threaded_task(
            2,
            "motion",
            HumanStatus::Started,
            project(THIS_REPO),
            20,
            "release",
        ),
        threaded_task(
            3,
            "deck",
            HumanStatus::Ready,
            project(THIS_REPO),
            25,
            "release",
        ),
        threaded_task(
            4,
            "done",
            HumanStatus::Done,
            project(THIS_REPO),
            30,
            "release",
        ),
    ];

    let view = query_board(
        &tasks,
        &BTreeSet::new(),
        Some(Path::new(THIS_REPO)),
        BoardLens::Projects,
        false,
        &ThreadFilter::All,
    );
    assert_eq!(view.sections, Vec::new(), "the index never lists tasks");
    assert_eq!(view.projects.len(), 1);
    let row = &view.projects[0];
    assert_eq!(row.path, THIS_REPO);
    assert_eq!(row.needs_you, 1);
    assert_eq!(row.in_motion, 1);
    assert_eq!(row.on_deck, 1, "ready and open tasks share ON DECK");
    assert_eq!(row.done, 1, "live done tasks have their own count");
    assert_eq!(
        row.threads,
        vec!["release".to_string()],
        "one distinct thread across open tasks"
    );
    assert!(row.current, "the invocation project is marked current");
}

#[test]
fn selection_stays_on_task_id_across_motion_reorder() {
    let mut alpha_task = task(1, "alpha", HumanStatus::Started, project(THIS_REPO), 10);
    alpha_task.history.push(TaskEvent {
        kind: TaskEventKind::StatusSet,
        at: epoch_plus(100),
    });
    let alpha = alpha_task.id;
    let mut beta_task = task(2, "beta", HumanStatus::Started, project(THIS_REPO), 20);
    beta_task.history.push(TaskEvent {
        kind: TaskEventKind::StatusSet,
        at: epoch_plus(200),
    });
    let beta = beta_task.id;
    let mut domain = domain_with_tasks(vec![alpha_task.clone(), beta_task.clone()]);
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));

    let before = model.queue_view();
    assert_eq!(
        section_ids(&before, SectionKind::InMotion),
        vec![beta, alpha]
    );

    let beta_index = model
        .visible_ids()
        .iter()
        .position(|id| *id == beta)
        .expect("beta is visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(beta_index),
        None,
    )
    .unwrap();

    // A fresh status change on alpha moves it above beta; selection stays pinned
    // to beta's id, not to beta's row position.
    alpha_task.history.push(TaskEvent {
        kind: TaskEventKind::StatusSet,
        at: epoch_plus(300),
    });
    alpha_task.updated_at = epoch_plus(300);
    let reordered = domain_with_tasks(vec![alpha_task, beta_task]);
    model.sync_from_domain(&reordered);

    let after = model.queue_view();
    assert_eq!(
        section_ids(&after, SectionKind::InMotion),
        vec![alpha, beta],
        "the newest status change leads IN MOTION"
    );
    assert_eq!(model.selected_id(), Some(beta));
    assert_eq!(
        model.visible_ids(),
        section_ids(&after, SectionKind::InMotion)
    );
    assert_eq!(model.selected_index(), Some(1));
    assert_eq!(
        section_ids(&after, SectionKind::InMotion)[model.selected_index().unwrap()],
        beta
    );
}

/// Two fresh models from the same store share no UI state and write no UI-state files.
fn board_rows(model: &BoardModel, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn arrow_navigation_crosses_painted_header_task_to_task() {
    let mut domain = DomainState::new();
    let review = domain
        .create(
            "review task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create review");
    domain
        .set_status(review, HumanStatus::Review)
        .expect("review");
    let ready = domain
        .create(
            "ready task",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create ready");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    let ids = model.visible_ids();
    assert_eq!(
        ids,
        vec![review, INBOX_HEADER_ROW_ID, ready],
        "needs-you first, then on deck and its inbox heading"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::SelectIndex(0), None)
        .expect("select first task");

    let rows = board_rows(&model, 80, 24);
    let first_y = rows
        .iter()
        .position(|row| row.contains("review task"))
        .expect("first task paints");
    let header_y = rows
        .iter()
        .position(|row| row.contains("NEEDS YOU"))
        .expect("the NEEDS YOU header paints");
    let second_y = rows
        .iter()
        .position(|row| row.contains("ready task"))
        .expect("second task paints");
    assert!(
        header_y < first_y && first_y < second_y,
        "a painted header must physically sit above its tasks:\n{}",
        rows.join("\n")
    );

    apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None)
        .expect("arrow navigation moves to the inbox heading");
    assert_eq!(
        model.selected_id(),
        None,
        "the selected inbox heading is chrome, not a task"
    );
    apply_intent(&mut domain, &mut model, BoardIntent::SelectNext, None)
        .expect("arrow navigation crosses the inbox heading");
    assert_eq!(
        model.selected_id(),
        Some(ready),
        "selection must land on the task below the inbox heading"
    );
}

#[test]
fn two_fresh_models_from_same_store_share_no_ui_state_and_no_ui_writes_under_state_or_config_dirs()
{
    let state_dir = temp_dir("state");
    let config_dir = temp_dir("config");
    let _g1 = TempGuard(state_dir.clone());
    let _g2 = TempGuard(config_dir.clone());

    let store = TaskStore::new(&state_dir);
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "shared",
            None,
            project(THIS_REPO),
            ProvenanceOrigin::Manual,
            None,
        )
        .unwrap();
    store.save(&domain).expect("save domain");

    let before_state = list_files_recursive(&state_dir);
    let before_config = list_files_recursive(&config_dir);

    let mut a = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let b = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(a.selected_id(), Some(id));
    assert_eq!(b.selected_id(), Some(id));

    // Mutate session-only UI on A; a fresh B must not inherit it.
    a.set_message("only on A");
    let _ = apply_intent(&mut domain, &mut a, BoardIntent::ToggleDoneDrawer, None);

    let b_fresh = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(
        b_fresh.message(),
        None,
        "fresh model has no carried message"
    );
    assert_eq!(
        b_fresh.selected_id(),
        Some(id),
        "fresh model re-seeds; does not inherit A's session UI"
    );
    assert_eq!(a.message(), Some("only on A"));
    assert!(a.drawer_open(), "A's drawer mutation stays in this session");
    assert!(
        !b_fresh.drawer_open(),
        "fresh model starts with drawer closed"
    );
    assert_eq!(
        b_fresh.selected_project(),
        Some(Path::new(THIS_REPO)),
        "fresh model starts on the invocation project's board"
    );
    assert_eq!(
        b_fresh.visible_ids(),
        vec![INBOX_HEADER_ROW_ID, id],
        "the startup board renders that project's rows and inbox heading"
    );

    let after_state = list_files_recursive(&state_dir);
    let after_config = list_files_recursive(&config_dir);
    assert_eq!(
        before_state, after_state,
        "BoardModel must not write UI-state files under the task store dir"
    );
    assert_eq!(
        before_config, after_config,
        "BoardModel must not write UI-state files under the config dir"
    );
}
