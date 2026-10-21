//! Headless human-status changes.

use std::path::{Path, PathBuf};

use crate::agents::AgentProfiles;
use crate::cli::parser::{BlockFlags, TaskAddress};
use crate::dispatch::{self, DispatchError, DispatchHost, DispatchResult, StartRoute};
use crate::domain::{
    actor_from_env, BlockDraft, BlockField, BlockKind, BlockOn, BlockPatch, DomainError,
    DomainState, HumanStatus, ReviewDraft, ReviewPatch, OWNER,
};
use crate::store::{default_state_dir, TaskStore};
use uuid::Uuid;

/// A successful status change, including an idempotent repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResult {
    pub number: u64,
    pub title: String,
    pub status: HumanStatus,
}

/// A status failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusError {
    UnknownTask,
    SoftDeletedTask,
    /// A block text is longer than the cap.
    TextTooLong(BlockField),
    /// `--on` names the task itself or a task that is not on the board.
    InvalidBlocker,
    /// Starting a dispatched task whose agent is gone, without `--again` or `--no-dispatch`.
    AgentGone(String),
    /// Starting an assigned task dispatched it, and the dispatch refused.
    Dispatch(DispatchError),
    /// Starting a task that still waits on prerequisites, without `--force`: `T3 runs after
    /// T2 (started)`.
    AfterNotDone(String),
    Store(String),
}

impl StatusError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::TextTooLong(_) => "text-too-long",
            Self::InvalidBlocker => "invalid-blocker",
            Self::AgentGone(_) => "agent-gone",
            Self::Dispatch(error) => error.code(),
            Self::AfterNotDone(_) => "after-not-done",
            Self::Store(_) => "store-error",
        }
    }
}

/// Set one task's human status. Repeating the same status is idempotent.
///
/// With `blocked`, the block flags open a block, or edit the task's open block in place.
/// With `review`, the review flags open a review round, or edit the open round in place.
/// Soft-deleted and unknown tasks refuse the same way as archive.
pub fn run(
    target: TaskAddress,
    status: HumanStatus,
    block: BlockFlags,
    state_dir: Option<PathBuf>,
) -> Result<StatusResult, StatusError> {
    run_as(target, status, block, state_dir, &actor_from_env())
}

/// [`run`] by `actor`.
fn run_as(
    target: TaskAddress,
    status: HumanStatus,
    block: BlockFlags,
    state_dir: Option<PathBuf>,
    actor: &str,
) -> Result<StatusResult, StatusError> {
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    // `--on` by the name rule: a block or review handed to an agent names its profile. A
    // malformed config never blocks the status change: the name is then kept as plain text.
    let on = BlockKind::for_status(status).and_then(|kind| {
        let value = block.on.as_deref()?;
        let profiles = AgentProfiles::load(&state_dir).unwrap_or_default();
        Some(profiles.resolve_on(value, kind))
    });
    let store = TaskStore::new(state_dir);
    store
        .locked_transition_if_changed(|state: &mut DomainState| {
            let found = state
                .tasks()
                .iter()
                .find(|task| target.matches(task))
                .map(|task| Found {
                    id: task.id,
                    number: task.number,
                    title: task.title.clone(),
                    current: task.status,
                    soft_deleted: task.soft_deleted,
                    open: task.block.as_ref().map(|block| block.kind),
                });
            Ok(apply(state, found, status, &block, on.clone(), actor))
        })
        .map_err(StatusError::Store)?
}

/// `tsk status <task> done` through the real host: a done that releases waiting tasks starts
/// them, launching an assigned one's agent.
pub fn run_done(
    target: TaskAddress,
    state_dir: Option<PathBuf>,
) -> Result<(StatusResult, Vec<dispatch::Released>), StatusError> {
    let (state_dir, mut host) = crate::cli::dispatch::system_host(state_dir);
    let outcome = run_done_with_host(
        target,
        Some(state_dir),
        dispatch::running_inside_herdr(),
        &actor_from_env(),
        &mut host,
    )?;
    name_released(&outcome.1);
    Ok(outcome)
}

/// Name each agent a done launched, best effort like naming itself: the dispatch succeeded.
pub fn name_released(released: &[dispatch::Released]) {
    for released in released {
        if let dispatch::ReleasedStart::Dispatched(result)
        | dispatch::ReleasedStart::Unrecorded(result, _) = &released.start
        {
            if let Some(naming) = &result.naming {
                let _ = dispatch::spawn_agent_naming_process(naming);
            }
        }
    }
}

/// Set a task done by `actor`. When that releases tasks waiting on it (`ready`, every
/// prerequisite now done), each starts through the start route. What it released is decided
/// inside the same locked transition as the done, so a prerequisite another process completed
/// concurrently is never missed: a plain start lands in that save, and an assigned task never
/// dispatched launches right after it. A failed launch leaves its task ready and never refuses
/// the done.
pub fn run_done_with_host(
    target: TaskAddress,
    state_dir: Option<PathBuf>,
    in_herdr: bool,
    actor: &str,
    host: &mut impl DispatchHost,
) -> Result<(StatusResult, Vec<dispatch::Released>), StatusError> {
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    let store = TaskStore::new(&state_dir);
    let (result, mut released, launches, token) = store
        .locked_transition_if_changed(|state: &mut DomainState| {
            let found = state
                .tasks()
                .iter()
                .find(|task| target.matches(task))
                .map(|task| Found {
                    id: task.id,
                    number: task.number,
                    title: task.title.clone(),
                    current: task.status,
                    soft_deleted: task.soft_deleted,
                    open: task.block.as_ref().map(|block| block.kind),
                });
            let _ = state.take_completed();
            let (result, changed) = apply(
                state,
                found,
                HumanStatus::Done,
                &BlockFlags::default(),
                None,
                actor,
            );
            let released = if changed {
                dispatch::plan_released_with_host(state, actor, in_herdr, None, host)
            } else {
                Vec::new()
            };
            let launches = state.take_pending_launches();
            let token = state.restore_token().map(str::to_owned);
            Ok(((result, released, launches, token), changed))
        })
        .map_err(StatusError::Store)?;
    let result = result?;
    if !launches.is_empty() {
        released.extend(launch_released_after_done(
            &store,
            &state_dir,
            in_herdr,
            host,
            launches,
            token.as_deref(),
        )?);
        released.sort_by_key(|released| released.number);
    }
    Ok((result, released))
}

