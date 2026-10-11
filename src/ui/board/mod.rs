//! Queue-board presentation, split by state, commands, reduction, chrome, and drawing.

mod apply;
mod block;
mod chrome;
mod cleanup_card;
mod commands;
mod draw;
mod model;

pub use apply::{
    apply_intent, board_intent_may_persist, NO_AGENT_PROFILES, ROW_DOUBLE_CLICK_WINDOW,
};
pub use block::{BlockCard, BlockCardField, BlockTarget, OnKind};
#[cfg(test)]
pub(crate) use block::{BLOCK_REPLACED, NOTHING_TO_SEND_BACK};
pub use chrome::DELETE_NOTICE_UNDO;
pub use cleanup_card::{CleanupChoice, CleanupOption, RowVerdict, CLEANUP_ENTER_GUARD};
pub use commands::{resolve_board_command, BoardCommand, CommandSurface};
pub use draw::{board_hit_map, board_verb_items, draw_board};
pub use model::{
    nothing_to_dispatch, project_option_label, BoardInputMode, BoardModel, BoardTab, BulkCleanup,
    BulkDispatchRun, CleanupPr, CleanupPrompt, CleanupRow, CleanupRowState, CleanupRun,
    CleanupRunRow, DispatchPrompt, FilterTab, IntentOutcome, ListPickerKind, MergedPr,
    PendingLaunch, PickerTab, ProjectScopeOption, ProjectsView, RelaunchPrompt, SaveResolution,
    StartAnywayPrompt, BOARD_TITLE, CLEANUP_BUSY, FINISHING_CLEANUP, REFRESHING_BRANCHES,
};
