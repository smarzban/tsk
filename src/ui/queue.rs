//! Queue Section Query: pure derivation of the board sections from a task snapshot.

use std::collections::BTreeSet;
use std::path::Path;

use uuid::Uuid;

use crate::domain::{HumanStatus, Task, TaskScope};
use crate::scope::PathIdentityCache;

/// The board's persistent navigation destinations.
///
/// Tabs never disappear and their digit meanings never shift: `1` desk, `2` the
/// selected project's board, `3` the projects index. Slot 2 with no selected project
/// opens the project picker instead of changing meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavTab {
    #[default]
    Desk,
    ProjectBoard,
    Projects,
}

/// The two tab lenses reachable directly by digit. Slot 2 is a [`NavTab::ProjectBoard`],
/// carried by the model's selected project rather than by this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoardTab {
    #[default]
    Desk,
    Projects,
}

impl From<BoardTab> for NavTab {
    fn from(tab: BoardTab) -> Self {
        match tab {
            BoardTab::Desk => NavTab::Desk,
            BoardTab::Projects => NavTab::Projects,
        }
    }
}

/// Which list region a section belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// Blocked and review tasks that need a human.
    NeedsYou,
    InMotion,
    OnDeck,
    /// Open tasks under ON DECK (header + dim rows, collapsible).
    Inbox,
    Done,
    /// The done drawer's archived group (header + dim rows, collapsible).
    Archived,
}

/// Row id the archived group's header occupies in `visible_task_ids`.
/// Chrome, not a task: `BoardModel::selected_id()` hides it from verbs.
pub const ARCHIVED_HEADER_ROW_ID: Uuid = Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_0000_0000_0000_0001);

/// Row id the inbox group's header occupies in `visible_task_ids`.
/// Chrome, not a task: `BoardModel::selected_id()` hides it from verbs.
pub const INBOX_HEADER_ROW_ID: Uuid = Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_0000_0000_0000_0002);

/// Session thread filter for the project board. It narrows every status section,
/// done drawer included; `All` is the unfiltered board.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ThreadFilter {
    #[default]
    All,
    /// Only tasks whose normalized thread matches this name (ASCII-case-insensitive).
    Named(String),
    /// Only tasks with no thread at all.
    Without,
}

impl ThreadFilter {
    /// Whether `task` passes this filter.
    fn admits(&self, task: &Task) -> bool {
        match self {
            ThreadFilter::All => true,
            ThreadFilter::Named(name) => task
                .thread
                .as_deref()
                .is_some_and(|thread| thread.eq_ignore_ascii_case(name)),
            ThreadFilter::Without => task.thread.is_none(),
        }
    }

    /// The label the board's thread control paints for this filter.
    pub fn label(&self) -> String {
        match self {
            ThreadFilter::All => "all".to_string(),
            ThreadFilter::Named(name) => format!("#{name}"),
            ThreadFilter::Without => "without a thread".to_string(),
        }
    }
}

/// Normalized content-search terms. Every term must match, but each may match a
/// different task field.
pub fn search_words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether a task matches every content-search term in its title, notes, steps,
/// thread, or painted task number. Search is Unicode-case-insensitive substring.
pub fn task_matches(task: &Task, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let mut searchable = task.title.to_lowercase();
    if let Some(notes) = task.notes.as_deref() {
        searchable.push('\n');
        searchable.push_str(&notes.to_lowercase());
    }
    for step in &task.steps {
        searchable.push('\n');
        searchable.push_str(&step.text.to_lowercase());
    }
    if let Some(thread) = task.thread.as_deref() {
        searchable.push('\n');
        searchable.push_str(&thread.to_lowercase());
    }
    if let Some(identifier) = task.board_identifier() {
        searchable.push('\n');
        searchable.push_str(&identifier.to_lowercase());
    }
    words.iter().all(|word| searchable.contains(word))
}

/// How the board query is scoped for one paint/selection pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardLens<'a> {
    Desk,
    Projects,
    Project(&'a Path),
    /// Flat cross-project board for one thread name (the projects index's thread View).
    ThreadView(&'a str),
    /// Read-only focus on an archived project (AC-41): the same shape as `Project`, but
    /// the project's archived state does not hide its tasks.
    ArchivedProject(&'a Path),
}

/// One selectable row of the projects index: a project and its open-work counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    /// Stored project scope path.
    pub path: String,
    /// Blocked + review count (live, open tasks only).
    pub needs_you: usize,
    /// Started count.
    pub in_motion: usize,
    /// Ready plus open tasks, matching the project's ON DECK section and inbox.
    pub on_deck: usize,
    /// Live completed tasks, excluding archived tasks and archived projects.
    pub done: usize,
    /// Distinct thread names on this project's open tasks, ordered by the most
    /// recently status-changed task first. Painted as the wide-width THREADS column.
    pub threads: Vec<String>,
    /// True when this is the invocation directory's project.
    pub current: bool,
}

/// One ordered section of the queue board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSection {
    pub kind: SectionKind,
    /// Project path for a project-group section. `None` for every status section on
    /// the desk, project board, thread view, and drawer.
    pub project_label: Option<String>,
    /// Exact flattening for task-only consumers and selection.
    pub task_ids: Vec<Uuid>,
    pub count: usize,
    /// True when a section has zero tasks (renderer paints the empty hint).
    pub empty_hint: bool,
}

/// Status-line counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusCounts {
    pub in_motion: usize,
    pub done: usize,
    pub need: usize,
}

/// Ordered sections plus status-line counts for one snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueView {
    pub sections: Vec<QueueSection>,
    pub counts: StatusCounts,
    /// The projects index rows, present only for the Projects lens.
    pub projects: Vec<ProjectRow>,
}

/// Derive queue sections for the active board lens.
pub fn query_lens(
    tasks: &[Task],
    current_repo: Option<&Path>,
    lens: BoardLens<'_>,
    drawer_open: bool,
) -> QueueView {
    query_board(
        tasks,
        &BTreeSet::new(),
        current_repo,
        lens,
        drawer_open,
        &ThreadFilter::All,
    )
}

/// Derive queue sections with an archived-project set. A task is hidden when it is
/// soft-deleted, archived, or belongs to an archived project: no working lens paints it.
/// `thread_filter` narrows the project board across every status, drawer included.
pub fn query_board(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    current_repo: Option<&Path>,
    lens: BoardLens<'_>,
    drawer_open: bool,
    thread_filter: &ThreadFilter,
) -> QueueView {
    query_board_search(
        tasks,
        archived_projects,
        current_repo,
        lens,
        drawer_open,
        thread_filter,
        "",
    )
}

/// Derive queue sections with the same lens rules as [`query_board`], then narrow
/// every task pool by the content query. Project-index rows are filtered separately
/// by the model because they match project names and paths, not task content.
pub fn query_board_search(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    current_repo: Option<&Path>,
    lens: BoardLens<'_>,
    drawer_open: bool,
    thread_filter: &ThreadFilter,
    query: &str,
) -> QueueView {
    let identities = PathIdentityCache::default();
    let words = search_words(query);
    let mut view = match lens {
        BoardLens::Desk => query_desk(tasks, archived_projects, drawer_open, &identities, &words),
        BoardLens::Projects => {
            query_projects_index(tasks, archived_projects, current_repo, &identities)
        }
        BoardLens::Project(path) => query_project_focus(
            tasks,
            archived_projects,
            path,
            drawer_open,
            thread_filter,
            &identities,
            &words,
        ),
        BoardLens::ThreadView(name) => query_thread_view(
            tasks,
            archived_projects,
            name,
            drawer_open,
            &identities,
            &words,
        ),
        // AC-41/AC-45: the read-only focus is the only lens that paints an archived
        // project's tasks. It is the project focus computed as if the project were live.
        BoardLens::ArchivedProject(path) => query_project_focus(
            tasks,
            &BTreeSet::new(),
            path,
            drawer_open,
            &ThreadFilter::All,
            &identities,
            &words,
        ),
    };
    if !words.is_empty() && !matches!(lens, BoardLens::Projects) {
        view.sections.retain(|section| !section.empty_hint);
    }
    view
}

