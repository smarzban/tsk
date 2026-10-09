//! The paper trail: a task's history as readable entries, newest first.
//!
//! One builder feeds the task page's PAPER TRAIL section and the plain `tsk list T` listing.
//! Entries come from the task's events (who and what, when the store recorded it) and its
//! closed blocks and review rounds, which expand in place on the page.

use std::time::{Duration, SystemTime};

use crate::domain::{
    Block, BlockOn, CheckState, CleanupOutcome, EditedField, HumanStatus, Resolution, Task,
    TaskEvent, TaskEventKind, OWNER,
};

/// Entries shown before `+ N earlier` on the page, and by the plain single-task listing.
pub const LATEST: usize = 5;

/// Consecutive repeats closer than this read as one grouped entry.
const GROUP_WINDOW: Duration = Duration::from_secs(60);

/// One paper-trail row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrailEntry {
    /// The newest moment the entry covers.
    pub at: SystemTime,
    /// `you` or an agent profile; `None` for an event an older store recorded.
    pub by: Option<String>,
    /// What happened, without who or when.
    pub text: String,
    /// The closed block or review round (an index into `past_blocks`) this row expands into.
    pub record: Option<usize>,
}

impl TrailEntry {
    /// The entry with its author and age: `open → started · @claude 2m`.
    pub fn line(&self, now: SystemTime) -> String {
        let age = crate::ui::render::format_age(now, self.at);
        match self.by.as_deref() {
            Some(by) => format!("{} · {} {age}", self.text, author(by)),
            None => format!("{} · {age}", self.text),
        }
    }
}

/// `you`, or `@profile`.
pub fn author(by: &str) -> String {
    if by == OWNER {
        OWNER.to_string()
    } else {
        format!("@{by}")
    }
}

/// The task's paper trail, newest first, with consecutive repeats grouped.
pub fn paper_trail(task: &Task) -> Vec<TrailEntry> {
    // Raw rows oldest first: events in recording order, and each closed record just before the
    // status change that closed it (the first one at or after its close), so the trail reads the
    // change first and then what it closed.
    let mut records: Vec<(SystemTime, usize)> = task
        .past_blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.closed_at.unwrap_or(block.at), index))
        .collect();
    records.sort();
    let mut records = records.into_iter().peekable();
    let mut raw: Vec<(SystemTime, Raw<'_>)> = Vec::new();
    for event in &task.history {
        if changes_status(event.kind) {
            while let Some((closed, index)) = records.next_if(|(closed, _)| *closed <= event.at) {
                raw.push((closed, Raw::Record(index)));
            }
        }
        if event_text(event).is_some() {
            raw.push((event.at, Raw::Event(event)));
        }
    }
    raw.extend(records.map(|(closed, index)| (closed, Raw::Record(index))));

    let mut groups: Vec<(Raw<'_>, SystemTime, SystemTime, usize)> = Vec::new();
    for (at, row) in raw.into_iter().rev() {
        if let Some((first, _, oldest, count)) = groups.last_mut() {
            if repeats(first, &row) && oldest.duration_since(at).unwrap_or_default() <= GROUP_WINDOW
            {
                *oldest = at;
                *count += 1;
                continue;
            }
        }
        groups.push((row, at, at, 1));
    }
    groups
        .into_iter()
        .map(|(row, newest, _, count)| match row {
            Raw::Event(event) => TrailEntry {
                at: newest,
                by: event.by.clone(),
                text: grouped_text(event, count),
                record: None,
            },
            Raw::Record(index) => {
                let block = &task.past_blocks[index];
                TrailEntry {
                    at: newest,
                    by: block.closed_by.clone(),
                    text: record_summary(block),
                    record: Some(index),
                }
            }
        })
        .collect()
}

/// Kinds that can close a block or review round.
fn changes_status(kind: TaskEventKind) -> bool {
    matches!(
        kind,
        TaskEventKind::StatusSet
            | TaskEventKind::Completed
            | TaskEventKind::Reopened
            | TaskEventKind::Dispatched
    )
}

#[derive(Clone, Copy)]
enum Raw<'a> {
    Event(&'a TaskEvent),
    Record(usize),
}

/// Two rows group when they are the same event by the same author saying the same thing.
fn repeats(first: &Raw<'_>, next: &Raw<'_>) -> bool {
    match (first, next) {
        (Raw::Event(a), Raw::Event(b)) => {
            a.kind == b.kind && a.by == b.by && event_text(a) == event_text(b)
        }
        _ => false,
    }
}

fn grouped_text(event: &TaskEvent, count: usize) -> String {
    let text = event_text(event).unwrap_or_default();
    if count == 1 {
        return text;
    }
    match text.strip_prefix("step ") {
        Some(verb) => format!("{count} steps {verb}"),
        None => format!("{text} ×{count}"),
    }
}

fn status_word(status: HumanStatus) -> &'static str {
    match status {
        HumanStatus::Open => "open",
        HumanStatus::Ready => "ready",
        HumanStatus::Started => "started",
        HumanStatus::Blocked => "blocked",
        HumanStatus::Review => "review",
        HumanStatus::Done => "done",
    }
}

