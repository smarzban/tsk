//! Queue-board presentation, split by state, commands, reduction, chrome, and drawing.

mod apply;
mod chrome;
mod commands;
mod draw;
mod model;

pub use apply::{apply_intent, board_intent_may_persist, NO_AGENT_PROFILES};
pub use chrome::DELETE_NOTICE_UNDO;
pub use commands::{resolve_board_command, BoardCommand, CommandSurface};
pub use draw::{board_hit_map, board_verb_items, draw_board};
pub use model::{
    nothing_to_dispatch, project_option_label, BoardInputMode, BoardModel, BoardTab, BulkCleanup,
    BulkDispatchRun, CleanupPrompt, CleanupRow, DispatchPrompt, FilterTab, IntentOutcome,
    ListPickerKind, PendingLaunch, PickerTab, ProjectScopeOption, ProjectsView, SaveResolution,
    BOARD_TITLE, REFRESHING_BRANCHES,
};
