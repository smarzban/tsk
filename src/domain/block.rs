//! Blocks: why a task is blocked, what it needs, and the replies that answer it.
//!
//! A task carries at most one open block while its human status is `blocked`. Leaving
//! `blocked` by any route closes it; closed blocks stay on the task, read-only.

use std::time::SystemTime;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The author name of the board's owner, for blocks, replies and closers.
pub const OWNER: &str = "you";

/// Longest why, needs, option or reply text accepted, in bytes. Longer text is refused,
/// never truncated.
pub const BLOCK_TEXT_MAX: usize = 4096;

/// What a block record is. Only `blocked` exists today; the field is kept so a later
/// record kind (a review card) can reuse the shape without another schema change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    #[default]
    Blocked,
}

impl BlockKind {
    fn is_blocked(&self) -> bool {
        *self == BlockKind::Blocked
    }
}

/// Who or what the block waits on. Serialized as `you`, `task:<number>` or `other:<text>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum BlockOn {
    #[default]
    You,
    Task(u64),
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

    /// The short label painted on board rows: `T169` or the free text.
    pub fn label(&self) -> String {
        match self {
            BlockOn::You => OWNER.to_string(),
            BlockOn::Task(number) => format!("T{number}"),
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
}

impl Block {
    /// A fresh open block from validated draft fields.
    pub fn open(draft: BlockDraft, by: &str, at: SystemTime) -> Self {
        Self {
            kind: BlockKind::Blocked,
            why: draft.why,
            needs: draft.needs,
            options: draft.options,
            on: draft.on,
            by: by.to_string(),
            at,
            edited_at: None,
            replies: Vec::new(),
            closed_at: None,
            closed_by: None,
        }
    }

    /// The last reply that was not deleted.
    pub fn last_reply(&self) -> Option<&Reply> {
        self.replies.iter().rev().find(|reply| !reply.deleted)
    }

    /// The owner has answered: the last live reply is the owner's.
    pub fn answered(&self) -> bool {
        self.last_reply().is_some_and(Reply::is_owner)
    }
}

/// Validated content for a new block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockDraft {
    pub why: Option<String>,
    pub needs: Option<String>,
    pub options: Vec<String>,
    pub on: BlockOn,
}

/// Which block text was too long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockField {
    Why,
    Needs,
    Option,
    Reply,
    On,
}

impl BlockField {
    pub fn name(self) -> &'static str {
        match self {
            BlockField::Why => "why",
            BlockField::Needs => "needs",
            BlockField::Option => "option",
            BlockField::Reply => "reply",
            BlockField::On => "on",
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
    std::env::var("TSK_AGENT")
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
