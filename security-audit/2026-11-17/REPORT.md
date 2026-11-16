# Security audit report

## Run

This is a partial pass. Profile `standard`, scoped to the delta `d1430bff16823bf3647bace6016dac30bfa8981f`..`d6016c00a673c76e6a0dd30de9cb6ad498c50be3` and the checks CLI, list and board step-progress output, Windows zip extraction, and untrusted-text paths those commits touch, plus the carried paint and Windows state-directory leads. No agent budget was set. Source ref `d6016c00a673c76e6a0dd30de9cb6ad498c50be3` on `main`. The worktree was clean.

The four commits are the dead `resolve_name` removal, `ZipFile.ExtractToDirectory` in the Windows installer, `tsk checks`, and step-progress labels on board rows and `tsk list`.

Execution policy was sandboxed source-and-local-only. This host can combine a user, mount, and net namespace. bubblewrap and firejail are absent. The cargo registry and `vendor/` are absent, and fetching crates is prohibited, so the crate was not compiled and target code was not executed. No scratch promoter was built. No scratch file was used as evidence.

Prior run: `origin/security-audit/2026-11-15` at `6b5743070220fc033f8acc33db3b3ffd3146a0bb`, which reviewed `d1430bff`. That run confirmed nothing. `ui.paint.project-basename.c0` and `windows-state-dir-acl` are carried. Their controls are not logically changed. There is no changed-source confirmed record to revalidate. The unfinished presenter injection unit was reopened because `presenter.rs` and list output changed.

Agent invocations: 4 reconnaissance, 4 first-wave hunters, 1 post-wave critic, 1 final-clean critic, 3 Phase 3 verifiers (one Windows reply was discarded because its trace used an illegal step kind, then a fresh verifier replaced it), 2 Phase 5 verifiers. 15 total.

Out of scope: 33 ledger units whose controls this delta does not change. They stay visible and are not called covered for this pass.

## Posture

tsk is a local terminal board for one OS user. `tsk checks` writes one review-check state under the store lock and does not change human status. Success output and the new step-progress labels escape or are numeric. The Windows installer still requires three exact entry names before `ExtractToDirectory`. No new confirmed vulnerability.

## Confirmed findings

None.

## Needs validation

| Title | Trace | Blocker | Local next step | Owner check |
| --- | --- | --- | --- | --- |
| Untitled project folder names can carry C0/C1 into board spans | `build_snapshot` to `put_line_at` | Ratatui is not in this tree and was not executed, so the tty bytes for a control in a fitting span were not observed | Render `draw_board` on a local `TestBackend` for an untitled project whose folder name contains U+001B and assert the header-state and group-header cells | None. This is a local render check |
| Windows state files inherit the parent directory DACL | `default_state_dir` to `tighten_file` | No Windows security descriptor is set in source, and this host cannot observe the inherited DACL | On a fresh non-admin Windows profile, run `tsk add` with a dummy title and record `icacls` of the new state directory and `tsk.json` without changing the ACL | On an existing store, record `icacls` of the state directory, `tsk.json`, and any launcher script, without changing the ACL or opening the files |

These are not confirmed vulnerabilities and have no severity.

The new commits do not change either lead. Step-progress text is a count and is passed through `present_line` on the board and `esc` in the demo. It is not the header-state span. `install.ps1` still does not create the state directory and does not set an ACL.

## Hardening

`parse_flag_checks` accepts a Rust `usize`, including underscores, so `1_2` selects check 12. The index is still bounds-checked against that review's checks.

`checks_rejected` prints a store I/O or JSON error without `terminal_text`, as older verbs do. The string is the store error, not check text. On Unix the store file is mode `0600`.

`ExtractToDirectory` reopens the zip after the name check. The file is in a per-run temp directory created by the installing user. A rewrite there is the same account.

`terminal_text` rewrites Unicode `Cc` only. Bidi format characters can still be painted. The board is one OS account.

CVE lookup was not performed. This delta does not change `Cargo.lock`.

## Positive patterns

`presenter::checks` runs `terminal_text` on stored check text. Usage errors do the same. `set_check` changes only that check's state, refuses a task that is not in review, and does not save when the state is unchanged. List and board step labels are `steps {done}/{total}` from counts, then escaped before display. The demo label goes through `esc` before `innerHTML`.

The installer requires one SHA256 match, then exactly the names `tsk.exe`, `LICENSE`, and `README.md`, then extracts. A name containing `..` or a separator fails that gate. Only `tsk.exe` is copied into the install directory.

`resolve_name` had no callers. `resolve_assignee` still requires an exact profile key.

The diff adds no secret, debug switch, or dynamic shell.

## Coverage

| Status | Count |
| --- | --- |
| covered | 8 |
| candidate | 3 |
| blocked | 0 |
| deferred | 0 |
| out_of_scope | 33 |

The candidates are the carried folder-name paint lead and the carried Windows lead (two units, one fingerprint). The post-wave critic and the final-clean critic each returned no missing units and no reassignment. This scoped pass does not claim the rest of the repository is clear.
