# Security audit report

Standard scoped delta of `smarzban/tsk` from `36610e8f012018e3ddd624304dbcf680ae4df56c` to `5b30b8d6475114bb1312f4bb451f65216d3e7990` (2 commits, worktree clean). Budget unset. This is a partial pass: only those two commits, the controls they touch, and the carried Windows state-directory lead were in scope. Execution policy was sandboxed source-and-local-only. Target code was not executed. `unshare` can isolate network. bubblewrap, firejail, and a disk-quota tool are absent, so a read-only toolchain mount was not combined with an empty environment, scratch-only writes, and a disk bound. No deployed service was contacted.

Prior run `origin/security-audit/2026-11-08` reviewed `36610e8` and recorded no confirmed finding. `windows-state-dir-acl` is carried. `src/fsperm.rs`, `src/store.rs`, `src/dispatch.rs`, and `src/agents.rs` are unchanged, and the prior line numbers still match. Phase 5 kept the lead `needs_validation`. There is no same-source confirmed exclusion.

`952eddf` pins golden-scene clocks in tests. `5b30b8d` corrects contributor docs, deletes two unused helpers, rewrites mouse comments, and moves picker rustdoc. `terminal_text` is outside the diff and still encodes every `char::is_control` character.

Judgment agents: 4 reconnaissance, 8 hunters (6 first-pass plus 2 fresh retries after malformed results), 1 post-wave critic, 1 final-clean critic, 1 Phase 3 verifier, 1 Phase 5 verifier. Both critics returned no missing units and no reassignment. The final critic stopped clean.

## Posture

tsk remains a single-user local board. This delta does not change dispatch, the store, launcher scripts, or terminal encoding. The deleted helpers were not the escape control and were not the merge gate. No new confirmed finding. The Windows state-directory ACL is still unresolved on this Linux host.

## Confirmed findings

No confirmed findings.

## NEEDS VALIDATION

| Title | Trace | Blocker | Next step |
| --- | --- | --- | --- |
| Windows state files inherit the parent directory DACL | `src/store.rs` `default_state_dir` to `src/fsperm.rs` `create_private_file`, and `src/dispatch.rs` `write_launcher` on non-Unix | No Windows DACL is set or observable from this Linux tree | Owner runs read-only `icacls` on a fresh non-admin profile and on one launcher script if present. No ACL changes. |

This is not a confirmed vulnerability and has no severity.

## Hardening

- `CleanupRow::merge_skip_reason` returned a display string and had no callers at `36610e8`. `pr::mergeable` still refuses a missing, closed, or wrong-base pull request before `gh pr merge`.
- `BoardModel::chrome_row_is_plain` returned `form.is_none()` and had no callers. Undo paint and the delete-notice hit still share one painter.
- The old mouse comment described a shrink-collapse side effect. `resolve_board_surface` only returns the model input mode, and the new comment matches that body.
- `present_line` still documents that bidi and format controls are left in place. This delta does not add a raw span.
- The golden clock is `set_clock` inside the test binary. Production `now()` still uses `SystemTime::now` when the clock is unset. Expiry uses `Instant::now`.
- Added lines contain no password, token, PEM, security TODO, or production debug switch. `Cargo.lock` is outside the diff.

## Positive patterns

`terminal_text` still encodes C0 and C1. Picker option rows, the picker title (through `present_line`), status text, and the title editor still reach it. `HostKind::from_env` is hosted only for `HERDR_ENV` exactly `1` or a trim-non-empty `REX_SESSION`. The corrected `AGENTS.md` sentence matches that predicate, and the dispatch sources did not change. Unix state files remain mode `0600` inside a `0700` directory.

## Coverage

32 units: 6 covered, 1 candidate, 25 out of scope. No blocked or deferred units. The candidate is the carried Windows lead. Out of scope: prior surfaces whose functions this diff does not change, including workflows, the update client, Unix file modes, Herdr context, the board demo, `gh` merge argv, Rex, cleanup paths, the save-recovery allowlist, launcher quoting, and the previous project-title scan. Final critic: stop, no missing units, no reassignment.
