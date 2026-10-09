//! Headless replies to a task's open block.

use std::path::PathBuf;

use uuid::Uuid;

use crate::cli::parser::TaskAddress;
use crate::dispatch::{self, Delivery, DispatchHost, StartRoute};
use crate::domain::{actor_from_env, DomainError, DomainState, OWNER};
use crate::store::{default_state_dir, TaskStore};

/// A stored reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyResult {
    pub number: u64,
    pub title: String,
    pub by: String,
}

/// A reply failure after parsing and before presenting output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    UnknownTask,
    SoftDeletedTask,
    NotBlocked,
    EmptyReply,
    TextTooLong,
    Store(String),
}

impl ReplyError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTask => "unknown-task",
            Self::SoftDeletedTask => "soft-deleted-task",
            Self::NotBlocked => "not-blocked",
            Self::EmptyReply => "empty-reply",
            Self::TextTooLong => "text-too-long",
            Self::Store(_) => "store-error",
        }
    }
}

/// What `--send` did with a stored reply. The task's status never changes here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// Submitted to the agent `@assignee`.
    Sent { assignee: String },
    /// Herdr refused or failed; the reply stays on the task only.
    NotDelivered {
        assignee: String,
        delivery: Delivery,
    },
    /// Nothing was tried, and why.
    Skipped(String),
}

impl SendOutcome {
    /// The CLI line under the reply acknowledgement.
    pub fn line(&self) -> String {
        match self {
            Self::Sent { assignee } => format!("sent to @{assignee}"),
            Self::NotDelivered {
                assignee,
                delivery: Delivery::AgentWaiting,
            } => format!("not sent: @{assignee} is waiting on a prompt"),
            Self::NotDelivered {
                assignee,
                delivery: Delivery::NotInPane,
            } => format!("not sent: @{assignee} is not in its pane"),
            Self::NotDelivered {
                delivery: Delivery::NotInHerdr,
                ..
            } => format!("not sent: {}", dispatch::NO_LAUNCH_NOT_IN_HERDR),
            Self::NotDelivered { assignee, .. } => format!("not sent: could not reach @{assignee}"),
            Self::Skipped(reason) => format!("not sent: {reason}"),
        }
    }
}

/// `tsk reply --send`: store the reply, then deliver that reply (and only it) to the task's
/// running dispatched agent. The task stays blocked.
pub fn run_send(
    target: TaskAddress,
    text: &str,
    state_dir: Option<PathBuf>,
) -> Result<(ReplyResult, SendOutcome), ReplyError> {
    let (state_dir, mut host) = crate::cli::dispatch::system_host(state_dir);
    run_send_with_host(
        target,
        text,
        Some(state_dir),
        &actor_from_env(),
        dispatch::running_inside_herdr(),
        &mut host,
    )
}

