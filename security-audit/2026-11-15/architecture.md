# Architecture

Scoped delta of smarzban/tsk at `d1430bff16823bf3647bace6016dac30bfa8981f`, parent `93d085e8244b57882eed0c1d1e071039593f0dda`. One commit makes `ctrl+l` re-enter the alternate screen and resend mouse, paste, and keyboard modes, and it removes focus reporting and the focus-in redraw. The worktree is clean. This is a standard scoped pass, so coverage is partial. No agent budget was set.

tsk is a local terminal task board for one OS user. Agents on that account use the CLI. There is no network service and no second tenant. The operator's terminal is the only peer on the board's stdin. Another local OS user is the principal for the carried Windows state-directory question.

Protected state is `tsk.json`, plus launchers under that directory. Unix forces mode `0700` directories and `0600` files in `src/fsperm.rs`. Windows sets no DACL. This delta does not change `src/fsperm.rs`, `src/store.rs`, `src/dispatch.rs`, or `src/agents.rs`.

The board loop is `run_board_loop` in `src/app.rs`. It enables terminal input once through `src/ui/mouse.rs`. `requested_terminal_input` asks for mouse capture and bracketed paste, plus keyboard disambiguation when the host reports support. `FocusChange` is gone. `TerminalInputGuard::restore` writes `EnterAlternateScreen`, then for each enabled capability writes a keyboard pop before the push, or the matching enable. Those calls are fixed crossterm commands. Task text is not an argument. `Drop` still disables the enabled set in reverse.

`is_redraw_request` is true only for a Press of `Char('l')` whose modifiers are exactly `CONTROL`. `redraw_on_request` calls `restore`, then `forget_frame`, which resizes to the current size and does not query the cursor. The loop continues, so that event does not reach `map_key`, paste, or mouse dispatch. `FocusGained` and `FocusLost` fall through the match wildcard. Repeat and Release are not Press, so they are not redrawn and the key arm ignores them. Bare `l` remains peek or header unfold. `ctrl+l` is not a `NORMAL_KEYMAP` status chord.

`terminal_text` escapes `char::is_control`. `wrap_text`, `present_line`, and the cleanup card's `wrapped_rows` call it before cells are built. The paint after `ctrl+l` is the next `draw_board`, the same painter. OSC 52 copies base64 of selected painted text and is not on the restore path.

The repo names Herdr and Rex as terminal hosts, and pins crossterm 0.29.0 and ratatui 0.30.2. Neither crate is vendored, and their sources are not in the local cargo registry. How a host applies DECSET 1049 or a keyboard-flag stack, and how crossterm turns bytes into `Event`, is outside this tree.

In-memory tests in `src/app.rs` and `src/ui/mouse.rs` name the mode bytes. They were not run. This host can combine `unshare` user, mount, and net namespaces, read-only binds, a sized tmpfs, `env -i`, `prlimit`, and `timeout`. `target/` and the cargo registry are absent, and fetching is prohibited, so the crate was not compiled. No scratch promoter was built, so scratch files are not evidence. A Windows DACL cannot be read here.

Prior audit `origin/security-audit/2026-11-14` at `83011bc2bdcc667ead8636cfec278529d2078f10` reviewed `93d085e`. It confirmed nothing. `windows-state-dir-acl` stays `needs_validation`: the permission helpers are unchanged, the record is carried, and the Phase 3 verifier is that unit's first owner. The repaint gate and the mode-byte guard changed and are reopened. Same-source covered units outside this delta stay out of scope. New work is an obvious-exposure pass on this diff and the paint path the redraw uses for task text.

Companions: `DESKTOP-MOBILE-AND-LOCAL-IPC.md` local-file ownership for the Windows lead only. Web, cloud, RPC, browser DOM, memory-safety, supply-chain, AI, data-isolation, and resource-exhaustion classes do not match this diff.
