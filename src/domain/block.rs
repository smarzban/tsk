//! Blocks and reviews: why a task is blocked or what is up for review, and the replies that
//! answer it.
//!
//! A task carries at most one open record: a block while its human status is `blocked`, a
//! review round while it is `review`. Leaving that status by any route closes the record;
//! closed records stay on the task, read-only.

use std::time::SystemTime;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The author name of the board's owner, for blocks, replies and closers.
pub const OWNER: &str = "you";

/// Set by dispatch in the launched agent's environment to its profile name.
pub const AGENT_ENV: &str = "TSK_AGENT";

/// Longest why, needs, option or reply text accepted, in bytes. Longer text is refused,
/// never truncated.
pub const BLOCK_TEXT_MAX: usize = 4096;

/// What a record is: a block (`blocked`) or one review round (`review`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    #[default]
    Blocked,
    Review,
}

impl BlockKind {
    fn is_blocked(&self) -> bool {
        *self == BlockKind::Blocked
    }

    /// The record kind a human status keeps open, if any.
    pub fn for_status(status: super::HumanStatus) -> Option<Self> {
        match status {
            super::HumanStatus::Blocked => Some(BlockKind::Blocked),
            super::HumanStatus::Review => Some(BlockKind::Review),
            _ => None,
        }
    }
}

/// The state of one review check. Enter on the task page cycles open → passed → failed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    #[default]
    Open,
    Passed,
    Failed,
}

impl CheckState {
    fn is_open(&self) -> bool {
        *self == CheckState::Open
    }

    pub fn cycle(self) -> Self {
        match self {
            CheckState::Open => CheckState::Passed,
            CheckState::Passed => CheckState::Failed,
            CheckState::Failed => CheckState::Open,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CheckState::Open => "open",
            CheckState::Passed => "passed",
            CheckState::Failed => "failed",
        }
    }
}

/// One thing the reviewer should check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub text: String,
    #[serde(default, skip_serializing_if = "CheckState::is_open")]
    pub state: CheckState,
}

/// How a review round closed: approved (it went to done) or sent back (it went to started).
/// Any other route out of review closes the round without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Approved,
    SentBack,
}

impl Resolution {
    pub fn name(self) -> &'static str {
        match self {
            Resolution::Approved => "approved",
            Resolution::SentBack => "sent_back",
        }
    }
}

/// Who or what the record waits on. Serialized as `you`, `task:<number>`, `agent:<profile>`
/// or `other:<text>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum BlockOn {
    #[default]
    You,
    Task(u64),
    /// An agent profile (a review handed to another agent). Validated against `config.toml`
    /// at the boundary; a removed profile keeps its name.
    Agent(String),
    Other(String),
}

impl BlockOn {
    pub fn is_you(&self) -> bool {
        *self == BlockOn::You
    }

    /// The wire and JSON spelling.
    pub fn wire(&self) -> String {
        match self {
            BlockOn::You => OWNER.to_string(),
            BlockOn::Task(number) => format!("task:{number}"),
            BlockOn::Agent(name) => format!("agent:{name}"),
            BlockOn::Other(text) => format!("other:{text}"),
        }
    }

    fn from_wire(value: &str) -> Option<Self> {
        if value == OWNER {
            return Some(BlockOn::You);
        }
        if let Some(number) = value.strip_prefix("task:") {
            return number.parse().ok().map(BlockOn::Task);
        }
        if let Some(name) = value.strip_prefix("agent:") {
            return (!name.is_empty()).then(|| BlockOn::Agent(name.to_string()));
        }
        value
            .strip_prefix("other:")
            .map(|text| BlockOn::Other(text.to_string()))
    }

    /// Parse a user-typed waiting-on value: `you`, a task address (`T12`, `t12`, `12`), or
    /// any other text. Empty input means `you`.
    pub fn parse_input(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case(OWNER) {
            return BlockOn::You;
        }
        let digits = value
            .strip_prefix('T')
            .or_else(|| value.strip_prefix('t'))
            .unwrap_or(value);
        if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
            if let Ok(number) = digits.parse() {
                return BlockOn::Task(number);
            }
        }
        BlockOn::Other(value.to_string())
    }

    /// The short label painted on board rows: `T169`, `@pi` or the free text.
    pub fn label(&self) -> String {
        match self {
            BlockOn::You => OWNER.to_string(),
            BlockOn::Task(number) => format!("T{number}"),
            BlockOn::Agent(name) => format!("@{name}"),
            BlockOn::Other(text) => text.clone(),
        }
    }
}

