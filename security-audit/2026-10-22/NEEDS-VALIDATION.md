# Needs validation

One unresolved lead. No severity. Not a confirmed vulnerability. Same fingerprint as the 2026-10-21 audit. `src/fsperm.rs` still applies owner-only modes only on Unix.

## Windows state files inherit the parent directory DACL

Fingerprint: `windows-state-dir-acl`

### Boundary

On Unix, tsk forces the state directory to `0700` and task files to `0600`. On Windows, the same helpers create the directory and files through `std::fs` and never set a security descriptor. The lower-trust reader, if the inherited DACL allows it, is a second local account that is not the owner, not Administrators, and not SYSTEM.

### Trace

1. Entrypoint. `src/store.rs` `default_state_dir` (line 1385). With `TSK_STATE_DIR` unset and `LOCALAPPDATA` non-empty, the Windows default is that directory joined with `tsk` (lines 1401–1403). Lines 1406–1409 use `USERPROFILE\.tsk` when `LOCALAPPDATA` is empty.
2. Propagation. `src/store.rs` `TaskStore::save_unlocked_supported` (line 774) calls `ensure_private_dir` (line 793), then `tighten_state_files` (line 795).
3. Propagation. `src/fsperm.rs` `ensure_private_dir` (line 45) uses `create_dir_all`. Windows only rejects reparse-point ancestors (lines 35–36). The `0700` tighten is `cfg(unix)` (lines 55–56).
4. Propagation. `src/store.rs` `tighten_state_files` (line 1148) passes each existing private state name, including `tsk.json`, to `tighten_file` (line 1156). Load (line 714) and save (line 795) both call this function.
5. Propagation. `src/fsperm.rs` `tighten_file` (line 302) discards the path off Unix.
6. Propagation. The save opens the temp document with `create_file` (`src/store.rs` line 826), implemented as `create_private_file` (`src/store.rs` line 283). `tighten_state_files` has already returned.
7. Sink. `src/fsperm.rs` `create_private_file` (line 181) opens the file. Mode `0600` is applied only under `cfg(unix)` (lines 176–179).

Evidence: `src/fsperm.rs` lines 9, 36, 55, 179, and 302; `src/store.rs` lines 284, 714, 1403, and 1409; `site/src/content/docs/docs/storage.md` line 13.

### Blockers

- This repository never sets or records a Windows security descriptor, so source cannot show whether a second non-admin account has an allow ACE.
- This audit host is Linux and cannot create a Windows profile or query a DACL.

### Owner-observed plan

Local, on a fresh Windows 10 or 11 profile that is not in Administrators and is not elevated: leave `TSK_STATE_DIR` unset, do not pass `--state-dir`, run `tsk add` with a dummy title, and run `icacls` with only the path (no `/grant`, `/deny`, `/remove`, `/reset`, `/setowner`, `/inheritance`, `/restore`, or `/replace`) on `%LOCALAPPDATA%\tsk` and `%LOCALAPPDATA%\tsk\tsk.json` (or `%USERPROFILE%\.tsk` if `LOCALAPPDATA` was empty). From a second non-admin local account, try only to read those two paths. Stop at access denied or at seeing the dummy text. Do not take ownership, change the ACL, or use an elevated token.

If that second account can read the dummy text, the boundary the Unix build enforces with `0700` and `0600` is crossed. If access is denied and the DACL allows only the creating user, SYSTEM, and Administrators, the inherited DACL already matches that account boundary.

On an existing store: run `icacls` with only the path on the state directory and `tsk.json`. Do not change the ACL, do not take ownership, and do not open the task file. Record whether any allow ACE grants read to a principal other than the owner, SYSTEM, and Administrators.
