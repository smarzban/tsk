//! Shared integration executable. Keep process-state-mutating suites isolated in Cargo.toml.
//! Add new suites here; cli_discovery checks that every root test file is registered once.

mod archive_launch_card;
mod cli_archive;
mod cli_discovery;
mod cli_edit;
mod cli_guide;
mod cli_help;
mod cli_router_process;
mod cli_status;
mod cli_steps;
mod cli_terminal_escape;
mod cli_trash;
mod docs_parity;
mod e2e_persist;
mod edit_target_binding;
mod f6_save_recovery;
mod host_scripts;
mod host_scripts_windows;
mod manifest;
mod queue_board_edit;
mod queue_board_model;
mod queue_board_mouse;
mod queue_board_render;
mod queue_board_verbs;
mod quick_add_capture;
mod quick_capture_process;
mod scope_resolver;
mod setup_herdr;
mod setup_safety;
mod store_persist;
mod trash_store;
mod v1_keymap_guard;
mod wide_task_split;
