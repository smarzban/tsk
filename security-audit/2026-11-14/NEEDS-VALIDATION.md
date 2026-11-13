# Needs validation

One carried lead. No severity. Not a confirmed vulnerability.

## Windows state files inherit the parent directory DACL

Fingerprint: `windows-state-dir-acl`

A second local Windows account, which is not the creating user and not Administrators or SYSTEM, might read the creating user's state files if the inherited directory DACL allows it. Unix creates the state directory mode `0700` and task files mode `0600`. Windows helpers do not set a security descriptor.

### Trace

1. `src/store.rs:1439` `default_state_dir`. When `TSK_STATE_DIR` is unset and `LOCALAPPDATA` is set, the Windows default is that directory joined with `tsk`.
2. `src/store.rs:829` save calls `ensure_private_dir`, then `tighten_state_files` at line 831.
3. `src/fsperm.rs:45` `ensure_private_dir` creates a missing directory with `create_dir_all`. The `0700` tighten is `cfg(unix)` at line 55.
4. `src/store.rs:1192` `tighten_state_files` calls `tighten_file` for private state names.
5. `src/fsperm.rs:302` `tighten_file` discards the path off Unix.
6. `src/store.rs:862` the temp document is created through `create_private_file`.
7. `src/fsperm.rs:181` `create_private_file` opens with no security descriptor. `mode(0o600)` is `cfg(unix)` at line 179.
8. `src/dispatch.rs:1143` `launcher_file` places the script under `launchers` in the state directory.
9. `src/dispatch.rs:1834` `write_launcher` off Unix uses `create_dir_all` and `std::fs::write`.

`create_private_temp` in `src/agents.rs:287` applies `0600` only on Unix. `powershell_script` at `src/agents.rs:650` base64-embeds profile env values into the launcher script.

### Blocker

The inherited DACL is an operating-system fact. This repository never sets a Windows security descriptor. This audit host is Linux and did not execute target code, so it cannot show whether a second non-admin account can read the directory, `tsk.json`, or a launcher script.

### Resolution

Local: on a fresh Windows 10 or 11 profile that is not an administrator and is not elevated, leave `TSK_STATE_DIR` unset. Run `tsk add` with a dummy title. Do not dispatch. As that user, run `icacls` with only the path, and no grant, deny, remove, reset, setowner, or inheritance switch, on the new state directory and on `tsk.json`. Record the DACL.

Owner observation: on an existing Windows store, run `icacls` with only the path on the state directory, on `tsk.json`, and, when present, on the `launchers` directory and one `.ps1` under it. Do not change the ACL, take ownership, dispatch, or open the files. Record whether any allow entry grants read to a principal other than the owner, SYSTEM, and Administrators.

Plaintext readable only by the creating account would not cross this boundary.
