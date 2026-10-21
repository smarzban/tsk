//! Queue-board presentation, split by state, commands, reduction, chrome, and drawing.

mod apply;
mod block;
mod chrome;
mod cleanup_card;
mod commands;
mod dispatch_card;
mod draw;
mod launch_card;
mod model;
mod selector;

pub use apply::{
    apply_intent, board_intent_may_persist, ctrl_s_starts, ROW_DOUBLE_CLICK_WINDOW,
    SHIFT_ENTER_AS_CTRL_J_HINT,
};
pub use block::{BlockCard, BlockCardField, BlockTarget, OnKind};
#[cfg(test)]
pub(crate) use block::{BLOCK_REPLACED, NOTHING_TO_SEND_BACK};
pub use chrome::DELETE_NOTICE_UNDO;
pub use cleanup_card::{CleanupChoice, CleanupOption, RowVerdict};
pub use commands::{resolve_board_command, BoardCommand, CommandSurface};
pub use dispatch_card::DispatchChoice;
pub use draw::{board_hit_map, board_verb_items, draw_board};
pub use launch_card::{LaunchChoice, LaunchPrompt};
pub use model::{
    nothing_to_dispatch, project_option_label, BoardInputMode, BoardLayout, BoardModel, BoardTab,
    BulkCleanup, BulkDispatchRun, CleanupPr, CleanupPrompt, CleanupRow, CleanupRowState,
    CleanupRun, CleanupRunRow, DispatchPrompt, FilterTab, IntentOutcome, ListPickerKind, MergedPr,
    PendingLaunch, PickerTab, ProjectScopeOption, ProjectsView, RelaunchPrompt, SaveResolution,
    StartAnywayPrompt, BOARD_TITLE, CLEANUP_BUSY, FINISHING_CLEANUP, REFRESHING_BRANCHES,
};
pub use selector::{CardChoice, CardOption, OptionCard, OptionSelector, CARD_ENTER_GUARD};