impl Serialize for BlockOn {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.wire())
    }
}

impl<'de> Deserialize<'de> for BlockOn {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        BlockOn::from_wire(&value)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid block on value {value:?}")))
    }
}

/// One answer on a block. Agent replies are frozen; owner replies may be edited or
/// soft-deleted, which keeps a stub in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub by: String,
    #[serde(with = "super::time_serde")]
    pub at: SystemTime,
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub edited: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

impl Reply {
    pub fn is_owner(&self) -> bool {
        self.by == OWNER
    }
}

/// One block record. Open while `closed_at` is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    #[serde(default, skip_serializing_if = "BlockKind::is_blocked")]
    pub kind: BlockKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Review: what was done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    /// Review: what the reviewer should check. Never moves into steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    /// Review: what comes after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// Review: the round number, from 1. Zero on a block.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub round: u32,
    #[serde(default, skip_serializing_if = "BlockOn::is_you")]
    pub on: BlockOn,
    /// `you` or the agent profile that made the block.
    pub by: String,
    #[serde(with = "super::time_serde")]
    pub at: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_time")]
    pub edited_at: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_time")]
    pub closed_at: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<String>,
    /// Review: how the owner closed the round, when they decided it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl Block {
    /// A fresh open block from validated draft fields.
    pub fn open(draft: BlockDraft, by: &str, at: SystemTime) -> Self {
        Self {
            kind: BlockKind::Blocked,
            why: draft.why,
            needs: draft.needs,
            options: draft.options,
            done: None,
            checks: Vec::new(),
            next: None,
            round: 0,
            on: draft.on,
            by: by.to_string(),
            at,
            edited_at: None,
            replies: Vec::new(),
            closed_at: None,
            closed_by: None,
            resolution: None,
        }
    }

    /// A fresh open review round from validated draft fields.
    pub fn open_review(draft: ReviewDraft, by: &str, at: SystemTime, round: u32) -> Self {
        Self {
            kind: BlockKind::Review,
            done: draft.done,
            checks: draft.checks,
            next: draft.next,
            round,
            on: draft.on,
            ..Self::open(BlockDraft::default(), by, at)
        }
    }

    pub fn is_review(&self) -> bool {
        self.kind == BlockKind::Review
    }

    /// The texts of the failed checks, in order.
    pub fn failed_checks(&self) -> Vec<&str> {
        self.checks
            .iter()
            .filter(|check| check.state == CheckState::Failed)
            .map(|check| check.text.as_str())
            .collect()
    }

    /// The last reply that was not deleted.
    pub fn last_reply(&self) -> Option<&Reply> {
        self.replies.iter().rev().find(|reply| !reply.deleted)
    }

    /// The owner has answered: the last live reply is the owner's.
    pub fn answered(&self) -> bool {
        self.last_reply().is_some_and(Reply::is_owner)
    }

    /// Who opened this block and when: a closed and reopened block never shares it.
    pub fn key(&self) -> BlockKey {
        BlockKey {
            by: self.by.clone(),
            at: self.at,
        }
    }
}

/// The identity of one block, for sessions that must stay bound to the block they opened on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockKey {
    pub by: String,
    pub at: SystemTime,
}

/// Validated content for a new block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockDraft {
    pub why: Option<String>,
    pub needs: Option<String>,
    pub options: Vec<String>,
    pub on: BlockOn,
}

/// Which block or review text was too long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockField {
    Why,
    Needs,
    Option,
    Reply,
    On,
    Done,
    Check,
    Next,
}

impl BlockField {
    pub fn name(self) -> &'static str {
        match self {
            BlockField::Why => "why",
            BlockField::Needs => "needs",
            BlockField::Option => "option",
            BlockField::Reply => "reply",
            BlockField::On => "on",
            BlockField::Done => "done",
            BlockField::Check => "check",
            BlockField::Next => "next",
        }
    }
}

