# Native distribution

The GitHub `Prepare release` workflow is manually dispatched and creates a **draft**, never a published release. Publishing and changing repository visibility require owner approval.

## Artifacts

Each stable `vX.Y.Z` release has six archives:

| Target | Build runner | Format |
| --- | --- | --- |
| `aarch64-apple-darwin` | `macos-15` | `.tar.gz` |
| `x86_64-apple-darwin` | `macos-15-intel` | `.tar.gz` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` | `.tar.gz` |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | `.tar.gz` |
| `aarch64-pc-windows-msvc` | `windows-11-arm` | `.zip` |
| `x86_64-pc-windows-msvc` | `windows-2025` | `.zip` |

Unix archives are named `tsk-vX.Y.Z-<target>.tar.gz`, containing `tsk`, `LICENSE`, and `README.md` at the archive root. Windows archives are named `tsk-vX.Y.Z-<target>.zip`, containing `tsk.exe`, `LICENSE`, and `README.md`. macOS builds set `MACOSX_DEPLOYMENT_TARGET=11.0`. Linux builds use a native musl compiler rather than depending on the runner's glibc version. Both Windows architectures are built and tested natively with MSVC. Actual minimum-OS installation still needs release smoke testing, not just a successful compile.

`SHA256SUMS` covers all six archives plus `install.sh` and `install.ps1`. `tsk.rb` remains generated from only the four Unix archive digests. This is checksum verification over HTTPS, not code signing or independent publisher authentication. The checksum file and binaries share the GitHub trust boundary.

## Local packaging

Python 3.11+ is required for maintainer scripts, not end-user installation. Set `TSK_TEST_BINARY` to a built binary to include the installer binary smoke and setup PTY test; Rust CI runs these after its release build. The installer workflow runs packaging tests and ShellCheck on installer, release-script, or packaging-test changes without rebuilding Rust.

```sh
python3 -m unittest discover -s tests/packaging
shellcheck site/public/install.sh
python3 scripts/release.py check-version v0.5.0
```

`check-version` requires the tag to match `Cargo.toml`, the `tsk-tui` entry in `Cargo.lock`, `herdr-plugin.toml`, and `site/src/version.mjs`; the workflow runs it first, so a partial bump fails before anything is built.

For a real release, use its actual stable tag and matching Cargo version, not the example above. Build from that tag with `cargo build --release --locked --target <target>`, then:

```text
python3 scripts/release.py package <tag> <target> <binary-path> <output-directory>
python3 scripts/release.py assemble <tag> <output-directory>
```

Package refuses to overwrite an archive. Assemble requires all six platform archives and validates their contents before writing checksums or a formula. It refuses existing files or symlinks for every output (`install.sh`, `install.ps1`, `SHA256SUMS`, `tsk.rb`); use a clean output directory. These commands do not build or validate the machine architecture of a supplied binary; the workflow's native build matrix owns that contract. Do not feed one host binary into multiple targets for a real release.

## Owner-gated release sequence

1. Review and merge the packaging, choose the next version, and update the crate, lockfile, plugin and site version together. Obtain approval before pushing the stable tag. The old `v0.5.0` tag does not include this workflow's scripts and cannot be used to package this change.
2. Confirm Actions runner availability and billing, then run `Prepare release` from the reviewed workflow with that existing tag. It resolves the tag once to a commit, uses the same commit for every platform, tests the target build, and checks that the tag has not moved before creating a draft. There is no automatic tag creation, public publishing, or replacement of existing releases. A tiny tag-move race remains between the final check and GitHub creating the draft: inspect the draft tag before publishing and enable GitHub immutable releases when available.
3. Review the `release-bundle` workflow artifact and draft assets. To rehearse the real user flow, flip the draft to a **pre-release** rather than publishing: `gh release edit vX.Y.Z --draft=false --prerelease`. GitHub excludes pre-releases from `releases/latest`, so default installers and the board's update nudge keep pointing at the previous stable release, while the assets become public. The pre-release is listed on `/releases` with a `Pre-release` badge and notifies release watchers; only a draft is fully hidden, and a draft cannot serve installer downloads. Test Unix with the pinned installer and isolated state: `TSK_VERSION=vX.Y.Z TSK_INSTALL_DIR=/tmp/tsk-rc/bin sh -c "$(curl -fsSL https://gettsk.sh/install.sh)"`. Test Windows 10/11 ARM64 and x64 with the matching release asset: `Invoke-WebRequest https://github.com/smarzban/tsk/releases/download/vX.Y.Z/install.ps1 -OutFile "$env:TEMP\install-tsk.ps1"; $env:TSK_VERSION='vX.Y.Z'; $env:TSK_INSTALL_DIR="$env:TEMP\tsk-rc\bin"; powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:TEMP\install-tsk.ps1"`. Test the formula from the asset: `gh release download vX.Y.Z -p tsk.rb -D /tmp/tsk-rc && HOMEBREW_DEVELOPER=1 brew install --formula /tmp/tsk-rc/tsk.rb`. Test installation on every supported architecture, including a locked Windows self-update. The public one-liners and cross-platform smokes are owner steps after promotion. Do not run `tsk update` on a pre-release copy before promotion: it follows `releases/latest`, still the previous stable. A failed rehearsal burns the tag, fix forward with the next patch version. Replace the draft checklist with actual release notes, inspect interrupted drafts before deleting anything.
4. Obtain approval to make the source repository publicly accessible if still private, and publish the release: from a pre-release rehearsal that is `gh release edit vX.Y.Z --prerelease=false --latest`, no rebuild, the tested assets are the shipped assets. The anonymous installer cannot download private or draft assets. Mark the intended stable release as latest; the installer uses GitHub's latest-release redirect, not a semantic-version sort or `main`.
5. Create the public `smarzban/homebrew-tap` repository with approval, using `packaging/homebrew/` as its scaffold. Put the generated `tsk.rb` in `Formula/tsk.rb`, test it, then commit and push. Repeat the formula update for every release, only after that release's assets exist publicly. No cross-repository write token is configured here.
6. Before publishing Windows instructions, confirm `releases/latest` exposes both Windows ZIPs, `SHA256SUMS`, and `install.ps1`. Then smoke the exact public commands on clean machines: `curl -fsSL https://gettsk.sh/install.sh | sh` and, in Windows PowerShell 5.1, `& { irm https://www.gettsk.sh/install.ps1 | iex }`. Confirm the PowerShell URL returns the script with HTTP 200 rather than a redirect or HTML response. Smoke the tap too.