pub fn run_send_with_host(
    target: TaskAddress,
    text: &str,
    state_dir: Option<PathBuf>,
    by: &str,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> Result<(ReplyResult, SendOutcome), ReplyError> {
    let result = run_as(target, text, state_dir.clone(), by)?;
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    let outcome = match store.load() {
        Ok(state) => match state.tasks().iter().find(|task| target.matches(task)) {
            Some(task) => send(task, text, &result.by, in_herdr, host),
            None => SendOutcome::Skipped("the task is gone".into()),
        },
        Err(error) => SendOutcome::Skipped(format!("could not read the board: {error}")),
    };
    Ok((result, outcome))
}

fn send(
    task: &crate::domain::Task,
    text: &str,
    by: &str,
    in_herdr: bool,
    host: &mut impl DispatchHost,
) -> SendOutcome {
    let skip = |reason: &str| SendOutcome::Skipped(reason.to_string());
    let Some(assignee) = task.assignee.clone() else {
        return skip("the task has no assignee");
    };
    if by != OWNER {
        return skip("only your replies are sent");
    }
    if task.dispatch.is_none() {
        return skip("never dispatched; a start dispatches it");
    }
    let check = match dispatch::start_route_checked(task, OWNER, in_herdr, host) {
        (StartRoute::AgentGone { .. }, _) => {
            return SendOutcome::Skipped(format!("@{assignee} is gone"))
        }
        (_, check) => check,
    };
    match dispatch::deliver_reply(task, &check, "reply", Some(text), host) {
        Some(Delivery::Sent) => SendOutcome::Sent { assignee },
        Some(delivery) => SendOutcome::NotDelivered { assignee, delivery },
        None => skip("no running agent"),
    }
}

/// Add a reply to the task's open block. The author is the dispatched agent named by
/// `TSK_AGENT`, or `you`. Not idempotent: each run adds a reply.
pub fn run(
    target: TaskAddress,
    text: &str,
    state_dir: Option<PathBuf>,
) -> Result<ReplyResult, ReplyError> {
    run_as(target, text, state_dir, &actor_from_env())
}

fn run_as(
    target: TaskAddress,
    text: &str,
    state_dir: Option<PathBuf>,
    by: &str,
) -> Result<ReplyResult, ReplyError> {
    let store = TaskStore::new(state_dir.unwrap_or_else(default_state_dir));
    store
        .locked_transition_if_changed(|state: &mut DomainState| {
            let found = state
                .tasks()
                .iter()
                .find(|task| target.matches(task))
                .map(|task| (task.id, task.number, task.title.clone(), task.soft_deleted));
            Ok(apply(state, found, text, by))
        })
        .map_err(ReplyError::Store)?
}

fn apply(
    state: &mut DomainState,
    found: Option<(Uuid, Option<u64>, String, bool)>,
    text: &str,
    by: &str,
) -> (Result<ReplyResult, ReplyError>, bool) {
    let Some((id, Some(number), title, soft_deleted)) = found else {
        return (Err(ReplyError::UnknownTask), false);
    };
    if soft_deleted {
        return (Err(ReplyError::SoftDeletedTask), false);
    }
    match state.reply(id, text, by) {
        Ok(_) => (
            Ok(ReplyResult {
                number,
                title,
                by: by.to_string(),
            }),
            true,
        ),
        Err(DomainError::NotBlocked(_)) => (Err(ReplyError::NotBlocked), false),
        Err(DomainError::EmptyReply) => (Err(ReplyError::EmptyReply), false),
        Err(DomainError::TextTooLong(_)) => (Err(ReplyError::TextTooLong), false),
        Err(DomainError::UnknownId(_)) => (Err(ReplyError::UnknownTask), false),
        Err(other) => (Err(ReplyError::Store(other.to_string())), false),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{run_send_with_host, SendOutcome};
    use crate::cli::parser::TaskAddress;
    use crate::dispatch::{
        CreatedWorktree, Delivery, DispatchHost, PaneAgent, PromptError, RootPaneError,
    };
    use crate::domain::{
        BlockDraft, Dispatch, DomainState, HumanStatus, ProvenanceOrigin, TaskScope,
    };
    use crate::store::TaskStore;

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new(label: &str) -> Self {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("tsk-cli-reply-{label}-{nanos}-{seq}"));
            std::fs::create_dir_all(&dir).expect("state dir");
            Temp(dir)
        }

        /// A blocked project task assigned to `builder`, optionally dispatched before.
        fn blocked(&self, dispatched: bool) -> u64 {
            let store = TaskStore::new(&self.0);
            let mut state = DomainState::new();
            let id = state
                .create_assigned(
                    "agent asks",
                    None,
                    TaskScope::Project {
                        path: "/repos/app".into(),
                    },
                    ProvenanceOrigin::Manual,
                    None,
                    Some("builder".into()),
                )
                .expect("create");
            store.reload_merge_save(&mut state).expect("save");
            if dispatched {
                state
                    .record_dispatch(
                        id,
                        Dispatch {
                            argv: vec!["true".into()],
                            worktree: "/tmp/tsk-cli-reply-earlier".into(),
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
                store.reload_merge_save(&mut state).expect("save");
            }
            let draft = BlockDraft::from_input(Some("which db?"), None, &[], Default::default())
                .expect("draft");
            state.block(id, draft, "builder").expect("block");
            store.reload_merge_save(&mut state).expect("save");
            state.get(id).and_then(|task| task.number).expect("number")
        }

        fn status(&self, number: u64) -> HumanStatus {
            TaskStore::new(&self.0)
                .load()
                .expect("load")
                .tasks()
                .iter()
                .find(|task| task.number == Some(number))
                .expect("task")
                .status
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A Herdr whose recorded workspace `w0` holds `agent` in pane `w0:p1` under `name`.
    /// Lookups only answer for the arguments they were given.
    #[derive(Default)]
    struct Host {
        agent: Option<bool>,
        name: Option<String>,
        gone: bool,
        root_failed: bool,
        prompt_error: Option<PromptError>,
        prompts: Vec<(String, String)>,
        /// The state dir, read at the prompt handoff.
        state: Option<PathBuf>,
        /// Replies on disk (the task's open block) when each prompt was submitted.
        disk_at_prompt: Vec<Vec<String>>,
    }

    impl DispatchHost for Host {
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
            Err("no launches here".into())
        }

        fn root_pane(&mut self, workspace: &str) -> Result<String, RootPaneError> {
            if self.gone || workspace != "w0" {
                return Err(RootPaneError::WorkspaceGone("gone".into()));
            }
            if self.root_failed {
                return Err(RootPaneError::Failed("could not run herdr".into()));
            }
            Ok("w0:p1".into())
        }

        fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
            Err("no launches here".into())
        }

        fn pane_agent(&mut self, pane: &str) -> Result<PaneAgent, String> {
            match self.agent {
                None => Err("herdr did not answer".into()),
                Some(true) if pane == "w0:p1" => Ok(PaneAgent::Present {
                    name: self.name.clone(),
                }),
                Some(_) => Ok(PaneAgent::Absent),
            }
        }

        fn prompt_agent(&mut self, pane: &str, text: &str) -> Result<(), PromptError> {
            if let Some(state) = self.state.as_ref() {
                let disk = TaskStore::new(state).load().expect("load at prompt");
                self.disk_at_prompt.push(
                    disk.tasks()
                        .iter()
                        .filter_map(|task| task.block.as_ref())
                        .flat_map(|block| block.replies.iter().map(|reply| reply.text.clone()))
                        .collect(),
                );
            }
            self.prompts.push((pane.into(), text.into()));
            self.prompt_error.clone().map_or(Ok(()), Err)
        }
    }

    fn running(temp: &Temp, number: u64) -> Host {
        Host {
            agent: Some(true),
            name: Some(crate::dispatch::agent_name(number, "builder")),
            state: Some(temp.0.clone()),
            ..Host::default()
        }
    }

    fn send_text(
        temp: &Temp,
        number: u64,
        text: &str,
        by: &str,
        in_herdr: bool,
        host: &mut Host,
    ) -> SendOutcome {
        run_send_with_host(
            TaskAddress::Number(number),
            text,
            Some(temp.0.clone()),
            by,
            in_herdr,
            host,
        )
        .expect("reply stored")
        .1
    }

    fn send(temp: &Temp, number: u64, by: &str, in_herdr: bool, host: &mut Host) -> SendOutcome {
        send_text(temp, number, "use postgres", by, in_herdr, host)
    }

    #[test]
    fn send_delivers_only_this_reply_and_leaves_the_task_blocked() {
        let temp = Temp::new("sent");
        let number = temp.blocked(true);
        let mut host = running(&temp, number);
        let outcome = send(&temp, number, "you", true, &mut host);
        assert_eq!(outcome.line(), "sent to @builder");
        let outcome = send_text(&temp, number, "add an index", "you", true, &mut host);
        assert_eq!(outcome.line(), "sent to @builder");
        assert_eq!(
            host.prompts,
            [
                (
                    "w0:p1".to_string(),
                    format!("[tsk T{number} reply] use postgres")
                ),
                (
                    "w0:p1".to_string(),
                    format!("[tsk T{number} reply] add an index")
                ),
            ],
            "the second send never repeats the first"
        );
        assert_eq!(
            host.disk_at_prompt,
            [
                vec!["use postgres".to_string()],
                vec!["use postgres".to_string(), "add an index".to_string()]
            ],
            "each reply was durable before its prompt"
        );
        assert_eq!(temp.status(number), HumanStatus::Blocked);
    }

    #[test]
    fn send_says_why_nothing_went_out() {
        let temp = Temp::new("not-sent");
        let probe = temp.blocked(true);
        let named = || running(&temp, probe);
        // (dispatched, author, in Herdr, host, line, prompts)
        let cases: Vec<(bool, &str, bool, Host, &str, usize)> = vec![
            (
                true,
                "you",
                true,
                Host {
                    prompt_error: Some(PromptError::AgentBlocked),
                    ..named()
                },
                "not sent: @builder is waiting on a prompt",
                1,
            ),
            (
                true,
                "you",
                true,
                Host {
                    prompt_error: Some(PromptError::Failed("herdr: boom".into())),
                    ..named()
                },
                "not sent: could not reach @builder",
                1,
            ),
            (
                true,
                "you",
                true,
                Host {
                    root_failed: true,
                    ..named()
                },
                "not sent: could not reach @builder",
                0,
            ),
            (
                true,
                "you",
                true,
                Host {
                    name: Some("reviewer".into()),
                    ..named()
                },
                "not sent: @builder is not in its pane",
                0,
            ),
            (
                true,
                "you",
                true,
                Host {
                    name: None,
                    ..named()
                },
                "not sent: @builder is not in its pane",
                0,
            ),
            (
                true,
                "you",
                true,
                Host {
                    gone: true,
                    ..named()
                },
                "not sent: @builder is gone",
                0,
            ),
            (
                false,
                "you",
                true,
                named(),
                "not sent: never dispatched; a start dispatches it",
                0,
            ),
            (true, "you", false, named(), "not sent: not in Herdr", 0),
            (
                true,
                "reviewer",
                true,
                named(),
                "not sent: only your replies are sent",
                0,
            ),
        ];
        for (dispatched, by, in_herdr, mut host, line, sends) in cases {
            let temp = Temp::new("not-sent");
            let number = temp.blocked(dispatched);
            assert_eq!(number, probe, "each fresh board numbers its task alike");
            host.state = Some(temp.0.clone());
            let outcome = send(&temp, number, by, in_herdr, &mut host);
            assert_eq!(outcome.line(), line);
            assert_eq!(host.prompts.len(), sends, "{line}");
            assert_eq!(temp.status(number), HumanStatus::Blocked);
            if sends == 1 {
                assert!(matches!(outcome, SendOutcome::NotDelivered { delivery, .. }
                    if delivery != Delivery::Sent));
            }
        }
    }
}
