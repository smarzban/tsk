# Needs validation

One carried lead. It has no severity. It is not a confirmed vulnerability. The project-title commit did not change the permission helpers or the non-Unix launcher write. Store line numbers moved. Source still cannot show the inherited Windows DACL.

## Windows state files inherit the parent directory DACL

Fingerprint: `windows-state-dir-acl`.

A second local Windows account, not the creating user and not Administrators or SYSTEM, might read the state directory, `tsk.json`, or a launcher script if the inherited DACL allows it. On Unix the same files are mode `0600` inside a `0700` directory. Plaintext readable only by the creating account would not cross this boundary.

### Trace

1. `src/store.rs` `default_state_dir` line 1439. When `TSK_STATE_DIR` is unset and `LOCALAPPDATA` is set, the Windows default is that directory joined with `tsk`. Line 1444 falls back to `USERPROFILE\.tsk`.
2. `src/store.rs` `save_unlocked_supported` line 829 calls `ensure_private_dir`, then `tighten_state_files` at line 831.
3. `src/fsperm.rs` `ensure_private_dir` line 45 creates a missing directory with `create_dir_all`. The `0700` tighten at line 55 is Unix-only.
4. `src/store.rs` `tighten_state_files` line 1192 calls `tighten_file`. Load at line 756 does the same. The scan is not recursive and does not name a launcher script.
5. `src/fsperm.rs` `tighten_file` line 302 discards the path off Unix.
6. `src/store.rs` line 862 creates the temp document through `create_private_file`.
7. `src/fsperm.rs` `create_private_file` line 181 opens the file. `mode 0600` at line 179 is Unix-only.
8. `src/dispatch.rs` `launcher_file` line 1143 joins the state directory with `launchers`.
9. `src/dispatch.rs` `write_launcher` line 1834, off Unix, uses `create_dir_all` and `std::fs::write`. The Unix branch uses `create_private_file`. The script from `powershell_script` (`src/agents.rs` line 650) embeds profile environment values.

### Blocker

The inherited DACL is an operating-system fact. This repository never sets a Windows security descriptor. This host is Linux, so the DACL was not observed. Target code was not executed.

### Local plan

On a fresh Windows 10 or 11 profile that is not a member of Administrators and is not elevated, leave `TSK_STATE_DIR` unset. Run `tsk add` with a dummy title so the default state directory and `tsk.json` are created. Do not dispatch. As that same user, run `icacls` with only the path, and no grant or deny switch, on the new state directory and on `tsk.json`. Record the DACL. This Linux host cannot run that check.

### Deployment plan

On an existing Windows store, run `icacls` with only the path on the state directory, on `tsk.json`, and, when present, on the `launchers` directory and one existing `.ps1` under it. Do not change the ACL, do not take ownership, do not dispatch, and do not open the task file or the launcher. Record whether any allow ACE, including an inherited ACE, grants read to a principal other than the owner, SYSTEM, and Administrators. Stop after recording the DACL.
