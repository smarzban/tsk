# tsk

[![CI](https://github.com/smarzban/tsk/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/smarzban/tsk/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platforms: Linux • macOS • Windows](https://img.shields.io/badge/platforms-Linux%20%E2%80%A2%20macOS%20%E2%80%A2%20Windows-blue.svg)](https://gettsk.sh/docs/install/)

A terminal task board for you and your agents: one shared queue, a TUI for you, a CLI for them.

Keep the next task, the work in progress, and the things waiting on you in view.
Press `+` to capture a thought, add notes and steps when it needs a plan, and give
your agent the task number when you're ready to work on it. New tasks start in the open inbox; pick the next one when you're ready.

### the flow

![The tsk board beside an agent working on the selected Redis-to-Postgres migration task](docs/images/board-in-herdr.png)

In Herdr, **prefix+t** opens or focuses your workspace's board, even across tabs;
other workspaces keep their own views of the same tasks. **prefix+a** opens quick capture.
Click a task's `T` number to copy it, then paste it into your agent conversation.

### Room to think

![A wide tsk board with the selected task's notes and steps open alongside it](docs/images/wide-task-page.png)

In a wide pane, click a task or press `→` / `l` to open its details beside the board; `←` / `h` brings you back.
Press `Enter` for a full-screen task and `Esc` to return. For a standalone board,
run `tsk` in your terminal.

### Coming soon

**From task to implementation.** The next phase brings agent execution into the
board: assign a task to an agent, start implementation from tsk, and run the work
in Herdr worktrees.

## Quickstart

### Install

macOS and Linux (ARM64 or x86-64):

```sh
curl -fsSL https://gettsk.sh/install.sh | sh
```

Or install with Homebrew:

```sh
brew install smarzban/tap/tsk
```

Windows 10/11 on ARM64 or x86-64, in a PowerShell window:

```powershell
irm https://www.gettsk.sh/install.ps1 | iex
```

The installers add tsk to your PATH and, when Herdr or coding agents are detected,
offer to set them up. If prompted, reopen your terminal. Options, upgrades, and
uninstall are in the [install guide](https://gettsk.sh/docs/install/).

### Add to Herdr

[Install Herdr](https://herdr.dev/docs/install/) if you haven't already, then:

```sh
tsk setup herdr
herdr server reload-config
```

### Give your agent the skill

Install the tsk skill so your agent knows how to read the board, add tasks, and
update their status and steps:

```sh
tsk setup
```

On a TTY that detects agents once and asks to install. Or name one agent
(`pi`, `omp`, `claude`, `cursor`, `grok`, `codex`, or `opencode`) to install into its
user-level skills directory.
[Other skill directories and setup options](https://gettsk.sh/docs/cli/#setup).

### Open the board

Run `tsk`, or press **prefix+t** in Herdr.

**No project setup needed.** Inside Git, tsk opens the repository project. Outside
Git, it opens your desk and keeps the current directory ready on the project tab.

### Add a task

```sh
tsk add -t "your task title"
```

Or press **prefix+a** in Herdr, or ask your agent to add a task to the board.

For example: “Add a task to the board to fix the login timeout, with steps to reproduce
the bug.”

[Installation details and upgrades](https://gettsk.sh/docs/install/).

## Usage

Mouse or keyboard, your choice. Click tabs to switch views, double-click a task
to open it, and scroll through your board. The actions along the bottom are
clickable too, including adding a task and changing its status. `Shift+M` marks
several tasks so one action covers them all.

Prefer the keyboard? A few keys for everyday use:

| Key | Action |
| --- | --- |
| `↑` / `↓` or `j` / `k` | Move between tasks |
| `Shift+M` | Enter or leave multi-select |
| `Shift+↑` / `Shift+↓` | In multi-select, mark the current task, then move |
| `Space` or click | In multi-select, mark or unmark a task |
| `→` / `←` or `l` / `h` | Peek at a task and close the peek in narrow panes; move between board and task views in wide panes |
| `+` | Add a task |
| `Enter` | Open the selected task |
| `ctrl+s` | Start an open or ready task |
| `ctrl+n` | Pick a task, moving it to ready |
| `ctrl+o` | Send a task to the open inbox |
| `ctrl+r` | Send a task to review, or back to ready |
| `ctrl+d` | Mark the selected task done |
| `p` | Switch projects |
| `d` | Show or hide completed tasks |
| `?` | Show all shortcuts |

[Full keymap](https://gettsk.sh/docs/keys/) ·
[Board guide](https://gettsk.sh/docs/board/) ·
[CLI reference](https://gettsk.sh/docs/cli/)

## Docs

See the [user guide](https://gettsk.sh/docs/) for installation, task management,
agent setup, and the full CLI reference.

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md) for development and pull-request guidance.
Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
