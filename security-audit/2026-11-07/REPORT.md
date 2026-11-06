# Security audit report

Standard scoped delta of `smarzban/tsk` from `8d714a201280318f92ed42d26d20bf233a1f5baf` to `94b1dc6f864c064f69631d240b1e91c9bece2cdd` (22 commits, worktree clean). Budget unset. This is a partial pass: only the changed trust boundaries, the code they rely on, the carried Windows state-directory lead, and a same-source recheck of two previously closed controls were in scope. Execution policy was sandboxed source-and-local-only. Target code was not executed. `unshare` can isolate network and remount a bind read-only. bubblewrap, firejail, and a user systemd bus are absent, so disk quota was not combined with an empty environment, a read-only toolchain, and the other required limits. No deployed service was contacted.

Prior run `origin/security-audit/2026-10-23` reviewed `8d714a2` and recorded no confirmed finding. `windows-state-dir-acl` is carried. Its trace still holds, including launcher scripts written on Windows without a security descriptor. Phase 5 kept it `needs_validation`. There is no same-source confirmed exclusion.

`src/backup.rs` is not in this diff. `scheme_token` still strips an http(s) userinfo when the scheme is not at byte zero, and that stripped URL is what git receives. `src/ui/mod.rs` `terminal_text` is not in this diff and still encodes C0 and C1. New Rex and project-label lines in `src/cli/presenter.rs` pass through it. Neither closed issue is a current finding.

Judgment agents: 4 reconnaissance, 4 hunters, 1 post-wave critic, 1 Phase 3 verifier, 1 final-clean critic, 1 Phase 5 verifier. Both critics returned no missing units and no reassignment. The final critic stopped clean.

## Posture

tsk remains a single-user local board. This range adds a Rex dispatch host, worktrees under the state directory unless config moves them, `tsk setup rex`, store version 16, and a refusal when two projects share a basename. Launcher text stays quoted, Rex calls stay JSON, and cleanup still removes only a recorded git worktree that is not the project root. No new confirmed finding. The Windows state-directory ACL is still unresolved on this Linux host.

## Confirmed findings

No confirmed findings.

## NEEDS VALIDATION

| Title | Trace | Blocker | Next step |
| --- | --- | --- | --- |
| Windows state files inherit the parent directory DACL | `src/store.rs` `default_state_dir` to `src/fsperm.rs` `create_private_file`, and `src/dispatch.rs` `write_launcher` on non-Unix | No Windows DACL is set or observable from this Linux tree | Owner runs read-only `icacls` on a fresh non-admin profile and tries a second non-admin read. No ACL changes. |

This is not a confirmed vulnerability and has no severity.

## Hardening

- A configured worktree root outside the project is followed if that path is a symlink. A symlink whose target is already inside the project is refused. The default root stays under the state directory.
- Standalone cleanup, and a Herdr cleanup whose workspace id is gone, do not apply the Rex directory-name rule. Removal still uses only the recorded absolute path, and only when git lists it, it is clean, and it is not the project root.
- Cleanup and `gh` reuse a stored branch and base without a second `check-ref-format`. Values tsk writes are a `tsk/t<n>` branch or a ref that already passed that check. A same-account edit of the record can place one leading-dash token in `gh` or `git ls-remote` argv.
- A legacy Rex id can close a one-window session that still has that task's agent label. The id parser refuses any other session shape. The actor is the OS user who dispatched it.
- A reply is pasted in a later Rex process after the label check. An exit in that gap could land the owner's reply in an idle terminal. The paste is JSON, and the agent already runs as that user.
- `list -p` prefers an exact stored bare scope such as `atlas` before basename matching. Two absolute paths that share a basename still refuse.
- The installer compares `SHA256SUMS` from the same GitHub release as the archive. The compare fails closed. There is no signature detached from that release.
- `terminal_text` encodes Unicode controls only. A stored path can still contain bidirectional format characters. That path is one the operator stored.

## Positive patterns

Unix launchers single-quote every argv and env value. The pane types only an allowlisted `/bin/sh` or PowerShell `-File` line. Rex API bodies are JSON, and a new tab command is the array `/bin/sh` plus the launcher path. `open_tab` requires a `session:` id with no slash. Two distinct project basenames are refused. `tsk setup rex` escapes the binary path for Lua and renames `tsk.lua` through a no-follow directory descriptor on Unix. `scheme_token` and `terminal_text` still do the jobs the previous audit closed. Unix state directories and files remain mode `0700` and `0600`.

## Coverage

22 units: 13 covered, 1 candidate, 8 out of scope. No blocked or deferred units. The candidate is the carried Windows lead. Out of scope: workflows, the update client, Unix file modes, Herdr context JSON, the board demo escape path, `gh` merge argv, keyboard chords, and the stars script. Those security-relevant sources were not edited in this range, or the edit did not touch the control. Final critic: stop, no missing units, no reassignment.