## User contracts

On Unix, the installer uses `curl`, `tar`, `sed`, and `sha256sum` or `shasum`. It detects the running OS/architecture (an Intel/Rosetta shell selects Intel), defaults to `~/.local/bin`, and accepts `TSK_INSTALL_DIR` and a public `TSK_VERSION=vX.Y.Z` release tag. It verifies the selected archive before staging and atomically replacing the executable. It refuses a symlink or directory at the destination rather than modifying a package-manager-owned installation. After installation, it appends a duplicate-safe PATH entry for Bash (`~/.bashrc` and the first existing login profile, default `~/.profile`) or Zsh (`${ZDOTDIR-$HOME}/.zshrc`) when needed. It never sources startup files or overwrites their content, and leaves symlinked, non-regular or unwritable files alone with manual guidance. Unsupported shells also get manual guidance. PATH coaching prints reopen/export guidance when needed. When `herdr` is already on PATH, an interactive install (stdin TTY or usable `/dev/tty`) asks whether to run the newly installed `tsk setup herdr`, and when agent skill roots are detected asks once to install or update the tsk skill; `CI` or no TTY skips the ask so the install never hangs. The install closes with one `Done.` line, followed by setup nudges where needed. A custom `TSK_INSTALL_DIR` skips both asks. Setup failure does not undo the binary install. Install paths cannot contain PATH separators (colon/newline). Task data is untouched. `tsk update` runs the installer with `TSK_UPDATE=1` and `TSK_CURRENT_VERSION`, then refreshes existing Herdr and skill integrations.

On Windows 10/11 ARM64 or x64, run `& { irm https://www.gettsk.sh/install.ps1 | iex }` with Windows PowerShell 5.1 or PowerShell 7, or download the script first when using options. It detects the native architecture, including from an emulated x64 PowerShell process, and selects the matching release ZIP. `-Help` prints offline syntax for CI and users. It defaults to `%LOCALAPPDATA%\Programs\tsk\bin`, accepts `TSK_INSTALL_DIR` and a public `TSK_VERSION` tag, bounds HTTPS redirects, time, downloaded bytes, and expanded bytes, then verifies the ZIP against `SHA256SUMS` before extraction. It adds the install directory to user PATH without changing machine PATH or duplicating the entry, preserving the raw value and its registry string kind; `-NoPathUpdate` skips that change. A PATH write failure does not turn an installed binary into a failed install; manual guidance is printed. It rejects reparse-point and directory destinations. First-install Herdr and agent setup prompts, noninteractive skips, custom-directory behavior, and the closing block match the Unix installer. Initial installs and ordinary upgrades use a same-volume atomic replacement. In `TSK_UPDATE` mode, when Windows locks the running executable, the installer leaves the verified executable staged and launches a detached stock Windows PowerShell helper; the helper waits for `TSK_UPDATE_PID` to exit and then atomically replaces `tsk.exe`. Paths containing semicolon or newline are refused. Task data is untouched.

Homebrew follows the formula's explicit release URLs and SHA-256 values. A release alone does not update Homebrew. Homebrew installs are noninteractive and do not run the curl installer's Herdr prompt; the formula caveat tells users to run `tsk setup herdr` and `tsk setup agents`. Uninstall through the channel used to install: `brew uninstall tsk`, or remove the installer-created executable. Neither should delete `~/.tsk`.

