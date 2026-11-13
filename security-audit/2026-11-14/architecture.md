# Architecture

Scoped delta of smarzban/tsk at `93d085e8244b57882eed0c1d1e071039593f0dda`, parent `5b30b8d6475114bb1312f4bb451f65216d3e7990`. One commit redraws the board on focus in and `ctrl+l`. The worktree is clean. This is a standard scoped pass, so coverage is partial. No agent budget was set.

tsk is a local terminal task board for one OS user. Agents on that account use the CLI. There is no network service and no second tenant. The operator's terminal is the only peer on the board's stdin. Another local OS user is the principal for the carried Windows state-directory question.

Protected state is `tsk.json`. Unix forces mode `0700` directories and `0600` files in `src/fsperm.rs`. Windows sets no DACL. This delta does not change `src/fsperm.rs`, `src/store.rs`, `src/dispatch.rs`, or `src/agents.rs`.

The board loop is `run_board_loop` in `src/app.rs`. It enables terminal input once through `src/ui/mouse.rs`. That module now requests focus reporting together with mouse capture and bracketed paste, plus optional keyboard disambiguation. `TerminalInputGuard` writes fixed crossterm sequences and restores them on drop, in reverse order. The in-repo test records `\x1b[?1004h` before `\x1b[?1004l`. Task text is not interpolated into those bytes. `enable_terminal_input` has one production caller, the board loop, which also serves quick capture.

After each read, `ReflowRowClick::observe` runs. Focus and other non-mouse events clear that gesture. `is_full_repaint_request` is true for `Event::FocusGained` and for a Press of `Char('l')` whose modifiers are exactly `CONTROL`. `forget_frame_on_repaint_request` calls `terminal.resize` at the current size, then the loop continues. That event does not reach `map_key`, paste, mouse dispatch, or the store. The next iteration paints with the existing `draw_board` path. `FocusLost` is ignored. Bare `l`, `alt+l`, and `ctrl+l` Release are not repaints. `ctrl+l` is not a `NORMAL_KEYMAP` status chord. `map_form_edit_key` returns no intent for an unbound Ctrl chord. The new help row in `src/ui/input.rs` is display text.

`terminal_text` is unchanged. A full repaint uses the same painter. Paste stays on `Event::Paste` and runs only when the repaint gate returns false. `map_edit_paste` still returns no intent for Normal, the project picker, and save recovery.

The repo names Herdr and Rex as terminal hosts, and ratatui plus crossterm as the TUI stack. It does not name another task board. Crossterm 0.29.0 is pinned in `Cargo.lock` and is not vendored. How a host turns bytes into `FocusGained` is outside this tree. The in-repo cursor test shows `Terminal::resize` on this path does not call `get_cursor_position`.

Offline tests live in `src/app.rs` and `src/ui/mouse.rs`. This host has `unshare` and `prlimit`. It does not have bubblewrap, firejail, or a disk quota tool. A read-only toolchain mount, an empty environment, and scratch-only writes were not established together. Target code is not executed. Scratch promotion was not built, so scratch files are not evidence. A Windows DACL cannot be read here.

Prior audit `origin/security-audit/2026-11-09` at `afddf36b` reviewed `5b30b8d`. It confirmed nothing. `windows-state-dir-acl` stays `needs_validation` because the permission helpers are outside this diff. Same-source covered units outside this delta stay out of scope. The mouse-routing unit is reopened: the loop now continues before the click arm, and focus clears the reflow gesture. The keyboard-chord unit is reopened: `input.rs` gained a `ctrl+l` help row, and that chord now meets the map. New work is the focus-mode bytes, the repaint gate, and an obvious-exposure pass on the diff.

Companions: `DESKTOP-MOBILE-AND-LOCAL-IPC.md` local-file ownership for the Windows lead only. Web, cloud, RPC, browser DOM, memory-safety, supply-chain, AI, data-isolation, and resource-exhaustion classes do not match this diff. A focus flood stays inside the operator's process.
