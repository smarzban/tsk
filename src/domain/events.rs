//! Provenance origin and append-only task event history.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// How a task was created. Allowed set includes at least these three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceOrigin {
    Manual,
    Capture,
    Selection,
}

/// Kind of domain mutation recorded on a task's event history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventKind {
    Created,
    Edited,
    StatusSet,
    Completed,
    Reopened,
    SoftDeleted,
    Restored,
    Archived,
    Unarchived,
    StepAdded,
    StepChecked,
    StepUnchecked,
    StepRenamed,
    StepRemoved,
    Assigned,
}

/// One append-only history record on a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub kind: TaskEventKind,
    #[serde(with = "super::time_serde")]
    pub at: SystemTime,
}
