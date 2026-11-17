# Needs validation

Neither lead is a confirmed vulnerability. Neither has a severity. The Due-date, thread, and store-format changes did not close them and did not add a new path that makes either one locally observable on this host.

## Untitled project folder name reaches a fitting Span

Fingerprint: `ui.paint.project-basename.c0`.

An untitled project's folder name can contain a C0 or C1 scalar on Unix. `ProjectNames::name` returns that folder name when no title is stored. The wide task-column state and the project group header place it in a Span. When the line fits, `bound_line` returns the spans unchanged, so `terminal_text` does not run, and `put_line_at` renders the line. The new Due footer escapes the due day. It does not escape this project name. While the Due editor is the active field, the state slot reads `editing due` and the name is absent only for that moment.

Lower-trust starting point: a directory name the operator opens as a Git project.

Boundary: terminal presentation of that name on the board.

Possible result: a control scalar reaches the tty from a fitting Span. Whether ratatui emits that scalar was not observed.

### Trace

1. `src/context.rs:99` `build_snapshot`. A Git root becomes `TaskScope::Project` from `to_string_lossy`, with no control-character filter.
2. `src/capture.rs:74` `capture_save`. A capture with no scope override stores `snapshot.default_scope`.
3. `src/scope.rs:123` `ProjectNames::name`. With no title, the name is `project_basename`.
4. `src/scope.rs:70` `project_basename`. The last non-empty component is kept. A separator is only `/`, or `\` on Windows.
5. `src/ui/board/model.rs:3806` `project_name`. The name is copied with no `terminal_text`.
6. `src/ui/board/draw.rs:2672` `task_header_state`. The state string is the status, a middle dot, and that name.
7. `src/ui/board/draw.rs:2942` `draw_wide_board`. That string is `TaskColumnHeader.state`.
8. `src/ui/render.rs:1649` `draw_task_column`. The state is a dim Span. `present_line` is applied to the title, not the state.
9. `src/ui/queue.rs:1559` `group_key`. A project group label is `names.name(path)`.
10. `src/ui/render.rs:6865` `paint_group_header`. The group word is its own Span.
11. `src/ui/render.rs:7784` `bound_line`. A fitting line is returned as-is. `present_line` runs only when the line is too wide.
12. `src/ui/render.rs:7768` `put_line_at`. The line is rendered with `Paragraph::new`.

### Blocker

Ratatui's tty backend is not in this workspace. This host has no crate registry, so the crate was not compiled and no pty capture was taken.

### Resolution

Local, with crates already cached and a pty: create a Unix directory whose name contains U+001B, initialize a Git repository there, open it as an untitled project, add a task, and use a wide frame with project grouping. Capture the task-column state cell and the project group-header cell while both lines fit. Compare those bytes with the visible escape `terminal_text` would emit. Repeat with the column narrower than the state line. Do not point this at another person's directory.

`project_option_label` in `src/ui/board/model.rs` already calls `terminal_text` on a folder name. The header and the group header do not use it.

## Windows state files inherit the parent directory DACL

Fingerprint: `windows-state-dir-acl`.

On Windows, `default_state_dir` selects `LOCALAPPDATA\tsk` or `USERPROFILE\.tsk`. `ensure_private_dir` creates that directory with `create_dir_all`. Mode `0700` and file mode `0600` are compiled only for Unix. `tighten_file` discards its path off Unix. No call in this repository sets a Windows security descriptor on the state directory or `tsk.json`. `src/fsperm.rs` is unchanged in this delta.

Lower-trust starting point: another OS account on the same Windows machine.

Boundary: read of the operator's state directory.

Possible result: that account can read `tsk.json` if the inherited DACL allows it.

### Trace

1. `src/store.rs:1420` `default_state_dir`. On Windows, selects `LOCALAPPDATA\tsk` when that variable is set, otherwise `USERPROFILE\.tsk`.
2. `src/fsperm.rs:45` `ensure_private_dir`. Creates a missing directory with `create_dir_all`. The `0700` tighten is Unix-only.
3. `src/fsperm.rs:302` `tighten_file`. Off Unix the function discards the path.

### Blockers

The inherited DACL is an operating-system fact. Source cannot show whether a second non-admin account has an allow entry on the state directory or `tsk.json`.

This host is Linux, so that DACL was not observed and the Windows path was not executed.

### Resolution

Local, on a Windows machine the owner controls: create the default state directory with `tsk`, record `icacls` for the directory and `tsk.json`, and try to read the file from a second non-admin account.

On an installed Windows copy: record `Get-Acl` for that directory and `tsk.json`, and whether a second non-admin account can read them. Do not test against another person's profile.
