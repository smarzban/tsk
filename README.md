# tsk

[![CI](https://github.com/smarzban/tsk/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/smarzban/tsk/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platforms: Linux • macOS • Windows](https://img.shields.io/badge/platforms-Linux%20%E2%80%A2%20macOS%20%E2%80%A2%20Windows-blue.svg)](https://gettsk.sh/docs/install/)

A terminal task board for you and your agents: one shared queue, a TUI for you, a CLI for them.

Keep the next task, the work in progress, and the things waiting on you in view.
Press `+` to capture a thought, add notes and steps when it needs a plan, and give
your agent the task number when you are ready. New tasks start in the open inbox.

### Beside your agent

![The tsk board beside an agent working on the selected Redis-to-Postgres migration task](docs/images/board-in-herdr.png)

In Herdr, `prefix+t` opens or focuses your workspace's board, even across tabs.
Other workspaces keep their own view of the same tasks. `prefix+a` opens quick capture.
Click a task's `T` number to copy it, then paste it into your agent conversation.

### Room to think

![A wide tsk board with the selected task's notes and steps open alongside it](docs/images/wide-task-page.png)

In a wide pane, click a task or press `→` or `l` to open it beside the board; `←` or `h`
brings you back. `Enter` opens the task full screen and `Esc` returns. Without Herdr,
run `tsk` in any terminal.

### From task to implementation

Define your agents once in `config.toml`, press `@` to assign a task, and press
`ctrl+s` to start it. Starting an assigned task dispatches it: tsk creates a Git
worktree and a Herdr workspace for the task and launches the agent there. When the
pull request is merged, `ctrl+d` completes the task and removes the worktree and
branch, never touching uncommitted or unmerged work. Dispatch runs in Herdr; on
Windows it is a preview.

## Quickstart

### Install

macOS and Linux (ARM64 or x86-64):

```sh
curl -fsSL https://gettsk.sh/install.sh | sh
```

Or with Homebrew:

```sh
brew install smarzban/tap/tsk
```

Windows 10/11 (ARM64 or x86-64), in a PowerShell window:

```powershell
irm https://www.gettsk.sh/install.ps1 | iex
```

The installers add tsk to your PATH and offer to set up Herdr and your coding agents
when they find them. If prompted, reopen your terminal. Options, upgrades, and
uninstall are in the [install guide](https://gettsk.sh/docs/install/).

### Add to Herdr

[Install Herdr](https://herdr.dev/docs/install/) first, then:

```sh
tsk setup herdr
herdr server reload-config
```

### Give your agent the skill

Two skills: `tsk-cli` teaches your agent to read the board, add tasks, and update
their status and steps. `tsk-coordinator` lets it brief builder agents, review their
pull requests, and report back on the board:

```sh
tsk setup
```

In a terminal, `tsk setup` finds your agents and asks once. To install for one agent,
name it: `tsk setup pi`, or `omp`, `claude`, `cursor`, `grok`, `codex`, or `opencode`.
[Skill directories and options](https://gettsk.sh/docs/cli/#setup).

### Open the board

Run `tsk`, or press `prefix+t` in Herdr. No project setup needed: inside a Git
repository, tsk opens that project. Outside Git, it opens your desk, with the current
directory one tab away.

### Add a task

```sh
tsk add -t "your task title"
```

Or press `+` on the board, `prefix+a` in Herdr, or ask your agent: “Add a task to fix
the login timeout, with steps to reproduce the bug.”

## Usage

Mouse or keyboard. Click tabs to switch views, double-click a task to open it, and
click the actions along the bottom. `shift+M` marks several tasks so one action
covers them all.

| Key | Action |
| --- | --- |
| `↑` / `↓` or `j` / `k` | Move between tasks |
| `Enter` | Open the selected task |
| `→` / `←` or `l` / `h` | Narrow pane: peek at a task, close the peek. Wide pane: move between board and task |
| `+` | Add a task |
| `ctrl+s` | Start an open, ready, or blocked task |
| `ctrl+n` | Pick a task: move it to ready |
| `ctrl+o` | Send a task back to the open inbox |
| `ctrl+r` | Send a task to review with what was done and what to check, or back to ready |
| `ctrl+d` | Mark the selected task done |
| `shift+M` | Enter or leave multi-select |
| `shift+↑` / `shift+↓` | In multi-select, mark the current task, then move |
| `Space` or click | In multi-select, mark or unmark a task |
| `p` | Switch projects |
| `d` | Show or hide done tasks |
| `?` | Show every shortcut |

[Full keymap](https://gettsk.sh/docs/keys/) ·
[Board guide](https://gettsk.sh/docs/board/) ·
[CLI reference](https://gettsk.sh/docs/cli/)

## Docs

The [user guide](https://gettsk.sh/docs/) covers installation, the board, agents,
and the full CLI reference.

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md) for development and pull-request guidance.
Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