/// A task survives the working-lens filter: not soft-deleted, not archived, and no
/// archived project owns its scope.
fn is_live(
    task: &Task,
    archived_projects: &BTreeSet<String>,
    identities: &PathIdentityCache,
) -> bool {
    if task.soft_deleted || task.archived {
        return false;
    }
    match &task.scope {
        TaskScope::Global => true,
        TaskScope::Project { path } => !identities.contains(archived_projects, path),
    }
}

/// Whether an archived project owns the task's scope (its own flag is irrelevant here).
fn task_owned_by_archived_project(
    task: &Task,
    archived_projects: &BTreeSet<String>,
    identities: &PathIdentityCache,
) -> bool {
    match &task.scope {
        TaskScope::Global => false,
        TaskScope::Project { path } => identities.contains(archived_projects, path),
    }
}

/// Task ids visible for selection, honoring inbox and archived group collapse.
pub fn visible_task_ids(
    view: &QueueView,
    archived_collapsed: bool,
    inbox_collapsed: bool,
) -> Vec<Uuid> {
    let mut out = Vec::new();
    for section in &view.sections {
        match section.kind {
            SectionKind::Archived => {
                // The archived header is always selectable so Enter can toggle it;
                // its rows paint only when the group is expanded.
                out.push(ARCHIVED_HEADER_ROW_ID);
                if !archived_collapsed {
                    out.extend(section.task_ids.iter().copied());
                }
            }
            SectionKind::Inbox => {
                out.push(INBOX_HEADER_ROW_ID);
                if !inbox_collapsed {
                    out.extend(section.task_ids.iter().copied());
                }
            }
            _ => out.extend(section.task_ids.iter().copied()),
        }
    }
    out
}

fn query_desk(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    drawer_open: bool,
    identities: &PathIdentityCache,
    search: &[String],
) -> QueueView {
    let live: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            is_live(task, archived_projects, identities)
                && (task_matches(task, search)
                    || (!drawer_open && task.status == HumanStatus::Done))
        })
        .collect();
    let archived_pool: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            !task_owned_by_archived_project(task, archived_projects, identities)
                && (!drawer_open || task_matches(task, search))
        })
        .collect();

    // NEEDS YOU is the global attention lane: blocked and review across every live
    // scope, desk included, with project attribution painted beside each row.
    let mut need: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_needs_you_status(t.status))
        .collect();
    sort_by_status_change_desc(&mut need);

    let mut motion: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| t.status == HumanStatus::Started)
        .collect();
    sort_by_status_change_desc(&mut motion);

    // ON DECK · desk is the personal backlog: desk-scope ready and open tasks only.
    // Project backlogs live on their project boards, never here.
    let deck: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_on_deck_status(t.status) && matches!(t.scope, TaskScope::Global))
        .collect();

    let mut sections = Vec::new();
    push_needs_you(&mut sections, &need);
    if !motion.is_empty() {
        sections.push(section_from(SectionKind::InMotion, None, &motion));
    }
    push_deck(&mut sections, None, &deck, &need);

    append_done(&mut sections, &live, drawer_open);
    append_archived(&mut sections, &archived_pool, drawer_open);

    QueueView {
        sections,
        counts: status_counts(&live),
        projects: Vec::new(),
    }
}

/// Per-project tallies gathered while building the index.
#[derive(Debug, Clone)]
struct ProjectCounts {
    needs_you: usize,
    in_motion: usize,
    on_deck: usize,
    done: usize,
    threads: Vec<String>,
}

/// The projects index: one selectable row per live project with open-work counts.
/// This is navigation, not a task list: no expanded task sections.
fn query_projects_index(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    current_repo: Option<&Path>,
    identities: &PathIdentityCache,
) -> QueueView {
    let live: Vec<&Task> = tasks
        .iter()
        .filter(|task| is_live(task, archived_projects, identities))
        .collect();

    let mut paths: Vec<String> = Vec::new();
    for task in &live {
        if let TaskScope::Project { path } = &task.scope {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
    }
    paths.sort();
    // The invocation directory's project is listed even when it has no tasks yet, so
    // opening tsk inside an empty repo still offers its board.
    if let Some(repo) = current_repo {
        let repo_string = repo.to_string_lossy().into_owned();
        if !identities.contains(archived_projects, &repo_string)
            && !paths
                .iter()
                .any(|path| identities.equivalent(path, &repo_string))
        {
            paths.push(repo_string);
            paths.sort();
        }
    }

    let counts_of = |paths: &[String]| -> Vec<ProjectCounts> {
        paths
            .iter()
            .map(|path| {
                let owned = |task: &&Task| {
                    matches!(&task.scope, TaskScope::Project { path: p } if identities.equivalent(p, path))
                };
                let mut owned_tasks: Vec<&Task> =
                    live.iter().copied().filter(|task| owned(task)).collect();
                sort_by_status_change_desc(&mut owned_tasks);
                // Insertion order is status-change recency; the set only guards uniqueness.
                let mut seen: BTreeSet<&str> = BTreeSet::new();
                let threads: Vec<String> = owned_tasks
                    .iter()
                    .filter(|task| task.status != HumanStatus::Done)
                    .filter_map(|task| task.thread.as_deref())
                    .filter(|thread| seen.insert(thread))
                    .map(str::to_string)
                    .collect();
                ProjectCounts {
                    needs_you: owned_tasks
                        .iter()
                        .filter(|task| is_needs_you_status(task.status))
                        .count(),
                    in_motion: owned_tasks
                        .iter()
                        .filter(|task| task.status == HumanStatus::Started)
                        .count(),
                    on_deck: owned_tasks
                        .iter()
                        .filter(|task| is_on_deck_status(task.status))
                        .count(),
                    done: owned_tasks
                        .iter()
                        .filter(|task| task.status == HumanStatus::Done)
                        .count(),
                    threads,
                }
            })
            .collect()
    };
    let counts = counts_of(&paths);

    // Current directory first, then alphabetical.
    let mut order: Vec<usize> = (0..paths.len()).collect();
    order.sort_by_key(|&index| {
        (
            usize::from(current_repo.is_some_and(|repo| {
                !identities.equivalent(&paths[index], &repo.to_string_lossy())
            })),
            paths[index].clone(),
        )
    });

    let projects = order
        .iter()
        .map(|index| {
            let ProjectCounts {
                needs_you,
                in_motion,
                on_deck,
                done,
                threads,
            } = counts[*index].clone();
            ProjectRow {
                path: paths[*index].clone(),
                needs_you,
                in_motion,
                on_deck,
                done,
                threads,
                current: current_repo.is_some_and(|repo| {
                    identities.equivalent(&paths[*index], &repo.to_string_lossy())
                }),
            }
        })
        .collect();

    QueueView {
        sections: Vec::new(),
        counts: status_counts(&live),
        projects,
    }
}

/// Flat cross-project board for one thread name: matching tasks of every live scope
/// grouped by status only, with project attribution painted beside each row. Desk
/// tasks keep their desk ownership; nothing moves between projects.
fn query_thread_view(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    name: &str,
    drawer_open: bool,
    identities: &PathIdentityCache,
    search: &[String],
) -> QueueView {
    let matches_thread = |task: &Task| {
        task.thread
            .as_deref()
            .is_some_and(|thread| thread.eq_ignore_ascii_case(name))
    };
    let live: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            is_live(task, archived_projects, identities)
                && matches_thread(task)
                && (task_matches(task, search)
                    || (!drawer_open && task.status == HumanStatus::Done))
        })
        .collect();
    let archived_pool: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            !task_owned_by_archived_project(task, archived_projects, identities)
                && matches_thread(task)
                && (!drawer_open || task_matches(task, search))
        })
        .collect();

    let mut need: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_needs_you_status(t.status))
        .collect();
    sort_by_status_change_desc(&mut need);
    let mut motion: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| t.status == HumanStatus::Started)
        .collect();
    sort_by_status_change_desc(&mut motion);
    let deck: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| is_on_deck_status(t.status))
        .collect();

    let mut sections = Vec::new();
    push_needs_you(&mut sections, &need);
    if !motion.is_empty() {
        sections.push(section_from(SectionKind::InMotion, None, &motion));
    }
    push_deck(&mut sections, None, &deck, &need);

    append_done(&mut sections, &live, drawer_open);
    append_archived(&mut sections, &archived_pool, drawer_open);

    QueueView {
        sections,
        counts: status_counts(&live),
        projects: Vec::new(),
    }
}

