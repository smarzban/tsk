# Architecture (scoped delta audit)

tsk is a single-user local terminal task board (Rust TUI plus headless CLI) and the Herdr plugin `herdr-tsk`. The human at the keyboard and coding agents invoked as that same OS user share one JSON store (`tsk.json`) under a state directory. There is no tenant authenticator. The OS account and filesystem permissions are the isolation boundary. Comparable baseline: a local-first CLI kanban with a terminal host (Herdr) and a curl-or-PowerShell updater; same-account actions are ordinary use.

Stack: Rust 1.96 (`src/main.rs` routes board, capture, CLI, setup, update), ratatui/crossterm, serde JSON store, Astro docs site, Python packaging. Offline checks are `cargo test` into `target/` and `npm test` in `site/` when dependencies are already present. This run does not fetch crates, hit GitHub, or drive a live Herdr socket.

Entry surfaces in this scoped pass:

- Setup and agent-skill output. Human stdout/stderr for skill paths and the gone Herdr plugin root now pass through `terminal_text` (`src/ui/mod.rs`), which rewrites Unicode `Cc` (C0/C1) as `\u{XXXX}`. JSON uses `serde_json`. The installer probe `skill_states_text` replaces controls with `?`.
- Text editing delta (`src/ui/edit.rs`, `src/ui/input.rs`, `src/ui/render.rs`, `src/ui/mouse.rs`, `src/ui/board/apply.rs`, `src/app.rs`). New word and line chords and an Enter-inserts-a-line hint. Draft paint still escapes via `terminal_text` before width measurement.
- Static board demo (`site/public/board-demo.js`) hint strings only; titles still go through `esc()` before `innerHTML`.
- Prior narrative leads with no ledger file on disk: background `TSK_UPDATE_CURL` (`src/update.rs` `fetch_latest` calls `curl_path` and `hardened_curl`), bidi left untouched by `present_line`, `PI_CONFIG_DIR` component filtering in `omp_config_root`, and Windows state-dir ACLs absent from `src/fsperm.rs`.

Trust boundaries and strongest source-visible control:

- Untrusted path or task text to a terminal: `terminal_text` / `present_line`. Bidi and format characters are explicitly not rewritten.
- Skill install destination: symlink refusal, atomic replace, and home-relative `PI_CONFIG_DIR` with the root prefix stripped. `..` components are preserved and then lexically popped.
- Unix state files: `0700`/`0600` via `ensure_private_dir`. Windows: reparse-point rejection only; the file says no permission hardening is claimed.
- Update fetch: absolute `TSK_UPDATE_CURL` or `/usr/bin/curl`, HTTPS-only curl flags, shared by `tsk update` and the background check. The chosen binary is executed as the user.

Starting paths: `src/ui/mod.rs`, `src/cli/presenter.rs`, `src/setup.rs`, `src/setup_agent.rs`, `src/ui/edit.rs`, `src/ui/input.rs`, `src/update.rs`, `src/cli/update.rs`, `src/fsperm.rs`, `site/public/board-demo.js`.

Prior runs: `git ls-remote` found no `security-audit/*` or `security-fix/*` branches and no `coverage-ledger.json` or `findings.json` in history. Narrative leads from the previous audit are current work, not carried fingerprints. The merged terminal-escape fix (commit `9b37456`, PR #210) changed the setup output path and is a revalidation target, not a hunter exclusion. Everything outside the delta and those four leads is `out_of_scope`.

Companions: desktop local-file (skill writes, Windows ACL), supply-chain update (background curl), client-side DOM (demo `innerHTML`). Not selected: web auth, cloud IAM, RPC brokers, multi-tenant data lifecycle, LLM tool authority (the skill file is embedded text; tsk does not call a model), memory-safety (no `unsafe` in `src/ui`), and resource exhaustion (cost stays in the invoking process).

Sandbox: `unshare --user --map-root-user --net --mount` works; `prlimit` and `timeout` exist; no bwrap/firejail. Parent-side no-follow promotion is available in Python. Target execution, if used, must stay inside that wrapper. This pass is partial: scoped standard, not a full-repo audit.
