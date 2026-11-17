# Security delta audit, 2026-11-18

Scoped standard pass of `smarzban/tsk` at `d5b1e7f3cef1d7f70569573c99b52e928f70811f`. The range is `d6016c00a673c76e6a0dd30de9cb6ad498c50be3..d5b1e7f3cef1d7f70569573c99b52e928f70811f` (due dates, store format 7 and refusal of dev formats 8-18, `tsk edit --thread` / `--clear-thread`, releases v0.14.0 and v0.14.1) plus the code those commits touch. Budget was unset. Nineteen agent invocations ran: four reconnaissance, seven hunters, two critics, four Phase 3 verifiers, two Phase 5 verifiers. Execution policy was sandboxed source and local only. This host can drop the network with a user namespace, and `cargo` is installed, but the crate registry and `target/` are absent. A build would fetch crates, so target-controlled execution did not run. Scratch files were not used as evidence.

This is a partial pass. It does not cover dispatch, the installer, the updater, or earlier surfaces left out of scope.

Prior run: `security-audit/2026-11-17` at `fee08e72`, source `d6016c00`. No same-source confirmed finding was carried. Both carried `needs_validation` leads were re-checked. `ui.paint.project-basename.c0` still matches the current paint path; the Due footer does not escape the project name. `windows-state-dir-acl` is unchanged because `src/fsperm.rs` did not change. The Unix mode unit that previously shared the Windows fingerprint is covered from source: Unix creates use owner-only modes. That fingerprint stays only on the Windows record.

The post-wave critic and the final-clean critic each returned no missing in-scope unit and no reassignment.

## Posture

Due days and thread names are checked before they are stored, and the new terminal and HTML sinks escape them. Store versions above 7, including dev formats 8-18, are refused rather than read as the released layout. No new confirmed vulnerability was established. Two earlier leads stay unresolved because the decisive fact is outside this host.

## Confirmed findings

None.

## Needs validation

| Lead | Trace | Blocker | Next check |
| --- | --- | --- | --- |
| Untitled project folder name reaches a fitting Span | `ProjectNames::name` to `task_header_state` and the project group header, then `bound_line` and `put_line_at` | Ratatui's tty backend is not in the tree, and this host cannot compile the crate, so the bytes for a C0 scalar in a fitting Span were not observed | With a cached registry and a pty, open an untitled project whose folder name contains U+001B and compare the fitting header and group-header cells with the escaped form `terminal_text` would emit |
| Windows state files inherit the parent DACL | `default_state_dir` to `create_dir_all` and the non-Unix `tighten_file` | The repository sets no Windows security descriptor, and this host is Linux | On Windows, create the default state directory with `tsk` and record whether a second non-admin account can read `tsk.json` |

These are not confirmed vulnerabilities and have no severity.

## Hardening and patterns that held

- `parse_due` accepts only a real Gregorian day and stores `YYYY-MM-DD`. CLI refusals use Debug formatting and then `terminal_text`. Quick-add and task-page refusals are painted through `present_line`.
- `--thread` is `normalize_thread` at the argv boundary. The refusal does not include the rejected character. `--clear-thread` stores no thread.
- Human `tsk list` lines call `terminal_text` on `due` and `thread`. JSON list output uses serde string encoding.
- `load_document` refuses format 0 and any version above 7. A version 7 file is read directly. `DispatchWire` aliases `herdr_workspace_id` and defaults `host` the same way `migrate_v6_to_v7` does. A file that carries both session keys fails closed under `deny_unknown_fields`.
- The board demo inserts the quick-add draft and `state.refuse` only through `esc`. `parsed.due` is not interpolated into HTML.
- The release commits bump versions and skill text. They do not change the release workflow. The diff adds no credential, debug gate, or dynamic execution of input.
- `project_option_label` does call `terminal_text` on a folder name. The task-column state and the group header do not use that helper.

## Coverage

| Status | Count |
| --- | --- |
| covered | 10 |
| candidate | 2 |
| blocked | 0 |
| deferred | 0 |
| out of scope | 38 |

The two candidate units are the carried leads above. Final critic: stop, no accepted gap inside this delta.