fn query_project_focus(
    tasks: &[Task],
    archived_projects: &BTreeSet<String>,
    path: &Path,
    drawer_open: bool,
    thread_filter: &ThreadFilter,
    identities: &PathIdentityCache,
    search: &[String],
) -> QueueView {
    let live: Vec<&Task> = tasks
        .iter()
        .filter(|task| {
            is_live(task, archived_projects, identities)
                && task_matches_scope(task, path, identities)
                && (task_matches(task, search)
                    || (!drawer_open && task.status == HumanStatus::Done))
        })
        .collect();
    let admits = |task: &Task| thread_filter.admits(task);

    let mut motion: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| t.status == HumanStatus::Started && admits(t))
        .collect();
    sort_by_status_change_desc(&mut motion);

    // `pending` carries both NEEDS YOU and ON DECK rows, unsorted: NEEDS YOU is
    // re-sorted to status-change recency after the split and `push_deck` orders
    // its own ready and inbox halves.
    let pending: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| !matches!(t.status, HumanStatus::Started | HumanStatus::Done) && admits(t))
        .collect();
    let (mut need, ready) = split_needs_you(&pending);
    sort_by_status_change_desc(&mut need);

    let label = live
        .iter()
        .find_map(|task| match &task.scope {
            TaskScope::Project { path: stored }
                if identities.equivalent(stored, &path.to_string_lossy()) =>
            {
                Some(stored.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| path.to_string_lossy().into_owned());

    let mut sections = Vec::new();
    push_needs_you(&mut sections, &need);
    if !motion.is_empty() {
        sections.push(section_from(SectionKind::InMotion, None, &motion));
    }
    push_deck(&mut sections, Some(label.clone()), &ready, &need);

    let done_pool: Vec<&Task> = live.iter().copied().filter(|t| admits(t)).collect();
    append_done(&mut sections, &done_pool, drawer_open);
    let in_scope: Vec<&Task> = tasks
        .iter()
        .filter(|task| task_matches_scope(task, path, identities))
        .filter(|task| !task_owned_by_archived_project(task, archived_projects, identities))
        .filter(|task| {
            let belongs_to_closed_drawer = task.status == HumanStatus::Done || task.archived;
            admits(task)
                && (task_matches(task, search) || (!drawer_open && belongs_to_closed_drawer))
        })
        .collect();
    append_archived(&mut sections, &in_scope, drawer_open);

    // An empty board (or a filter that hides everything) keeps one hinted section, so
    // the project board always answers with an add affordance instead of a bare pane.
    if !sections.iter().any(|section| {
        !matches!(
            section.kind,
            SectionKind::Done | SectionKind::Archived | SectionKind::Inbox
        )
    }) {
        sections.push(QueueSection {
            kind: SectionKind::OnDeck,
            project_label: Some(label),
            task_ids: Vec::new(),
            count: 0,
            empty_hint: true,
        });
    }

    // Status counts come from the filtered set, so the chrome never advertises work
    // the active thread filter is hiding.
    let counted: Vec<&Task> = live.iter().copied().filter(|t| admits(t)).collect();
    QueueView {
        sections,
        counts: status_counts(&counted),
        projects: Vec::new(),
    }
}

fn append_done(sections: &mut Vec<QueueSection>, live: &[&Task], drawer_open: bool) {
    if !drawer_open {
        return;
    }
    let mut done: Vec<&Task> = live
        .iter()
        .copied()
        .filter(|t| t.status == HumanStatus::Done)
        .collect();
    sort_by_status_change_desc(&mut done);
    if !done.is_empty() {
        sections.push(section_from(SectionKind::Done, None, &done));
    }
}

/// The done drawer's archived group: individually archived tasks the drawer's scope
/// would otherwise show (not soft-deleted, not owned by an archived project), newest
/// status change first. Paints only while the drawer is open, and only when non-empty.
fn append_archived(sections: &mut Vec<QueueSection>, in_scope: &[&Task], drawer_open: bool) {
    if !drawer_open {
        return;
    }
    let mut archived: Vec<&Task> = in_scope
        .iter()
        .copied()
        .filter(|task| task.archived && !task.soft_deleted)
        .collect();
    sort_by_status_change_desc(&mut archived);
    if !archived.is_empty() {
        sections.push(section_from(SectionKind::Archived, None, &archived));
    }
}

fn status_counts(live: &[&Task]) -> StatusCounts {
    let in_motion = live
        .iter()
        .filter(|t| t.status == HumanStatus::Started)
        .count();
    let done = live
        .iter()
        .filter(|t| t.status == HumanStatus::Done)
        .count();
    let need = live
        .iter()
        .filter(|t| is_needs_you_status(t.status))
        .count();
    StatusCounts {
        in_motion,
        done,
        need,
    }
}

fn is_needs_you_status(status: HumanStatus) -> bool {
    matches!(status, HumanStatus::Blocked | HumanStatus::Review)
}

fn is_on_deck_status(status: HumanStatus) -> bool {
    matches!(status, HumanStatus::Ready | HumanStatus::Open)
}

fn split_needs_you<'a>(tasks: &[&'a Task]) -> (Vec<&'a Task>, Vec<&'a Task>) {
    let mut need = Vec::new();
    let mut rest = Vec::new();
    for task in tasks {
        if is_needs_you_status(task.status) {
            need.push(*task);
        } else {
            rest.push(*task);
        }
    }
    (need, rest)
}

fn push_needs_you(sections: &mut Vec<QueueSection>, need: &[&Task]) {
    if !need.is_empty() {
        sections.push(section_from(SectionKind::NeedsYou, None, need));
    }
}

/// Keep an empty desk/ON DECK header only when NEEDS YOU is also empty.
/// Ready rows sit on ON DECK; open rows follow under a collapsible inbox heading.
fn push_deck(
    sections: &mut Vec<QueueSection>,
    project_label: Option<String>,
    deck: &[&Task],
    need: &[&Task],
) {
    let mut ready: Vec<&Task> = deck
        .iter()
        .copied()
        .filter(|task| task.status == HumanStatus::Ready)
        .collect();
    let mut inbox: Vec<&Task> = deck
        .iter()
        .copied()
        .filter(|task| task.status == HumanStatus::Open)
        .collect();
    sort_ready_by_pick_asc(&mut ready);
    sort_by_created_asc(&mut inbox);
    if ready.is_empty() && inbox.is_empty() && !need.is_empty() {
        return;
    }
    let count = ready.len() + inbox.len();
    sections.push(QueueSection {
        kind: SectionKind::OnDeck,
        project_label,
        task_ids: ready.iter().map(|task| task.id).collect(),
        count,
        empty_hint: count == 0,
    });
    if !inbox.is_empty() {
        sections.push(section_from(SectionKind::Inbox, None, &inbox));
    }
}

