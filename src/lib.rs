//! tsk library root.

pub mod agents;
pub mod announcements;
pub mod app;
pub mod board_pane;
pub mod capture;
pub mod cli;
pub mod context;
pub mod delivery;
pub mod dispatch;
pub mod domain;
pub(crate) mod fsperm;
pub mod guides;
pub mod save_recovery;
pub mod scope;
pub mod setup;
pub mod setup_agent;
pub mod store;
pub(crate) mod text;
pub mod ui;
pub mod update;

pub use board_pane::{find_board_pane_from_stdin, find_board_pane_id};

/// Run the tsk binary entrypoint with argv-style arguments.
///
/// Default mode is the Tasks board. Pass `capture` (or set `TSK_MODE=capture`) for
/// quick capture: the board session seeded onto the expanded quick-add page (exits
/// after save or cancel).
///
/// `--find-board-pane` is handled by the binary (`main`) before this entry.
pub fn run(
    args: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<(), Box<dyn std::error::Error>> {
    app::run(args)
}
