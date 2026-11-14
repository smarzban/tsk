# Security audit report

## Run

This is a partial pass. Profile `standard`, scoped to the delta `93d085e8244b57882eed0c1d1e071039593f0dda`..`d1430bff16823bf3647bace6016dac30bfa8981f` and the terminal mode, input, and untrusted-text paths that commit touches, plus the carried Windows state-directory lead. No agent budget was set. Source ref `d1430bff16823bf3647bace6016dac30bfa8981f` on `main`. The worktree was clean.

Execution policy was sandboxed source-and-local-only. This host can combine a user, mount, and net namespace, read-only binds, a sized tmpfs, an empty environment, `prlimit`, and a wall-clock limit. The cargo registry and `target/` are absent, and fetching crates is prohibited, so the crate was not compiled and target code was not executed. No scratch promoter was built. No scratch file was used as evidence.

Prior run: `origin/security-audit/2026-11-14` at `83011bc2bdcc667ead8636cfec278529d2078f10`, which reviewed `93d085e`. That run confirmed nothing. `windows-state-dir-acl` is carried. Its permission helpers are not in this diff. There is no changed-source confirmed record to revalidate. The repaint gate and the mode-byte guard changed and were reopened.

Agent invocations: 4 reconnaissance, 4 first-wave hunters, 1 reassignment after a malformed paint reply was discarded, 1 post-wave critic (a follow-up withdrew its only reassignment), 1 final-clean critic, 2 Phase 3 verifiers, 2 Phase 5 verifiers. 16 total.

Out of scope: 32 ledger units whose controls this delta does not change, including older dispatch, store, installer, and paint units. They stay visible and are not called covered for this pass.

## Posture

tsk is a local terminal board for one OS user. `ctrl+l` now re-enters the alternate screen and resends mouse, paste, and keyboard modes, then forgets the frame. That key does not change task status, dispatch an agent, or write the store. Focus reporting is no longer requested, and focus-in no longer redraws. The mode bytes are fixed crossterm commands. No new confirmed vulnerability.

## Confirmed findings

None.

## Needs validation

| Title | Trace | Blocker | Local next step | Owner check |
| --- | --- | --- | --- | --- |
| Untitled project folder names can carry C0/C1 into board spans | `build_snapshot` to `put_line_at` | Ratatui is not in this tree and was not executed, so the tty bytes for a control in a fitting span were not observed | Render `draw_board` on a local `TestBackend` for an untitled project whose folder name contains U+001B and assert the header-state and group-header cells | None. This is a local render check |
| Windows state files inherit the parent directory DACL | `default_state_dir` to non-Unix `write_launcher` | Inherited Windows DACL is not in source and was not observed | On a fresh non-admin Windows profile, create a dummy task with `tsk add` and record `icacls` of the new state directory and `tsk.json`, without changing the ACL | On an existing store, record `icacls` of the state directory, `tsk.json`, and any launcher script, without changing the ACL or opening the files |

These are not confirmed vulnerabilities and have no severity.

## Hardening

A keyboard-protocol pop on `ctrl+l` writes to this process's stdout. What an outer terminal does with an extra pop is not in this repository. That effect stays with the operator's own terminal.

`terminal_text` rewrites Unicode `Cc` only. Bidi format characters can still be painted. The board is one OS account.

A project title or thread that bypasses the setter and is loaded from the store is a write by the account that owns the Unix `0600` state file. That path was not treated as a second principal.

CVE lookup was not performed. This delta does not change `Cargo.lock`.

## Positive patterns

`restore` writes only fixed mode commands. Task text is not an argument. `ctrl+l` is consumed before key, paste, and mouse dispatch. Repeat, Release, and `ctrl+l` with another modifier are not redraws and are not status chords. Focus events hit an empty match arm. Row labels, live lines, peek text, cleanup values, and OSC 52 do not take the unescaped group-header path. OSC 52 payloads are standard base64. The five-file diff adds no secret, debug switch, or dynamic shell.

## Coverage

| Status | Count |
| --- | --- |
| covered | 3 |
| candidate | 2 |
| blocked | 0 |
| deferred | 0 |
| out_of_scope | 32 |

The candidates are the new folder-name paint lead and the carried Windows lead. The final-clean critic returned no missing units and no reassignment. This scoped pass does not claim the rest of the repository is clear.
