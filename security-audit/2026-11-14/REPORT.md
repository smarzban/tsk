# Security audit report

## Run

This is a partial pass. Profile `standard`, scoped to the delta `5b30b8d6475114bb1312f4bb451f65216d3e7990`..`93d085e8244b57882eed0c1d1e071039593f0dda` and the terminal input, focus, and redraw paths that commit touches, plus the carried Windows state-directory lead. No agent budget was set. Source ref `93d085e8244b57882eed0c1d1e071039593f0dda` on `main`. The worktree was clean.

Execution was sandboxed source-and-local-only in policy, and source-only in practice. This host has `unshare` and `prlimit`. It does not have bubblewrap, firejail, or a disk quota tool. A read-only toolchain, an empty environment, and scratch-only writes were not established together, so target code was not executed. Scratch promotion was not built. No scratch file was used as evidence.

Prior run: `origin/security-audit/2026-11-09` at `afddf36b7823a9b5de6971caa84ee98e442fca1e`, which reviewed `5b30b8d`. That run confirmed nothing. `windows-state-dir-acl` is carried. Its permission helpers are not in this diff. There is no changed-source confirmed record to revalidate.

Agent invocations: 4 reconnaissance, 6 hunter calls (two malformed replies were discarded and the units were reassigned), 1 post-wave critic, 1 final-clean critic, 1 Phase 3 verifier, 1 Phase 5 verifier. 14 total.

Out of scope: 29 ledger units whose controls this delta does not change, including older dispatch, store, installer, and paint units. They stay visible and are not called covered for this pass.

## Posture

tsk is a local terminal board for one OS user. The new code turns focus-in and `ctrl+l` into a full repaint. Those events resize the terminal buffer and return to the loop. They do not change task status, dispatch an agent, or write the store. Focus-mode bytes are fixed crossterm sequences, restored when the input guard drops. No new confirmed vulnerability.

## Confirmed findings

None.

## Needs validation

| Title | Trace | Blocker | Local next step | Owner check |
| --- | --- | --- | --- | --- |
| Windows state files inherit the parent directory DACL | `default_state_dir` to non-Unix `write_launcher` | Inherited Windows DACL is not in source and was not observed | On a fresh non-admin Windows profile, create a dummy task with `tsk add` and record `icacls` of the new state directory and `tsk.json`, without changing the ACL | On an existing store, record `icacls` of the state directory, `tsk.json`, and any launcher script, without changing the ACL or opening the files |

This is not a confirmed vulnerability and has no severity.

## Hardening

If a terminal accepts a fixed mode sequence and then returns an error, the guard records the capability only after success, but `disable_all` ignores a later disable error. A failed enable still returns before the event loop. The bytes are not taken from task text.

`ctrl+l` is matched with modifiers exactly `CONTROL` and kind `Press`. Bare `l` stays a navigation key. Save recovery still answers `r`, `c`, and `Esc`.

## Positive patterns

The repaint gate runs before key, paste, and mouse dispatch. The next click is read only after a paint that refreshes the hit map. Bracketed paste stays enabled. `terminal_text` is unchanged. The five-file diff adds no secret, debug switch, or dynamic shell.

## Coverage

| Status | Count |
| --- | --- |
| covered | 5 |
| candidate | 1 |
| blocked | 0 |
| deferred | 0 |
| out_of_scope | 29 |

The candidate is the carried Windows lead. Both critics returned no missing units and no reassignment. This scoped pass does not claim the rest of the repository is clear.
