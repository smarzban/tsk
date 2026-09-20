---
title: Install
description: Install tsk, connect Herdr, and give your agent the skill.
---

Available for macOS, Linux, and Windows 10/11 on ARM64 and x86-64. Windows package-manager submissions and code signing are not included yet.

## Install

### macOS and Linux

```sh
curl -fsSL https://gettsk.sh/install.sh | sh
```

Or use Homebrew:

```sh
brew install smarzban/tap/tsk
```

Reopen your terminal if prompted, or run the printed `export` command.

### Windows 10/11 ARM64 and x86-64

Download the complete installer before running it in Windows PowerShell 5.1 or PowerShell 7:

```powershell
Invoke-WebRequest https://gettsk.sh/install.ps1 -OutFile "$env:TEMP\install-tsk.ps1"
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$env:TEMP\install-tsk.ps1"
```

The installer verifies the release ZIP against `SHA256SUMS`, installs `tsk.exe` under `%LOCALAPPDATA%\Programs\tsk\bin`, and adds that directory to your user PATH. Open a new terminal, then run `tsk setup herdr` and `tsk setup` if wanted. Windows artifacts are checksum-verified but not code-signed.

On macOS and Linux, if Herdr is already installed, the curl installer asks whether to run `tsk setup herdr` when a terminal is available. When global agent skill roots are detected, it also asks once whether to install or update the tsk skill for those agents. The installer closes with one `Done.` line — `Done. Run tsk in a project directory to open the board`, plus `, or press prefix+t in Herdr` after an accepted Herdr setup — and one row per step that did not run: `Herdr plugin:  tsk setup herdr` when Herdr setup was declined, skipped under CI or no TTY, or failed, and `Agent skills:  tsk setup` when the skill ask was declined or skipped. With `TSK_INSTALL_DIR` set, the installer skips both asks and prints `Custom install directory: setup was not run. When you are ready:` with the full binary path in both rows. Homebrew stays noninteractive and prints the `tsk setup herdr` and `tsk setup agents` commands as a caveat.

## Add to Herdr

