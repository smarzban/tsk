# Needs validation

These leads are not confirmed vulnerabilities. They have no severity. Do not send audit traffic to a deployed service.

## Untitled project folder names can carry C0/C1 into board spans

Fingerprint: `ui.paint.project-basename.c0`

Boundary: an untitled project's folder name is copied into the task-header state and the project group header. `bound_line` keeps a fitting line, so those spans skip `terminal_text`.

Lower-trust input: a directory name on disk, used as a Git project with no title. The operator who opens that directory is the same OS user as the board.

### Trace

1. `src/context.rs:99` `build_snapshot`. A Git root becomes the project scope path through `to_string_lossy`, which keeps valid C0 and C1 scalars from the directory name.
2. `src/capture.rs:74` `capture_save`. A capture with no scope override stores that path on the new task.
3. `src/scope.rs:123` `ProjectNames::name`. With no project title, `name` returns `project_basename`, with no control-character filter.
4. `src/ui/board/model.rs:3780` `BoardModel::project_name`. Returns that name and does not call `terminal_text`.
5. `src/ui/board/draw.rs:2640` `task_header_state`. The header state is the status word plus `project_name`.
6. `src/ui/render.rs:1647` `draw_task_column`. The state string is placed in `Span::styled`. The title beside it goes through `present_line`. The state span does not.
7. `src/ui/render.rs:7777` `bound_line`. When the line fits, `bound_line` returns the original line.
8. `src/ui/render.rs:7768` `put_line_at`. A fitting line is painted. The folder name is still in that line.

The same folder name is the project group word. `paint_group_header` at `src/ui/render.rs:6858` puts it in a span, and the group row is drawn at `src/ui/render.rs:5716`.

`project_option_label` at `src/ui/board/model.rs:7928` does call `terminal_text`. The header state and the group key do not use that helper.

This delta's step-progress label is a count. `present_line` escapes it. It is not the header-state span.

### Blocker

Cargo and the tsk binary were not run, and the cargo registry is absent, so ratatui was not executed. No `TestBackend` or tty capture shows the bytes written for a span that contains a C0 or C1 scalar. Ratatui's buffer and backend sources are not in this workspace, so whether a control stored in a cell symbol is emitted to the tty or dropped cannot be read here.

### Resolution

Local: with a local toolchain, render `draw_board` into a ratatui `TestBackend` for an untitled project whose folder name contains U+001B, with one task in that project and a frame wide enough that the header state fits. Assert the state cell symbol still contains U+001B. Repeat with grouping set to project and assert the group-header word cell. A second local backend that records its write can show whether that symbol is emitted as the ESC byte. Keep the check on a local `TestBackend`.

No deployment check. This is not a live service.

## Windows state files inherit the parent directory DACL

Fingerprint: `windows-state-dir-acl`

Boundary: state directories and files are created with `create_dir_all`, `OpenOptions::open`, and `std::fs::write`, and no Windows security descriptor. Owner-only mode and `tighten_file` are compiled only for Unix.

### Trace

1. `src/store.rs:1435` `default_state_dir`. On Windows the state directory is `LOCALAPPDATA\tsk` when `LOCALAPPDATA` is non-empty, otherwise `USERPROFILE\.tsk`. The function returns a path and does not attach a security descriptor.
2. `src/fsperm.rs:45` `ensure_private_dir`. A missing directory is created with `create_dir_all`. `tighten_private_dir` is `cfg(unix)` at line 55.
3. `src/store.rs:862` `save_unlocked_supported`. After `ensure_private_dir` at line 829, the save creates the temp document that replaces `tsk.json`. `create_file` delegates to `create_private_file`.
4. `src/fsperm.rs:173` `create_private_file`. Mode `0600` is applied only inside `cfg(unix)` at lines 176-179. The Windows open supplies no security descriptor.
5. `src/dispatch.rs:1834` `write_launcher`. The non-Unix branch creates the launchers directory with `create_dir_all` and writes the script with `fs::write`. `launcher_file` at line 1143 places that script under the state directory.
6. `src/agents.rs:283` `create_private_temp`. Config temps set mode `0600` only on Unix.
7. `src/fsperm.rs:300` `tighten_file`. Off Unix the helper ignores the path.

`site/public/install.ps1:767` installs into `LOCALAPPDATA\Programs\tsk\bin`, not the state directory. Line 849 creates that directory with `New-Item`. The script contains no `Set-Acl`. The extract change in this delta does not touch the state directory.

`Cargo.toml:76` enables `Win32_Security` on `windows-sys`. No Rust source calls a DACL API.

### Blocker

The inherited DACL is an operating-system fact. This repository never sets a Windows security descriptor, so source cannot show whether a second non-admin account has an allow ACE on the state directory, `tsk.json`, or a launcher script. This host is Linux, so the DACL was not observed and target code was not executed.

### Resolution

Local: on a fresh non-admin Windows profile, run `tsk add` with a dummy title and record `icacls` of the new state directory and `tsk.json` without changing the ACL.

Owner check: on an existing store, record `icacls` of the state directory, `tsk.json`, and any launcher script, without changing the ACL or opening the files.
