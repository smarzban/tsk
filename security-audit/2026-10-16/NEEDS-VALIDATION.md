# Needs validation

One unresolved lead. No severity. Not a confirmed vulnerability.

## Windows state files inherit the parent directory DACL

Fingerprint: `windows-state-dir-acl`

### Boundary

On Unix, tsk forces the state directory to `0700` and task files to `0600` so another local user cannot read task data. On Windows, the same helpers create the directory and files through `std::fs` and never set a security descriptor. The lower-trust reader, if the inherited DACL allows it, is a second local account that is not the owner, not Administrators, and not SYSTEM. Administrators and SYSTEM are comparable to root, which can already read the Unix files.

### Trace

1. Entrypoint. `src/store.rs` `default_state_dir` (line 952). With `TSK_STATE_DIR` unset, the Windows default is `LOCALAPPDATA\tsk`. If `LOCALAPPDATA` is empty, line 957 uses `USERPROFILE\.tsk`.
2. Propagation. `src/store.rs` `TaskStore::save_unlocked_supported` (line 623) calls `ensure_private_dir`, then `tighten_state_files` at line 625.
3. Propagation. `src/fsperm.rs` `ensure_private_dir` (line 45) uses `create_dir_all`. Windows only rejects reparse-point ancestors (line 36). The `0700` tighten is `cfg(unix)` (line 56).
4. Propagation. The save creates the temp document with `AtomicFilesystem::create_file` (`src/store.rs` line 656), implemented as `create_private_file` (`src/store.rs` line 215).
5. Sink. `src/fsperm.rs` `create_private_file` (line 181) opens the file. Mode `0600` is applied only under `cfg(unix)` (lines 176–179). `tighten_file` is a no-op off Unix (line 302), including the load path at `src/store.rs` line 578.

Evidence: `src/fsperm.rs` lines 9, 36, and 302; `src/store.rs` lines 952 and 957; `site/src/content/docs/docs/storage.md` line 12, which names the Windows default and does not document an ACL.

### Blockers

- This repository never sets or records a Windows security descriptor, so source cannot show whether a second non-admin account has an allow ACE.
- This audit host is Linux and cannot create a Windows profile or query a DACL.

### Owner-observed plan

Local, on a fresh Windows 10 or 11 profile that is not in Administrators and is not elevated: leave `TSK_STATE_DIR` unset, run `tsk add` with a dummy title, and run `icacls` on `%LOCALAPPDATA%\tsk` and `%LOCALAPPDATA%\tsk\tsk.json` (or `%USERPROFILE%\.tsk` if `LOCALAPPDATA` was empty). From a second non-admin local account, try only to read those two paths. Stop at access denied or at seeing the dummy text. Do not take ownership, change the ACL, or use an elevated token.

If that second account can read the dummy text, the boundary the Unix build enforces with `0700` and `0600` is crossed. If access is denied and the DACL allows only the creating user, SYSTEM, and Administrators, the inherited DACL already matches that account boundary.

On an existing store: run `icacls` on the state directory and `tsk.json` only. Do not change the ACL, do not take ownership, and do not open the task file. Record whether any allow ACE grants read to a principal other than the owner, SYSTEM, and Administrators.

## Prior leads that are not still open

- Terminal controls in setup path output. Fixed in `9b37456`. Revalidated. Human setup output rewrites C0/C1.
- `TSK_UPDATE_CURL` on the background check. The background thread uses the same absolute-path rule and hardened HTTPS argv as `tsk update`. No project file sets the variable. Same OS user.
- Bidi characters. `present_line` leaves them in place. No source-visible security decision uses the visual order of a title. Same OS account.
- `PI_CONFIG_DIR` `..` segments. Lexical normalization can leave `HOME`. The only writer of the variable in source is the process environment of the user who runs `tsk setup`. Same principal, matching the documented OMP `path.join` behavior.
