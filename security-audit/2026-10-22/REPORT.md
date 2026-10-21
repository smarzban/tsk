# Security audit report

Standard scoped delta of `smarzban/tsk` from `c6b8de10c28c7aa530b18655e46b3d45440040ea` to `b5b92d60f2a70f29798354e6096b52ae30871a04` (316 commits, worktree clean). Budget unset. Execution was source review plus one sandboxed local harness for the backup credential functions: user and network namespaces, empty environment, `prlimit`, and `timeout`. No deployed service was contacted.

Prior run `origin/security-audit/2026-10-21` reviewed `c6b8de10` and had no confirmed finding. `windows-state-dir-acl` is carried. Its source is unchanged. There is no same-source confirmed exclusion. Changed files since `c6b8de10`, and the controls they call, were in scope. `src/context.rs` and same-user text editing outside the injection, chord, and bidi units stayed out of scope.

Agents: 4 reconnaissance, 6 hunters, 1 post-wave critic, 1 final-clean critic, 3 Phase 3 verifiers (one backup verdict was discarded as schema-invalid and rerun), 2 Phase 5 verifiers. Both critics returned no missing units and no reassignment. The final critic also stopped clean.

## Posture

tsk is a single-user local board. The new dispatch, launcher, cleanup, merge, and backup paths keep task text out of shells and keep worktree removal inside git's registered worktrees. One confirmed gap remains in backup URL parsing: `credentials()` and `redacted()` do not agree on where a scheme starts, so a bare https token with a prefix is pushed and printed. The Windows state-directory ACL is still unresolved on this Linux host.

## Confirmed findings

| Severity | Title | Boundary | Observed result |
| --- | --- | --- | --- |
| medium | A bare https token stays in the backup when the URL does not start at the scheme | Backup URL credential removal | Leading space or newline: `without_credentials` returns `None` and `shown_location` keeps `dummy-token`. The clean URL is stripped. |

### A bare https token stays in the backup when the URL does not start at the scheme

- Location: `src/backup.rs` `credentials` (line 1140), reached from `src/cli/backup.rs` `backup_add` (line 487) and pushed from `backup_to_git` (line 1244).
- Lower-trust principal: a reader of the backup repository.
- Reproduction: call `without_credentials`, `shown_location`, and `redacted` on `https://dummy-token@example.test/repo.git`, the same URL with one leading space, and the same URL with one leading newline. No git remote.
- Conditions: the stored URL has bytes before the scheme, the userinfo has no colon, and a later backup pushes `config.toml`. `tsk backup add` stores a non-directory location as written.
- Result: the clean URL becomes `https://example.test/repo.git` and is shown as `https://***@example.test/repo.git`. The prefixed URLs return `without=None`, and `shown_location` still contains `dummy-token`. `redacted` masks all three.
- Impact: a token the product says it removes is written into the backup repository. Likelihood is low because the URL must not start at the scheme. Overall severity is medium.
- Fix: classify `http` and `https` from the trailing scheme token `redacted()` already scans, and add a regression test on `sanitized_config` and `shown_location`.

## NEEDS VALIDATION

| Title | Trace | Blocker | Next step |
| --- | --- | --- | --- |
| Windows state files inherit the parent directory DACL | `src/store.rs` `default_state_dir` to `src/fsperm.rs` `create_private_file` | No Windows DACL is set or observable from this Linux tree | Owner runs read-only `icacls` on a fresh non-admin profile and tries a second non-admin read. No ACL changes. |

This is not a confirmed vulnerability and has no severity.

## Hardening

- `sh_launch_line` allows parentheses. A fish login shell can treat `(...)` inside the double-quoted launcher path as a command. The bytes are the invoking user's state directory.
- `omp_config_root` keeps `..` after stripping an absolute root, matching the host path join. `PI_CONFIG_DIR` is that user's own setting.
- `terminal_text` escapes C0 and C1 and leaves bidi and format characters. Selection and cleanup use task ids and stored paths.
- `hardened_curl` does not pass `--disable`, so the same user's curlrc can add flags.
- The updater runs the HTTPS installer from gettsk.sh. The release archive is then checked against `SHA256SUMS`. That execution is the designed update.
- A backup URL query string is not treated as userinfo. Git http credentials use userinfo.
- Agent `command` and `prompt` strings are pushed. Only `[agent.*.env]` values are replaced.

## Positive patterns

Unix launchers single-quote every argument and env value. Windows launchers embed base64 and refuse a batch-only program. Cleanup requires a git-registered worktree, refuses the project root, and deletes a branch only after a confirmed merge. `gh pr merge` is an argument array without `--admin`. Pull-request workflows use `contents: read`. The Vercel token is used only on a push to `main`. The board demo escapes text before `innerHTML`. The stars script writes `textContent`.

## Coverage

37 units: 33 covered, 2 candidate, 2 out of scope. No blocked or deferred units. Candidates are the confirmed backup finding and the carried Windows lead. Out of scope: Herdr capture JSON (`src/context.rs` unchanged and not called by dispatch) and same-user text editing beyond the injection, chord, and bidi units. Final critic: stop, no missing units, no reassignment.
