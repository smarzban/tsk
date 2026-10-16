# Needs validation

One unresolved lead. No severity. Not a confirmed vulnerability. Same fingerprint as the 2026-10-16 audit. `src/fsperm.rs` and `src/store.rs` are unchanged since `ceca7657`.

## Windows state files inherit the parent directory DACL

Fingerprint: `windows-state-dir-acl`

### Boundary

On Unix, tsk forces the state directory to `0700` and task files to `0600` so another local user cannot read task data. On Windows, the same helpers create the directory and files through `std::fs` and never set a security descriptor. The lower-trust reader, if the inherited DACL allows it, is a second local account that is not the owner, not Administrators, and not SYSTEM. Administrators and SYSTEM are comparable to root, which can already read the Unix files.

### Trace

1. Entrypoint. `src/store.rs` `default_state_dir` (line 952). With `TSK_STATE_DIR` unset and `LOCALAPPDATA` non-empty, the Windows default is that directory joined with `tsk`. Lines 955–957 use `USERPROFILE\.tsk` when `LOCALAPPDATA` is empty.
2. Propagation. `src/store.rs` `TaskStore::save_unlocked_supported` (line 623) calls `ensure_private_dir`, then `tighten_state_files` at line 625.
3. Propagation. `src/fsperm.rs` `ensure_private_dir` (line 45) uses `create_dir_all`. Windows only rejects reparse-point ancestors (line 36). The `0700` tighten is `cfg(unix)` (lines 55–56).
4. Propagation. `src/store.rs` `tighten_state_files` (line 862) passes each existing private state name, including `tsk.json`, to `tighten_file`. Load (line 578) and save (line 625) both call this function.
5. Propagation. `src/fsperm.rs` `tighten_file` (line 302) discards the path off Unix. No mode bits and no security descriptor are written.
6. Propagation. The save opens the temp document with `filesystem.create_file` (`src/store.rs` line 656), implemented as `create_private_file` (`src/store.rs` line 215). `tighten_state_files` has already returned.
7. Sink. `src/fsperm.rs` `create_private_file` (line 181) opens the file. Mode `0600` is applied only under `cfg(unix)` (lines 176–179).

Evidence: `src/fsperm.rs` lines 9, 36, 55, 179, and 302; `src/store.rs` lines 215, 578, 952, and 957; `site/src/content/docs/docs/storage.md` line 12, which names the Windows default and does not document an ACL.

### Blockers

- This repository never sets or records a Windows security descriptor, so source cannot show whether a second non-admin account has an allow ACE.
- This audit host is Linux and cannot create a Windows profile or query a DACL.

### Owner-observed plan

Local, on a fresh Windows 10 or 11 profile that is not in Administrators and is not elevated: leave `TSK_STATE_DIR` unset, do not pass `--state-dir`, run `tsk add` with a dummy title, and run `icacls` with only the path (no `/grant`, `/deny`, `/remove`, `/reset`, `/setowner`, `/inheritance`, `/restore`, or `/replace`) on `%LOCALAPPDATA%\tsk` and `%LOCALAPPDATA%\tsk\tsk.json` (or `%USERPROFILE%\.tsk` if `LOCALAPPDATA` was empty). From a second non-admin local account, try only to read those two paths. Stop at access denied or at seeing the dummy text. Do not take ownership, change the ACL, or use an elevated token.

If that second account can read the dummy text, the boundary the Unix build enforces with `0700` and `0600` is crossed. If access is denied and the DACL allows only the creating user, SYSTEM, and Administrators, the inherited DACL already matches that account boundary.

On an existing store: run `icacls` with only the path on the state directory and `tsk.json`. Do not change the ACL, do not take ownership, and do not open the task file. Record whether any allow ACE grants read to a principal other than the owner, SYSTEM, and Administrators.

## Prior leads that are not still open

- Terminal controls in setup path output. Fixed in `9b37456` (`#210`). Re-read at `82e934e9`. Human setup output still rewrites C0/C1. Not re-opened.
- Demo thread names. `#214` rejects characters outside the board charset before HTML, and `esc()` still runs. No new lead.
- What's new notice text. Catalog id 5 is compiled in, and paint escapes controls. No new lead.
- Release version `0.12.0`. Compile-time literal. The installer accepts `TSK_CURRENT_VERSION` only as a dotted tag. No new lead.