## References

- [Homebrew tap structure](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap)
- [Formula configuration and tests](https://docs.brew.sh/Formula-Cookbook)
- [GitHub runner images](https://github.com/actions/runner-images)

crates.io publishing is separate task T29. `publish = false` remains in Cargo.toml.

## Herdr setup and upgrades

`tsk setup herdr` requires Herdr 0.9+ on PATH, checked from `herdr --version` before any write; an older host is refused with an update message. It uses `HERDR_CONFIG_PATH`, otherwise
`$XDG_CONFIG_HOME/herdr/config.toml`, otherwise `~/.config/herdr/config.toml`.
Empty config-path environment values are treated as unset. Config-file and final-directory symlinks are refused. On Unix, an open directory descriptor anchors all setup writes, backups, renames and cleanup, so a replaced parent cannot redirect them. Windows refuses reparse-point ancestors and final paths around each operation. A kernel lock on `.tsk-setup.lock` serializes setup and is released on process death. The lock file
stays on disk and does not imply a running setup.

The embedded manifest and launchers are materialized in `tsk-plugins/<content-hash>`
beside that config. The root name uses fixed FNV-1a-64 over versioned, length-prefixed UTF-8 asset names and contents, not Rust's implementation-dependent DefaultHasher. This is change detection, not cryptographic authentication. The manifest includes the crate version, so upgrading changes
the root. Herdr 0.9.0 replaces registrations by plugin ID: the online handler inserts
into its ID-keyed map; the offline CLI removes entries with that ID then inserts
the replacement. Setup checks the new registration before removing the intact old
managed root. It does not call `plugin unlink herdr-tsk` after linking: that would
unlink the new registration. Source-checkout and other installation roots are never
removed. A missing prior root needs no cleanup. Modified stale assets are retained and reported with their path.

References: Herdr 0.9.0
[`handle_plugin_link`](https://github.com/herdrdev/herdr/blob/v0.9.0/src/app/api/plugins/mod.rs)
and [`persist_plugin_offline`](https://github.com/herdrdev/herdr/blob/v0.9.0/src/cli/plugin.rs).

Setup retains the invoked binary path, including the unversioned Homebrew symlink,
not its canonical Cellar target. Board and capture share it. Reopen boards after a
binary upgrade. `tsk update` reruns setup for a registration that already binds both
plugin commands; otherwise rerun `tsk setup herdr` to update the manifest and launchers.
Explicit setup replaces an existing `herdr-tsk` link.

The shortcut planner preserves the configured prefix and unrelated settings.
It asks before replacing each conflicting prefix+t / prefix+a binding. Declining
keeps that shortcut; a noninteractive conflict aborts before filesystem or host
changes. If an accepted builtin override has no remaining bindings, it is removed,
restoring Herdr's default. Candidate config is checked by Herdr before registration;
existing config bytes are staged in a temporary backup before linking. The config is replaced atomically afterward, then the backup is promoted to its final timestamped name, `config.toml.tsk-backup-<YYYYMMDD-HHMMSS>` in UTC (`-1`, `-2`, ... when that second already has a backup). A failed link removes the temporary backup; a config replacement or backup-promotion failure names any retained recovery copy.
Registration failure leaves config intact; post-registration failures identify the
partial state. Success prints any backup path and the shortcuts; the content-addressed
plugin root appears only in error messages. When the previously registered root no longer
exists on disk (a rehearsal that relinked the live Herdr to a temp dir), setup says so on
stderr before re-registering. Reload config with `herdr server reload-config` or restart
Herdr to apply shortcuts.

To remove integration, close its board/popups, run `herdr plugin unlink herdr-tsk`,
remove its two command bindings, and reload config. Restore a setup backup only if
it will not discard later edits. Removing the binary alone leaves Herdr config and
`~/.tsk` intact.

For isolated smoke tests, override XDG config/state roots **and** `HERDR_SOCKET_PATH`,
as well as `TSK_STATE_DIR`. `HERDR_CONFIG_PATH` alone does not
isolate Herdr's running session or plugin registry. Never relink a daily plugin for
a test. A failed asset integrity check names the file to inspect; do not erase an
unrelated checkout or package-manager installation to recover setup.

Review scope decisions (F-1/F-7/F-16, F-6, F-11): ancestor symlinks in the Herdr
config path are trusted; the final directory is fd-pinned and final-component
symlinks are refused. Rejecting a symlinked `~/.config` would break common dotfile
setups, and Herdr follows that ancestor too. `TSK_INSTALL_DIR` is likewise a trusted
installation location: an attacker who can swap it can replace the executable
directly, so protecting that directory is the owner's boundary (F-6). F-11 is an
accepted workflow-wiring coverage limitation: artifact and draft-handoff tests do
not prove the native runner matrix; owner-run release smoke remains that check.
