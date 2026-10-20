# Security audit report

## Run

Scoped standard pass of `smarzban/tsk` at `c6b8de10c28c7aa530b18655e46b3d45440040ea`, delta from the last audited commit `82e934e9dc77192fedb184f986b1e1a26c3a8a7d`. Worktree clean. Budget unset. This is a partial pass: the ledger covers the one commit in that range (`#220`, landing status-cycle labels) and the carried Windows ACL lead, and records the rest of the product as out of scope.

Execution stayed on source. Target tests were not run. `unshare`, `prlimit`, and `timeout` exist. This run did not mount the tree read-only, and there is no bubblewrap or firejail profile, so target-controlled execution was not approved. No confirmed finding depended on execution.

Prior run: `origin/security-audit/2026-10-17` (commit `7428886`, audited `82e934e`). The earlier branch `origin/security-audit/2026-10-16` was already consumed by that run. The carried record `windows-state-dir-acl` is still `needs_validation`: `src/fsperm.rs` and `src/store.rs` are byte-identical to `82e934e`, and the Phase 3 and Phase 5 checks kept that fingerprint. There were no prior confirmed findings to carry.

Agents: 4 reconnaissance, 2 wave-1 hunters, 1 post-wave critic, 1 final-clean critic, 1 Phase 3 verifier, 1 Phase 5 verifier. Both critics returned no missing in-scope units and no reassignments.

## Posture

The delta does not add a confirmed boundary violation. `#220` changes two author literals in `site/public/landing.js`: the status-cycle section label for `blocked` and `review` is `NEEDS YOU`. `paint` writes those fields with `textContent`. A visitor cannot supply the strings, and they do not reach `innerHTML`, the clipboard, or the board-demo stage event. One earlier lead remains unresolved: Windows state files are not given an owner-only ACL in this repository, and the inherited DACL was not observed.

## Confirmed findings

None. No new confirmed findings.

| Severity | Title | Boundary | Observed result |
| --- | --- | --- | --- |
| — | — | — | — |

## NEEDS VALIDATION

| Title | Trace | Blocker | Next step |
| --- | --- | --- | --- |
| Windows state files inherit the parent directory DACL | `src/store.rs` `default_state_dir` to `src/fsperm.rs` `create_private_file` | Inherited Windows DACL is not in source and was not queried (this host is Linux) | Owner runs `icacls` on a fresh non-admin store and tries a read from a second non-admin account. Do not change the ACL. |

This is not a confirmed vulnerability and has no severity. It is the same fingerprint as the 2026-10-17 lead.

## Hardening and positive patterns

- `site/public/landing.js` `steps` is a closed array of object literals. The blocked and review `sec` values are the author string `NEEDS YOU` (lines 53 and 67). `paint` (lines 80–85) assigns `sec`, `glyph`, `key`, and `next` with `textContent`, which does not parse HTML. The local index advances only by one step modulo the array length. `prefers-reduced-motion` still paints the unchanged `inbox` literal once.
- The cycle path does not read `location`, `document.referrer`, `window.name`, `postMessage`, storage, or `dataset`. The matching nodes in `site/src/pages/index.astro` (lines 515–528) are static markup. The script tag at line 711 is a fixed path.
- The changed strings stop at the cycle text nodes. `cols.innerHTML` interpolates a measured column count. Pin bodies copy author `BOARD_PINS` templates. The copy button reads install-example text. `tsk:set-stage` sends `board` or `split`.
- `site/vercel.json` sets no Content-Security-Policy for `/` or `/landing.js`. That header gap has no attacker-controlled source on this path.
- The two-line diff contains no secret, security TODO, debug flag, `eval`, cookie, redirect, or dynamic import.
- Human setup output still rewrites C0/C1 in the unchanged `terminal_text` path from the prior audit. This delta does not touch it.

## Coverage

| Status | Count |
| --- | --- |
| covered | 2 |
| candidate | 1 (`windows-state-dir-acl`, needs validation) |
| blocked | 0 |
| deferred | 0 |
| out of scope | 20 |

Out of scope, recorded so a later full run can pick them up: the 2026-10-17 units whose source did not change, plus same-file sinks the new labels do not reach (column readout, pin callouts, clipboard, stage event).

Final critic: stop, no missing in-scope units, no reassignment.