/// What one event says on the trail, or `None` for one the trail leaves out: edits to an open
/// block or review round (its closed record carries them) and an edit that changed no field.
pub fn event_text(event: &TaskEvent) -> Option<String> {
    let detail = event.detail.as_ref();
    let status = || {
        detail.and_then(|detail| match (detail.from, detail.to) {
            (Some(from), Some(to)) => Some(format!("{} → {}", status_word(from), status_word(to))),
            _ => None,
        })
    };
    Some(match event.kind {
        TaskEventKind::Created => "created".to_string(),
        TaskEventKind::StatusSet => status().unwrap_or_else(|| "status changed".to_string()),
        TaskEventKind::Completed => status().unwrap_or_else(|| "done".to_string()),
        TaskEventKind::Reopened => status().unwrap_or_else(|| "reopened".to_string()),
        TaskEventKind::Edited => match detail {
            None => "edited".to_string(),
            Some(detail) if detail.fields.is_empty() => return None,
            Some(detail) => format!("{} edited", field_list(&detail.fields)),
        },
        TaskEventKind::Assigned => match detail {
            None => "assignee changed".to_string(),
            Some(detail) => match detail.assignee.as_deref() {
                Some(name) => format!("assigned @{name}"),
                None => "unassigned".to_string(),
            },
        },
        TaskEventKind::BaseSet => match detail {
            None => "base changed".to_string(),
            Some(detail) => match detail.base.as_deref() {
                Some(base) => format!("base ⎇ {base}"),
                None => "base cleared".to_string(),
            },
        },
        TaskEventKind::Dispatched => match detail {
            None => "dispatched".to_string(),
            Some(detail) => {
                let mut text = if detail.relaunch {
                    "relaunched".to_string()
                } else {
                    "dispatched".to_string()
                };
                if let Some(branch) = detail.branch.as_deref() {
                    text.push(' ');
                    text.push_str(branch);
                }
                if let Some(base) = detail.base.as_deref() {
                    text.push_str(" from ");
                    text.push_str(base);
                }
                if let Some(sha) = detail.sha.as_deref() {
                    text.push_str(" @ ");
                    text.push_str(sha);
                }
                text
            }
        },
        TaskEventKind::Cleaned => match detail.and_then(|detail| detail.outcome) {
            None => "cleaned".to_string(),
            Some(CleanupOutcome::Removed) => "cleaned · worktree and branch removed".to_string(),
            Some(CleanupOutcome::BranchKept) => {
                "cleaned · worktree removed, branch kept".to_string()
            }
            Some(CleanupOutcome::Missing) => "cleaned · worktree already gone".to_string(),
        },
        TaskEventKind::SoftDeleted => "deleted".to_string(),
        TaskEventKind::Restored => "restored".to_string(),
        TaskEventKind::Archived => "archived".to_string(),
        TaskEventKind::Unarchived => "unarchived".to_string(),
        TaskEventKind::StepAdded => "step added".to_string(),
        TaskEventKind::StepChecked => "step checked".to_string(),
        TaskEventKind::StepUnchecked => "step unchecked".to_string(),
        TaskEventKind::StepRenamed => "step renamed".to_string(),
        TaskEventKind::StepRemoved => "step removed".to_string(),
        TaskEventKind::BlockEdited
        | TaskEventKind::Replied
        | TaskEventKind::ReplyEdited
        | TaskEventKind::ReplyDeleted
        | TaskEventKind::ReviewEdited
        | TaskEventKind::CheckSet => return None,
    })
}

