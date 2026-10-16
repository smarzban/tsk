# Architecture (scoped delta audit)

tsk is a single-user local terminal task board (Rust TUI and headless CLI) and the Herdr plugin `herdr-tsk`. The human at the keyboard and agents invoked as that same OS user share one JSON store. There is no tenant authenticator. The OS account and filesystem permissions are the isolation boundary. Comparable baseline: a local-first CLI kanban with a terminal host and a curl-or-PowerShell updater. Same-account actions are ordinary use.

Stack: Rust (`src/main.rs` routes board, CLI, setup, update), ratatui, serde JSON, an Astro docs site, Python packaging. Offline checks would be `cargo test` and `npm test` in `site/` when dependencies are already present. This run does not fetch crates, hit GitHub, or drive a live Herdr socket. `unshare`, `prlimit`, and `timeout` exist; bubblewrap and firejail do not. No read-only target mount was prepared, so this wave does not execute target code.

Reviewed commit `82e934e9dc77192fedb184f986b1e1a26c3a8a7d`, clean worktree, delta from `ceca7657a52d244d1d89bff04c70a9cc5ea93d7e`:

- `#214` (`9582b05`) replaces `site/public/capture.js` `normalizeThread`. It no longer rewrites illegal characters to dashes. It accepts only `^[A-Za-z0-9][A-Za-z0-9.-]{0,31}$` and lowercases, matching `src/domain/thread.rs` `normalize_thread`. `site/public/board-demo.js` still imports `parseCapture` and paints row labels through `esc()` before `innerHTML`.
- `#216` (`82e934e`) releases v0.12.0: `Cargo.toml`, `Cargo.lock`, `herdr-plugin.toml`, and `site/src/version.mjs` move from 0.11.6 to 0.12.0. `CHANGELOG.md` renames Unreleased to v0.12.0. `src/announcements/catalog.toml` appends announcement id 5. `src/cli/update.rs` passes `concat!("v", env!("CARGO_PKG_VERSION"))` as `TSK_CURRENT_VERSION`. `site/public/install.sh` prints that value and compares it only after `^v[0-9]+\.[0-9]+\.[0-9]+$`.

Entry surfaces in this pass:

- Demo capture text to `normalizeThread`, then task `thread`, then `esc()` before HTML.
- Bundled catalog (`include_str!` in `src/announcements.rs`) seeded by `seed_on_open` into one notice task via `create_notice`. Notes are painted with `wrap_text` (`src/ui/edit.rs`), which runs `terminal_text` per character, then `paint_notes_line`. CLI detail uses the same `wrap_text` inside `append_wrapped`.
- Compiled version string into the update environment. The background curl selector in `src/update.rs` did not change.
- Setup stdout and stderr. `terminal_text` (`src/ui/mod.rs`) still rewrites Unicode `Cc` before human setup output. That control was fixed in `9b37456` (#210) and revalidated at `ceca7657`. Its source is unchanged.

Trust boundaries and the strongest source-visible control:

- Demo thread to DOM: the new charset cannot contain `<`, quotes, or controls, and `esc()` still runs on the label.
- Notice text to a terminal: `terminal_text` inside `wrap_text`. Bidi and format characters are still not rewritten.
- Update version to a shell: the string is a compile-time literal, and the installer rejects any other shape before comparing.
- Unix state files: `0700`/`0600`. Windows: reparse-point rejection only. That lead stays carried.
- Skill paths: `terminal_text` on human setup output. `--skill-states` still replaces controls with `?`.

Starting paths: `site/public/capture.js`, `site/public/board-demo.js`, `src/domain/thread.rs`, `src/announcements.rs`, `src/announcements/catalog.toml`, `src/ui/edit.rs`, `src/ui/markdown.rs`, `src/ui/mod.rs`, `src/cli/presenter.rs`, `src/setup.rs`, `src/setup_agent.rs`, `src/cli/update.rs`, `src/update.rs`, `site/public/install.sh`, `Cargo.toml`.

Prior run: `security-audit/2026-10-16` at `ceca7657` (`/tmp/prior-audit/security-audit/2026-10-16/`). No other `security-audit/*` branch exists. One carried record, `windows-state-dir-acl` (`needs_validation`): `src/fsperm.rs` and `src/store.rs` are unchanged, so it stays on the hunter exclusion list and goes to the Phase 3 verifier. The demo DOM unit is `prior_covered_changed_source` because `capture.js` changed the value that reaches `esc()`. Setup terminal injection is `prior_covered_same_source` and is re-read because this run must confirm #210 still holds. The obvious-exposure unit is `prior_covered_changed_source` because the changelog section was rewritten. Unchanged covered units and the prior out-of-scope units stay `out_of_scope`. This pass is partial.

Companions: client-side DOM for the demo; supply-chain release and update for the version handoff. Not selected for the notice: it is terminal presentation of bundled text, covered by ordinary injection, not a browser, helper, or updater. Not selected: web auth, cloud IAM, RPC, multi-tenant lifecycle, LLM tools, memory safety, resource exhaustion.