/// Launch what a durable done released, outside the lock, then record the launches. `token` is
/// the restore token the done's transition loaded with: a restore since then replaced the
/// store, so what the done released may not be there and nothing launches.
fn launch_released_after_done(
    store: &TaskStore,
    state_dir: &Path,
    in_herdr: bool,
    host: &mut impl DispatchHost,
    launches: Vec<(Uuid, u64)>,
    token: Option<&str>,
) -> Result<Vec<dispatch::Released>, StatusError> {
    let mut state = store
        .load()
        .map_err(|error| StatusError::Store(error.to_string()))?;
    if state.restore_token() != token {
        return Err(StatusError::Store(format!(
            "{}; nothing launched: check the task, then run it again",
            crate::store::StoreError::Restored
        )));
    }
    for (id, after) in launches {
        state.hold_launch(id, after);
    }
    let launched = dispatch::launch_released_with_host(&mut state, state_dir, in_herdr, host);
    let recorded = store.reload_merge_save(&mut state);
    Ok(launched
        .into_iter()
        .map(|mut released| {
            if let Err(error) = &recorded {
                released.start = dispatch::unsaved(released.start, error);
            }
            released
        })
        .collect())
}

/// What `tsk status <task> started` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartOutcome {
    /// A plain status change (or the task was already started).
    Status(StatusResult),
    /// An assigned task started without a launch because dispatch cannot work here (outside
    /// Herdr, or a desk task); the reason is printed, not refused.
    NoLaunch(StatusResult, &'static str),
    /// The start dispatched (or, with `--again`, relaunched) the task's agent, which set it
    /// started.
    Dispatched(StatusResult, Box<DispatchResult>),
    /// A plain start that sent a block back or unblocked it, and tried to deliver the record's
    /// replies to the running agent: the status-row line saying how it went.
    Delivered(StatusResult, String),
}

/// How `started` may launch: `--again` relaunches a gone agent, `--no-dispatch` never launches,
/// `--force` starts a task whose prerequisites are not all done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartFlags {
    pub again: bool,
    pub no_dispatch: bool,
    pub force: bool,
}

/// `tsk status <task> started` through the real host: launches where a start launches.
pub fn run_started(
    target: TaskAddress,
    flags: StartFlags,
    state_dir: Option<PathBuf>,
) -> Result<StartOutcome, StatusError> {
    let (state_dir, mut host) = crate::cli::dispatch::system_host(state_dir);
    let outcome = run_started_with_host(
        target,
        flags,
        Some(state_dir),
        dispatch::running_inside_herdr(),
        &actor_from_env(),
        &mut host,
    )?;
    if let StartOutcome::Dispatched(_, result) = &outcome {
        if let Some(naming) = &result.naming {
            // Best effort, like naming itself: the dispatch already succeeded.
            let _ = dispatch::spawn_agent_naming_process(naming);
        }
    }
    Ok(outcome)
}

/// Start a task the way the board's `ctrl+s` does, by `actor`:
///
/// - unassigned, already started, done, archived, `--no-dispatch`, or the caller is the
///   assigned agent itself: a plain status change;
/// - assigned and never dispatched: dispatch it, which sets started; outside Herdr or on the
///   desk it cannot launch, so it starts plainly and says why ([`StartOutcome::NoLaunch`]);
/// - dispatched and the agent still running (or Herdr cannot say): a plain status change;
/// - dispatched and the agent gone: refuse with `agent-gone`, unless `--again` relaunches.
pub fn run_started_with_host(
    target: TaskAddress,
    flags: StartFlags,
    state_dir: Option<PathBuf>,
    in_herdr: bool,
    actor: &str,
    host: &mut impl DispatchHost,
) -> Result<StartOutcome, StatusError> {
    if !flags.force {
        let store = TaskStore::new(state_dir.clone().unwrap_or_else(default_state_dir));
        let state = store
            .load()
            .map_err(|error| StatusError::Store(error.to_string()))?;
        if let Some(task) = state.tasks().iter().find(|task| {
            target.matches(task) && !task.soft_deleted && task.status != HumanStatus::Started
        }) {
            if let Some(waiting) = state.waiting_text(task) {
                return Err(StatusError::AfterNotDone(waiting));
            }
        }
    }
    let plain = |state_dir| {
        run_as(
            target,
            HumanStatus::Started,
            BlockFlags::default(),
            state_dir,
            actor,
        )
        .map(StartOutcome::Status)
    };
    if flags.no_dispatch {
        return plain(state_dir);
    }
    let store = TaskStore::new(state_dir.clone().unwrap_or_else(default_state_dir));
    let state = store
        .load()
        .map_err(|error| StatusError::Store(error.to_string()))?;
    let Some(task) = state.tasks().iter().find(|task| target.matches(task)) else {
        return plain(state_dir);
    };
    let settled = task.status == HumanStatus::Started && !flags.again;
    if settled
        || task.soft_deleted
        || task.archived
        || task.status == HumanStatus::Done
        || task.number.is_none()
    {
        return plain(state_dir);
    }
    let closing = Closing::before(task, actor);
    let again = match dispatch::start_route(task, actor, in_herdr, host) {
        StartRoute::Plain => {
            let result = plain(state_dir.clone())?;
            return Ok(match (result, closing) {
                (StartOutcome::Status(result), Some(closing)) => {
                    match closing.deliver(&store, target, in_herdr, host) {
                        Some(line) => StartOutcome::Delivered(result, line),
                        None => StartOutcome::Status(result),
                    }
                }
                (result, _) => result,
            });
        }
        StartRoute::NoLaunch { reason } => {
            return plain(state_dir).map(|outcome| match outcome {
                StartOutcome::Status(result) => StartOutcome::NoLaunch(result, reason),
                other => other,
            })
        }
        StartRoute::Dispatch => false,
        StartRoute::AgentGone { .. } if flags.again => true,
        StartRoute::AgentGone { assignee } => return Err(StatusError::AgentGone(assignee)),
    };
    let result = crate::cli::dispatch::run_with_host(target, again, state_dir, in_herdr, host)
        .map_err(|error| match error {
            DispatchError::Store(detail) => StatusError::Store(detail),
            other => StatusError::Dispatch(other),
        })?;
    let status = StatusResult {
        number: result.number,
        title: result.title.clone(),
        status: HumanStatus::Started,
    };
    Ok(StartOutcome::Dispatched(status, Box::new(result)))
}

