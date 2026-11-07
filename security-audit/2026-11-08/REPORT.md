# Security audit report

Standard scoped delta of `smarzban/tsk` from `94b1dc6f864c064f69631d240b1e91c9bece2cdd` to `36610e8f012018e3ddd624304dbcf680ae4df56c` (1 commit, worktree clean). Budget unset. This is a partial pass: only the project-title change, the code it touches, the carried Windows state-directory lead, and a same-source recheck of `scheme_token` were in scope. Execution policy was sandboxed source-and-local-only. Target code was not executed. `unshare` can isolate network and remount a bind read-only. bubblewrap, firejail, and a user systemd bus are absent, so disk quota was not combined with an empty environment, a read-only toolchain, and the other required limits. No deployed service was contacted.

Prior run `origin/security-audit/2026-11-07` reviewed `94b1dc6` and recorded no confirmed finding. `windows-state-dir-acl` is carried. Store line numbers moved. The permission helpers and the non-Unix launcher write did not. Phase 5 kept it `needs_validation`. There is no same-source confirmed exclusion.

`src/backup.rs` is not in this diff. `scheme_token` still strips an http(s) userinfo when the scheme is not at byte zero, and that stripped URL is what git receives. `src/ui/mod.rs` `terminal_text` is not in this diff and still encodes C0 and C1. New title lines in the CLI and the launch card pass through it.

Judgment agents: 4 reconnaissance, 5 hunters, 1 post-wave critic, 1 final-clean critic, 1 Phase 3 verifier, 1 Phase 5 verifier. Both critics returned no missing units and no reassignment. The final critic stopped clean.

## Posture

tsk remains a single-user local board. This commit adds a display title per project, accepted by `-p` and the project verbs, with store version 17. Titles are unique ignoring case, resolve before folder names, and are refused when they contain a control character or a path separator. They are not passed to a shell, a launcher script, or a filesystem path. No new confirmed finding. The Windows state-directory ACL is still unresolved on this Linux host.

## Confirmed findings

No confirmed findings.

## NEEDS VALIDATION

| Title | Trace | Blocker | Next step |
| --- | --- | --- | --- |
| Windows state files inherit the parent directory DACL | `src/store.rs` `default_state_dir` to `src/fsperm.rs` `create_private_file`, and `src/dispatch.rs` `write_launcher` on non-Unix | No Windows DACL is set or observable from this Linux tree | Owner runs read-only `icacls` on a fresh non-admin profile and on one launcher script if present. No ACL changes. |

This is not a confirmed vulnerability and has no severity.

## Hardening

- `set_project_title` refuses `char::is_control`. It does not refuse bidi or other format characters. `terminal_text` documents that it leaves those characters in place. The writer is the same OS account that owns the terminal.
- `paint_group_header` paints the group word as a raw span. A title that passed the setter has no control character, so the span matches an escaped one. Load does not re-apply the refusal. A same-user edit of the store can place a control in that span. Unix state files stay mode `0600`.
- Quick-add `!p` takes one whitespace-delimited word. A title with a space is not one token. The resolver still refuses an ambiguous name.
- On Unix a backslash is not a path separator, so a title may contain one. It is not used as a path. A slash token does not match that title.
- Two processes can each save the same title. Resolution then returns ambiguous and does not pick one.

## Positive patterns

`set_project_title` normalizes whitespace, then refuses controls, path separators, the names desk and global, length over 80, and a case-insensitive duplicate. CLI title lines, list labels, status text, the launch card, and project-row labels go through `terminal_text` or serde JSON. `render_template` still substitutes task fields only. Dispatch and launcher scripts do not read the project title. A title match that hits two stored projects is ambiguous on both resolvers. Undo refuses to restore a title another project holds. Store v17 adds an optional title and leaves a v16 document otherwise unchanged. Concurrent save reapplies title and archive field by field on the named key. `scheme_token` still strips backup URL userinfo before git. Unix state directories and files remain mode `0700` and `0600`.

## Coverage

26 units: 7 covered, 1 candidate, 18 out of scope. No blocked or deferred units. The candidate is the carried Windows lead. Out of scope: workflows, the update client, Unix file modes, Herdr context JSON, the board demo escape path, `gh` merge argv, Rex, cleanup, keyboard chords, the v16 host migration, launcher quoting itself, and the previous delta's secret scan. Those sources were not edited in this range, or the edit did not touch the control. Final critic: stop, no missing units, no reassignment.
