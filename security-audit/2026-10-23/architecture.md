# Architecture (scoped delta)

Source ref `8d714a201280318f92ed42d26d20bf233a1f5baf` on `main`, delta `b5b92d60f2a70f29798354e6096b52ae30871a04..8d714a201280318f92ed42d26d20bf233a1f5baf` (7 commits). Worktree clean. Profile `standard`, scoped. Budget unset. Prior run `origin/security-audit/2026-10-22` reviewed `b5b92d60` and is complete.

## Product and principals

tsk is a single-user terminal task board and CLI. The OS account that runs the binary is the operator. There is no application login. Other local OS users, a configured git backup remote, the GitHub releases document, and Herdr's unsigned context JSON are the lower-trust principals that matter here. Protected resources are `tsk.json`, `config.toml` (agent env and backup URLs), backup clones, and the terminal the operator is watching.

Comparable baseline, from the repo's own git and Herdr integration: a backup is a local `git` client, and the board is a Herdr pane. Git's own rule that `argv` is visible to other local users and the environment is not is the trade-off the new backup helper documents (`src/backup.rs`).

## Stack and execution limits

Rust binary `tsk` (toolchain 1.96.0), Astro docs site, shell and PowerShell installers. Offline checks would be `cargo test` against temp state dirs and the in-crate backup git fixtures. This VM can `unshare` a user and network namespace and Node exposes `O_NOFOLLOW`, but it does not enforce a read-only target mount, an empty allowlisted environment, or CPU, memory, process, file-size, and disk limits together. Target-controlled execution is blocked for this run. Evidence is source review only.

## Entry surfaces in this delta

Backup URLs enter from `config.toml` and `tsk backup add` (`src/backup.rs` `parse_target`, `add_target_locked`). `Remote::of` strips http(s) userinfo into `TSK_BACKUP_*` for a constant shell helper; clone, fetch, push, and `ls-remote` receive `remote.url` after `--`. `sanitized_config` strips those credentials and agent env values before a push. `redacted` and `target_error` mask URLs in errors. `scheme_token` is the fix for the prior bare-https finding.

The board open path now keeps an `UpdateCheck` and polls it (`src/app.rs`, `src/update.rs`, `src/ui/board/model.rs`). The notice string is the release `tag_name`. Paint goes through `present_line` (`src/ui/render.rs`). Fetch still uses the fixed GitHub URL and `hardened_curl` or the Windows downloader.

`tsk setup` writes both embedded skills. Human output runs the skills root through `terminal_text` (`src/cli/presenter.rs`). JSON emits raw paths. `skills_root` walks two parents of the skill file path.

The static demo parses `!a`, `!b`, and `!w` in `site/public/capture.js` and assigns HTML with `esc` before `innerHTML` (`site/public/board-demo.js`). Deleted board helpers in `src/ui/input.rs`, `src/ui/capture.rs`, and `src/ui/render.rs` had no remaining callers.

`src/git_base.rs` production process-group and job-object code is unchanged; the delta rewrites Windows tests. `src/fsperm.rs` and `src/store.rs` are unchanged.

## Trust boundaries

Unix state files are mode `0700`/`0600` (`src/fsperm.rs`). Windows creation rejects reparse points and does not set a DACL. Backup git is pinned with an empty hooks directory, `core.symlinks=false`, no interactive prompts, and a host-matched helper only when the URL carried an http(s) login. The remote is not trusted to plant symlinks (`write_in_clone`). Update metadata is unauthenticated JSON over TLS, displayed, not executed. Skill installs refuse symlink destinations.

## Prior coverage

Carried needs_validation `windows-state-dir-acl` still matches current `src/fsperm.rs` (Unix-only tighten). Phase 3 and Phase 5 kept it `needs_validation` because this host cannot observe a Windows DACL. Prior confirmed `backup.https-bare-userinfo.scheme-prefix` was revalidated in this range: `scheme_token` and `Remote::of` strip a bare https token even when bytes precede the scheme, so it is not a current finding. Same-source areas outside the delta are `out_of_scope` for this partial pass. Changed backup, update, setup, presenter, demo, and keymap-adjacent units were hunted and closed covered.

## Companions

`DATA-ISOLATION-AND-LIFECYCLE.md` for backup copies and restore. `SUPPLY-CHAIN-AND-RELEASE.md` for the release check. `CLIENT-SIDE.md` for the demo DOM. `DESKTOP-MOBILE-AND-LOCAL-IPC.md` for state-file ACLs, skill paths, and local git writes. No cloud IAM, broker, or model-tool boundary is in this delta.
