# Security audit report

## Run

Scoped standard pass of `smarzban/tsk` at `82e934e9dc77192fedb184f986b1e1a26c3a8a7d`, delta from the last audited commit `ceca7657a52d244d1d89bff04c70a9cc5ea93d7e`. Worktree clean. Budget unset. This is a partial pass: the ledger covers the two commits in that range (`#214` demo thread normalizer, `#216` v0.12.0 changelog, version bump, and What's new notice) and the carried Windows ACL lead, and records the rest of the product as out of scope.

Execution stayed on source. Target tests were not run. `unshare`, `prlimit`, and `timeout` exist, but this run did not mount the tree read-only for a target-controlled process, and there is no bubblewrap or firejail profile.

Prior run: `origin/security-audit/2026-10-16` (commit `d0798da`, audited `ceca7657`). No earlier `security-audit/*` branch. The carried record `windows-state-dir-acl` is still `needs_validation`: `src/fsperm.rs` and `src/store.rs` are unchanged, and the Phase 3 and Phase 5 checks kept that fingerprint. The setup-path terminal-control fix (`9b37456`, `#210`) was re-read on current source. It holds. It is not re-opened.

The demo DOM unit is `prior_covered_changed_source` because `site/public/capture.js` changed the thread value that reaches `esc()`. The obvious-exposure unit is `prior_covered_changed_source` because the changelog section was rewritten. Unchanged covered units from 2026-10-16 stay out of scope.

Agents: parent source map (the four reconnaissance roles were not separate delegated calls), 5 wave-1 hunters plus 3 fresh re-reads after malformed hunter JSON, 1 post-wave critic, 1 final-clean critic, 1 Phase 3 verifier, 1 Phase 5 verifier. Both critics returned no missing in-scope units and no reassignments.

## Posture

The delta does not add a confirmed boundary violation. The demo thread normalizer now rejects any name outside the board's ASCII thread charset, and the demo still escapes thread and project text before `innerHTML`. The What's new notice is compiled catalog text; board paint runs it through `terminal_text` before the terminal, and the CLI does not address notice rows. The release version is the compile-time literal `0.12.0`, and the installer compares `TSK_CURRENT_VERSION` only after a dotted-tag match. Human setup output still rewrites C0/C1. One earlier lead remains unresolved: Windows state files are not given an owner-only ACL in this repository, and the inherited DACL was not observed.

## Confirmed findings

None. No new confirmed findings.

| Severity | Title | Boundary | Observed result |
| --- | --- | --- | --- |
| — | — | — | — |

## NEEDS VALIDATION

| Title | Trace | Blocker | Next step |
| --- | --- | --- | --- |
| Windows state files inherit the parent directory DACL | `src/store.rs` `default_state_dir` to `src/fsperm.rs` `create_private_file` | Inherited Windows DACL is not in source and was not queried (this host is Linux) | Owner runs `icacls` on a fresh non-admin store and tries a read from a second non-admin account. Do not change the ACL. |

This is not a confirmed vulnerability and has no severity. It is the same fingerprint as the 2026-10-16 lead.

## Hardening and positive patterns

- `site/public/capture.js` `normalizeThread` accepts only `^[A-Za-z0-9][A-Za-z0-9.-]{0,31}$` and lowercases, matching `src/domain/thread.rs` `normalize_thread` on that alphabet. A refusal returns null and does not keep the raw `!t` token. `esc()` still runs before the demo's `innerHTML`.
- Announcement id 5 is `include_str!` catalog text. Task-page, peek, and fence paint go through `wrap_text`, which rewrites `char::is_control` per character. `tsk list` drops notice rows, and `TaskAddress::matches` refuses them.
- `Cargo.toml`, `herdr-plugin.toml`, and `site/src/version.mjs` are the literal `0.12.0`. `tsk update` passes `concat!("v", env!("CARGO_PKG_VERSION"))` as `TSK_CURRENT_VERSION` and does not put that string on the curl argv. `install.sh` and `install.ps1` compare it only after `^v[0-9]+\.[0-9]+\.[0-9]+$`. `CHANGELOG.md` is not executed.
- Human setup stdout and stderr still pass skill paths, the gone plugin root, Herdr diagnostics, and setup errors through `terminal_text` (`src/ui/mod.rs` lines 18–30). `--skill-states` still replaces controls with `?`. The `#210` fix holds.
- The eight delta files contain no secret, security TODO, production debug gate, or dynamic command. `herdr-plugin.toml` argv is static.

## Coverage

| Status | Count |
| --- | --- |
| covered | 5 |
| candidate | 1 (`windows-state-dir-acl`, needs validation) |
| blocked | 0 |
| deferred | 0 |
| out of scope | 11 |

Out of scope, recorded so a later full run can pick them up: CI workflows, the installer artifact download body, Herdr context JSON, Unix store modes, `PI_CONFIG_DIR`, the `--skill-states` probe, the text-editing chords, `present_line` bidi handling, and the background curl selector. Their source did not change in this delta.

Final critic: stop, no missing in-scope units, no reassignment.
