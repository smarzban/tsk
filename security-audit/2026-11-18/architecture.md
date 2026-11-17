# Architecture (delta 2026-11-18)

tsk is a local terminal task board for one operator and the agents they run. It stores tasks in `tsk.json`, paints a ratatui board, and serves a static browser demo. There is no account system. The operator's state directory is the trust boundary with other OS users. Task text is untrusted at the terminal: `terminal_text` in `src/ui/mod.rs` rewrites C0/C1 controls before most paints and CLI lines.

This run reviews `d6016c00..d5b1e7f` (due dates, store format 7 with refusal of versions 8-18, `tsk edit --thread` / `--clear-thread`, releases v0.14.0 and v0.14.1) plus code those edits touch. Prior audit `security-audit/2026-11-17` at `fee08e72` covered an earlier delta ending at `d6016c00`. No comparable product is named in the repo docs.

## Stack and execution

Rust binary (`ratatui`, `crossterm`, serde JSON) and an Astro site. Offline `cargo test` is the natural check. This host has `cargo` but no crate registry and no `target/`. A build would fetch crates, which this run forbids. User namespaces can drop the network. Target-controlled execution is blocked. Scratch files are not evidence.

## Paths in this delta

Due text enters at `parse_due` (`src/domain/due.rs`), CLI `--due` (`src/cli/add.rs`, `src/cli/edit.rs`), quick-add `!d` (`src/capture.rs`), and the task-page Due draft (`src/ui/board/apply.rs` `form_due`). Stored days are `YYYY-MM-DD`. Refusals interpolate the raw token, then `set_message` and `present_line` run `terminal_text`. List and task-page meta call `terminal_text` on the stored day. `tsk list --json` emits `due` through serde JSON, which escapes controls. Load does not re-run `parse_due` (`src/store.rs` `load_document`).

`--thread` is `normalize_thread` at the argv boundary (`src/cli/parser.rs`). The refusal text does not include the raw name. `--clear-thread` stores no thread.

Store format is 7 (`STORE_FORMAT_VERSION`). `load_document` migrates versions below 7, reads 7 as-is, and refuses 0 and anything above 7, including dev formats 8-18. `migrate_v6_to_v7` fills dispatch `host` and renames `herdr_workspace_id` to `session`. A dev v7 file is read by `DispatchWire`, which aliases that field and defaults `host` the same way. `deny_unknown_fields` stays on `Task` and `DomainState`.

`site/public/capture.js` `parseCapture` accepts `!d`. `dueRefusal` embeds the token in `refuse`. `board-demo.js` inserts `state.refuse` and the draft only through `esc`. The demo does not paint `due` into HTML.

Releases v0.14.0 and v0.14.1 change version numbers, changelog, and skill text. They do not change the release workflow.

## Carried leads

`ui.paint.project-basename.c0`: an untitled project's folder name still reaches the task-header state (`src/ui/board/draw.rs` `task_header_state`) and the meta scope string without `terminal_text`. `bound_line` (`src/ui/render.rs`) escapes only when the line is wider than the row. Line numbers moved because the Due footer was inserted. The control flow is the same. Ratatui's tty write of a C0 scalar in a fitting span is still unobserved.

`windows-state-dir-acl`: `src/fsperm.rs` is unchanged. Unix create paths set `0700`/`0600`. Windows creates still set no DACL. This host is Linux.

## Companions

Client-side DOM XSS for the demo (`CLIENT-SIDE.md`). Local file ownership for the state directory (`DESKTOP-MOBILE-AND-LOCAL-IPC.md`). Migration confusion for the format rewrite (`DATA-ISOLATION-AND-LIFECYCLE.md`). Ordinary injection for terminal and CLI text. Supply chain, web auth, memory safety, and LLM companions do not match a new boundary in this delta: the release commits do not change CI or update verification, and the skill text is maintainer copy.

## Starting paths

`src/domain/due.rs`, `src/capture.rs`, `src/cli/add.rs`, `src/cli/edit.rs`, `src/cli/parser.rs`, `src/cli/presenter.rs`, `src/store.rs`, `src/domain/task.rs`, `src/ui/board/draw.rs`, `src/ui/render.rs`, `src/ui/mod.rs`, `src/fsperm.rs`, `src/scope.rs`, `site/public/capture.js`, `site/public/board-demo.js`.