/// `tsk status <task> ready` through the real host: a block left for ready sends its replies
/// to the running agent, as the board's `ctrl+b` does. Returns the status-row line when a
/// delivery was tried.
pub fn run_ready(
    target: TaskAddress,
    state_dir: Option<PathBuf>,
) -> Result<(StatusResult, Option<String>), StatusError> {
    let (state_dir, mut host) = crate::cli::dispatch::system_host(state_dir);
    run_ready_with_host(
        target,
        Some(state_dir),
        dispatch::running_inside_herdr(),
        &actor_from_env(),
        &mut host,
    )
}

pub fn run_ready_with_host(
    target: TaskAddress,
    state_dir: Option<PathBuf>,
    in_herdr: bool,
    actor: &str,
    host: &mut impl DispatchHost,
) -> Result<(StatusResult, Option<String>), StatusError> {
    let store = TaskStore::new(state_dir.clone().unwrap_or_else(default_state_dir));
    let closing = store.load().ok().and_then(|state| {
        state
            .tasks()
            .iter()
            .find(|task| target.matches(task))
            .and_then(|task| Closing::before(task, actor))
    });
    let result = run_as(
        target,
        HumanStatus::Ready,
        BlockFlags::default(),
        state_dir,
        actor,
    )?;
    let line = closing.and_then(|closing| closing.deliver(&store, target, in_herdr, host));
    Ok((result, line))
}

/// A task about to leave its open record by your hand, with a dispatched agent to tell.
struct Closing {
    /// How many records the task had closed before this change.
    closed_before: usize,
}

impl Closing {
    fn before(task: &crate::domain::Task, actor: &str) -> Option<Self> {
        (actor == OWNER && task.block.is_some() && task.dispatch.is_some()).then_some(Self {
            closed_before: task.past_blocks.len(),
        })
    }

    /// After the change is durable: when it closed the record, send the agent what that
    /// record owes it ([`dispatch::closing_message`]) and return the status-row line. A block
    /// left for ready sends only when it holds replies.
    fn deliver(
        self,
        store: &TaskStore,
        target: TaskAddress,
        in_herdr: bool,
        host: &mut impl DispatchHost,
    ) -> Option<String> {
        let state = store.load().ok()?;
        let task = state.tasks().iter().find(|task| target.matches(task))?;
        let closed_now = task.past_blocks.len() == self.closed_before + 1
            && task.past_blocks.last()?.closed_by.as_deref() == Some(OWNER);
        let message = closed_now
            .then(|| dispatch::closing_message(task))
            .flatten()?;
        if task.status == HumanStatus::Ready && message.entries == 0 {
            return None;
        }
        let assignee = task.agent_assignee()?;
        let (_, check) = dispatch::start_route_checked(task, OWNER, in_herdr, host);
        let delivery =
            dispatch::deliver_reply(task, &check, message.label, message.text.as_deref(), host)?;
        let status = if task.status == HumanStatus::Ready {
            "ready"
        } else {
            "started"
        };
        Some(dispatch::delivery_message(
            status,
            assignee,
            &message.noun,
            &delivery,
        ))
    }
}

struct Found {
    id: Uuid,
    number: Option<u64>,
    title: String,
    current: HumanStatus,
    soft_deleted: bool,
    open: Option<BlockKind>,
}

