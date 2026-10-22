# Needs validation

One carried lead. It has no severity and is not a confirmed vulnerability. Do not change an ACL, take ownership, or send traffic to a deployed service.

## Windows state files inherit the parent directory DACL

Fingerprint: `windows-state-dir-acl`.

The lower-trust reader is a second local Windows account that is not the creating user, not a member of Administrators, and not SYSTEM or an elevated token. On Unix, tsk forces the state directory to `0700` and task files such as `tsk.json` to `0600`. On Windows, `ensure_private_dir`, `create_private_file`, and `tighten_file` set no security descriptor. A missing state directory is created with `create_dir_all`, and `tsk.json` is opened without an owner-only mode. `tighten_file` is a no-op off Unix, including on a later load. `src/fsperm.rs` and `src/store.rs` are unchanged from `b5b92d60` through `8d714a2`. The installer creates `%LOCALAPPDATA%\Programs\tsk\bin` and does not ACL the state directory. Source cannot show the inherited DACL. Plaintext readable only by the creating account would not cross this boundary.

### Trace

1. `src/store.rs:1385` `default_state_dir`. With `TSK_STATE_DIR` unset, Windows uses `%LOCALAPPDATA%\tsk`, or `%USERPROFILE%\.tsk` when `LOCALAPPDATA` is empty (`src/store.rs:1403`, `src/store.rs:1408`).
2. `src/store.rs:774` `save_unlocked_supported` calls `ensure_private_dir` at line 793 and `tighten_state_files` at line 795.
3. `src/fsperm.rs:45` `ensure_private_dir` creates a missing directory with `create_dir_all`. The Windows-only extra step is the reparse check at lines 35-36. Mode `0700` is `cfg(unix)` at lines 55-56.
4. `src/store.rs:1148` `tighten_state_files` passes each private state name, including `tsk.json`, to `tighten_file` at line 1156. Load at line 714 calls the same function.
5. `src/fsperm.rs:302` `tighten_file` discards the path off Unix.
6. `src/store.rs:826` opens the temp document with `create_file`. `StdFilesystem::create_file` at line 284 calls `create_private_file`.
7. `src/fsperm.rs:181` `create_private_file` opens with write, create, and truncate. `mode(0o600)` exists only inside `cfg(unix)` at lines 176-179.

### Evidence

- `src/fsperm.rs:9` says no permission is claimed off Unix.
- `src/fsperm.rs:36` calls `reject_reparse_ancestors` and attaches no security descriptor.
- `src/fsperm.rs:55` gates directory tightening with `cfg(unix)`.
- `src/fsperm.rs:179` puts `mode(0o600)` inside the Unix block.
- `src/fsperm.rs:302` ignores the path off Unix.
- `src/store.rs:284` implements `create_file` as `create_private_file`.
- `src/store.rs:714` calls `tighten_state_files` on load.
- `src/store.rs:1403` and `src/store.rs:1408` are the Windows default paths.
- `site/src/content/docs/docs/storage.md:13` documents those paths and no Windows ACL.
- `site/public/install.ps1:767` and `site/public/install.ps1:847` create the program directory, not the state directory. The repository has no DACL, `icacls`, or `Set-Acl` call.

### Blockers

- The inherited DACL is an operating-system fact. This repository never sets or records a Windows security descriptor, so source cannot show whether a second non-admin account has an allow ACE.
- A Windows DACL observation is required and was not performed. This host is Linux, so it cannot create a Windows profile or query a DACL, and target code was not executed.

### Local plan

On a fresh Windows 10 or 11 profile that is not a member of Administrators and is not elevated, leave `TSK_STATE_DIR` unset and do not pass `--state-dir`. Run `tsk add` with a dummy title so the default state directory and `tsk.json` are created. As that same user, run `icacls` with only the path, and no `/grant`, `/deny`, `/remove`, `/reset`, `/setowner`, `/inheritance`, `/restore`, or `/replace`, on `%LOCALAPPDATA%\tsk` and on `%LOCALAPPDATA%\tsk\tsk.json` and record every ACE and inheritance flag. If `LOCALAPPDATA` was unset, use `%USERPROFILE%\.tsk` instead. From a second local account that is also not a member of Administrators, not SYSTEM, and not elevated, attempt only to read those two paths and stop at access denied or at seeing the dummy text. Do not take ownership, change the ACL, or use an elevated token. If that second account can read the dummy text, the boundary the Unix build enforces with `0700` and `0600` is crossed. If access is denied and the DACL allows only the creating user, SYSTEM, and Administrators, the inherited DACL already matches that account boundary.

### Deployment plan

On an existing Windows store, run `icacls` with only the path on the state directory and on `tsk.json`. Do not change the ACL, do not take ownership, and do not open the task file. Record whether any allow ACE, including an inherited ACE, grants read to a principal other than the owner, SYSTEM, and Administrators. Stop after recording the DACL.
