# Architecture

Scoped delta of smarzban/tsk at `94b1dc6f864c064f69631d240b1e91c9bece2cdd`, from `8d714a201280318f92ed42d26d20bf233a1f5baf` (22 commits, worktree clean). Profile: standard. This is a partial pass. In scope: trust boundaries the diff edits, code those paths rely on, the carried Windows state-directory lead, and a same-source recheck of two controls closed in the prior run.

## Product and principals

tsk is a single-user terminal task board. The operator is the OS account that runs the binary. Protected resources are the state directory (`tsk.json`, `config.toml`, launcher scripts), git worktrees and branches created for a dispatch, and the Rex or Herdr session named by that user's environment.

There is no login and no tenant id. Lower-trust data inside the account is task title, notes, steps, and replies; profile argv in `config.toml`; a stored dispatch record; and stdout from `rex`, `herdr`, `git`, and `gh`. Another OS account matters only where tsk sets a file mode or DACL.

A local analogue is a user-owned git worktree plus a terminal multiplexer. The user can already run git and a shell. A finding has to cross a control tsk claims (path rule, quoting, private files, ambiguous-project refusal), not the user acting on their own account.

## Stack and execution limits

Rust 1.96.0 binary (`src/main.rs`), an Astro docs site, and shell and PowerShell installers. Host kind is `HERDR_ENV=1`, else a non-empty `REX_SESSION`, else standalone (`src/dispatch/host.rs`). `cargo test` writes `target/` and can fetch crates. `npm ci` fetches. Both are prohibited here. Rex and Herdr are not installed. Tests stub them.

This host can `unshare` a user, mount, and net namespace and remount a bind read-only. bubblewrap, firejail, and a user systemd bus are absent. One sandbox that also enforces an empty environment, a read-only toolchain, scratch-only writes, and low CPU, memory, process, file-size, disk, and wall-clock limits was not established. Target code is not executed. A claim that needs a run stays `needs_validation`.

## Entry paths in this delta

Store v16 records `host` and `session` on a dispatch. Rex builds `<worktree-root>/<repo basename>/<sanitized branch>` with `git worktree add`, then sends JSON to `rex api call` (`src/dispatch/rex.rs`). The command is `/bin/sh` and a launcher file. The root defaults to `<state>/worktrees`. `[dispatch] worktrees` may be any absolute path that does not put `<root>/<repo>` inside the project (`worktree_root`).

Cleanup removes a worktree only when the path is absolute, not the project root, listed by `git worktree list` for that project, clean, and `follows_path_rule` (directory name is the sanitized branch, parent name is the repo basename) or the Herdr checkout matches.

Task fields are substituted into the profile argv and single-quoted in the Unix launcher, or base64-encoded in PowerShell (`src/agents.rs`). The pane types only `sh_launch_line` or the PowerShell `-File` line. A reply is JSON-pasted into a Rex block, or one argument to `herdr agent prompt`.

`tsk setup rex` writes `tsk.lua` next to Rex `init.lua`. The binary path goes through `lua_string` (`src/setup/rex.rs`).

A bare project token that matches two basenames is refused (`src/scope.rs`). The permissive resolver used by `list -p` still prefers an exact stored bare scope.

`site/public/install.sh` calls `setup rex`. It still downloads a release archive and checks `SHA256SUMS`. `site/package.json` bumps Astro and related packages. `board-demo.js` changes one displayed path string. `esc` is unchanged.

## Controls

Worktree location: refuse a root whose `<root>/<repo>` sits inside the project. Config may point anywhere else.

Cleanup: git registration, a clean tree, not the project root, and the basename path rule. The rule does not require the state directory.

Shell: launcher quoting, plus a character allowlist on the typed path.

Rex: JSON bodies via `serde_json`. `open_tab` requires a session id that starts with `session:` and contains no `/`.

Wrong project: two basename matches error. One match selects that stored path.

Unix other users: `0700` and `0600` in `src/fsperm.rs`.

Windows other users: this tree sets no DACL.

Backup userinfo: `scheme_token` still strips a trailing http(s) scheme. `src/backup.rs` is not in the diff.

Terminal controls: `terminal_text` still encodes C0 and C1. `src/cli/presenter.rs` changed and must be checked.

## Starting paths

`src/dispatch.rs`, `src/dispatch/host.rs`, `src/dispatch/rex.rs`, `src/agents.rs`, `src/setup/rex.rs`, `src/setup/rex.lua`, `src/scope.rs`, `src/store.rs`, `src/domain/task.rs`, `src/cli/presenter.rs`, `src/cli/reply.rs`, `src/fsperm.rs`, `src/backup.rs`, `src/ui/mod.rs`, `site/public/install.sh`, `site/package.json`.

## Prior coverage

`security-audit/2026-10-23` at `8d714a2` confirmed nothing. `windows-state-dir-acl` is carried. `src/fsperm.rs` is unchanged, and `default_state_dir` still uses `%LOCALAPPDATA%\tsk` with no security descriptor. Hunters exclude that fingerprint. A Phase 3 verifier owns that unit.

`scheme_token` and `terminal_text` are re-read because the prior run closed those issues and this request asks for a recheck. `presenter.rs` is new caller surface for the terminal control. Surfaces whose security-relevant files this diff does not edit are out of scope, not covered again.

## Companions

DESKTOP-MOBILE-AND-LOCAL-IPC.md: Rex and Herdr CLIs, launcher files, setup writes, Windows ACL.

DATA-ISOLATION-AND-LIFECYCLE.md: cleanup, project basename, store v16 migration.

AI-AND-LLM.md: task fields become an agent argv and a reply paste. A user dispatching their own agent is not a finding by itself.

SUPPLY-CHAIN-AND-RELEASE.md: `install.sh` and the site lockfile bump.

Not selected: WEB (no session server), CLOUD (no IAM), PROTOCOLS (no protocol implementation), CLIENT-SIDE (demo edit is a string; `esc` was not edited), MEMORY-SAFETY (no new unsafe parser on these paths), RESOURCE-EXHAUSTION (no shared quota).