/// `title`, `title and notes`, `title, notes and thread`.
fn field_list(fields: &[EditedField]) -> String {
    let names: Vec<&str> = fields
        .iter()
        .map(|field| match field {
            EditedField::Title => "title",
            EditedField::Notes => "notes",
            EditedField::Thread => "thread",
            EditedField::Project => "project",
        })
        .collect();
    match names.split_last() {
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Who or what a record waits on: `on you`, `on T12`, `on @claude`.
pub fn on_text(on: &BlockOn) -> String {
    match on {
        BlockOn::You => "on you".to_string(),
        BlockOn::Task(number) => format!("on T{number}"),
        BlockOn::Agent(name) => format!("on @{name}"),
        BlockOn::Other(text) => format!("on {text}"),
    }
}

/// The one-line summary of a closed block (`blocked on you · 2 replies`) or review round
/// (`review round 2 · sent back · 1 failed`).
pub fn record_summary(block: &Block) -> String {
    let replies = block.replies.iter().filter(|reply| !reply.deleted).count();
    let mut parts = Vec::new();
    if block.is_review() {
        parts.push(format!("review round {}", block.round.max(1)));
        parts.push(
            match block.resolution {
                Some(Resolution::SentBack) => "sent back",
                Some(Resolution::Approved) => "approved",
                None => "closed",
            }
            .to_string(),
        );
        let failed = block
            .checks
            .iter()
            .filter(|check| check.state == CheckState::Failed)
            .count();
        if failed > 0 {
            parts.push(format!("{failed} failed"));
        }
        if replies > 0 {
            parts.push(plural(replies, "feedback", "feedback"));
        }
    } else {
        parts.push(format!("blocked {}", on_text(&block.on)));
        if let Some(why) = block.why.as_deref() {
            parts.push(why.to_string());
        }
        if replies > 0 {
            parts.push(plural(replies, "reply", "replies"));
        }
    }
    parts.join(" · ")
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// One line of an expanded record: a label (`why    `, `└ you 2m  `) and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordLine {
    pub lead: String,
    pub text: String,
}

/// The lines an expanded closed record shows under its summary row.
pub fn record_lines(block: &Block, now: SystemTime) -> Vec<RecordLine> {
    let age = |at| crate::ui::render::format_age(now, at);
    let line = |lead: &str, text: String| RecordLine {
        lead: lead.to_string(),
        text,
    };
    let mut lines = Vec::new();
    let opened = if block.is_review() {
        format!("set by {} {}", author(&block.by), age(block.at))
    } else {
        format!("blocked by {} {}", author(&block.by), age(block.at))
    };
    lines.push(line("", opened));
    if block.is_review() {
        lines.push(line("", on_text(&block.on)));
        if let Some(done) = block.done.as_deref() {
            lines.push(line("done   ", done.to_string()));
        }
        for check in &block.checks {
            let glyph = match check.state {
                CheckState::Open => "○ ",
                CheckState::Passed => "✓ ",
                CheckState::Failed => "✗ ",
            };
            lines.push(line(glyph, check.text.clone()));
        }
        if let Some(next) = block.next.as_deref() {
            lines.push(line("next   ", next.to_string()));
        }
    } else {
        if let Some(why) = block.why.as_deref() {
            lines.push(line("why    ", why.to_string()));
        }
        if let Some(needs) = block.needs.as_deref() {
            lines.push(line("needs  ", needs.to_string()));
        }
        for option in &block.options {
            lines.push(line("○ ", option.clone()));
        }
    }
    for reply in &block.replies {
        let lead = format!("└ {} {}  ", author(&reply.by), age(reply.at));
        if reply.deleted {
            lines.push(line(&lead, "deleted".to_string()));
        } else {
            lines.push(line(&lead, reply.text.clone()));
        }
    }
    if let Some(closed_at) = block.closed_at {
        let by = block.closed_by.as_deref().map(author);
        lines.push(line(
            "",
            match by {
                Some(by) => format!("closed by {by} {}", age(closed_at)),
                None => format!("closed {}", age(closed_at)),
            },
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        acting_as, BlockDraft, DomainState, EventDetail, ProvenanceOrigin, ReviewDraft, TaskScope,
    };

    fn texts(task: &Task) -> Vec<String> {
        paper_trail(task)
            .into_iter()
            .map(|entry| entry.text)
            .collect()
    }

    fn new_task(state: &mut DomainState) -> uuid::Uuid {
        state
            .create(
                "Ship it",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create")
    }

    #[test]
    fn the_trail_reads_newest_first_with_who_and_what() {
        let mut state = DomainState::new();
        let id = new_task(&mut state);
        state.set_status(id, HumanStatus::Ready).expect("ready");
        acting_as("claude", || {
            state.set_status(id, HumanStatus::Started).expect("start");
        });
        let trail = paper_trail(state.get(id).expect("task"));
        assert_eq!(
            trail
                .iter()
                .map(|entry| (entry.text.as_str(), entry.by.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("ready → started", Some("claude")),
                ("open → ready", Some("you")),
                ("created", Some("you")),
            ]
        );
    }

    #[test]
    fn consecutive_step_checks_within_a_minute_group() {
        let mut state = DomainState::new();
        let id = new_task(&mut state);
        let steps: Vec<_> = (0..3)
            .map(|index| state.add_step(id, format!("s{index}")).expect("step"))
            .collect();
        for step in &steps {
            state.toggle_step(id, *step).expect("check");
        }
        assert_eq!(
            texts(state.get(id).expect("task")),
            vec!["3 steps checked", "3 steps added", "created"]
        );
    }

    #[test]
    fn repeats_further_apart_than_a_minute_stay_separate() {
        let mut state = DomainState::new();
        let id = new_task(&mut state);
        let step = state.add_step(id, "s").expect("step");
        state.toggle_step(id, step).expect("check");
        state.toggle_step(id, step).expect("uncheck");
        state.toggle_step(id, step).expect("check again");
        let mut task = state.get(id).expect("task").clone();
        let last = task.history.len() - 1;
        task.history[last].at += Duration::from_secs(120);
        assert_eq!(
            texts(&task),
            vec![
                "step checked",
                "step unchecked",
                "step checked",
                "step added",
                "created"
            ]
        );
    }

    #[test]
    fn older_events_render_without_who_or_what() {
        let mut state = DomainState::new();
        let id = new_task(&mut state);
        let mut task = state.get(id).expect("task").clone();
        let at = task.created_at;
        task.history = vec![
            TaskEvent::new(TaskEventKind::Created, at),
            TaskEvent::new(TaskEventKind::StatusSet, at + Duration::from_secs(100)),
            TaskEvent::new(TaskEventKind::Edited, at + Duration::from_secs(200)),
        ];
        let trail = paper_trail(&task);
        assert_eq!(
            trail
                .iter()
                .map(|entry| (entry.text.as_str(), entry.by.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("edited", None),
                ("status changed", None),
                ("created", None)
            ]
        );
        assert_eq!(
            trail[0].line(at + Duration::from_secs(260)),
            "edited · 1m",
            "no author on an older event"
        );
    }

    #[test]
    fn edits_name_the_fields_without_a_diff_and_quiet_edits_stay_off() {
        let mut state = DomainState::new();
        let id = new_task(&mut state);
        state
            .edit(
                id,
                "Ship it now",
                Some("why".into()),
                TaskScope::Global,
                None,
            )
            .expect("edit");
        state
            .edit(
                id,
                "Ship it now",
                Some("why".into()),
                TaskScope::Global,
                None,
            )
            .expect("no-op edit");
        assert_eq!(
            texts(state.get(id).expect("task")),
            vec!["title and notes edited", "created"]
        );
    }

    #[test]
    fn closed_blocks_and_review_rounds_are_expandable_rows() {
        let mut state = DomainState::new();
        let id = new_task(&mut state);
        acting_as("claude", || {
            state
                .block(
                    id,
                    BlockDraft {
                        why: Some("need creds".into()),
                        ..BlockDraft::default()
                    },
                    "claude",
                )
                .expect("block");
        });
        state.reply(id, "here", OWNER).expect("reply");
        state.set_status(id, HumanStatus::Started).expect("unblock");
        state
            .review(id, ReviewDraft::default(), "claude")
            .expect("review");
        state
            .set_status(id, HumanStatus::Started)
            .expect("send back");
        let task = state.get(id).expect("task");
        let trail = paper_trail(task);
        let records: Vec<_> = trail
            .iter()
            .filter_map(|entry| entry.record.map(|index| (index, entry.text.as_str())))
            .collect();
        assert_eq!(
            records,
            vec![
                (1, "review round 1 · sent back"),
                (0, "blocked on you · need creds · 1 reply"),
            ]
        );
        assert_eq!(trail[0].text, "review → started", "{trail:#?}");
        assert_eq!(trail[1].text, "review round 1 · sent back");
        let lines = record_lines(&task.past_blocks[0], SystemTime::now());
        assert!(lines.iter().any(|line| line.text == "need creds"));
        assert!(lines.iter().any(|line| line.text == "here"));
    }

    #[test]
    fn dispatch_cleanup_and_assignment_carry_their_detail() {
        let mut state = DomainState::new();
        let id = new_task(&mut state);
        state.assign(id, Some("claude".into())).expect("assign");
        state.set_base(id, Some("origin/dev".into())).expect("base");
        let mut task = state.get(id).expect("task").clone();
        task.history.push(TaskEvent {
            kind: TaskEventKind::Dispatched,
            at: SystemTime::now(),
            by: Some(OWNER.into()),
            detail: Some(EventDetail {
                branch: Some("tsk/t1-ship".into()),
                base: Some("origin/dev".into()),
                sha: Some("abc1234".into()),
                relaunch: true,
                ..EventDetail::default()
            }),
        });
        task.history.push(TaskEvent {
            kind: TaskEventKind::Cleaned,
            at: SystemTime::now(),
            by: Some(OWNER.into()),
            detail: Some(EventDetail {
                outcome: Some(CleanupOutcome::BranchKept),
                ..EventDetail::default()
            }),
        });
        assert_eq!(
            texts(&task),
            vec![
                "cleaned · worktree removed, branch kept",
                "relaunched tsk/t1-ship from origin/dev @ abc1234",
                "base ⎇ origin/dev",
                "assigned @claude",
                "created",
            ]
        );
    }
}