/// Trim block text; empty becomes none. Refuses text over [`BLOCK_TEXT_MAX`] bytes.
pub fn block_text(value: Option<&str>, field: BlockField) -> Result<Option<String>, BlockField> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > BLOCK_TEXT_MAX {
        return Err(field);
    }
    Ok(Some(value.to_string()))
}

impl BlockDraft {
    /// Build a draft from raw input, trimming every value and dropping empty options.
    pub fn from_input(
        why: Option<&str>,
        needs: Option<&str>,
        options: &[String],
        on: BlockOn,
    ) -> Result<Self, BlockField> {
        let on = match on {
            BlockOn::Other(text) => match block_text(Some(&text), BlockField::On)? {
                Some(text) => BlockOn::Other(text),
                None => BlockOn::You,
            },
            on => on,
        };
        Ok(Self {
            why: block_text(why, BlockField::Why)?,
            needs: block_text(needs, BlockField::Needs)?,
            options: options
                .iter()
                .map(|option| block_text(Some(option), BlockField::Option))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect(),
            on,
        })
    }
}

/// Validated content for a review round.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewDraft {
    pub done: Option<String>,
    pub checks: Vec<Check>,
    pub next: Option<String>,
    pub on: BlockOn,
}

impl ReviewDraft {
    /// Build a draft from raw input, trimming every value and dropping empty checks. Every
    /// check starts open.
    pub fn from_input(
        done: Option<&str>,
        checks: &[String],
        next: Option<&str>,
        on: BlockOn,
    ) -> Result<Self, BlockField> {
        let on = match on {
            BlockOn::Other(text) => match block_text(Some(&text), BlockField::On)? {
                Some(text) => BlockOn::Other(text),
                None => BlockOn::You,
            },
            on => on,
        };
        Ok(Self {
            done: block_text(done, BlockField::Done)?,
            checks: checks
                .iter()
                .map(|check| block_text(Some(check), BlockField::Check))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .map(|text| Check {
                    text,
                    state: CheckState::Open,
                })
                .collect(),
            next: block_text(next, BlockField::Next)?,
            on,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.done.is_none() && self.checks.is_empty() && self.next.is_none() && self.on.is_you()
    }
}

/// Changes to an open review round. `None` keeps a field; `Some` replaces it. Replaced checks
/// keep the state of a check with the same text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewPatch {
    pub done: Option<Option<String>>,
    pub checks: Option<Vec<Check>>,
    pub next: Option<Option<String>>,
    pub on: Option<BlockOn>,
}

impl ReviewPatch {
    pub fn is_empty(&self) -> bool {
        self.done.is_none() && self.checks.is_none() && self.next.is_none() && self.on.is_none()
    }

    /// Every field of `draft`, replacing the round's content wholesale.
    pub fn replace_with(draft: ReviewDraft) -> Self {
        Self {
            done: Some(draft.done),
            checks: Some(draft.checks),
            next: Some(draft.next),
            on: Some(draft.on),
        }
    }
}

/// Changes to an open block. `None` keeps a field; `Some` replaces it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockPatch {
    pub why: Option<Option<String>>,
    pub needs: Option<Option<String>>,
    pub options: Option<Vec<String>>,
    pub on: Option<BlockOn>,
}

impl BlockPatch {
    pub fn is_empty(&self) -> bool {
        self.why.is_none() && self.needs.is_none() && self.options.is_none() && self.on.is_none()
    }

    /// Every field of `draft`, replacing the block's content wholesale.
    pub fn replace_with(draft: BlockDraft) -> Self {
        Self {
            why: Some(draft.why),
            needs: Some(draft.needs),
            options: Some(draft.options),
            on: Some(draft.on),
        }
    }
}

/// The resolved author of a block or reply: the dispatched agent's profile from
/// `TSK_AGENT`, or the owner.
pub fn actor_from_env() -> String {
    std::env::var(AGENT_ENV)
        .ok()
        .and_then(|value| super::normalize_thread(value.trim()).ok())
        .unwrap_or_else(|| OWNER.to_string())
}

