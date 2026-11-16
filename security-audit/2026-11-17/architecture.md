# Architecture

Scoped delta of smarzban/tsk at `d6016c00a673c76e6a0dd30de9cb6ad498c50be3`, parent `d1430bff16823bf3647bace6016dac30bfa8981f`. Four commits: dead `resolve_name` removal, Windows `ZipFile.ExtractToDirectory`, `tsk checks`, and step-progress labels. The worktree is clean. Profile is standard and scoped, so coverage is partial. No agent budget was set.

tsk is a local terminal task board for one OS user. Agents on that account use the CLI. There is no network service and no second tenant. Protected state is `tsk.json`. Unix modes are `0700` and `0600` in `src/fsperm.rs`. Windows sets no DACL. This delta does not change `src/fsperm.rs`.

Comparable hosts named in the repo are Herdr, Rex, and a GitHub release zip checked over HTTPS. They calibrate the installer and the board. They do not dismiss a demonstrated defect.

`tsk checks` enters at `src/main.rs` `Surface::Checks`, then `parse_flag_checks` and `checks::run`. The parser accepts a task id, the action `set`, a position of at least 1, and the state `open`, `passed`, or `failed`. `--state-dir` is an optional path. `set_check` updates one check on an open review and does not change human status. A same-state set does not save. Success stdout is `terminal_text` of the stored check text. Usage errors go through `terminal_text`. A store I/O or JSON error is printed raw, as on older verbs.

`terminal_text` rewrites `char::is_control`, which is Unicode `Cc` (C0 and C1). `append_row` escapes the title, then appends `steps {done}/{total}` from counts. The board label uses `present_line`, which calls `terminal_text`, before the span. `step_progress_label` formats only those counts, and only for started, blocked, and review rows that have steps. The demo joins the same count and passes the string through `esc` before `innerHTML`.

The wide header state is unchanged: `task_header_state` copies `project_name` into a span. `ProjectNames::name` returns the folder basename when there is no title. `bound_line` keeps a fitting line. That is the carried `ui.paint.project-basename.c0` path. Group-header words are the same class. This delta does not edit those functions. Line numbers in `src/ui/render.rs` shifted because `step_progress_label` was inserted above them.

`resolve_name` had no callers at the parent commit. `resolve_assignee` remains the profile-name gate.

The Windows installer still verifies SHA256, then requires exactly three entry names, `tsk.exe`, `LICENSE`, and `README.md`, and a size cap, then calls `ExtractToDirectory`. Only `tsk.exe` is copied out. Whether the .NET extractor would honor `..` is unobservable here. The allowlist rejects those names before that call. A swap of the temp zip after the check is a same-account race unless the temp directory is shared, which source does not record.

Offline `cargo test` cannot run: there is no `vendor/` tree and no cargo registry cache, and fetching crates is prohibited. `unshare` user, mount, and net namespaces work. bubblewrap and firejail are absent. No scratch promoter was built. Target code was not executed. Windows DACL, .NET extract behavior, and ratatui's tty bytes for a raw span are not in this tree.

Prior audit `origin/security-audit/2026-11-15` at `6b5743070220fc033f8acc33db3b3ffd3146a0bb` reviewed `d1430bff`. It confirmed nothing. Carry `ui.paint.project-basename.c0` and `windows-state-dir-acl`. Their permission and header-span controls are not logically changed. Phase 3 owns those units. The unfinished presenter injection unit is reopened because `presenter.rs` gained `checks` and list step counts. Same-source units outside this delta stay out of scope.

Companions: `DESKTOP-MOBILE-AND-LOCAL-IPC.md` for the Windows state-directory lead and the installer path; `SUPPLY-CHAIN-AND-RELEASE.md` for the release zip handoff; `CLIENT-SIDE.md` for the demo label. Web, cloud, RPC, memory-safety, AI, data-isolation, and resource-exhaustion classes do not match this diff.