[Install Herdr](https://herdr.dev/docs/install/) first. Setup requires Herdr 0.9 or newer: an older `herdr` on PATH is refused with `herdr X.Y.Z found; tsk needs 0.9.0 or newer` before anything is written, so update Herdr and run `tsk setup herdr` again.

```sh
tsk setup herdr
herdr server reload-config
```

| Shortcut | Action |
| --- | --- |
| **prefix+t** | Open or focus the board in this workspace |
| **prefix+a** | Quick capture |

Use your configured Herdr prefix. An existing board in this workspace is focused across tabs without resetting its view. If none exists here, a new board opens; other workspaces keep their boards. Windows setup writes PowerShell launchers and uses `%APPDATA%\herdr\config.toml`; Herdr currently describes Windows plugin support as preview. [Herdr keyboard guide](https://herdr.dev/docs/keyboard/).

## Agent skill

```sh
tsk setup
tsk setup pi
```

On a TTY, bare `tsk setup` detects installed agents and asks once to install or update the skill. Use `omp`, `claude`, `cursor`, `grok`, `codex`, or `opencode` instead of `pi` for a single named target. Setup installs the CLI skill in that agent's user-level skills directory; OMP uses its active profile.

[Detection, version updates, and overwrite options](/docs/cli/#setup).

## First task

Open the board and press `+`, type a title, then `Enter`. Or use the CLI:

```sh
tsk add -t "your task title"
```

Give your agent the task number to work on it. [Board guide](/docs/board/).

For standalone use, run `tsk` directly in your terminal.

## Upgrade

Run `tsk update` to upgrade an installer-managed copy. It downloads the same checksum-verifying installer used for the first install, and runs it only once the download completed. It always follows the latest published release (a `TSK_VERSION` in your shell is ignored) and refuses to move backwards if the running copy is newer than that release. On Windows, close every other running board first. The verified replacement waits in the install directory until the `tsk update` process exits, then an out-of-process PowerShell helper atomically replaces it and refreshes existing integrations; a refresh failure is recorded as `.tsk-update-error.log` beside `tsk.exe` with the command to retry. A Homebrew copy stays under Homebrew's control: `tsk update` prints `brew update && brew upgrade tsk` instead.

| Installed with | Upgrade |
| --- | --- |
| Installer | `tsk update` |
| Homebrew | `brew update && brew upgrade tsk` |
| Source | Pull changes and rebuild |

After the binary is replaced, `tsk update` refreshes what is already set up:

- A registered Herdr plugin (both plugin commands bound, on any keys) is re-registered without asking and reported as `Herdr plugin refreshed.` Reload Herdr's config afterwards. If Herdr is on PATH but not set up, it asks as on a first install.
- Installed agent skills at an older version are updated. On a terminal it asks once (`tsk skill installed for claude (v1.2.0), codex (v1.2.0); update to v1.3.0? [Y/n]`, Enter means yes); without one it updates unattended. It then lists the agents it updated. Skills that are already current print nothing. An update never installs a skill for an agent that did not have one; if no skill is installed anywhere, it offers the first-install ask for the detected agents.
- A refresh that fails leaves the binary in place, prints `tsk setup`'s reason, and prints the matching `tsk setup` row. Updating onto a release older than this behaviour prints the plain `tsk setup` rows instead.

Close and reopen running boards to use the new binary.

The board shows a notice when a newer release is available. [Update-check settings](/docs/storage/#update-check).

## Uninstall

| Installed with | Remove |
| --- | --- |
| Installer | Delete `tsk` on macOS/Linux, or `%LOCALAPPDATA%\Programs\tsk\bin\tsk.exe` on Windows |
| Homebrew | `brew uninstall tsk` |

Task data remains in place.

To remove Herdr integration, run `herdr plugin unlink herdr-tsk`, remove the two shortcut bindings, and reload Herdr's config.

To undo installer PATH changes on macOS/Linux, remove the `# tsk PATH` comment and its following `case` line from the startup files named during installation. On Windows, remove the tsk install directory from your user `Path` environment variable. Neither uninstall path removes task data.

## macOS and Linux installer details

The installer verifies the release's SHA-256 checksum and installs to `~/.local/bin`. It does not require sudo or change task data.

| Setting | Purpose |
| --- | --- |
| `TSK_INSTALL_DIR` | Choose an absolute installation directory (no colons or newlines); the post-install setup asks are skipped |
| `TSK_VERSION=vX.Y.Z` | Install a specific stable release |
| `--help` | Show help without downloading |

Requires `curl`, `tar`, `sed`, and `sha256sum` or `shasum`.

## Windows installer details

`install.ps1` requires Windows 10/11 ARM64 or x86-64 and Windows PowerShell 5.1 or PowerShell 7. It detects the native machine architecture even from an emulated x86-64 process, downloads the matching MSVC ZIP and `SHA256SUMS` over TLS with finite time, redirect, compressed-size, and expanded-size limits, requires one exact matching digest, validates the archive's three root entries, and stages `tsk.exe` beside the destination before replacement. It refuses unsupported architectures, reparse-point destinations and install paths, directory destinations, relative paths, and PATH separators.

| Setting | Purpose |
| --- | --- |
| `TSK_INSTALL_DIR` | Choose an absolute directory; a first install prints setup commands rather than running them |
| `TSK_VERSION=vX.Y.Z` | Install a specific stable release |
| `-NoPathUpdate` | Install without changing the user PATH |
| `-Help` | Show help without network access |

The default is `%LOCALAPPDATA%\Programs\tsk\bin`. Unless `-NoPathUpdate` is used, the installer updates user PATH only, case-insensitively and without duplicates, while preserving environment-variable references and the registry value's existing string kind. `tsk update` supplies its process ID so a detached stock Windows PowerShell helper can wait out Windows executable locking; existing bound Herdr setup and outdated installed agent skills are refreshed after replacement.

### macOS and Linux PATH

If the install directory is missing from PATH, the installer adds it to:

| Shell | Startup files |
| --- | --- |
| Zsh | `$ZDOTDIR/.zshrc`, or `~/.zshrc` |
| Bash | `~/.bashrc` and the first existing login profile; defaults to `~/.profile` |

The installer does not source these files. Symlinked, non-regular, or unwritable files are skipped with manual instructions. Other shells require manual PATH setup. Successful edits remain if another file cannot be updated.

### Herdr prompt

When `herdr` is on PATH after the binary is installed, the installer may ask to run plugin setup:

- Interactive terminal (stdin TTY, or `/dev/tty` under `curl | sh`) using the default `~/.local/bin` destination: asks `[y/N]`. Yes runs the newly installed `tsk setup herdr`. An overridden `TSK_INSTALL_DIR` never executes the newly published binary; the closing block prints the `tsk setup herdr` and `tsk setup` commands with the full binary path instead.
- `CI` set, or no usable TTY: skips the ask so the install never hangs.
- Herdr absent: no Herdr prompt.

The install always ends with one closing block:

- Setup accepted: `Done. Run tsk in a project directory to open the board, or press prefix+t in Herdr.`
- Declined, CI/no-TTY skip, or setup failure: `Done. Run tsk in a project directory to open the board.` followed by a `    Herdr plugin:  tsk setup herdr` row.
- Agents detected but the skill batch declined or skipped: a `    Agent skills:  tsk setup` row.
- Herdr absent: the `Done.` line only (no Herdr row).

Setup failures do not undo the install. Homebrew does not run this prompt; use the formula caveat or run `tsk setup herdr` yourself.

## Herdr setup with an installed binary

Setup registers bundled plugin files using the installed executable. No source checkout is needed.

- Shortcut conflicts ask for confirmation. Declining keeps the existing binding, and the output says so.
- A noninteractive conflict stops before writing.
- Config changes create a backup beside the config, `config.toml.tsk-backup-<YYYYMMDD-HHMMSS>` in UTC; a second setup within the same second appends `-1`, `-2`, ....
- Rerunning setup updates the registration without duplicating bindings.
- Use the stable command on PATH, not a versioned Homebrew Cellar path.

Installation alone does not update an existing plugin registration unless you answer yes to the curl installer's Herdr prompt. [Setup recovery and configuration](https://github.com/smarzban/tsk/blob/main/packaging/README.md#herdr-setup-and-upgrades).

## Manual archives

Download the archive for your platform and `SHA256SUMS` from the same [release](https://github.com/smarzban/tsk/releases).

| Platform | Archive target |
| --- | --- |
| macOS Apple silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux ARM64 | `aarch64-unknown-linux-musl` |
| Linux x86-64 | `x86_64-unknown-linux-musl` |
| Windows 10/11 ARM64 | `aarch64-pc-windows-msvc` |
| Windows 10/11 x86-64 | `x86_64-pc-windows-msvc` |

Unix archives are named `tsk-vX.Y.Z-<target>.tar.gz`; Windows archives use `.zip` and contain `tsk.exe`. Verify with `shasum -a 256` on macOS, `sha256sum` on Linux, or `Get-FileHash -Algorithm SHA256` on Windows. Compare the full digest with `SHA256SUMS`, then extract the executable into a directory on PATH.

## Source build

Requires Rust 1.96.0:

```sh
git clone https://github.com/smarzban/tsk.git
cd tsk
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

For plugin development from a checkout, Herdr 0.9.0+ supports `herdr plugin link "$PWD"`. Rebuild after pulling changes.

[Contributing](https://github.com/smarzban/tsk/blob/main/CONTRIBUTING.md) · [Storage and configuration](/docs/storage/)
