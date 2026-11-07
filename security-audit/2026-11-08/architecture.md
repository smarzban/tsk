# Architecture

Scoped delta of smarzban/tsk at `36610e8f012018e3ddd624304dbcf680ae4df56c` (parent `94b1dc6f864c064f69631d240b1e91c9bece2cdd`). One commit adds a per-project display title. tsk is a local terminal board for one OS user. Agents on that account use the CLI. There is no network service and no second tenant.

Protected state is `tsk.json` under the state directory. Unix creates it mode `0600` inside a `0700` directory (`src/fsperm.rs`). Windows sets no DACL in this tree. Dispatch launchers are scripts under that directory. Profile argv substitutes task fields only: `{number} {title} {notes} {steps} {worktree} {branch} {base}` (`src/agents.rs`). A project title is not one of those fields.

This commit’s paths:

- CLI `tsk project title` parses in `src/cli/parser.rs` and writes through `set_project_title` (`src/domain/task.rs`, `src/cli/archive.rs`).
- `-p`, `!p`, and project verbs resolve in `resolve_project_path` and `resolve_permissive_project_path` (`src/scope.rs`). A title match runs first. Two stored matches return ambiguous.
- The TUI editor is `src/ui/board/project_title.rs`. `Enter` calls the same setter.
- Store v17 adds optional `ProjectRecord.title`. `migrate_v16_to_v17` returns the document unchanged (`src/store.rs`). `merge_project_records` reapplies this process’s title and archive intents field by field.
- Load uses serde and does not re-run the setter’s refusal.

`set_project_title` trims whitespace, then refuses `char::is_control`, `/` (and `\` on Windows), the names desk and global, length over 80, and a case-insensitive duplicate. `terminal_text` (`src/ui/mod.rs`) still encodes C0/C1 and does not strip bidi. CLI title lines, list labels, the launch card, and `project_option_label` call it. `paint_group_header` paints `names.name()` as a raw span. JSON uses serde. `project_name` does not appear in `src/dispatch.rs` or `src/agents.rs`.

`scheme_token` in `src/backup.rs` is outside the diff and still strips a non-leading scheme before git sees a backup URL. `terminal_text` is outside the diff. The Windows state-directory lead is unchanged in `src/fsperm.rs` and in non-Unix `write_launcher`; `src/store.rs` line numbers moved.

No comparable product is named in the repo docs. Offline checks would be `cargo test`. This host cannot observe a Windows DACL. `unshare` can isolate mount and net. bubblewrap and a combined disk quota are absent, so target code stays unexecuted.

Prior run `origin/security-audit/2026-11-07` at `94b1dc6` had no confirmed finding. `windows-state-dir-acl` is carried. Same-source rechecks: `scheme_token`, launcher quoting. Changed-source rechecks: presenter output, project resolution, store migration (new v17 unit; v16 host migration is untouched).

Companions: data-isolation migration for the v17 merge; desktop local-file ownership for the Windows lead. AI prompt classes are out: titles are not template fields. Web, cloud, RPC, client DOM, memory-safety, and supply-chain classes do not match this diff. The skill-hash edit in `src/setup_agent.rs` is a pin bump, not a new fetch or signing path.