mod opt_time {
    use std::time::SystemTime;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        time: &Option<SystemTime>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match time {
            Some(time) => super::super::time_serde::serialize(time, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<SystemTime>, D::Error> {
        #[derive(Deserialize)]
        struct Wrapped(#[serde(with = "super::super::time_serde")] SystemTime);
        Ok(Option::<Wrapped>::deserialize(deserializer)?.map(|Wrapped(time)| time))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_round_trips_its_wire_spelling() {
        for on in [
            BlockOn::You,
            BlockOn::Task(169),
            BlockOn::Other("design review: a:b".into()),
        ] {
            let json = serde_json::to_string(&on).expect("encode");
            assert_eq!(serde_json::from_str::<BlockOn>(&json).expect("decode"), on);
        }
        assert_eq!(
            serde_json::to_string(&BlockOn::Task(7)).unwrap(),
            "\"task:7\""
        );
        assert!(serde_json::from_str::<BlockOn>("\"task:x\"").is_err());
        assert!(serde_json::from_str::<BlockOn>("\"someone\"").is_err());
    }

    #[test]
    fn on_input_reads_you_task_addresses_and_free_text() {
        assert_eq!(BlockOn::parse_input(""), BlockOn::You);
        assert_eq!(BlockOn::parse_input("You"), BlockOn::You);
        assert_eq!(BlockOn::parse_input("T169"), BlockOn::Task(169));
        assert_eq!(BlockOn::parse_input("t5"), BlockOn::Task(5));
        assert_eq!(BlockOn::parse_input("12"), BlockOn::Task(12));
        assert_eq!(
            BlockOn::parse_input(" legal sign-off "),
            BlockOn::Other("legal sign-off".into())
        );
        assert_eq!(BlockOn::parse_input("T"), BlockOn::Other("T".into()));
    }

    #[test]
    fn draft_trims_drops_empty_and_refuses_long_text() {
        let draft = BlockDraft::from_input(
            Some("  why  "),
            Some("   "),
            &["a".into(), " ".into(), " b ".into()],
            BlockOn::Other("  ".into()),
        )
        .expect("valid");
        assert_eq!(draft.why.as_deref(), Some("why"));
        assert_eq!(draft.needs, None);
        assert_eq!(draft.options, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(draft.on, BlockOn::You);

        let long = "x".repeat(BLOCK_TEXT_MAX + 1);
        assert_eq!(
            BlockDraft::from_input(Some(&long), None, &[], BlockOn::You),
            Err(BlockField::Why)
        );
        assert_eq!(
            BlockDraft::from_input(None, None, &[long], BlockOn::You),
            Err(BlockField::Option)
        );
        let exact = "x".repeat(BLOCK_TEXT_MAX);
        assert!(BlockDraft::from_input(Some(&exact), None, &[], BlockOn::You).is_ok());
    }

    #[test]
    fn answered_reads_the_last_live_reply() {
        let mut block = Block::open(BlockDraft::default(), "claude", SystemTime::now());
        assert!(!block.answered());
        let reply = |by: &str, deleted| Reply {
            by: by.into(),
            at: SystemTime::now(),
            text: "x".into(),
            edited: false,
            deleted,
        };
        block.replies.push(reply("claude", false));
        assert!(!block.answered());
        block.replies.push(reply(OWNER, false));
        assert!(block.answered());
        block.replies.push(reply(OWNER, true));
        assert!(block.answered(), "a deleted stub is skipped");
        block.replies.push(reply("claude", false));
        assert!(!block.answered());
    }

    #[test]
    fn minimal_block_wire_omits_defaults_and_refuses_unknown_fields() {
        let block = Block::open(BlockDraft::default(), OWNER, SystemTime::UNIX_EPOCH);
        let value = serde_json::to_value(&block).expect("encode");
        assert_eq!(value, serde_json::json!({"by": "you", "at": [0, 0]}));
        let mut extra = value.clone();
        extra["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Block>(extra).is_err());
        assert_eq!(serde_json::from_value::<Block>(value).unwrap(), block);
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::domain::{DomainError, DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
    use uuid::Uuid;

    fn task(state: &mut DomainState) -> Uuid {
        state
            .create("t", None, TaskScope::Global, ProvenanceOrigin::Manual, None)
            .expect("create")
    }

    fn draft(why: &str) -> BlockDraft {
        BlockDraft::from_input(
            Some(why),
            Some("a decision"),
            &["yes".into(), "no".into()],
            BlockOn::You,
        )
        .expect("draft")
    }

    #[test]
    fn entering_blocked_opens_an_empty_block_and_leaving_closes_it() {
        let mut state = DomainState::new();
        let id = task(&mut state);
        state
            .set_status_by(id, HumanStatus::Blocked, "claude")
            .expect("block");
        let block = state.get(id).unwrap().block.clone().expect("open block");
        assert_eq!(block.by, "claude");
        assert_eq!(block.why, None);

        state.set_status(id, HumanStatus::Ready).expect("unblock");
        let task = state.get(id).unwrap();
        assert_eq!(task.block, None);
        assert_eq!(task.past_blocks.len(), 1);
        assert_eq!(task.past_blocks[0].closed_by.as_deref(), Some(OWNER));
        assert!(task.past_blocks[0].closed_at.is_some());
    }

    #[test]
    fn every_route_out_of_blocked_closes_the_block() {
        let mut state = DomainState::new();
        let completed = task(&mut state);
        let batch = task(&mut state);
        let dispatched = task(&mut state);
        for id in [completed, batch, dispatched] {
            state.block(id, draft("why"), OWNER).expect("block");
        }
        state.complete(completed).expect("complete");
        state.complete_batch(&[batch]).expect("complete batch");
        state
            .record_dispatch(
                dispatched,
                crate::domain::Dispatch {
                    argv: vec!["a".into()],
                    worktree: "/w".into(),
                    branch: "b".into(),
                    base: None,
                    base_ref: None,
                    base_commit: None,
                    base_remote: None,
                    herdr_workspace_id: "w1".into(),
                    at: SystemTime::now(),
                    cleaned: false,
                },
            )
            .expect("dispatch");
        for id in [completed, batch, dispatched] {
            let task = state.get(id).unwrap();
            assert_eq!(task.block, None, "{:?}", task.status);
            assert_eq!(task.past_blocks.len(), 1);
            assert_eq!(task.past_blocks[0].why.as_deref(), Some("why"));
        }
    }

    #[test]
    fn a_second_block_refuses_and_edit_marks_the_same_block_edited() {
        let mut state = DomainState::new();
        let id = task(&mut state);
        state.block(id, draft("first"), "claude").expect("block");
        assert_eq!(
            state.block(id, draft("second"), "claude"),
            Err(DomainError::AlreadyBlocked(id))
        );
        let changed = state
            .edit_block(
                id,
                BlockPatch {
                    why: Some(Some("second".into())),
                    ..BlockPatch::default()
                },
            )
            .expect("edit");
        assert!(changed);
        let block = state.get(id).unwrap().block.clone().unwrap();
        assert_eq!(block.why.as_deref(), Some("second"));
        assert_eq!(block.needs.as_deref(), Some("a decision"));
        assert!(block.edited_at.is_some());
        assert!(state.get(id).unwrap().past_blocks.is_empty());
        assert!(!state.edit_block(id, BlockPatch::default()).expect("noop"));
    }

    #[test]
    fn replies_append_and_only_owner_replies_edit_or_delete() {
        let mut state = DomainState::new();
        let id = task(&mut state);
        assert_eq!(
            state.reply(id, "hi", OWNER),
            Err(DomainError::NotBlocked(id))
        );
        state.block(id, draft("why"), "claude").expect("block");
        let agent = state.reply(id, "more context", "claude").expect("agent");
        let mine = state.reply(id, "  go with yes ", OWNER).expect("owner");
        assert_eq!(state.reply(id, "  ", OWNER), Err(DomainError::EmptyReply));
        assert_eq!(
            state.reply(id, &"x".repeat(BLOCK_TEXT_MAX + 1), OWNER),
            Err(DomainError::TextTooLong(BlockField::Reply))
        );
        assert!(state.get(id).unwrap().block.as_ref().unwrap().answered());

        assert_eq!(
            state.edit_reply(id, agent, "changed"),
            Err(DomainError::UnknownReply(agent))
        );
        state.edit_reply(id, mine, "go with no").expect("edit mine");
        let reply = &state.get(id).unwrap().block.as_ref().unwrap().replies[mine];
        assert_eq!(reply.text, "go with no");
        assert!(reply.edited);

        state.delete_reply(id, mine).expect("delete mine");
        let block = state.get(id).unwrap().block.clone().unwrap();
        assert!(block.replies[mine].deleted, "a stub stays");
        assert_eq!(block.replies.len(), 2);
        assert!(!block.answered());
        assert_eq!(
            state.delete_reply(id, mine),
            Err(DomainError::UnknownReply(mine))
        );
    }

    #[test]
    fn block_batch_is_one_undo_entry_that_restores_each_status() {
        let mut state = DomainState::new();
        let ready = task(&mut state);
        let started = task(&mut state);
        let already = task(&mut state);
        state.set_status(ready, HumanStatus::Ready).unwrap();
        state.set_status(started, HumanStatus::Started).unwrap();
        state.block(already, draft("kept"), OWNER).unwrap();
        let undo_before = state.last_undo().cloned();

        assert!(state
            .block_batch(&[ready, started, already], &draft("shared"), OWNER)
            .expect("batch"));
        for id in [ready, started] {
            let block = state.get(id).unwrap().block.clone().unwrap();
            assert_eq!(block.why.as_deref(), Some("shared"));
            assert_eq!(state.get(id).unwrap().status, HumanStatus::Blocked);
        }
        assert_eq!(
            state
                .get(already)
                .unwrap()
                .block
                .as_ref()
                .unwrap()
                .why
                .as_deref(),
            Some("kept"),
            "an open block is left alone"
        );
        assert!(matches!(
            state.last_undo(),
            Some(crate::domain::UndoEntry::Batch { entries }) if entries.len() == 2
        ));

        state.undo().expect("undo");
        assert_eq!(state.get(ready).unwrap().status, HumanStatus::Ready);
        assert_eq!(state.get(started).unwrap().status, HumanStatus::Started);
        for id in [ready, started] {
            let task = state.get(id).unwrap();
            assert_eq!(task.block, None);
            assert!(task.past_blocks.is_empty(), "undo drops, never closes");
        }
        assert_eq!(state.last_undo().cloned(), undo_before);
    }

    #[test]
    fn a_reply_after_blocking_makes_the_block_undo_stale() {
        let mut state = DomainState::new();
        let id = task(&mut state);
        state.block_batch(&[id], &draft("why"), OWNER).unwrap();
        state.reply(id, "answer", OWNER).unwrap();
        assert_eq!(state.undo(), Err(DomainError::StaleUndo(id)));
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use crate::domain::{DomainError, DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
    use uuid::Uuid;

    fn task(state: &mut DomainState) -> Uuid {
        state
            .create("t", None, TaskScope::Global, ProvenanceOrigin::Manual, None)
            .expect("create")
    }

    fn draft(done: &str, checks: &[&str]) -> ReviewDraft {
        let checks: Vec<String> = checks.iter().map(|check| check.to_string()).collect();
        ReviewDraft::from_input(Some(done), &checks, Some("docs"), BlockOn::You).expect("draft")
    }

    fn round(state: &DomainState, id: Uuid) -> Block {
        state.get(id).unwrap().block.clone().expect("open round")
    }

    #[test]
    fn review_opens_round_one_and_re_running_it_edits_the_same_round() {
        let mut state = DomainState::new();
        let id = task(&mut state);
        state
            .review(
                id,
                draft("built it", &["tests pass", "no flicker"]),
                "claude",
            )
            .expect("review");
        let first = round(&state, id);
        assert!(first.is_review());
        assert_eq!(first.round, 1);
        assert_eq!(first.by, "claude");
        assert_eq!(state.get(id).unwrap().status, HumanStatus::Review);
        assert_eq!(
            state.review(id, draft("again", &[]), "claude"),
            Err(DomainError::AlreadyBlocked(id)),
            "an open round is edited, not reopened"
        );

        state.set_check(id, 0, CheckState::Passed).expect("pass");
        let changed = state
            .edit_review(
                id,
                ReviewPatch {
                    done: Some(Some("built it, fixed flicker".into())),
                    checks: Some(draft("", &["tests pass", "docs build"]).checks),
                    ..ReviewPatch::default()
                },
            )
            .expect("edit");
        assert!(changed);
        let edited = round(&state, id);
        assert_eq!(edited.round, 1, "the same round");
        assert_eq!(edited.key(), first.key());
        assert_eq!(edited.done.as_deref(), Some("built it, fixed flicker"));
        assert_eq!(edited.next.as_deref(), Some("docs"), "untouched field kept");
        assert_eq!(
            edited
                .checks
                .iter()
                .map(|check| (check.text.as_str(), check.state))
                .collect::<Vec<_>>(),
            vec![
                ("tests pass", CheckState::Passed),
                ("docs build", CheckState::Open)
            ],
            "a check with unchanged text keeps its state"
        );
        assert!(edited.edited_at.is_some());
        assert!(state.get(id).unwrap().past_blocks.is_empty());
    }

    #[test]
    fn a_check_cycles_open_passed_failed_and_refuses_outside_review() {
        assert_eq!(CheckState::Open.cycle(), CheckState::Passed);
        assert_eq!(CheckState::Passed.cycle(), CheckState::Failed);
        assert_eq!(CheckState::Failed.cycle(), CheckState::Open);
        let mut state = DomainState::new();
        let id = task(&mut state);
        assert_eq!(
            state.set_check(id, 0, CheckState::Passed),
            Err(DomainError::NotInReview(id))
        );
        state.review(id, draft("x", &["a"]), "claude").unwrap();
        assert_eq!(
            state.set_check(id, 3, CheckState::Passed),
            Err(DomainError::UnknownCheck(3))
        );
        assert!(state.set_check(id, 0, CheckState::Failed).unwrap());
        assert!(!state.set_check(id, 0, CheckState::Failed).unwrap());
        assert_eq!(round(&state, id).failed_checks(), vec!["a"]);
        assert_eq!(round(&state, id).checks[0].state, CheckState::Failed);
    }

    /// Send back (started) closes round N as sent back; the agent's next review opens round
    /// N+1; done closes it as approved; any other route closes it without a resolution.
    #[test]
    fn rounds_close_into_history_with_how_they_ended() {
        let mut state = DomainState::new();
        let id = task(&mut state);
        state
            .review(id, draft("first", &["a", "b"]), "claude")
            .unwrap();
        state.set_check(id, 1, CheckState::Failed).unwrap();
        state.reply(id, "fix b", OWNER).unwrap();
        state
            .set_status(id, HumanStatus::Started)
            .expect("send back");
        let task_now = state.get(id).unwrap();
        assert_eq!(task_now.block, None);
        let closed = task_now.past_blocks.last().unwrap();
        assert_eq!(closed.round, 1);
        assert_eq!(closed.resolution, Some(Resolution::SentBack));
        assert_eq!(closed.failed_checks(), vec!["b"]);
        assert_eq!(closed.replies[0].text, "fix b");

        state
            .set_status_by(id, HumanStatus::Review, "claude")
            .expect("plain review opens a round");
        assert_eq!(round(&state, id).round, 2);
        assert_eq!(round(&state, id).by, "claude");
        state
            .set_status(id, HumanStatus::Ready)
            .expect("back to ready");
        assert_eq!(
            state
                .get(id)
                .unwrap()
                .past_blocks
                .last()
                .unwrap()
                .resolution,
            None
        );

        state.review(id, draft("third", &[]), "claude").unwrap();
        assert_eq!(round(&state, id).round, 3);
        state.complete(id).expect("approve");
        let history = &state.get(id).unwrap().past_blocks;
        assert_eq!(history.len(), 3);
        assert_eq!(history[2].resolution, Some(Resolution::Approved));
        assert!(history.iter().all(Block::is_review));
    }

    #[test]
    fn blocking_a_review_closes_the_round_and_review_closes_a_block() {
        let mut state = DomainState::new();
        let id = task(&mut state);
        state.review(id, draft("x", &[]), "claude").unwrap();
        state
            .block(id, BlockDraft::default(), "claude")
            .expect("block a review");
        let task_now = state.get(id).unwrap();
        assert!(!task_now.block.as_ref().unwrap().is_review());
        assert!(task_now.past_blocks[0].is_review());
        assert_eq!(task_now.past_blocks[0].resolution, None);
        state.review(id, draft("y", &[]), "claude").unwrap();
        let task_now = state.get(id).unwrap();
        assert!(task_now.block.as_ref().unwrap().is_review());
        assert_eq!(task_now.block.as_ref().unwrap().round, 2);
        assert!(!task_now.past_blocks[1].is_review());
        assert_eq!(
            state.edit_block(id, BlockPatch::default()),
            Err(DomainError::NotBlocked(id)),
            "a review round is not a block"
        );
    }

    /// A marked set put up for review is one undo entry; undo restores each status and
    /// reopens a block the review closed.
    #[test]
    fn review_batch_is_one_undo_entry_that_restores_each_status() {
        let mut state = DomainState::new();
        let ready = task(&mut state);
        let blocked = task(&mut state);
        let already = task(&mut state);
        state.set_status(ready, HumanStatus::Ready).unwrap();
        state
            .block(
                blocked,
                BlockDraft::from_input(Some("why"), None, &[], BlockOn::You).unwrap(),
                "claude",
            )
            .unwrap();
        state.review(already, draft("kept", &[]), "claude").unwrap();
        let undo_before = state.last_undo().cloned();

        assert!(state
            .review_batch(&[ready, blocked, already], &draft("shared", &["c"]), OWNER)
            .expect("batch"));
        for id in [ready, blocked] {
            assert_eq!(state.get(id).unwrap().status, HumanStatus::Review);
            assert_eq!(round(&state, id).done.as_deref(), Some("shared"));
        }
        assert_eq!(round(&state, already).done.as_deref(), Some("kept"));
        assert!(matches!(
            state.last_undo(),
            Some(crate::domain::UndoEntry::Batch { entries }) if entries.len() == 2
        ));

        state.undo().expect("undo");
        assert_eq!(state.get(ready).unwrap().status, HumanStatus::Ready);
        assert_eq!(state.get(ready).unwrap().block, None);
        assert!(state.get(ready).unwrap().past_blocks.is_empty());
        let restored = state.get(blocked).unwrap();
        assert_eq!(restored.status, HumanStatus::Blocked);
        assert_eq!(
            restored
                .block
                .as_ref()
                .and_then(|block| block.why.as_deref()),
            Some("why"),
            "the block the review closed is open again"
        );
        assert!(restored.past_blocks.is_empty());
        assert_eq!(state.last_undo().cloned(), undo_before);
    }

    #[test]
    fn review_wire_round_trips_and_agent_on_spells_its_profile() {
        let mut block = Block::open_review(
            ReviewDraft::from_input(
                Some("done"),
                &["a".into()],
                None,
                BlockOn::Agent("pi".into()),
            )
            .unwrap(),
            "claude",
            SystemTime::UNIX_EPOCH,
            2,
        );
        block.checks[0].state = CheckState::Failed;
        block.resolution = Some(Resolution::SentBack);
        let value = serde_json::to_value(&block).expect("encode");
        assert_eq!(
            value,
            serde_json::json!({
                "kind": "review",
                "done": "done",
                "checks": [{"text": "a", "state": "failed"}],
                "round": 2,
                "on": "agent:pi",
                "by": "claude",
                "at": [0, 0],
                "resolution": "sent_back"
            })
        );
        assert_eq!(serde_json::from_value::<Block>(value).unwrap(), block);
        assert_eq!(BlockOn::Agent("pi".into()).label(), "@pi");
        assert!(serde_json::from_str::<BlockOn>("\"agent:\"").is_err());
    }
}
