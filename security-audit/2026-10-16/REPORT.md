# Security audit report

## Run

Scoped standard pass of `smarzban/tsk` at `ceca7657a52d244d1d89bff04c70a9cc5ea93d7e`, delta from the last audited commit `ade0385fa49f28035c35dc846ce90158c4117207`. Worktree clean. Budget unset. This is a partial pass: the ledger covers the two commits in that range and four narrative prior leads, and records the rest of the product as out of scope.

Execution stayed on source. Target tests were not run. `unshare` is available, but this run did not mount the tree read-only for a target-controlled process, and there is no bubblewrap or firejail profile. A parent-written `std::path` replica showed that `PI_CONFIG_DIR=../../../tmp/pwn` under `HOME=/home/user` normalizes to `/tmp/pwn`. That replica is not tsk and was not used as retained evidence.

No compatible prior `coverage-ledger.json` or `findings.json` exists on `security-audit/*` or in history. Narrative leads were re-checked without carried fingerprints. The merged terminal-control fix (commit `9b37456`, PR #210) was revalidated on the current source. It holds. It is not re-opened.

Agents: 4 reconnaissance, 4 wave-1 hunters, 1 wave-1 critic, 1 wave-2 hunter, 1 wave-2 critic, 1 final-clean critic, 1 Phase 3 verifier, 1 Phase 5 verifier.

The wave-1 critic added the `--skill-states` TSV probe. The wave-2 critic and the final-clean critic both returned no further in-scope units and no reassignments.

## Posture

The delta does not add a new confirmed boundary violation. Human setup output now rewrites C0/C1 controls before they reach a terminal. The text-editing change stays inside the same user's draft and does not add a raw control paint path or a status-verb bypass. Three earlier leads (background `TSK_UPDATE_CURL`, bidi characters, `PI_CONFIG_DIR` `..` segments) are not still open: source shows same-OS-user behavior, not a second principal. One lead remains unresolved: Windows state files are not given an owner-only ACL in this repository, and the inherited DACL was not observed.

## Confirmed findings

None. No new confirmed findings.

| Severity | Title | Boundary | Observed result |
| --- | --- | --- | --- |
| — | — | — | — |

## NEEDS VALIDATION

| Title | Trace | Blocker | Next step |
| --- | --- | --- | --- |
| Windows state files inherit the parent directory DACL | `src/store.rs` `default_state_dir` to `src/fsperm.rs` `create_private_file` | Inherited Windows DACL is not in source and was not queried (this host is Linux) | Owner runs `icacls` on a fresh non-admin store and tries a read from a second non-admin account. Do not change the ACL. |

This is not a confirmed vulnerability and has no severity.

## Hardening and positive patterns

- Human setup stdout and stderr pass skill paths, the gone plugin root, Herdr diagnostics, and setup errors through `terminal_text` (`src/ui/mod.rs`). Tests reject a raw ESC in those strings. The prior Medium finding stays fixed.
- `--skill-states` replaces `char::is_control` with `?`. Installed versions that survive `normalize_version` are `major.minor.patch` digits. The installer awk split cannot be given an extra field or a raw ESC from those columns.
- JSON setup output uses `serde_json`, which does not place a raw C0 byte inside a string.
- Draft paint (`escaped_draft_rows`, `wrap_text`) escapes before the terminal. New word and line chords are not in the persist set and are not on the save-recovery allowlist. Notes Enter inserts a line. Save-recovery `r` / `c` / Esc still reach Retry and Cancel.
- The board demo's new hint strings are literals. Titles go through `esc()` before `innerHTML`.
- Background release check and `tsk update` share `curl_path` and `hardened_curl` (absolute path, HTTPS-only flags). Nothing in the repo reads `TSK_UPDATE_CURL` from a project file. The binary the user names still runs as that user. That is the documented override, not a second principal.
- `present_line` still does not rewrite bidi or format characters. Status actions use the selected row id, not the visual order of a title. Same-account display is not a boundary violation.
- `PI_CONFIG_DIR` keeps `..` and then pops lexically, matching the documented OMP `path.join` behavior. Enough `..` segments can resolve outside `HOME` (parent `std::path` replica: `/tmp/pwn`). The variable is read only from the process environment of the user who runs `tsk setup`. That user can already write any path they can write. Absolute paths still have their root stripped and stay under `HOME`. Not a confirmed finding.

## Coverage

| Status | Count |
| --- | --- |
| covered | 10 |
| candidate | 1 (`windows-state-dir-acl`, needs validation) |
| blocked | 0 |
| deferred | 0 |
| out of scope | 4 |

Out of scope, recorded so a later full run can pick them up: Herdr context JSON, Unix store modes, the explicit `tsk update` installer download, and CI workflows.

Final critic: stop, no missing in-scope units, no reassignment.
