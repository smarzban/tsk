# Architecture

Scoped delta of smarzban/tsk at `5b30b8d6475114bb1312f4bb451f65216d3e7990` (parent range starts at `36610e8f012018e3ddd624304dbcf680ae4df56c`). Two commits. `952eddf` pins the golden-scene clock in tests. `5b30b8d` edits contributor docs, deletes two unused helpers, corrects mouse comments, and moves picker rustdoc. tsk is a local terminal board for one OS user. Agents on that account use the CLI. There is no network service and no second tenant.

Protected state is `tsk.json` under the state directory. Unix creates it mode `0600` inside a `0700` directory (`src/fsperm.rs`). Windows sets no DACL in this tree. Dispatch launchers are scripts under that directory. Profile argv substitutes `{number} {title} {notes} {steps} {worktree} {branch} {base}` only (`src/agents.rs`).

This delta does not change store, dispatch, launcher, or `terminal_text` code.

- `terminal_text` (`src/ui/mod.rs`) still encodes every `char::is_control` character, which covers C0 and C1, as `\u{XXXX}`. `present_line` calls it. The doc still says bidi and format controls are not reinterpreted.
- `CleanupRow::merge_skip_reason` and `BoardModel::chrome_row_is_plain` are gone. A tree search finds no remaining names. `merge_skip_reason` built a display string. `pr::mergeable` (`src/dispatch/pr.rs`) still refuses a missing, closed, or wrong-base pull request. `CleanupRow::pr_line` still calls that helper.
- `chrome_row_is_plain` was `self.form.is_none()` and had no callers. Paint and hit testing still use `open_field_edit`, `form`, and `board_hit_map`.
- `resolve_board_surface` (`src/app.rs`) returns `board_input_mode_for_area`, which returns the model mode. It does not mutate the model and does not close a surface on shrink. The new comments match that body. The function body is unchanged.
- `paint_scope_dropdown` (`src/ui/render.rs`) lost only a moved doc comment and a stranded clippy allow. The function body is unchanged.
- `AGENTS.md` now says dispatch is refused when neither `HERDR_ENV=1` nor a non-empty `REX_SESSION` is set. `HostKind::from_env` (`src/dispatch/host.rs`) already implements that. `launch_unavailable` refuses a launch when the host is not hosted.
- Tests pin `BoardModel` to `UNIX_EPOCH + 1_700_000_000` via `set_clock`. The fixture age string changes from `1088d` to `41m`. Production `now()` still uses `SystemTime::now` when no clock is set.

No comparable product is named in the repo docs. Offline checks would be `cargo test`. This host has `unshare` and `prlimit`. It has no bubblewrap, firejail, or disk-quota tool, and a read-only toolchain mount was not shown together with an empty environment and a disk bound. Target code stays unexecuted. A Windows DACL cannot be read here.

Prior run `origin/security-audit/2026-11-08` at `36610e8` had no confirmed finding. `windows-state-dir-acl` is carried. `src/fsperm.rs`, `src/store.rs`, `src/dispatch.rs`, and `src/agents.rs` are unchanged, and the prior line numbers still match. Same-source covered units whose functions are outside this diff stay out of scope. New work is the deleted helpers, the mouse comments, the picker painter, the dispatch sentence, the test clock, and an obvious-exposure pass on the diff.

Companions: desktop local-file ownership for the Windows lead only. Web, cloud, RPC, browser DOM, memory-safety, supply-chain, AI prompt, and resource-exhaustion classes do not match this diff. The deleted helpers are not the terminal encoder from the earlier escape fix.
