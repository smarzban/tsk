# Architecture

tsk is a local terminal task board for one OS user. The binary paints a board, accepts a headless CLI, and stores tasks under a state directory. This delta adds dispatch (git worktree, agent launcher, cleanup, pull-request merge), git backup and restore, paper trail, store versions through v15, a coordinator skill, and site scripts for the board demo and GitHub stars.

The OS user who runs tsk is the authority. Lower-trust inputs are task fields, `config.toml`, a restored or hand-edited `tsk.json`, Herdr environment variables, a backup remote, the task repository, the update download, pull-request CI, and a browser on the static site. Another local account is the carried Windows state-directory question. `TSK_AGENT` and `HERDR_ENV` are environment strings, not peer credentials.

Comparable baseline: a personal git helper. The user's repository and config are that user's own authority. Same-account actions are not cross-user findings. A cross-account read, a visitor script run, a CI secret reach, or a credential sent to a remote is.

Stack: Rust 1.96 (Cargo), an Astro site, Python packaging, and POSIX plus PowerShell launchers. `cargo test` and `node --test` can run offline when caches exist. This host has `unshare`, `prlimit`, and `timeout`. bubblewrap and firejail are absent. Hunters stay on source. A verifier may run a target test only inside a recorded user and network namespace, with a read-only tree, an empty allowlisted environment, and scratch-only writes.

Entry paths: CLI and env (`src/main.rs`, `src/cli/`), profiles (`src/agents.rs`), the store (`src/store.rs`), Herdr and git arguments (`src/dispatch.rs`, `src/git_base.rs`, `src/dispatch/pr.rs`), backup and restore (`src/backup.rs`, `src/cli/backup.rs`), replies (`src/cli/reply.rs`), update (`src/cli/update.rs`, `src/update.rs`), `site/public/board-demo.js`, `landing.js`, `github-stars.js`, and `.github/workflows/`.

Controls on those paths:

- Task text reaches an agent only through a launcher script. On Unix every argument and env value is single-quoted, and the pane types `/bin/sh` plus the launcher path. On Windows values are base64, and a batch-only program is refused.
- Cleanup removes a worktree only when git lists it, the path is not the project root, and the tree is clean. A branch is deleted only after a confirmed merge into the recorded base. `gh pr merge` is an argument array and does not pass `--admin`.
- Backup push strips URL userinfo and agent env values. Display redacts URL credentials. Restore checks the document, refuses an open board, and does not replace `config.toml`.
- Unix state files are mode `0700` and `0600`. Windows creation rejects reparse points and sets no ACL in source.
- Update fetches the installer over HTTPS and then executes it. That execution is the feature.
- Workflows on `pull_request` set `contents: read`. The Vercel token is used only on a push to `main`.
- The board demo escapes text before `innerHTML`. The stars script writes `textContent`.

Prior run `origin/security-audit/2026-10-21` reviewed `c6b8de10` as a landing-label delta. It has no confirmed finding. `windows-state-dir-acl` stays `needs_validation`: `src/fsperm.rs` is unchanged and this host is Linux. That unit waits for the Phase 3 verifier. It is not a hunter exclusion. Units whose files changed since `c6b8de10` are current work. `src/context.rs` did not change and dispatch does not call it, so capture JSON stays out of scope.

Companions: `DESKTOP-MOBILE-AND-LOCAL-IPC.md` for launchers, local files, and Herdr arguments. `DATA-ISOLATION-AND-LIFECYCLE.md` for backup, restore, migration, and cleanup. `CLIENT-SIDE.md` for the site. `SUPPLY-CHAIN-AND-RELEASE.md` for CI, release, and update. `MEMORY-SAFETY-AND-BINARY.md` only for `git_base` process-group teardown. `AI-AND-LLM.md` is excluded: tsk fills a user-owned argv and does not read model output. Resource-exhaustion, web-protocol, cloud, and protocol companions are excluded: there is no shared service, session, cloud IAM, or broker.

Profile: standard, scoped to the delta plus code it calls. Budget: unset.