/// Whether a task belongs to the project at `path`, tolerating the same directory
/// reached through different path spellings (symlinks, `/tmp` vs `/private/tmp`).
fn task_matches_scope(task: &Task, path: &Path, identities: &PathIdentityCache) -> bool {
    match &task.scope {
        TaskScope::Project { path: stored } => {
            identities.equivalent(stored, &path.to_string_lossy())
        }
        TaskScope::Global => false,
    }
}

/// Section ordering rule. NEEDS YOU, IN MOTION, DONE and ARCHIVED put the most
/// recent status change first (`status_changed_at`: the last `StatusSet`,
/// `Completed` or `Reopened` event, falling back to `created_at`). Ready rows on
/// ON DECK are oldest pick first (`status_changed_at` ascending). Inbox (`open`)
/// stays FIFO by `created_at`. Plain edits, step changes, archive and restore
/// never reorder a section. Ties break by `created_at` then `id` so the order is
/// total and stable across reloads.
fn sort_by_status_change_desc(tasks: &mut [&Task]) {
    tasks.sort_by(|a, b| {
        b.status_changed_at()
            .cmp(&a.status_changed_at())
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Backlogs are FIFO: oldest capture first, ties by id. Notice rows (the starter tour,
/// release notes) lead regardless: they are seeded once and would otherwise sink under an
/// existing user's backlog, unseen.
fn sort_by_created_asc(tasks: &mut [&Task]) {
    tasks.sort_by(|a, b| {
        b.is_notice()
            .cmp(&a.is_notice())
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Ready picks: oldest `status_changed_at` first. Notices still lead.
fn sort_ready_by_pick_asc(tasks: &mut [&Task]) {
    tasks.sort_by(|a, b| {
        b.is_notice()
            .cmp(&a.is_notice())
            .then_with(|| a.status_changed_at().cmp(&b.status_changed_at()))
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn section_from(kind: SectionKind, project_label: Option<String>, tasks: &[&Task]) -> QueueSection {
    let task_ids: Vec<Uuid> = tasks.iter().map(|t| t.id).collect();
    let count = task_ids.len();
    QueueSection {
        kind,
        project_label,
        task_ids,
        count,
        empty_hint: false,
    }
}

/// Basename of a project path, for labels and search.
pub fn short_project_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .find(|component| !component.is_empty())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    use crate::domain::{HumanStatus, ProvenanceOrigin, Step, TaskEvent, TaskEventKind, TaskScope};

    fn task(
        id: u128,
        status: HumanStatus,
        scope: TaskScope,
        soft_deleted: bool,
        updated_secs: u64,
    ) -> Task {
        task_with_thread(id, status, scope, soft_deleted, updated_secs, None)
    }

    fn task_with_thread(
        id: u128,
        status: HumanStatus,
        scope: TaskScope,
        soft_deleted: bool,
        updated_secs: u64,
        thread: Option<&str>,
    ) -> Task {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(updated_secs);
        Task {
            id: Uuid::from_u128(id),
            number: None,
            notice: None,
            revision: Uuid::from_u128(id),
            merge_base_revision: None,
            title: format!("task-{id}"),
            notes: None,
            thread: thread.map(str::to_string),
            assignee: None,
            status,
            scope,
            provenance: ProvenanceOrigin::Manual,
            history: vec![TaskEvent {
                kind: TaskEventKind::Created,
                at,
            }],
            steps: Vec::new(),
            soft_deleted,
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

    /// Appends a status-change event at `at_secs`, the only history kind that may
    /// reorder a section.
    fn with_status_event(task: Task, kind: TaskEventKind, at_secs: u64) -> Task {
        let mut task = task;
        task.history.push(TaskEvent {
            kind,
            at: SystemTime::UNIX_EPOCH + Duration::from_secs(at_secs),
        });
        task
    }

    /// Simulates `record_mutation` for a non-status mutation (edit, step tick):
    /// `updated_at` moves and a non-status event lands, but the task's
    /// status-change time does not.
    fn mutated(task: Task, kind: TaskEventKind, at_secs: u64) -> Task {
        let mut task = task;
        task.updated_at = SystemTime::UNIX_EPOCH + Duration::from_secs(at_secs);
        task.history.push(TaskEvent {
            kind,
            at: task.updated_at,
        });
        task
    }

    fn ids(section: &QueueSection) -> Vec<Uuid> {
        section.task_ids.clone()
    }

    #[test]
    fn task_search_requires_every_word_across_content_fields_and_number() {
        let mut candidate = task_with_thread(
            52,
            HumanStatus::Started,
            project("/repos/app"),
            false,
            10,
            Some("auth"),
        );
        candidate.title = "Ship Login flow".into();
        candidate.notes = Some("Handle OAuth callback".into());
        candidate.steps = vec![Step {
            id: Uuid::from_u128(99),
            text: "Rotate TOKEN".into(),
            done: false,
        }];
        candidate.number = Some(52);

        let words = search_words("LOGIN callback token AUTH t52");
        assert!(task_matches(&candidate, &words));
        assert!(!task_matches(&candidate, &search_words("login billing")));
    }

    #[test]
    fn task_search_combines_with_project_and_thread_lenses() {
        let mut project_auth = task_with_thread(
            1,
            HumanStatus::Started,
            project("/repos/app"),
            false,
            30,
            Some("auth"),
        );
        project_auth.title = "login callback".into();
        let mut project_api = task_with_thread(
            2,
            HumanStatus::Started,
            project("/repos/app"),
            false,
            20,
            Some("api"),
        );
        project_api.title = "login endpoint".into();
        let mut other_auth = task_with_thread(
            3,
            HumanStatus::Started,
            project("/repos/other"),
            false,
            10,
            Some("auth"),
        );
        other_auth.title = "login session".into();
        let tasks = vec![project_auth, project_api, other_auth];

        let project_view = query_board_search(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Project(Path::new("/repos/app")),
            false,
            &ThreadFilter::Named("auth".into()),
            "login",
        );
        assert_eq!(project_view.sections[0].task_ids, vec![Uuid::from_u128(1)]);

        let thread_view = query_board_search(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::ThreadView("auth"),
            false,
            &ThreadFilter::All,
            "login",
        );
        assert_eq!(
            thread_view.sections[0].task_ids,
            vec![Uuid::from_u128(1), Uuid::from_u128(3)]
        );
    }

    #[test]
    fn task_search_prunes_empty_sections_and_recounts_hidden_done_work() {
        let mut tasks = vec![
            task(1, HumanStatus::Started, TaskScope::Global, false, 40),
            task(2, HumanStatus::Started, TaskScope::Global, false, 30),
            task(3, HumanStatus::Done, TaskScope::Global, false, 20),
            task(4, HumanStatus::Done, TaskScope::Global, false, 10),
        ];
        tasks[0].title = "login motion".into();
        tasks[1].title = "unrelated motion".into();
        tasks[2].title = "login completed".into();
        tasks[3].title = "unrelated completed".into();

        let closed = query_board_search(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Desk,
            false,
            &ThreadFilter::All,
            "login",
        );
        assert_eq!(
            section_ids(&closed, SectionKind::InMotion),
            vec![Uuid::from_u128(1)]
        );
        assert_eq!(closed.counts.in_motion, 1);
        assert_eq!(closed.counts.done, 2, "closed drawer is not searched");
        assert!(
            closed.sections.iter().all(|section| !section.empty_hint),
            "content search hides empty section hints"
        );

        let open = query_board_search(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Desk,
            true,
            &ThreadFilter::All,
            "login",
        );
        assert_eq!(
            section_ids(&open, SectionKind::Done),
            vec![Uuid::from_u128(3)]
        );
    }

    fn section_ids(view: &QueueView, kind: SectionKind) -> Vec<Uuid> {
        view.sections
            .iter()
            .filter(|s| s.kind == kind)
            .flat_map(|s| s.task_ids.iter().copied())
            .collect()
    }

    fn all_listed_ids(view: &QueueView) -> Vec<Uuid> {
        view.sections
            .iter()
            .flat_map(|s| s.task_ids.iter().copied())
            .collect()
    }

    #[test]
    fn archived_group_follows_the_drawer_scope() {
        let mut gone = BTreeSet::new();
        gone.insert("/repos/gone".to_string());
        let mut tasks = vec![
            task(1, HumanStatus::Done, project("/repos/a"), false, 10),
            task(2, HumanStatus::Done, TaskScope::Global, false, 20),
            task(3, HumanStatus::Done, project("/repos/gone"), false, 30),
        ];
        for task in &mut tasks {
            task.archived = true;
        }

        // Desk drawer: every scope's archived tasks, newest status change first. A task whose
        // project is archived stays hidden (task 3).
        let home = query_board(
            &tasks,
            &gone,
            None,
            BoardLens::Desk,
            true,
            &ThreadFilter::All,
        );
        let group = home
            .sections
            .iter()
            .find(|s| s.kind == SectionKind::Archived)
            .expect("archived group at desk");
        assert_eq!(
            ids(group),
            vec![Uuid::from_u128(2), Uuid::from_u128(1)],
            "newest status change first, archived project's task excluded"
        );
        assert_eq!(group.count, 2);

        // Project focus: only that project's archived tasks.
        let focus = query_board(
            &tasks,
            &gone,
            None,
            BoardLens::Project(Path::new("/repos/a")),
            true,
            &ThreadFilter::All,
        );
        let group = focus
            .sections
            .iter()
            .find(|s| s.kind == SectionKind::Archived)
            .expect("archived group in project focus");
        assert_eq!(ids(group), vec![Uuid::from_u128(1)]);
    }

    #[test]
    fn an_archived_only_thread_paints_no_filter_match_anywhere() {
        let mut tasks = vec![task_with_thread(
            1,
            HumanStatus::Ready,
            project("/repos/a"),
            false,
            30,
            Some("release"),
        )];
        tasks[0].archived = true;

        // Project board: the thread's only open task is archived, so the filter
        // offers no match and the deck shows the empty hint instead.
        let deck = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Project(Path::new("/repos/a")),
            false,
            &ThreadFilter::Named("release".to_string()),
        );
        let on_deck = deck
            .sections
            .iter()
            .find(|s| s.kind == SectionKind::OnDeck)
            .expect("project board on deck");
        assert!(
            on_deck.empty_hint,
            "an archived-only thread paints the hint"
        );

        // Thread view: no sections for the thread at all beyond the hint.
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::ThreadView("release"),
            false,
            &ThreadFilter::All,
        );
        assert!(
            !view.sections.iter().any(|s| !s.empty_hint),
            "an archived-only thread must not paint on the thread view"
        );
    }

    #[test]
    fn archived_project_hides_its_tasks_from_index_threadview_and_desk() {
        let mut archived = BTreeSet::new();
        archived.insert("/repos/gone".to_string());
        let tasks = vec![
            task(1, HumanStatus::Started, project("/repos/gone"), false, 50),
            task(2, HumanStatus::Ready, project("/repos/gone"), false, 40),
            task_with_thread(
                3,
                HumanStatus::Ready,
                project("/repos/gone"),
                false,
                30,
                Some("release"),
            ),
            task(4, HumanStatus::Started, TaskScope::Global, false, 20),
            task(5, HumanStatus::Ready, project("/repos/here"), false, 10),
        ];

        // Projects index: the archived project paints no row, the live one stays.
        let index = query_board(
            &tasks,
            &archived,
            None,
            BoardLens::Projects,
            false,
            &ThreadFilter::All,
        );
        assert!(
            !index.projects.iter().any(|row| row.path == "/repos/gone"),
            "archived project must not paint an index row"
        );
        assert!(index.projects.iter().any(|row| row.path == "/repos/here"));

        // Thread view: the archived project's thread contributes nothing.
        let view = query_board(
            &tasks,
            &archived,
            None,
            BoardLens::ThreadView("release"),
            false,
            &ThreadFilter::All,
        );
        assert!(
            !view
                .sections
                .iter()
                .any(|s| s.task_ids.contains(&Uuid::from_u128(3))),
            "an archived project's thread must not paint on the thread view"
        );

        // Desk IN MOTION: the archived project's started task is gone, the global one stays.
        let desk = query_board(
            &tasks,
            &archived,
            None,
            BoardLens::Desk,
            false,
            &ThreadFilter::All,
        );
        let motion = section_ids(&desk, SectionKind::InMotion);
        assert!(!motion.contains(&Uuid::from_u128(1)));
        assert!(motion.contains(&Uuid::from_u128(4)));
    }

    #[test]
    fn desk_in_motion_is_global_started_sorted_by_status_change_desc() {
        // created_at order (90, 50, 10) is the reverse of status-change order
        // (60, 80, 100): the newest status change leads, not the newest edit.
        let tasks = vec![
            with_status_event(
                task(1, HumanStatus::Started, TaskScope::Global, false, 90),
                TaskEventKind::StatusSet,
                60,
            ),
            with_status_event(
                task(2, HumanStatus::Started, project("/repos/a"), false, 10),
                TaskEventKind::StatusSet,
                100,
            ),
            task(3, HumanStatus::Started, TaskScope::Global, true, 40),
            task(4, HumanStatus::Ready, TaskScope::Global, false, 50),
            with_status_event(
                task(5, HumanStatus::Started, project("/repos/b"), false, 50),
                TaskEventKind::StatusSet,
                80,
            ),
        ];

        let view = query_lens(&tasks, None, BoardLens::Desk, false);

        let motion: Vec<_> = view
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::InMotion)
            .collect();
        assert_eq!(motion.len(), 1);
        assert_eq!(
            ids(motion[0]),
            vec![Uuid::from_u128(2), Uuid::from_u128(5), Uuid::from_u128(1),],
            "newest status change first across every live scope"
        );

        let desk: Vec<_> = view
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::OnDeck && s.project_label.is_none())
            .collect();
        assert_eq!(desk.len(), 1);
        assert_eq!(ids(desk[0]), vec![Uuid::from_u128(4)]);
        assert!(!section_ids(&view, SectionKind::OnDeck).contains(&Uuid::from_u128(2)));
    }

    #[test]
    fn ticking_a_step_on_the_older_started_task_does_not_reorder_in_motion() {
        let older = with_status_event(
            task(1, HumanStatus::Started, TaskScope::Global, false, 10),
            TaskEventKind::StatusSet,
            20,
        );
        let newer = with_status_event(
            task(2, HumanStatus::Started, TaskScope::Global, false, 30),
            TaskEventKind::StatusSet,
            50,
        );
        // The step tick lands long after both status changes and bumps updated_at
        // past the newer task's; the section order must not move.
        let tasks = vec![mutated(older, TaskEventKind::StepChecked, 200), newer];

        let view = query_lens(&tasks, None, BoardLens::Desk, false);

        assert_eq!(
            section_ids(&view, SectionKind::InMotion),
            vec![Uuid::from_u128(2), Uuid::from_u128(1)],
            "a step tick never reorders IN MOTION"
        );
    }

    #[test]
    fn setting_status_moves_a_task_to_the_top_of_its_new_section() {
        let tasks = vec![
            with_status_event(
                task(1, HumanStatus::Started, TaskScope::Global, false, 10),
                TaskEventKind::StatusSet,
                20,
            ),
            with_status_event(
                task(2, HumanStatus::Started, TaskScope::Global, false, 30),
                TaskEventKind::StatusSet,
                40,
            ),
            // Task 3 is the oldest created but was just started at 100.
            with_status_event(
                task(3, HumanStatus::Started, TaskScope::Global, false, 5),
                TaskEventKind::StatusSet,
                100,
            ),
        ];

        let view = query_lens(&tasks, None, BoardLens::Desk, false);

        assert_eq!(
            section_ids(&view, SectionKind::InMotion),
            vec![Uuid::from_u128(3), Uuid::from_u128(2), Uuid::from_u128(1)],
            "the freshly started task leads IN MOTION"
        );
    }

    #[test]
    fn on_deck_is_oldest_first_and_an_edit_does_not_move_a_row() {
        let tasks = vec![
            task(1, HumanStatus::Ready, TaskScope::Global, false, 10),
            // Task 2 is edited at 90, past both neighbours: it stays in FIFO place.
            mutated(
                task(2, HumanStatus::Ready, TaskScope::Global, false, 20),
                TaskEventKind::Edited,
                90,
            ),
            task(3, HumanStatus::Ready, TaskScope::Global, false, 30),
        ];

        let view = query_lens(&tasks, None, BoardLens::Desk, false);

        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)],
            "ON DECK is the backlog: oldest created first, edits never reorder"
        );
    }

    #[test]
    fn notices_lead_on_deck_ahead_of_an_older_backlog() {
        // A starter guide seeded today (created 90) on a desk with tasks from 10 and 20:
        // FIFO alone would bury it, so notices lead and the backlog keeps its own order.
        let mut guide = task(3, HumanStatus::Ready, TaskScope::Global, false, 90);
        guide.notice = Some(crate::domain::Notice {
            catalog_id: "guide.welcome".into(),
            number: None,
        });
        let tasks = vec![
            task(1, HumanStatus::Ready, TaskScope::Global, false, 10),
            task(2, HumanStatus::Ready, TaskScope::Global, false, 20),
            guide,
        ];

        let view = query_lens(&tasks, None, BoardLens::Desk, false);

        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(3), Uuid::from_u128(1), Uuid::from_u128(2)],
        );
    }

    #[test]
    fn a_task_without_a_status_event_sorts_by_created_among_status_changed_ones() {
        // Task 1: status set at 50. Task 2: no status event, updated_at bumped to
        // 99 by an edit, so only its created_at (20) may order it. Task 3: no
        // status event, created at 30.
        let tasks = vec![
            with_status_event(
                task(1, HumanStatus::Blocked, TaskScope::Global, false, 10),
                TaskEventKind::StatusSet,
                50,
            ),
            mutated(
                task(2, HumanStatus::Blocked, TaskScope::Global, false, 20),
                TaskEventKind::Edited,
                99,
            ),
            task(3, HumanStatus::Blocked, TaskScope::Global, false, 30),
        ];

        let view = query_lens(&tasks, None, BoardLens::Desk, false);

        assert_eq!(
            section_ids(&view, SectionKind::NeedsYou),
            vec![Uuid::from_u128(1), Uuid::from_u128(3), Uuid::from_u128(2)],
            "no status event falls back to created_at, never updated_at"
        );
    }

    #[test]
    fn desk_needs_you_is_global_across_every_live_scope() {
        let tasks = vec![
            task(1, HumanStatus::Blocked, TaskScope::Global, false, 10),
            task(2, HumanStatus::Review, project("/repos/a"), false, 30),
            task(3, HumanStatus::Ready, TaskScope::Global, false, 20),
            task(4, HumanStatus::Blocked, project("/repos/b"), false, 40),
            task(5, HumanStatus::Review, project("/repos/a"), false, 50),
            task(6, HumanStatus::Blocked, project("/repos/gone"), false, 60),
        ];
        let mut gone = BTreeSet::new();
        gone.insert("/repos/gone".to_string());

        let view = query_board(
            &tasks,
            &gone,
            None,
            BoardLens::Desk,
            false,
            &ThreadFilter::All,
        );
        assert_eq!(
            section_ids(&view, SectionKind::NeedsYou),
            vec![
                Uuid::from_u128(5),
                Uuid::from_u128(4),
                Uuid::from_u128(2),
                Uuid::from_u128(1),
            ],
            "blocked/review from every live project plus desk, newest status change first"
        );
        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(3)],
            "project ready tasks never leak into the desk backlog"
        );
        assert_eq!(view.sections[0].kind, SectionKind::NeedsYou);
    }

    #[test]
    fn project_board_puts_blocked_and_review_in_needs_you() {
        let tasks = vec![
            task(1, HumanStatus::Started, project("/repos/a"), false, 100),
            task(2, HumanStatus::Blocked, project("/repos/a"), false, 80),
            task(3, HumanStatus::Review, project("/repos/a"), false, 90),
            task(4, HumanStatus::Ready, project("/repos/a"), false, 70),
            task(5, HumanStatus::Blocked, project("/repos/b"), false, 60),
        ];

        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Project(Path::new("/repos/a")),
            false,
            &ThreadFilter::All,
        );
        assert_eq!(
            section_ids(&view, SectionKind::NeedsYou),
            vec![Uuid::from_u128(3), Uuid::from_u128(2)]
        );
        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(4)]
        );
        assert!(!all_listed_ids(&view).contains(&Uuid::from_u128(5)));
        assert_eq!(view.sections[0].kind, SectionKind::NeedsYou);
    }

    #[test]
    fn empty_deck_is_omitted_when_needs_you_has_rows() {
        let desk_tasks = vec![task(1, HumanStatus::Blocked, TaskScope::Global, false, 10)];
        let desk = query_lens(&desk_tasks, None, BoardLens::Desk, false);
        assert!(desk.sections.iter().all(|s| s.kind != SectionKind::OnDeck));
        assert!(!desk.sections.iter().any(|s| s.empty_hint));

        let project_tasks = vec![task(2, HumanStatus::Review, project("/repos/a"), false, 10)];
        let project = query_board(
            &project_tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Project(Path::new("/repos/a")),
            false,
            &ThreadFilter::All,
        );
        assert!(project
            .sections
            .iter()
            .all(|s| s.kind != SectionKind::OnDeck));
        assert!(!project.sections.iter().any(|s| s.empty_hint));
    }

    #[test]
    fn empty_project_board_keeps_one_hinted_deck_section() {
        let tasks = vec![task(9, HumanStatus::Ready, project("/repos/b"), false, 10)];
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Project(Path::new("/repos/a")),
            false,
            &ThreadFilter::All,
        );
        let deck = view
            .sections
            .iter()
            .find(|s| s.kind == SectionKind::OnDeck)
            .expect("empty project keeps the deck section");
        assert!(deck.empty_hint);
        assert!(deck.task_ids.is_empty());
    }

    #[test]
    fn thread_filter_narrows_every_project_status_and_the_drawer() {
        let tasks = vec![
            task_with_thread(
                1,
                HumanStatus::Blocked,
                project("/a"),
                false,
                50,
                Some("nav"),
            ),
            task_with_thread(
                2,
                HumanStatus::Started,
                project("/a"),
                false,
                40,
                Some("nav"),
            ),
            task_with_thread(3, HumanStatus::Ready, project("/a"), false, 30, Some("nav")),
            task_with_thread(4, HumanStatus::Done, project("/a"), false, 20, Some("nav")),
            task_with_thread(
                5,
                HumanStatus::Blocked,
                project("/a"),
                false,
                60,
                Some("docs"),
            ),
            task_with_thread(6, HumanStatus::Ready, project("/a"), false, 70, None),
        ];
        let lens = BoardLens::Project(Path::new("/a"));

        let nav = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            lens,
            true,
            &ThreadFilter::Named("nav".to_string()),
        );
        assert_eq!(
            section_ids(&nav, SectionKind::NeedsYou),
            vec![Uuid::from_u128(1)]
        );
        assert_eq!(
            section_ids(&nav, SectionKind::InMotion),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            section_ids(&nav, SectionKind::OnDeck),
            vec![Uuid::from_u128(3)]
        );
        assert_eq!(
            section_ids(&nav, SectionKind::Done),
            vec![Uuid::from_u128(4)]
        );
        assert_eq!(nav.counts.need, 1, "status counts reflect the filtered set");

        let without = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            lens,
            false,
            &ThreadFilter::Without,
        );
        assert_eq!(
            section_ids(&without, SectionKind::OnDeck),
            vec![Uuid::from_u128(6)],
            "unthreaded tasks are the Without-a-thread filter's matches"
        );
        assert!(section_ids(&without, SectionKind::NeedsYou).is_empty());
    }

    #[test]
    fn thread_view_is_flat_across_projects_and_desk_with_no_nesting() {
        let tasks = vec![
            task_with_thread(
                1,
                HumanStatus::Started,
                project("/repos/a"),
                false,
                30,
                Some("release"),
            ),
            task_with_thread(
                2,
                HumanStatus::Ready,
                TaskScope::Global,
                false,
                20,
                Some("release"),
            ),
            task_with_thread(
                3,
                HumanStatus::Review,
                project("/repos/b"),
                false,
                10,
                Some("release"),
            ),
            task_with_thread(
                4,
                HumanStatus::Ready,
                project("/repos/a"),
                false,
                5,
                Some("other"),
            ),
            task_with_thread(
                5,
                HumanStatus::Blocked,
                project("/repos/a"),
                false,
                8,
                Some("release"),
            ),
        ];

        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::ThreadView("release"),
            false,
            &ThreadFilter::All,
        );
        assert_eq!(
            section_ids(&view, SectionKind::NeedsYou),
            vec![Uuid::from_u128(3), Uuid::from_u128(5)]
        );
        assert_eq!(
            section_ids(&view, SectionKind::InMotion),
            vec![Uuid::from_u128(1)]
        );
        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(2)]
        );
        assert!(
            view.sections.iter().all(|s| s.project_label.is_none()),
            "thread view sections are plain status groups, no thread/project nesting"
        );
        assert!(!all_listed_ids(&view).contains(&Uuid::from_u128(4)));
    }

    #[test]
    fn thread_view_matches_names_ascii_case_insensitively_and_includes_done() {
        let tasks = vec![
            task_with_thread(
                1,
                HumanStatus::Ready,
                project("/a"),
                false,
                10,
                Some("Release"),
            ),
            task_with_thread(
                2,
                HumanStatus::Done,
                project("/a"),
                false,
                30,
                Some("RELEASE"),
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
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(1)]
        );
        assert_eq!(
            section_ids(&view, SectionKind::Done),
            vec![Uuid::from_u128(2)]
        );
    }

    #[test]
    fn projects_index_lists_counts_current_first_and_keeps_same_named_projects() {
        let tasks = vec![
            task(1, HumanStatus::Blocked, project("/w/launch"), false, 10),
            task(2, HumanStatus::Started, project("/w/launch"), false, 20),
            task(3, HumanStatus::Ready, project("/w/launch"), false, 30),
            task(4, HumanStatus::Ready, project("/w/launch"), false, 35),
            task(9, HumanStatus::Open, project("/w/launch"), false, 36),
            task(5, HumanStatus::Review, project("/w/herdr"), false, 40),
            task(6, HumanStatus::Done, project("/w/herdr"), false, 50),
            task(7, HumanStatus::Ready, project("/x/site"), false, 60),
            task(8, HumanStatus::Ready, project("/y/site"), false, 70),
        ];

        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            Some(Path::new("/w/herdr")),
            BoardLens::Projects,
            false,
            &ThreadFilter::All,
        );

        let names: Vec<&str> = view.projects.iter().map(|row| row.path.as_str()).collect();
        assert_eq!(
            names,
            vec!["/w/herdr", "/w/launch", "/x/site", "/y/site"],
            "the invocation project leads, the rest sort by path"
        );
        assert!(view.projects[0].current);
        assert!(!view.projects[1].current);

        let launch = &view.projects[1];
        assert_eq!(launch.needs_you, 1, "blocked counts as needs-you");
        assert_eq!(launch.in_motion, 1);
        assert_eq!(
            launch.on_deck, 3,
            "ready and open tasks share the project's ON DECK tally"
        );
        assert_eq!(
            view.projects[0].done, 1,
            "live done tasks have their own tally"
        );

        assert!(
            view.sections.is_empty(),
            "the index is navigation, never an expanded task list"
        );
    }

    #[test]
    fn projects_index_lists_the_invocation_project_even_with_no_tasks() {
        let tasks = vec![task(1, HumanStatus::Ready, project("/w/other"), false, 10)];
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            Some(Path::new("/w/fresh")),
            BoardLens::Projects,
            false,
            &ThreadFilter::All,
        );
        assert_eq!(view.projects.len(), 2);
        assert_eq!(view.projects[0].path, "/w/fresh");
        assert!(view.projects[0].current);
        assert_eq!(
            (
                view.projects[0].needs_you,
                view.projects[0].in_motion,
                view.projects[0].on_deck,
                view.projects[0].done
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn projects_index_done_excludes_archived_tasks_and_archived_projects() {
        let mut archived_task = task(2, HumanStatus::Done, project("/w/live"), false, 20);
        archived_task.archived = true;
        let tasks = vec![
            task(1, HumanStatus::Done, project("/w/live"), false, 10),
            archived_task,
            task(3, HumanStatus::Done, project("/w/gone"), false, 30),
        ];
        let archived_projects = BTreeSet::from(["/w/gone".to_string()]);
        let view = query_board(
            &tasks,
            &archived_projects,
            None,
            BoardLens::Projects,
            false,
            &ThreadFilter::All,
        );

        assert_eq!(
            view.projects.len(),
            1,
            "archived projects paint no index row"
        );
        assert_eq!(view.projects[0].path, "/w/live");
        assert_eq!(view.projects[0].done, 1, "only live done tasks count");
    }

    #[test]
    fn project_board_matches_tasks_across_path_spellings() {
        use std::fs;

        // The stored scope was captured through one spelling of the directory; the
        // invocation resolved the same directory through a symlink (the /tmp vs
        // /private/tmp mismatch). The lens must still match, without rewriting the
        // stored identity.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tsk-path-spelling-{}-{seq}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let link = std::env::temp_dir().join(format!(
            "tsk-path-spelling-link-{}-{seq}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, &link).expect("symlink");
        #[cfg(not(unix))]
        {
            let _ = &link;
            fs::remove_dir_all(&dir).expect("cleanup");
            return;
        }

        let stored = dir.to_string_lossy().into_owned();
        let invoked = link.to_string_lossy().into_owned();
        let tasks = vec![task(1, HumanStatus::Ready, project(&stored), false, 10)];
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Project(Path::new(&invoked)),
            false,
            &ThreadFilter::All,
        );
        let matched = section_ids(&view, SectionKind::OnDeck) == vec![Uuid::from_u128(1)];
        fs::remove_file(&link).ok();
        fs::remove_dir_all(&dir).ok();
        assert!(
            matched,
            "equivalent path spellings must match the stored project scope"
        );
    }

    #[test]
    fn done_drawer_lists_non_deleted_done_by_status_change_desc() {
        let tasks = vec![
            task(1, HumanStatus::Done, TaskScope::Global, false, 10),
            task(2, HumanStatus::Done, project("/repos/a"), false, 30),
            task(3, HumanStatus::Done, TaskScope::Global, true, 40),
            task(4, HumanStatus::Ready, TaskScope::Global, false, 50),
        ];

        let closed = query_lens(&tasks, None, BoardLens::Desk, false);
        assert!(closed.sections.iter().all(|s| s.kind != SectionKind::Done));

        let open = query_lens(&tasks, None, BoardLens::Desk, true);
        let done: Vec<_> = open
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Done)
            .collect();
        assert_eq!(done.len(), 1);
        assert_eq!(ids(done[0]), vec![Uuid::from_u128(2), Uuid::from_u128(1),]);
    }

    #[test]
    fn project_board_filters_sections_to_matching_project() {
        let tasks = vec![
            task(1, HumanStatus::Started, project("/repos/a"), false, 100),
            task(2, HumanStatus::Started, project("/repos/b"), false, 90),
            task(10, HumanStatus::Ready, project("/repos/a"), false, 70),
            task(20, HumanStatus::Done, project("/repos/a"), false, 40),
        ];

        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Project(Path::new("/repos/a")),
            true,
            &ThreadFilter::All,
        );

        assert_eq!(
            section_ids(&view, SectionKind::InMotion),
            vec![Uuid::from_u128(1)]
        );
        assert_eq!(
            section_ids(&view, SectionKind::Done),
            vec![Uuid::from_u128(20)]
        );
        assert!(!all_listed_ids(&view).contains(&Uuid::from_u128(2)));
    }

    #[test]
    fn visible_task_ids_keeps_done_drawer_rows_selectable() {
        let tasks = vec![
            task(1, HumanStatus::Ready, project("/repos/a"), false, 10),
            task(2, HumanStatus::Done, project("/repos/a"), false, 40),
        ];
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Project(Path::new("/repos/a")),
            true,
            &ThreadFilter::All,
        );
        assert_eq!(
            section_ids(&view, SectionKind::Done),
            vec![Uuid::from_u128(2)]
        );

        // No archived tasks: the drawer paints DONE rows only.
        let visible = visible_task_ids(&view, false, false);
        assert_eq!(
            visible,
            vec![Uuid::from_u128(1), Uuid::from_u128(2)],
            "painted DONE drawer rows must stay in the selectable set"
        );

        // With an archived task present, the header row is always selectable so Enter
        // can toggle the group; the collapsed group paints only its header.
        let mut tasks = tasks.clone();
        let mut archived =
            task_with_thread(3, HumanStatus::Ready, project("/repos/a"), false, 50, None);
        archived.archived = true;
        tasks.push(archived);
        let view = query_board(
            &tasks,
            &BTreeSet::new(),
            None,
            BoardLens::Project(Path::new("/repos/a")),
            true,
            &ThreadFilter::All,
        );
        let expanded = visible_task_ids(&view, false, false);
        assert!(
            expanded.contains(&ARCHIVED_HEADER_ROW_ID) && expanded.contains(&Uuid::from_u128(3))
        );
        let collapsed = visible_task_ids(&view, true, false);
        assert_eq!(
            collapsed,
            vec![
                Uuid::from_u128(1),
                Uuid::from_u128(2),
                ARCHIVED_HEADER_ROW_ID
            ],
            "the collapsed archived group paints only its header"
        );
    }

    #[test]
    fn on_deck_puts_ready_picks_above_inbox_open_rows() {
        let tasks = vec![
            with_status_event(
                task(1, HumanStatus::Ready, TaskScope::Global, false, 10),
                TaskEventKind::StatusSet,
                80,
            ),
            with_status_event(
                task(2, HumanStatus::Ready, TaskScope::Global, false, 20),
                TaskEventKind::StatusSet,
                40,
            ),
            task(3, HumanStatus::Open, TaskScope::Global, false, 30),
            task(4, HumanStatus::Open, TaskScope::Global, false, 5),
        ];
        let view = query_lens(&tasks, None, BoardLens::Desk, false);
        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(2), Uuid::from_u128(1)],
            "ready rows are oldest pick first"
        );
        assert_eq!(
            section_ids(&view, SectionKind::Inbox),
            vec![Uuid::from_u128(4), Uuid::from_u128(3)],
            "inbox is oldest created first"
        );
        let deck = view
            .sections
            .iter()
            .find(|section| section.kind == SectionKind::OnDeck)
            .expect("on deck");
        assert_eq!(deck.count, 4, "ON DECK totals ready plus open");
    }

    #[test]
    fn inbox_heading_is_absent_without_open_and_present_without_ready() {
        let ready_only = vec![task(1, HumanStatus::Ready, TaskScope::Global, false, 10)];
        let ready_view = query_lens(&ready_only, None, BoardLens::Desk, false);
        assert!(
            ready_view
                .sections
                .iter()
                .all(|section| section.kind != SectionKind::Inbox),
            "no open tasks means no inbox heading"
        );

        let open_only = vec![task(2, HumanStatus::Open, TaskScope::Global, false, 10)];
        let open_view = query_lens(&open_only, None, BoardLens::Desk, false);
        let deck = open_view
            .sections
            .iter()
            .find(|section| section.kind == SectionKind::OnDeck)
            .expect("on deck still paints");
        assert!(deck.task_ids.is_empty());
        assert_eq!(deck.count, 1);
        assert!(!deck.empty_hint);
        assert_eq!(
            section_ids(&open_view, SectionKind::Inbox),
            vec![Uuid::from_u128(2)]
        );
    }

    #[test]
    fn folded_inbox_hides_open_rows() {
        let tasks = vec![
            task(1, HumanStatus::Ready, TaskScope::Global, false, 10),
            task(2, HumanStatus::Open, TaskScope::Global, false, 20),
        ];
        let view = query_lens(&tasks, None, BoardLens::Desk, false);
        let expanded = visible_task_ids(&view, false, false);
        assert_eq!(
            expanded,
            vec![Uuid::from_u128(1), INBOX_HEADER_ROW_ID, Uuid::from_u128(2)]
        );
        let collapsed = visible_task_ids(&view, false, true);
        assert_eq!(
            collapsed,
            vec![Uuid::from_u128(1), INBOX_HEADER_ROW_ID],
            "a folded inbox paints only its heading"
        );
        assert!(
            !collapsed.contains(&Uuid::from_u128(2)),
            "selection cannot rest on a folded inbox row"
        );
    }

    #[test]
    fn desk_on_deck_excludes_project_ready_and_open() {
        let tasks = vec![
            task(1, HumanStatus::Ready, TaskScope::Global, false, 10),
            task(2, HumanStatus::Open, TaskScope::Global, false, 20),
            task(3, HumanStatus::Ready, project("/repos/a"), false, 30),
            task(4, HumanStatus::Open, project("/repos/a"), false, 40),
        ];
        let view = query_lens(&tasks, None, BoardLens::Desk, false);
        assert_eq!(
            section_ids(&view, SectionKind::OnDeck),
            vec![Uuid::from_u128(1)]
        );
        assert_eq!(
            section_ids(&view, SectionKind::Inbox),
            vec![Uuid::from_u128(2)]
        );
        assert!(!all_listed_ids(&view).contains(&Uuid::from_u128(3)));
        assert!(!all_listed_ids(&view).contains(&Uuid::from_u128(4)));
    }
}
