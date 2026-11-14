# Needs validation

Two leads. No severity. Not confirmed vulnerabilities.

## Untitled project folder names can carry C0/C1 into board spans

Fingerprint: `ui.paint.project-basename.c0`

On Unix, a directory name may contain a C0 or C1 character. If the operator opens that directory as a Git project and the project has no title, the board copies the raw folder name into the wide task-column state and into a project group header. `bound_line` returns a fitting line without `terminal_text`. `put_line` renders those spans on the next paint, including the paint after `ctrl+l`. Whether the terminal then receives the scalar as a control was not observed.

A title passed through `set_project_title` is refused when it contains a control. This lead is the untitled folder name, not that setter.

### Trace

1. `src/context.rs:99` `build_snapshot` stores the Git root with `to_string_lossy`.
2. `src/capture.rs:74` `capture_save` uses `snapshot.default_scope` when no scope override is set.
3. `src/scope.rs:123` `ProjectNames::name` returns `project_basename` when the project has no title.
4. `src/ui/board/model.rs:3780` `project_name` returns that name and does not call `terminal_text`.
5. `src/ui/board/draw.rs:2640` `task_header_state` joins the status word to that name.
6. `src/ui/render.rs:1627` the state string is a `Span`. The title beside it goes through `present_line`.
7. `src/ui/render.rs:7757` `bound_line` returns the line when it fits.
8. `src/ui/render.rs:7748` `put_line_at` renders that line.

`src/ui/queue.rs:1555` `group_key` uses the same name. `src/ui/render.rs:6838` `paint_group_header` puts it in a `Span` and calls `bound_line`.

### Blocker

The crate was not executed. Ratatui's sources are not in this tree, so it is unobserved whether a C0 or C1 scalar stored in these spans is emitted to the terminal.

### Resolution

Local: with a local toolchain, render the board on a `TestBackend` for an untitled project whose folder name contains U+001B, on a frame wide enough that the header state fits. Assert the state cell. Repeat with grouping set to project and assert the group-header cell. A backend that records its write can show whether the symbol is emitted as the ESC byte. Keep the check local.

## Windows state files inherit the parent directory DACL

Fingerprint: `windows-state-dir-acl`

A second local Windows account, which is not the creating user and not Administrators or SYSTEM, might read the creating user's state files if the inherited directory DACL allows it. Unix creates the state directory mode `0700` and task files mode `0600`. Windows helpers do not set a security descriptor. This delta does not change those helpers.

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
