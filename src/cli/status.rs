//! Headless human-status changes.

use std::path::PathBuf;

use crate::agents::AgentProfiles;
use crate::cli::parser::{BlockFlags, TaskAddress};
use crate::dispatch::{self, DispatchError, DispatchHost, DispatchResult, StartRoute};
use crate::domain::{
    actor_from_env, normalize_thread, BlockDraft, BlockField, BlockKind, BlockOn, BlockPatch,
    DomainError, DomainState, HumanStatus, ReviewDraft, ReviewPatch, OWNER,
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
    let state_dir = state_dir.unwrap_or_else(default_state_dir);
    // A review handed to another agent names its profile. A malformed config never blocks
    // the status change: the name is then kept as plain text.
    let review_on = (status == HumanStatus::Review)
        .then(|| block.on.as_deref().map(|on| review_on(on, &state_dir)))
        .flatten();
    let store = TaskStore::new(state_dir);
    let actor = actor_from_env();
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
            Ok(apply(
                state,
                found,
                status,
                &block,
                review_on.clone(),
                &actor,
            ))
        })
        .map_err(StatusError::Store)?
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
}

/// How `started` may launch: `--again` relaunches a gone agent, `--no-dispatch` never launches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartFlags {
    pub again: bool,
    pub no_dispatch: bool,
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
    let plain = |state_dir| {
        run(
            target,
            HumanStatus::Started,
            BlockFlags::default(),
            state_dir,
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
    let again = match dispatch::start_route(task, actor, in_herdr, host) {
        StartRoute::Plain => return plain(state_dir),
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

struct Found {
    id: Uuid,
    number: Option<u64>,
    title: String,
    current: HumanStatus,
    soft_deleted: bool,
    open: Option<BlockKind>,
}

/// Read a review's `--on`: `you`, an agent profile (with or without `@`), or any other text.
fn review_on(value: &str, state_dir: &std::path::Path) -> BlockOn {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case(OWNER) {
        return BlockOn::You;
    }
    let name = value.strip_prefix('@').unwrap_or(value);
    let profile = normalize_thread(name).ok().filter(|name| {
        AgentProfiles::load(state_dir).is_ok_and(|profiles| profiles.get(name).is_some())
    });
    match profile {
        Some(name) => BlockOn::Agent(name),
        None => BlockOn::Other(value.to_string()),
    }
}

fn apply(
    state: &mut DomainState,
    found: Option<Found>,
    status: HumanStatus,
    flags: &BlockFlags,
    review_on: Option<BlockOn>,
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
        let draft = match block_draft(state, flags, number) {
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
            review_on.unwrap_or_default(),
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

/// Validate the flags into a draft. `--on` must name another task on the board.
fn block_draft(
    state: &DomainState,
    flags: &BlockFlags,
    own_number: u64,
) -> Result<BlockDraft, StatusError> {
    let on = flags
        .on
        .as_deref()
        .map(BlockOn::parse_input)
        .unwrap_or_default();
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
                    assigned.then(|| "builder".to_string()),
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
            self.ran += 1;
            Ok(())
        }

        fn pane_has_agent(&mut self, _: &str) -> Result<bool, String> {
            self.agent.ok_or_else(|| "herdr did not answer".to_string())
        }
    }

    const PLAIN: StartFlags = StartFlags {
        again: false,
        no_dispatch: false,
    };

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
            assert!(matches!(outcome, StartOutcome::Status(_)), "{agent:?}");
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
        assert!(matches!(outcome, StartOutcome::Status(_)));
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
        assert!(matches!(outcome, StartOutcome::Status(_)));
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
}
