# Native distribution

How tsk is built, packaged, and released, for maintainers. This file owns the release
steps. The user-facing install guide is [install.md](https://gettsk.sh/docs/install/).

## Artifacts

Each stable `vX.Y.Z` release carries six archives, each built natively:

| Target | Build runner | Format |
| --- | --- | --- |
| `aarch64-apple-darwin` | `macos-15` | `.tar.gz` |
| `x86_64-apple-darwin` | `macos-15-intel` | `.tar.gz` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` | `.tar.gz` |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | `.tar.gz` |
| `aarch64-pc-windows-msvc` | `windows-11-arm` | `.zip` |
| `x86_64-pc-windows-msvc` | `windows-2025` | `.zip` |

- Unix archives are `tsk-vX.Y.Z-<target>.tar.gz`, holding `tsk`, `LICENSE`, and `README.md`
  at the root. Windows archives are `tsk-vX.Y.Z-<target>.zip`, holding `tsk.exe`,
  `LICENSE`, and `README.md`.
- macOS builds set `MACOSX_DEPLOYMENT_TARGET=11.0`. Linux builds use a native musl
  compiler, independent of the runner's glibc. Both Windows architectures build and test
  natively with MSVC.
- `SHA256SUMS` covers the six archives plus `install.sh` and `install.ps1`.
- `tsk.rb`, the Homebrew formula, pins the four Unix archives by URL and SHA-256.
- tsk is not published to crates.io: `Cargo.toml` keeps `publish = false`.

## Build locally

Python 3.11+ is needed for the maintainer scripts, not for users.

```sh
python3 -m unittest discover -s tests/packaging
shellcheck site/public/install.sh
python3 scripts/release.py check-version vX.Y.Z
```

- `check-version` requires the tag to match `Cargo.toml`, the `tsk-tui` entry in
  `Cargo.lock`, `herdr-plugin.toml`, and `site/src/version.mjs`. The workflow runs it
  first, so a partial bump fails before anything is built.
- Set `TSK_TEST_BINARY` to a built binary to add the installer binary smoke and the setup
  PTY test.
- The `Installer` workflow runs the packaging tests and ShellCheck on installer,
  release-script, or packaging-test changes, without rebuilding Rust. Rust CI runs the
  binary smokes after its release build.

To package by hand, build from the release tag with
`cargo build --release --locked --target <target>`, then:

```text
python3 scripts/release.py package <tag> <target> <binary-path> <output-directory>
python3 scripts/release.py assemble <tag> <output-directory>
```

- `package` refuses to overwrite an archive.
- `assemble` requires all six archives and validates their contents before writing
  checksums or a formula. It refuses an existing file or symlink at every output
  (`install.sh`, `install.ps1`, `SHA256SUMS`, `tsk.rb`): use a clean output directory.
- Neither checks a binary's machine architecture; the workflow's native build matrix owns
  that. Never feed one host binary into several targets for a real release.

## Release steps

1. **Bump the version (one PR).** Update `Cargo.toml`, `Cargo.lock`, `herdr-plugin.toml`,
   and `site/src/version.mjs` together. Rename the `CHANGELOG.md` Unreleased section to
   `## vX.Y.Z` and open a fresh, empty Unreleased above it. Append an `[[announcement]]` to
   `src/announcements/catalog.toml` if the release deserves a board notice.
   `python3 scripts/release.py check-version vX.Y.Z` must pass. Merge.
2. **Tag.** With maintainer approval, push the stable tag `vX.Y.Z`. Tags match
   `v[0-9]+.[0-9]+.[0-9]+` only: the workflow, `release.py`, and both installers refuse
   anything else, so there are no `-rc` tags.
3. **Build the draft.** Confirm Actions runner availability and billing, then dispatch
   `Prepare release` with the existing tag. It pins the tag to one commit, tests and builds
   the six targets from it, and creates a **draft** holding the archives, `SHA256SUMS`,
   both installers, and a version-pinned `tsk.rb`. It refuses a moved tag or an existing
   release, and never publishes or updates the tap. The draft is named with
   `--title "$TAG"`: an unnamed GitHub release shows the tagged commit's subject instead.
4. **Check the draft.** Review the `release-bundle` workflow artifact and the draft assets,
   and confirm the draft's tag still points at the built commit.
5. **Make it a pre-release**, not a release:
   `gh release edit vX.Y.Z --draft=false --prerelease`. GitHub excludes pre-releases from
   `releases/latest`, so installers and the board's update nudge stay on the previous
   stable while the assets are public. A pre-release is listed on `/releases` with a
   `Pre-release` badge and notifies watchers. Only a draft is fully hidden, and a draft
   cannot serve installer downloads.
6. **Rehearse the user flows** with isolated state (see [Rehearsal traps](#rehearsal-traps)).
   Unix installer, then the formula from the release asset:

   ```sh
   TSK_VERSION=vX.Y.Z TSK_INSTALL_DIR=/tmp/tsk-rc/bin sh -c "$(curl -fsSL https://gettsk.sh/install.sh)"
   env TSK_STATE_DIR=/tmp/tsk-rc/state /tmp/tsk-rc/bin/tsk
   gh release download vX.Y.Z -p tsk.rb -D /tmp/tsk-rc && HOMEBREW_DEVELOPER=1 brew install --formula /tmp/tsk-rc/tsk.rb
   ```

   Windows 10/11, ARM64 and x64, from the release asset:

   ```powershell
   Invoke-WebRequest https://github.com/smarzban/tsk/releases/download/vX.Y.Z/install.ps1 -OutFile "$env:TEMP\install-tsk.ps1"
   $env:TSK_VERSION='vX.Y.Z'; $env:TSK_INSTALL_DIR="$env:TEMP\tsk-rc\bin"
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:TEMP\install-tsk.ps1"
   ```

   - The site serves both installers from `main`. If one changed in this release, fetch
     that installer from `releases/download/vX.Y.Z/` instead.
   - Smoke `tsk setup herdr` under isolated roots ([AGENTS.md](../AGENTS.md#isolated-state)).
   - Rehearse the update path against a throwaway `HOME` holding an outdated skill and a
     Herdr config on a custom key:

     ```sh
     env HOME=/tmp/x/home XDG_CONFIG_HOME=/tmp/x/home/.config HERDR_SOCKET_PATH=/tmp/x/none.sock \
       TSK_VERSION=vX.Y.Z TSK_UPDATE=1 TSK_CURRENT_VERSION=vPREV TSK_INSTALL_DIR=/tmp/x/bin \
       sh /tmp/x/install.sh
     ```

   - Install on every supported architecture, including a locked Windows self-update.
7. **If the rehearsal fails**, the tag is burnt: fix forward with the next patch version.
   Leave the bad pre-release, or delete it with approval. Inspect an interrupted draft
   before deleting anything.
8. **Write the notes.** Replace the draft's skeleton and checklist with the version's
   `CHANGELOG.md` section verbatim, dropping empty subsections. Keep the one-line Install
   section.
9. **Promote** the tested assets, no rebuild:
   `gh release edit vX.Y.Z --prerelease=false --latest`. Confirm
   `https://github.com/smarzban/tsk/releases/latest` redirects to the new tag: installers
   follow that redirect, not a version sort or `main`.
10. **Update the tap.** Then, and only then, copy the generated `tsk.rb` to
    `Formula/tsk.rb` in `smarzban/homebrew-tap`, test it, and push. A release alone never
    updates Homebrew, and no cross-repository token is configured here.
    `packaging/homebrew/README.md` is the tap's README and maintenance checklist.
11. **Smoke the public flows** on clean machines:
    - `releases/latest` exposes both Windows ZIPs, `SHA256SUMS`, and `install.ps1`.
    - `curl -fsSL https://gettsk.sh/install.sh | sh`
    - In a Windows PowerShell 5.1 window, `irm https://www.gettsk.sh/install.ps1 | iex`.
      The URL returns the script with HTTP 200, not a redirect or HTML.
    - `brew install smarzban/tap/tsk`

### Rehearsal traps

- Never run `tsk update` on a pre-release copy before promotion. It follows
  `releases/latest`, still the previous stable, so it downgrades the binary, and the
  downgraded binary then refuses the store the pre-release migrated.
- Never point a candidate binary at a store you did not create for the rehearsal. Pass the
  state override as `env TSK_STATE_DIR=… /path/tsk`. In a Herdr pane, read until an idle
  prompt first: a pending shell prompt can eat the first character of a `VAR=value`
  prefix, and the candidate then opens and migrates the real `~/.tsk`.
- `brew install --formula tsk.rb` on a machine with the tap's `tsk` installed replaces the
  daily copy (same formula name) and removes the old keg. Use a throwaway machine, or
  `brew unlink` first and expect to `brew reinstall smarzban/tap/tsk` afterwards.

### Who runs what

- Maintainer steps: pushing the tag, `Prepare release`, publishing, the tap, the public
  installer flows, and the Linux, Intel, and both Windows architecture smokes.
- An agent sandbox cannot execute a downloaded script, and the checked-in rehearsal can
  only run the macOS ARM archive binary directly. An agent verifies the checksums, the
  archive binary, the upgrade path, and setup, and says which of these it did not run.

## Installer contracts

What the installers promise; `tsk update` and support answers rely on it. The user-facing
description lives in [install.md](https://gettsk.sh/docs/install/).

| Contract | `install.sh` (macOS, Linux) | `install.ps1` (Windows 10/11) |
| --- | --- | --- |
| Needs | `curl`, `tar`, `sha256sum` or `shasum`, and standard POSIX tools | Windows PowerShell 5.1 or PowerShell 7, 64-bit |
| Platform | From `uname`; an Intel or Rosetta shell selects Intel | Native architecture, even from an emulated x64 PowerShell |
| Default directory | `~/.local/bin` | `%LOCALAPPDATA%\Programs\tsk\bin` |
| Options | `TSK_INSTALL_DIR` (absolute), `TSK_VERSION=vX.Y.Z` (a public release tag) | The same, plus `-Help` (offline syntax) and `-NoPathUpdate` |
| Download | Archive verified against `SHA256SUMS` before staging | Redirects, time, downloaded and expanded bytes bounded; ZIP verified against `SHA256SUMS` before extraction |
| Replace | Atomic | Same-volume atomic |
| Refuses | A symlink or directory at the destination; a path containing `:` or a newline | A reparse point or directory at the destination; a path containing `;`, CR, or a newline |
| PATH | A duplicate-safe entry for Bash (`~/.bashrc` and the first existing login profile, default `~/.profile`) or Zsh (`${ZDOTDIR-$HOME}/.zshrc`). Never sources or overwrites startup files. Symlinked, non-regular, or unwritable files and other shells get manual guidance | User PATH only, without duplicates, keeping the raw value and its registry string kind. A failed write prints guidance; the install still succeeds |
| Asks | When interactive (stdin TTY or a usable `/dev/tty`): run `tsk setup herdr` if `herdr` is on PATH, and install or update the agent skill once if skill roots exist. `CI` or no TTY skips the asks; a custom `TSK_INSTALL_DIR` skips both on install | The same |
| Finish | One `Done.` line, then setup nudges | The same |
| Never | Undoes the binary when setup fails; touches task data | The same |

`tsk update` downloads the platform installer and runs it with `TSK_UPDATE=1`,
`TSK_CURRENT_VERSION`, and `TSK_INSTALL_DIR` set to the running binary's directory. A
Homebrew install gets `brew upgrade` guidance instead.

- In update mode the installer refreshes integrations: a Herdr registration that binds
  both plugin commands is refreshed without asking, even under `CI`; otherwise the usual
  ask applies. It reads `tsk setup herdr --check` and `tsk setup --skill-states` for this
  (stable probes; see AGENTS.md, Host integration).
- On Windows, when the running `tsk.exe` is locked, the installer stages the verified
  binary and starts a detached Windows PowerShell helper. The helper waits for
  `TSK_UPDATE_PID` to exit, then replaces `tsk.exe` atomically. The installer refuses while
  other `tsk.exe` processes run from the same path.

### Homebrew

- The formula follows explicit release URLs and SHA-256 values.
- Homebrew installs are noninteractive and never run the curl installer's asks. The
  formula caveat tells users to run `tsk setup herdr` and `tsk setup agents`.
- Uninstall through the channel used to install: `brew uninstall tsk`, or remove the
  installer-created executable. Neither deletes `~/.tsk`.

## Herdr setup internals

How `tsk setup herdr` edits Herdr's config and registers the plugin. What users see is in
[install.md](https://gettsk.sh/docs/install/#herdr-setup-details).

### Config and safe writes

- Setup requires Herdr 0.9+ on PATH, or Herdr's `0.9.0-preview.*` Windows build, checked
  with `herdr --version` before any write. An older host is refused with an update
  message.
- Config path: `HERDR_CONFIG_PATH`, else `$XDG_CONFIG_HOME/herdr/config.toml`, else
  `~/.config/herdr/config.toml`. On Windows the fallback is `%APPDATA%\herdr\config.toml`.
  Empty values count as unset.
- A symlinked config file or config directory (dotfiles managers) is followed: setup edits
  the link's target in place and keeps the link. The staged candidate, atomic rename, and
  backup land beside the real file; generated assets and the lock stay beside the
  configured path. A dangling or looping link is refused before any read or write.
- On Unix an open directory descriptor anchors every write, backup, rename, and cleanup,
  so a replaced parent cannot redirect them. Windows refuses reparse-point ancestors and
  final paths around each operation.
- A kernel lock on `.tsk-setup.lock` serializes setup: a concurrent run fails at once, and
  the lock is released on process death. The file stays on disk and does not mean a setup
  is running.

### Plugin assets

- The embedded manifest and launchers are written to `tsk-plugins/<content-hash>` beside
  the config. The hash is FNV-1a-64 over versioned, length-prefixed UTF-8 asset names and
  contents, not Rust's implementation-dependent `DefaultHasher`. The manifest includes the
  crate version, so an upgrade changes the root.
- Herdr 0.9.0 replaces registrations by plugin ID: the online handler inserts into its
  ID-keyed map, and the offline CLI removes entries with that ID before inserting the
  replacement. Setup checks the new registration before removing the intact old managed
  root.
- Setup never calls `plugin unlink herdr-tsk` after linking: that would unlink the new
  registration. Explicit setup replaces an existing `herdr-tsk` link.
- Source-checkout and other installation roots are never removed, and a missing prior root
  needs no cleanup. Modified stale assets are kept and reported with their path.
- When the previously registered root no longer exists (a rehearsal that relinked the live
  Herdr to a temp dir), setup says so on stderr before re-registering.
- Setup keeps the invoked binary path, such as the unversioned Homebrew symlink, not its
  canonical Cellar target. Board and capture share it. Reopen boards after a binary
  upgrade.

References: Herdr 0.9.0
[`handle_plugin_link`](https://github.com/herdrdev/herdr/blob/v0.9.0/src/app/api/plugins/mod.rs)
and [`persist_plugin_offline`](https://github.com/herdrdev/herdr/blob/v0.9.0/src/cli/plugin.rs).

### Shortcuts and backups

- The shortcut planner keeps the configured prefix and unrelated settings. It asks before
  replacing each conflicting `prefix+t` / `prefix+a` binding; declining keeps that
  shortcut. A noninteractive conflict aborts before any filesystem or host change.
- An accepted builtin override left with no bindings is removed, restoring Herdr's
  default.
- Herdr checks the candidate config before registration. The existing config is staged as
  a temporary backup before linking and the config is replaced atomically afterwards.
  Then the backup is promoted to `config.toml.tsk-backup-<YYYYMMDD-HHMMSS>` in UTC (`-1`,
  `-2`, … when that second already has one).
- A failed link removes the temporary backup. A failed config replace or backup promotion
  names any recovery copy it kept.
- A registration failure leaves the config intact; a later failure names the partial
  state. Success prints any backup path and the shortcuts; the content-addressed root
  appears only in error messages.
- Shortcuts apply after `herdr server reload-config` or a Herdr restart.

### Upgrades

`tsk update` refreshes a registration that already binds both plugin commands without
asking. Otherwise its installer asks as on a fresh install; `tsk setup herdr` updates the
manifest and launchers at any time.

### Remove or recover

To remove the integration:

1. Close the board and its popups.
2. Run `herdr plugin unlink herdr-tsk`.
3. Remove its two command bindings.
4. Reload the config.

- Restore a setup backup only if it will not discard later edits.
- Removing the binary alone leaves Herdr config and `~/.tsk` intact.
- A failed asset integrity check names the file to inspect. Never erase an unrelated
  checkout or package-manager installation to recover setup.
- Smoke tests isolate state as [AGENTS.md](../AGENTS.md#isolated-state) says. Never relink
  a daily plugin for a test.

## Accepted risks

- **Checksums, not signatures.** `SHA256SUMS` over HTTPS is not code signing or independent
  publisher authentication: the checksum file and the binaries share GitHub's trust
  boundary.
- **Tag-move race.** A small window remains between the workflow's last tag check and
  GitHub creating the draft. Inspect the draft's tag before publishing, and enable GitHub
  immutable releases when available.
- **Minimum OS.** A successful compile does not prove installation on the minimum OS;
  release smoke tests it.
- **Workflow wiring.** Artifact and draft-handoff tests do not prove the native runner
  matrix; the maintainer-run release smoke is that check.
- **Herdr config ancestors.** On Unix, existing symlinks among the config path's ancestors
  are trusted; a linked config file or directory is resolved first, then fd-pinned.
  Rejecting a symlinked `~/.config` would break common dotfile setups, and Herdr follows
  that ancestor too. Symlinks inside `tsk-plugins` and at `.tsk-setup.lock` are refused.
- **Install directory.** `TSK_INSTALL_DIR` is a trusted location: anyone who can swap it
  can replace the executable directly, so protecting it is the user's boundary.
- **Plugin root hash.** FNV-1a detects changes; it is not cryptographic authentication.

## References

- [Homebrew tap structure](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap)
- [Formula configuration and tests](https://docs.brew.sh/Formula-Cookbook)
- [GitHub runner images](https://github.com/actions/runner-images)
