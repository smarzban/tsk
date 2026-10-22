# Security audit report

Standard scoped delta of `smarzban/tsk` from `b5b92d60f2a70f29798354e6096b52ae30871a04` to `8d714a201280318f92ed42d26d20bf233a1f5baf` (7 commits, worktree clean). Budget unset. This is a partial pass: only the changed code, the code it touches, and the carried Windows state-directory lead were in scope. Execution policy was sandboxed source-and-local-only. Target code was not executed. `unshare` is present, but bubblewrap and firejail are absent, and a read-only target was not combined with CPU, memory, process, and file-size limits. No deployed service was contacted.

Prior run `origin/security-audit/2026-10-22` reviewed `b5b92d60` and is complete. `windows-state-dir-acl` is carried. Its source is unchanged, and Phase 5 kept it `needs_validation`. There is no same-source confirmed exclusion. Changed-source revalidation of `backup.https-bare-userinfo.scheme-prefix` found the gap closed: `scheme_token` strips a bare https token even when bytes precede the scheme, and http(s) userinfo does not reach git argv or `.git/config`. It is not a current finding. Surfaces whose security-relevant source was not edited in this range stayed out of scope.

Agents spent against the planned minimum: 4 reconnaissance, 6 hunters, 1 post-wave critic, 1 final-clean critic, 1 Phase 3 verifier, 1 Phase 5 verifier. Both critics returned no missing units and no reassignment. The final critic stopped clean.

## Posture

tsk remains a single-user local board. This range keeps backup URL credentials out of git arguments and the clone config, paints the release notice through the existing control-character encoder, and names both installed skills without a new trust boundary. No new confirmed finding. The Windows state-directory ACL is still unresolved on this Linux host.

## Confirmed findings

No confirmed findings.

## NEEDS VALIDATION

| Title | Trace | Blocker | Next step |
| --- | --- | --- | --- |
| Windows state files inherit the parent directory DACL | `src/store.rs` `default_state_dir` to `src/fsperm.rs` `create_private_file` | No Windows DACL is set or observable from this Linux tree | Owner runs read-only `icacls` on a fresh non-admin profile and tries a second non-admin read. No ACL changes. |

This is not a confirmed vulnerability and has no severity.

## Hardening

- A double quote in a backup host or password can break the credential helper's double quotes. The URL writer is the OS user who runs tsk.
- A longer scheme such as `xhttps://` is not classified as https. That string is not an https URL.
- A Windows grandchild started between spawn and job assignment can still escape the job. That window is unchanged production code.
- Skill-install symlink refusal is not atomic with staging. A same-user writer of the skills directory can already replace the skill file.
- A release tag may contain Unicode bidi. `present_line` encodes only C0 and C1. The tag publisher is the same principal that publishes the binary the notice names.
- The update notice is escaped at the status-row sink rather than by an allowlist in `parse_latest_release`.
- Placeholder backup tokens stay inside the test module.

## Positive patterns

`scheme_token` and `Remote::of` strip http(s) userinfo before git runs, and the login is passed only through the helper environment. Backup git sets `core.symlinks=false`, an empty hooks directory, and `--no-verify`. `terminal_text` encodes control characters before the status row is painted. The skill-state probe replaces control characters in the path field. The board demo escapes text before `innerHTML`. Unix state directories and files are mode `0700` and `0600`.

## Coverage

41 units: 16 covered, 1 candidate, 23 out of scope, 1 not applicable. No blocked or deferred units. The candidate is the carried Windows lead. Out of scope: security-relevant source that this delta did not edit. Not applicable: board helpers deleted in this range with no remaining callers. Final critic: stop, no missing units, no reassignment.