fn apply(
    state: &mut DomainState,
    found: Option<Found>,
    status: HumanStatus,
    flags: &BlockFlags,
    on: Option<BlockOn>,
    actor: &str,
) -> (Result<StatusResult, StatusError>, bool) {
    let Some(found) = found else {
        return (Err(StatusError::UnknownTask), false);
    };
    let Some(number) = found.number else {
        return (Err(StatusError::UnknownTask), false);
    };
    if found.soft_deleted {
        return (Err(StatusError::SoftDeletedTask), false);
    }
    let result = StatusResult {
        number,
        title: found.title,
        status,
    };
    let outcome = if status == HumanStatus::Blocked && !flags.is_empty() {
        let draft = match block_draft(state, flags, on, number) {
            Ok(draft) => draft,
            Err(error) => return (Err(error), false),
        };
        if found.open == Some(BlockKind::Blocked) {
            state.edit_block(found.id, patch_from(flags, draft))
        } else {
            state.block(found.id, draft, actor).map(|()| true)
        }
    } else if status == HumanStatus::Review && !flags.is_empty() {
        let draft = match ReviewDraft::from_input(
            flags.done.as_deref(),
            &flags.checks,
            flags.next.as_deref(),
            on.unwrap_or_default(),
        ) {
            Ok(draft) => draft,
            Err(field) => return (Err(StatusError::TextTooLong(field)), false),
        };
        if found.open == Some(BlockKind::Review) {
            state.edit_review(found.id, review_patch_from(flags, draft))
        } else {
            state.review(found.id, draft, actor).map(|()| true)
        }
    } else if found.current == status {
        Ok(false)
    } else {
        state.set_status_by(found.id, status, actor).map(|()| true)
    };
    match outcome {
        Ok(changed) => (Ok(result), changed),
        Err(DomainError::UnknownId(_)) => (Err(StatusError::UnknownTask), false),
        Err(other) => (Err(StatusError::Store(other.to_string())), false),
    }
}

/// Validate the flags into a draft. A task `--on` names must be another task on the board.
fn block_draft(
    state: &DomainState,
    flags: &BlockFlags,
    on: Option<BlockOn>,
    own_number: u64,
) -> Result<BlockDraft, StatusError> {
    let on = on.unwrap_or_default();
    if let BlockOn::Task(number) = on {
        let exists = state
            .tasks()
            .iter()
            .any(|task| task.number == Some(number) && !task.soft_deleted && !task.is_notice());
        if number == own_number || !exists {
            return Err(StatusError::InvalidBlocker);
        }
    }
    BlockDraft::from_input(
        flags.why.as_deref(),
        flags.needs.as_deref(),
        &flags.options,
        on,
    )
    .map_err(StatusError::TextTooLong)
}

/// Re-blocking replaces only the fields that were given.
fn patch_from(flags: &BlockFlags, draft: BlockDraft) -> BlockPatch {
    BlockPatch {
        why: flags.why.is_some().then_some(draft.why),
        needs: flags.needs.is_some().then_some(draft.needs),
        options: (!flags.options.is_empty()).then_some(draft.options),
        on: flags.on.is_some().then_some(draft.on),
    }
}

/// Re-running review replaces only the fields that were given.
fn review_patch_from(flags: &BlockFlags, draft: ReviewDraft) -> ReviewPatch {
    ReviewPatch {
        done: flags.done.is_some().then_some(draft.done),
        checks: (!flags.checks.is_empty()).then_some(draft.checks),
        next: flags.next.is_some().then_some(draft.next),
        on: flags.on.is_some().then_some(draft.on),
    }
}

