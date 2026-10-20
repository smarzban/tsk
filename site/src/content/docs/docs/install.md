---
title: Install
description: Install tsk, connect Herdr, give your agent the skill, and upgrade or remove tsk.
---

Install tsk on macOS, Linux, or Windows 10/11, on ARM64 or x86-64. Then connect it to Herdr and give your agent the skill. Windows builds are checksum-verified but not code-signed, and are not in a Windows package manager yet.

## Install

### macOS and Linux

```sh
curl -fsSL https://gettsk.sh/install.sh | sh
```

Or use Homebrew:

```sh
brew install smarzban/tap/tsk
```

If the installer asks you to, reopen your terminal or run the `export` command it prints.

### Windows

Run this in a PowerShell window (Windows PowerShell 5.1 or PowerShell 7). From Command Prompt, run `powershell` first.

```powershell
irm https://www.gettsk.sh/install.ps1 | iex
```

It installs `tsk.exe` in `%LOCALAPPDATA%\Programs\tsk\bin` and adds that directory to your user PATH. Open a new terminal to use it.

### What the installer does

1. Downloads the release for your platform and checks it against the release's `SHA256SUMS`.
2. Installs `tsk` without sudo and adds it to your [PATH](#path).
3. If Herdr is installed, asks whether to run `tsk setup herdr`.
4. If it finds coding agents, asks once whether to install the tsk skill for them.
5. Ends with a [closing message](#prompts-and-closing-message) naming any setup step still to run.

Homebrew never asks. Its formula prints `tsk setup herdr` and `tsk setup agents` for you to run.

## Add to Herdr

[Install Herdr](https://herdr.dev/docs/install/) 0.9 or newer, then:

```sh
tsk setup herdr
herdr server reload-config
```

| Shortcut | Action |
| --- | --- |
| `prefix+t` | Open the board in this workspace, or focus it across tabs |
| `prefix+a` | Quick capture |

- Use your configured Herdr prefix. See the [Herdr keyboard guide](https://herdr.dev/docs/keyboard/).
- Focusing an open board keeps its view. Other workspaces keep their own boards.
- Herdr's Windows plugin support is a preview.

More: [Herdr setup details](#herdr-setup-details).

## Agent skill

`tsk setup` installs two skills for each agent:

| Skill | Teaches your agent to |
| --- | --- |
| `tsk-cli` | Read and update the board |
| `tsk-coordinator` | Run builder agents for you: brief them, review their pull requests, and live-test. [Coordinator skill](/docs/coordinator/) |

```sh
tsk setup        # find your agents and ask once
tsk setup pi     # install for one agent
```

In a terminal, bare `tsk setup` finds your installed agents and asks once to install or update the skills for all of them. To install for one agent, name it: `pi`, `omp`, `claude`, `cursor`, `grok`, `codex`, or `opencode`.

Skill directories and options: [`tsk setup`](/docs/cli/#setup).

## First task

Open the board with `tsk`, or `prefix+t` in Herdr. Press `+`, type a title, and press `Enter`. Or use the CLI:

```sh
tsk add -t "your task title"
```

Give your agent the task number to work on it. [Board guide](/docs/board/).

## Upgrade

| Installed with | Upgrade |
| --- | --- |
| Installer | `tsk update` |
| Homebrew | `brew update && brew upgrade tsk` |
| Source | Pull and rebuild |

`tsk update` runs the same checksum-verifying installer for the latest stable release. It ignores a `TSK_VERSION` in your shell and never moves to an older release. On a Homebrew copy it prints the `brew` command instead.

After it replaces the binary, `tsk update` refreshes what you already set up:

- **Herdr plugin:** re-registered without asking when both plugin shortcuts are bound, on any keys. Reload Herdr's config afterwards. If Herdr is installed but not set up, it asks as a first install does.
- **Agent skills:** outdated skills are updated. In a terminal it asks once, for example `tsk skill installed for claude (v1.2.0), codex (v1.2.0); update to v1.3.0? [Y/n]`; without one it updates them unattended. An agent with the tsk skill also gets the coordinator skill. It never adds a skill for an agent that had none, unless no agent has one: then it offers the first-install ask.
- A refresh that fails leaves the new binary in place and prints the `tsk setup` command to run.

Close and reopen running boards to use the new binary.

**Windows:** close every other running board first. `tsk update` stages the new `tsk.exe` and returns. The running file is replaced, and the refresh finished, after `tsk` exits. If that step fails, `.tsk-update-error.log` beside `tsk.exe` says why and how to retry.

The board shows a notice when a newer release is out. [Update check settings](/docs/storage/#update-check).

## Uninstall

| Installed with | Remove |
| --- | --- |
| Installer | Delete `tsk` from its install directory, `~/.local/bin` by default. On Windows, delete `%LOCALAPPDATA%\Programs\tsk\bin\tsk.exe` |
| Homebrew | `brew uninstall tsk` |

Your [task data](/docs/storage/#location) stays in place.

- **Herdr:** run `herdr plugin unlink herdr-tsk`, remove the two shortcut bindings from Herdr's config, and reload it.
- **PATH on macOS and Linux:** remove the `# tsk PATH` comment and the `case` line under it from the startup files the installer named.
- **PATH on Windows:** remove the tsk install directory from your user `Path` variable.

## Installer reference

### Options

| Option | Installer | Effect |
| --- | --- | --- |
| `TSK_VERSION=vX.Y.Z` | Both | Install that release instead of the latest stable one |
| `TSK_INSTALL_DIR=<dir>` | Both | Install in this absolute directory. The setup asks are skipped |
| `--help` | `install.sh` | Show help without downloading |
| `-NoPathUpdate` | `install.ps1` | Leave your user PATH unchanged |
| `-Help` | `install.ps1` | Show help without network access |

Set the variables in the environment, for example `curl -fsSL https://gettsk.sh/install.sh | TSK_INSTALL_DIR="$HOME/bin" sh`. To pass a PowerShell switch, download the script first, then run `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -NoPathUpdate`.

- `TSK_INSTALL_DIR` must be absolute and must not contain a newline or a PATH separator (`:` on macOS and Linux, `;` on Windows).
- `install.sh` needs `curl`, `tar`, and `sha256sum` or `shasum`, plus standard tools such as `sed` and `awk`.

### PATH

When the install directory is not on PATH, the installer adds it:

| Shell | Where |
| --- | --- |
| Zsh | `$ZDOTDIR/.zshrc`, or `~/.zshrc` |
| Bash | `~/.bashrc`, and the first of `~/.bash_profile`, `~/.bash_login`, or `~/.profile` that exists (`~/.profile` if none does) |
| Windows | Your user PATH |

- Other shells: the installer prints the directory to add yourself.
- It never sources your startup files. It skips a symlinked or unwritable one and says what to add by hand.
- A PATH change that fails never undoes the install.

### Prompts and closing message

The installer asks only in an interactive terminal, and only when it installs in the default directory. `curl | sh` can still ask: it reads your answer from the terminal.

| Prompt | When | Yes runs |
| --- | --- | --- |
| `Herdr detected. Set up the Herdr plugin now? [y/N]` | Herdr is on PATH | `tsk setup herdr` |
| `Agents detected: claude, pi. Install the tsk skill for them? [y/N]` | Coding agents are found | `tsk setup agents --yes` |

With `CI` set, or without a terminal, it skips both prompts so it never hangs. A setup that fails never undoes the install.

It ends with one closing message, with a row for each setup step still to run:

```text
Done. Run tsk in a project directory to open the board.

    Herdr plugin:  tsk setup herdr
    Agent skills:  tsk setup
```

- After Herdr setup ran, the first line ends `, or press prefix+t in Herdr.` instead.
- `Herdr plugin:` appears when Herdr is installed but its setup was declined, skipped, or failed.
- `Agent skills:` appears when agents were found but the skill prompt was declined or skipped.
- With `TSK_INSTALL_DIR`, the installer never runs the new binary. It prints `Custom install directory: setup was not run. When you are ready:`, then the setup rows with the binary's full path.

## Herdr setup details

`tsk setup herdr` registers the plugin with your installed `tsk`. No source checkout is needed.

- **Version:** Herdr 0.9 or newer, including Herdr's `0.9.0-preview.*` Windows builds. An older Herdr is refused before anything is written: `herdr 0.8.2 found; tsk needs 0.9.0 or newer. Update Herdr, then run tsk setup herdr again`.
- **Config file:** `HERDR_CONFIG_PATH`, else `$XDG_CONFIG_HOME/herdr/config.toml`, else `~/.config/herdr/config.toml`. On Windows: `HERDR_CONFIG_PATH`, else `%APPDATA%\herdr\config.toml`.
- **Backup:** before changing the config, setup saves a copy beside it, `config.toml.tsk-backup-<YYYYMMDD-HHMMSS>` in UTC. A second setup in the same second adds `-1`, `-2`, and so on.
- **Symlinks:** a symlinked config file or `herdr` directory (dotfiles) is followed. Setup edits the real file, keeps the link, and saves the copy beside the real file. A dangling or looping link is refused before any change.
- **Shortcut conflicts:** setup asks before replacing a binding. Declining keeps yours, and the output says so. Without a terminal, a conflict stops setup before it writes anything.
- **Your own keys:** a plugin command you already bound to another key keeps it. Setup does not add the default shortcut beside it.
- **Reruns:** running setup again updates the registration without duplicating bindings.
- **Binary path:** setup records the `tsk` you ran. Run the one on your PATH, not a versioned Homebrew Cellar path, so upgrades keep working.
- **Windows:** setup writes PowerShell launchers.

Installing a new tsk does not refresh the registration by itself. `tsk update` refreshes it, and so does running `tsk setup herdr` again.

## Manual archives

Download your platform's archive and `SHA256SUMS` from the same [release](https://github.com/smarzban/tsk/releases).

| Platform | Archive target |
| --- | --- |
| macOS Apple silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux ARM64 | `aarch64-unknown-linux-musl` |
| Linux x86-64 | `x86_64-unknown-linux-musl` |
| Windows 10/11 ARM64 | `aarch64-pc-windows-msvc` |
| Windows 10/11 x86-64 | `x86_64-pc-windows-msvc` |

Archives are named `tsk-vX.Y.Z-<target>.tar.gz`. Windows archives are `.zip` files holding `tsk.exe`.

1. Hash the archive: `shasum -a 256` on macOS, `sha256sum` on Linux, or `Get-FileHash -Algorithm SHA256` on Windows.
2. Compare the full digest with the archive's line in `SHA256SUMS`.
3. Extract the executable into a directory on your PATH.

## Source build

Requires Rust 1.96.0:

```sh
git clone https://github.com/smarzban/tsk.git
cd tsk
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

To develop the Herdr plugin from a checkout, run `herdr plugin link "$PWD"` (Herdr 0.9 or newer). Rebuild after pulling changes.

[Contributing](https://github.com/smarzban/tsk/blob/main/CONTRIBUTING.md) · [Storage and configuration](/docs/storage/)