#[cfg(test)]
mod start_tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::dispatch::CreatedWorktree;
    use crate::domain::{Dispatch, ProvenanceOrigin, TaskScope};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("tsk-cli-start-{label}-{nanos}-{seq}"));
            std::fs::create_dir_all(&dir).expect("state dir");
            std::fs::write(
                dir.join("config.toml"),
                "[agent.builder]\ncommand = [\"true\"]\n",
            )
            .expect("profiles");
            Temp(dir)
        }

        fn store(&self) -> TaskStore {
            TaskStore::new(&self.0)
        }

        /// One project task, optionally assigned to `builder` and dispatched before.
        fn task(&self, assigned: bool, dispatched: bool, status: HumanStatus) -> u64 {
            let mut state = DomainState::new();
            let id = state
                .create_assigned(
                    "start me",
                    None,
                    TaskScope::Project {
                        path: "/repos/app".into(),
                    },
                    ProvenanceOrigin::Manual,
                    None,
                    assigned.then(|| "builder".into()),
                )
                .expect("create");
            self.store().reload_merge_save(&mut state).expect("save");
            if dispatched {
                state
                    .record_dispatch(
                        id,
                        Dispatch {
                            argv: vec!["true".into()],
                            worktree: "/tmp/tsk-cli-start-earlier".into(),
                            branch: "tsk/earlier".into(),
                            base: Some("main".into()),
                            base_ref: None,
                            base_commit: None,
                            base_remote: None,
                            herdr_workspace_id: "w0".into(),
                            at: std::time::SystemTime::now(),
                            cleaned: false,
                        },
                    )
                    .expect("record");
                self.store().reload_merge_save(&mut state).expect("save");
            }
            state.set_status(id, status).expect("status");
            self.store().reload_merge_save(&mut state).expect("save");
            state.get(id).and_then(|task| task.number).expect("number")
        }

        fn status(&self, number: u64) -> HumanStatus {
            self.store()
                .load()
                .expect("load")
                .tasks()
                .iter()
                .find(|task| task.number == Some(number))
                .expect("task")
                .status
        }

        fn start(
            &self,
            number: u64,
            flags: StartFlags,
            actor: &str,
            host: &mut Host,
        ) -> Result<StartOutcome, StatusError> {
            run_started_with_host(
                TaskAddress::Number(number),
                flags,
                Some(self.0.clone()),
                true,
                actor,
                host,
            )
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Records every pane command (one per launch, a relaunch included).
    #[derive(Default)]
    struct Host {
        ran: usize,
        agent: Option<bool>,
        root: Option<crate::dispatch::RootPaneError>,
        /// Every pane command fails, as a launch Herdr refuses.
        refuse_launch: bool,
        /// Another writer edits the task titled `second` in this store as its launch is refused,
        /// so the save putting it back to ready meets a changed task.
        edit_on_refusal: Option<PathBuf>,
        /// A launch leaves this store unreadable, so saving its record fails.
        break_store_on_launch: Option<PathBuf>,
        /// A whole-store restore of this document lands in this store as a launch runs.
        restore_on_launch: Option<(PathBuf, Vec<u8>)>,
        /// Every workspace whose worktree was removed.
        removed: Vec<String>,
    }

    impl DispatchHost for Host {
        fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
            Ok(true)
        }

        fn resolve_base(&mut self, _: &Path) -> Result<String, String> {
            Ok("main".into())
        }

        fn create_worktree(
            &mut self,
            _: &Path,
            branch: &str,
            _: Option<&str>,
            _: &str,
        ) -> Result<CreatedWorktree, String> {
            Ok(CreatedWorktree {
                path: "/tmp/tsk-cli-start-worktree".into(),
                branch: branch.into(),
                workspace_id: "w1".into(),
                root_pane_id: "w1:p1".into(),
            })
        }

        fn root_pane(&mut self, _: &str) -> Result<String, crate::dispatch::RootPaneError> {
            match self.root.clone() {
                Some(error) => Err(error),
                None => Ok("w0:p1".into()),
            }
        }

        fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
            if self.refuse_launch {
                if let Some(dir) = self.edit_on_refusal.take() {
                    let store = TaskStore::new(dir);
                    let mut other = store.load().expect("load");
                    let id = other
                        .tasks()
                        .iter()
                        .find(|task| task.title == "second")
                        .expect("second")
                        .id;
                    other
                        .set_base(id, Some("other".into()))
                        .expect("edit elsewhere");
                    store.reload_merge_save(&mut other).expect("save elsewhere");
                }
                return Err("pane refused".into());
            }
            if let Some(dir) = self.break_store_on_launch.take() {
                std::fs::write(dir.join("tsk.json"), "{").expect("break the store");
            }
            if let Some((dir, document)) = self.restore_on_launch.take() {
                TaskStore::new(dir)
                    .restore_document(&document, std::time::SystemTime::now())
                    .expect("restore");
            }
            self.ran += 1;
            Ok(())
        }

        fn remove_herdr_worktree(&mut self, workspace_id: &str) -> Result<(), String> {
            self.removed.push(workspace_id.into());
            Ok(())
        }

        fn pane_has_agent(&mut self, _: &str) -> Result<bool, String> {
            self.agent.ok_or_else(|| "herdr did not answer".to_string())
        }
    }

    const PLAIN: StartFlags = StartFlags {
        again: false,
        no_dispatch: false,
        force: false,
    };

    /// A start whose launch ran but whose record could not be saved takes the launch back: a
    /// retry then starts one agent, never a second beside the first.
    #[test]
    fn a_start_whose_record_cannot_be_saved_takes_its_launch_back() {
        let temp = Temp::new("unrecorded");
        let number = temp.task(true, false, HumanStatus::Ready);
        let store = std::fs::read(temp.0.join("tsk.json")).expect("read");
        let mut host = Host {
            break_store_on_launch: Some(temp.0.clone()),
            ..Host::default()
        };
        let error = temp
            .start(number, PLAIN, "you", &mut host)
            .expect_err("the record save fails");
        let StatusError::Store(detail) = error else {
            panic!("a store error, exit 3: {error:?}");
        };
        assert!(detail.ends_with("; launch undone"), "{detail}");
        assert_eq!((host.ran, host.removed.len()), (1, 1));

        std::fs::write(temp.0.join("tsk.json"), store).expect("repair the store");
        let outcome = temp.start(number, PLAIN, "you", &mut host).expect("retry");
        assert!(matches!(outcome, StartOutcome::Dispatched(..)));
        assert_eq!(temp.status(number), HumanStatus::Started);
        assert_eq!(host.ran - host.removed.len(), 1, "one agent runs");
    }

    #[test]
    fn an_unassigned_start_only_sets_the_status() {
        let temp = Temp::new("unassigned");
        let number = temp.task(false, false, HumanStatus::Ready);
        let mut host = Host::default();
        let outcome = temp.start(number, PLAIN, "you", &mut host).expect("start");
        assert!(matches!(outcome, StartOutcome::Status(_)));
        assert_eq!(temp.status(number), HumanStatus::Started);
        assert_eq!(host.ran, 0);
    }

    /// A person assignee (you, or a named person) never dispatches: a start only sets started,
    /// even when the name matches a profile, and a dispatch record from an earlier agent
    /// assignee never asks to relaunch.
    #[test]
    fn a_person_assignees_start_only_sets_the_status() {
        for (person, dispatched) in [
            (crate::domain::Assignee::you(), false),
            (crate::domain::Assignee::person("builder"), false),
            (crate::domain::Assignee::person("dana"), true),
        ] {
            let temp = Temp::new("person");
            let number = temp.task(true, dispatched, HumanStatus::Ready);
            let mut state = temp.store().load().expect("load");
            let id = state.tasks()[0].id;
            state
                .assign(id, Some(person.clone()))
                .expect("assign person");
            temp.store().reload_merge_save(&mut state).expect("save");
            let mut host = Host {
                agent: Some(false),
                ..Host::default()
            };
            let outcome = temp
                .start(number, PLAIN, "you", &mut host)
                .expect("plain start");
            assert!(
                matches!(outcome, StartOutcome::Status(_)),
                "{person:?}: {outcome:?}"
            );
            assert_eq!(temp.status(number), HumanStatus::Started);
            assert_eq!(host.ran, 0, "{person:?} launched nothing");
        }
    }

    #[test]
    fn an_assigned_start_dispatches_and_no_dispatch_does_not() {
        let temp = Temp::new("assigned");
        let number = temp.task(true, false, HumanStatus::Ready);
        let mut host = Host::default();
        let outcome = temp.start(number, PLAIN, "you", &mut host).expect("start");
        assert!(matches!(outcome, StartOutcome::Dispatched(..)));
        assert_eq!(host.ran, 1);
        assert_eq!(temp.status(number), HumanStatus::Started);

        let other = Temp::new("assigned-no-dispatch");
        let number = other.task(true, false, HumanStatus::Ready);
        let mut host = Host::default();
        let flags = StartFlags {
            no_dispatch: true,
            ..PLAIN
        };
        let outcome = other.start(number, flags, "you", &mut host).expect("start");
        assert!(matches!(outcome, StartOutcome::Status(_)));
        assert_eq!(host.ran, 0);
        assert_eq!(other.status(number), HumanStatus::Started);
    }

    #[test]
    fn an_assigned_start_outside_herdr_or_on_the_desk_starts_and_says_why() {
        let temp = Temp::new("no-herdr");
        let number = temp.task(true, false, HumanStatus::Ready);
        let mut host = Host::default();
        let outcome = run_started_with_host(
            TaskAddress::Number(number),
            PLAIN,
            Some(temp.0.clone()),
            false,
            "you",
            &mut host,
        )
        .expect("plain start");
        assert!(matches!(
            outcome,
            StartOutcome::NoLaunch(_, crate::dispatch::NO_LAUNCH_NOT_IN_HERDR)
        ));
        assert_eq!(temp.status(number), HumanStatus::Started);
        assert_eq!(host.ran, 0);

        let desk = Temp::new("desk");
        let mut state = DomainState::new();
        state
            .create_assigned(
                "desk errand",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
                Some("builder".into()),
            )
            .expect("create");
        desk.store().reload_merge_save(&mut state).expect("save");
        let outcome = desk.start(1, PLAIN, "you", &mut host).expect("plain start");
        assert!(matches!(
            outcome,
            StartOutcome::NoLaunch(_, crate::dispatch::NO_LAUNCH_DESK)
        ));
        assert_eq!(desk.status(1), HumanStatus::Started);
        assert_eq!(host.ran, 0);
    }

    /// The agent's own `tsk status N started` never launches another copy, whatever Herdr
    /// says about its pane (agent status flaps).
    #[test]
    fn the_agents_own_start_on_its_dispatched_task_launches_nothing() {
        for agent in [Some(true), Some(false), None] {
            let temp = Temp::new("own");
            let number = temp.task(true, true, HumanStatus::Review);
            let mut host = Host {
                agent,
                ..Host::default()
            };
            let outcome = temp
                .start(number, PLAIN, "builder", &mut host)
                .expect("start");
            assert!(matches!(outcome, StartOutcome::Status(_)), "{agent:?}");
            assert_eq!(host.ran, 0, "{agent:?}");
            assert_eq!(temp.status(number), HumanStatus::Started);
        }
    }

    #[test]
    fn a_running_agent_makes_a_plain_start() {
        for agent in [Some(true), None] {
            let temp = Temp::new("running");
            let number = temp.task(true, true, HumanStatus::Review);
            let mut host = Host {
                agent,
                ..Host::default()
            };
            let outcome = temp.start(number, PLAIN, "you", &mut host).expect("start");
            // From review, the plain start sends the round back and says how delivery went.
            let StartOutcome::Delivered(_, line) = outcome else {
                panic!("{agent:?}: {outcome:?}");
            };
            let expected = match agent {
                Some(_) => "started · feedback not sent: @builder is not in its pane; feedback kept on the task",
                None => "started · could not reach @builder; feedback kept on the task",
            };
            assert_eq!(line, expected, "{agent:?}");
            assert_eq!(host.ran, 0, "{agent:?}");
        }
    }

    #[test]
    fn a_gone_agent_refuses_unless_again_or_no_dispatch() {
        let temp = Temp::new("gone");
        let number = temp.task(true, true, HumanStatus::Review);
        let mut host = Host {
            agent: Some(false),
            ..Host::default()
        };
        let error = temp
            .start(number, PLAIN, "you", &mut host)
            .expect_err("agent gone");
        assert_eq!(error.code(), "agent-gone");
        assert_eq!(temp.status(number), HumanStatus::Review);
        assert_eq!(host.ran, 0);

        let flags = StartFlags {
            no_dispatch: true,
            ..PLAIN
        };
        assert!(matches!(
            temp.start(number, flags, "you", &mut host),
            Ok(StartOutcome::Status(_))
        ));
        assert_eq!(host.ran, 0);
        assert_eq!(temp.status(number), HumanStatus::Started);

        let flags = StartFlags {
            again: true,
            ..PLAIN
        };
        assert!(matches!(
            temp.start(number, flags, "you", &mut host),
            Ok(StartOutcome::Dispatched(..))
        ));
        assert_eq!(
            host.ran, 1,
            "--again relaunches even an already started task"
        );
    }

    /// Herdr failing to answer (not a definitive `workspace_not_found`) and a start outside
    /// Herdr are uncertainty: a plain start, never `agent-gone`, never a launch.
    #[test]
    fn uncertainty_about_a_dispatched_agent_is_a_plain_start() {
        let temp = Temp::new("root-failed");
        let number = temp.task(true, true, HumanStatus::Review);
        let mut host = Host {
            root: Some("could not run herdr".into()),
            ..Host::default()
        };
        let outcome = temp.start(number, PLAIN, "you", &mut host).expect("start");
        assert!(
            matches!(&outcome, StartOutcome::Delivered(_, line) if line == "started · could not reach @builder; feedback kept on the task"),
            "{outcome:?}"
        );
        assert_eq!(host.ran, 0);

        let temp = Temp::new("not-in-herdr-dispatched");
        let number = temp.task(true, true, HumanStatus::Review);
        let mut host = Host {
            agent: Some(false),
            root: Some(crate::dispatch::RootPaneError::WorkspaceGone("gone".into())),
            ..Host::default()
        };
        let outcome = run_started_with_host(
            TaskAddress::Number(number),
            PLAIN,
            Some(temp.0.clone()),
            false,
            "you",
            &mut host,
        )
        .expect("start");
        assert!(
            matches!(&outcome, StartOutcome::Delivered(_, line) if line == "started · feedback not sent: not in Herdr"),
            "{outcome:?}"
        );
        assert_eq!(host.ran, 0);
        assert_eq!(temp.status(number), HumanStatus::Started);
    }

    #[test]
    fn a_workspace_herdr_reports_gone_is_agent_gone() {
        let temp = Temp::new("workspace-gone");
        let number = temp.task(true, true, HumanStatus::Review);
        let mut host = Host {
            root: Some(crate::dispatch::RootPaneError::WorkspaceGone("gone".into())),
            ..Host::default()
        };
        let error = temp
            .start(number, PLAIN, "you", &mut host)
            .expect_err("gone");
        assert_eq!(error.code(), "agent-gone");
    }

    /// Starting an assigned done or archived task is a status correction: no launch, no
    /// refusal.
    #[test]
    fn an_assigned_done_or_archived_start_is_a_plain_correction() {
        let temp = Temp::new("done");
        let number = temp.task(true, false, HumanStatus::Done);
        let mut host = Host::default();
        let outcome = temp.start(number, PLAIN, "you", &mut host).expect("start");
        assert!(matches!(outcome, StartOutcome::Status(_)));
        assert_eq!(temp.status(number), HumanStatus::Started);
        assert_eq!(host.ran, 0);

        let temp = Temp::new("archived");
        let number = temp.task(true, false, HumanStatus::Ready);
        let mut state = temp.store().load().expect("load");
        let id = state.tasks()[0].id;
        state.archive_task(id).expect("archive");
        temp.store().reload_merge_save(&mut state).expect("save");
        let outcome = temp.start(number, PLAIN, "you", &mut host).expect("start");
        assert!(matches!(outcome, StartOutcome::Status(_)));
        assert_eq!(temp.status(number), HumanStatus::Started);
        assert_eq!(host.ran, 0);
    }

    /// A prerequisite and an assigned ready task waiting on it, in one project.
    fn released_pair(temp: &Temp) -> (u64, u64) {
        let mut state = DomainState::new();
        let scope = TaskScope::Project {
            path: "/repos/app".into(),
        };
        let first = state
            .create("first", None, scope.clone(), ProvenanceOrigin::Manual, None)
            .expect("create");
        let second = state
            .create_assigned(
                "second",
                None,
                scope,
                ProvenanceOrigin::Manual,
                None,
                Some("builder".into()),
            )
            .expect("create");
        temp.store().reload_merge_save(&mut state).expect("save");
        let number = |id| state.get(id).and_then(|task| task.number).expect("number");
        let (first_number, second_number) = (number(first), number(second));
        state.set_after(second, &[first_number]).expect("after");
        temp.store().reload_merge_save(&mut state).expect("save");
        state.set_status(second, HumanStatus::Ready).expect("ready");
        temp.store().reload_merge_save(&mut state).expect("save");
        (first_number, second_number)
    }

    #[test]
    fn a_done_dispatches_an_assigned_task_it_released() {
        let temp = Temp::new("released-dispatch");
        let (first, second) = released_pair(&temp);
        let mut host = Host::default();
        let (result, released) = run_done_with_host(
            TaskAddress::Number(first),
            Some(temp.0.clone()),
            true,
            "you",
            &mut host,
        )
        .expect("done");
        assert_eq!(result.status, HumanStatus::Done);
        assert_eq!(host.ran, 1, "the waiting task's agent launched");
        assert!(matches!(
            released.as_slice(),
            [dispatch::Released { number, after, start: dispatch::ReleasedStart::Dispatched(_) }]
                if *number == second && *after == first
        ));
        assert_eq!(temp.status(first), HumanStatus::Done);
        assert_eq!(temp.status(second), HumanStatus::Started);
        let state = temp.store().load().expect("load");
        let task = state
            .tasks()
            .iter()
            .find(|task| task.number == Some(second))
            .expect("task");
        assert!(task.dispatch.is_some());
        let event = task.history.last().expect("event");
        assert_eq!(
            event.detail.as_ref().and_then(|detail| detail.after),
            Some(first)
        );
        assert_eq!(
            dispatch::released_message(&released).as_deref(),
            Some(format!("T{second} dispatched to @builder · T{first} done").as_str())
        );
    }

    #[test]
    fn a_failed_launch_keeps_the_released_task_ready_and_the_done() {
        let temp = Temp::new("released-failed");
        let (first, second) = released_pair(&temp);
        let mut host = Host {
            refuse_launch: true,
            ..Host::default()
        };
        let (_, released) = run_done_with_host(
            TaskAddress::Number(first),
            Some(temp.0.clone()),
            true,
            "you",
            &mut host,
        )
        .expect("the done stands");
        assert!(matches!(
            released.as_slice(),
            [dispatch::Released {
                start: dispatch::ReleasedStart::BackToReady(_),
                ..
            }]
        ));
        assert_eq!(temp.status(first), HumanStatus::Done);
        assert_eq!(temp.status(second), HumanStatus::Ready);
        let message = dispatch::released_message(&released).expect("message");
        assert!(
            message.starts_with(&format!("T{second} back to ready: ")),
            "{message}"
        );
    }

    /// The launch fails and the save putting the task back to ready fails too: the task stays
    /// started on disk with no agent, and the CLI warns instead of failing the durable done.
    #[test]
    fn a_failed_launch_whose_rollback_cannot_save_says_it_is_started() {
        let temp = Temp::new("released-rollback");
        let (first, second) = released_pair(&temp);
        let mut host = Host {
            refuse_launch: true,
            edit_on_refusal: Some(temp.0.clone()),
            ..Host::default()
        };
        let (_, released) = run_done_with_host(
            TaskAddress::Number(first),
            Some(temp.0.clone()),
            true,
            "you",
            &mut host,
        )
        .expect("the done stands");
        assert_eq!(temp.status(first), HumanStatus::Done);
        assert_eq!(temp.status(second), HumanStatus::Started);
        let warnings = dispatch::released_warnings(&released);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].starts_with(&format!(
                "T{second} is started but no agent launched; couldn't save it back to ready: "
            )),
            "{warnings:?}"
        );
        let message = dispatch::released_message(&released).expect("message");
        assert!(message.starts_with(&warnings[0]), "{message}");
    }

    /// A restore lands while the agent a done released launches. Its record cannot save over the
    /// restored store: the done stands as the command's result, and the warning says the agent
    /// runs without a record and to check or stop it before running again.
    #[test]
    fn a_restore_while_a_released_agent_launches_keeps_the_restored_store() {
        let temp = Temp::new("released-restore");
        let (first, second) = released_pair(&temp);
        let backup = std::fs::read(temp.0.join("tsk.json")).expect("backup");
        let mut host = Host {
            restore_on_launch: Some((temp.0.clone(), backup)),
            ..Host::default()
        };
        let (result, released) = run_done_with_host(
            TaskAddress::Number(first),
            Some(temp.0.clone()),
            true,
            "you",
            &mut host,
        )
        .expect("the done itself saved before the restore");
        assert_eq!(result.status, HumanStatus::Done);
        assert_eq!(host.ran, 1, "the agent launched before the restore");
        let warnings = dispatch::released_warnings(&released);
        assert_eq!(warnings.len(), 1, "{released:?}");
        assert!(
            warnings[0].starts_with(&format!(
                "T{second}'s agent @builder is running but its record did not save"
            )) && warnings[0].ends_with(
                "the store was restored from a backup while this command ran; its changes are \
                 not in the restored store; check or stop it before running again"
            ),
            "{warnings:?}"
        );
        assert_eq!(temp.status(first), HumanStatus::Open, "the restored store");
        assert_eq!(temp.status(second), HumanStatus::Ready);
        let state = temp.store().load().expect("load");
        assert!(state.tasks().iter().all(|task| task.dispatch.is_none()));
    }

    /// A restore lands after a done's transition and before its launch tail loads the store. The
    /// tail refuses before launching: the restored store's release may not be the done's.
    #[test]
    fn a_restore_before_the_launch_tail_launches_nothing() {
        let temp = Temp::new("released-restore-gap");
        let (first, second) = released_pair(&temp);
        let backup = std::fs::read(temp.0.join("tsk.json")).expect("backup");
        let store = temp.store();
        let mut state = store.load().expect("load");
        let token = state.restore_token().map(str::to_owned);
        let id = |number| {
            state
                .tasks()
                .iter()
                .find(|task| task.number == Some(number))
                .expect("task")
                .id
        };
        let launches = vec![(id(second), first)];
        state
            .set_status(id(first), HumanStatus::Done)
            .expect("done");
        store.reload_merge_save(&mut state).expect("the done");
        store
            .restore_document(&backup, std::time::SystemTime::now())
            .expect("restore");
        let restored = std::fs::read(temp.0.join("tsk.json")).expect("restored");

        let mut host = Host::default();
        let error = launch_released_after_done(
            &store,
            &temp.0,
            true,
            &mut host,
            launches,
            token.as_deref(),
        )
        .expect_err("the store was restored since the done");
        let StatusError::Store(detail) = error else {
            panic!("a store error, exit 3: {error:?}");
        };
        assert!(
            detail.starts_with("the store was restored from a backup while this command ran")
                && detail.ends_with("; nothing launched: check the task, then run it again"),
            "{detail}"
        );
        assert_eq!(host.ran, 0, "no agent launched");
        assert_eq!(
            std::fs::read(temp.0.join("tsk.json")).expect("live"),
            restored
        );
    }

    #[test]
    fn a_released_task_outside_herdr_starts_and_says_why() {
        let temp = Temp::new("released-no-herdr");
        let (first, second) = released_pair(&temp);
        let mut host = Host::default();
        let (_, released) = run_done_with_host(
            TaskAddress::Number(first),
            Some(temp.0.clone()),
            false,
            "you",
            &mut host,
        )
        .expect("done");
        assert_eq!(host.ran, 0);
        assert_eq!(temp.status(second), HumanStatus::Started);
        assert_eq!(
            dispatch::released_message(&released).as_deref(),
            Some(format!("T{second} started · no launch: not in Herdr · T{first} done").as_str())
        );
    }
}
