---
title: Capture
description: Add a task from the board, a Herdr popup, or the terminal.
---

Add a task without leaving your work: quick-add on the board, the Herdr popup, or `tsk capture`. New tasks land in the **inbox** with status `open`. Every key: [Quick-add keys](/docs/keys/#quick-add).

## Quick-add

1. Press `+` or click **+ add**. The board stays visible.
2. Type a title, with [title tokens](#title-tokens) if you like.
3. Press `Enter` to save, or `shift+Enter` to save and add another.

- The saved task flashes and is selected in the inbox.
- Invalid input keeps the line open and says why.
- Clicking a task while the line is open discards the draft and selects that task.

## Details

Press `Tab` on the line to open the full draft in Notes. It holds the task page's fields in the same [order](/docs/task-page/#edit): notes, steps, assignee, base, after, thread, and scope.

- `shift+Enter` saves.
- `Esc` returns to the line, and `Tab` brings your details back.

## Destination

| Where you add | Default destination |
| --- | --- |
| Project board | That project |
| Desk or Projects view | The board's launch repository inside Git; desk outside Git |
| Herdr popup | The focused pane's repository inside Git; desk outside Git |

Title tokens or the draft's Scope field change the destination. Archived projects cannot receive new tasks. Launching inside an archived project: [Done drawer and archive](/docs/board/#done-drawer-and-archive).

## Title tokens

Set a project, thread, assignee, base branch, or the tasks to run after while you type the title:

```text
Fix login timeout !p atlas !t auth !a reviewer !b main
Ship the release notes !w T202 !w T205
Buy coffee !p !a
```

| Token | Sets |
| --- | --- |
| `!p` | Desk |
| `!p name` | The existing project with that basename, ignoring case |
| `!p /path` | An existing absolute directory, creating a project there if needed |
| `!t` | No thread |
| `!t name` | That thread |
| `!a` | Unassigned |
| `!a name` | That agent profile, lowercased; it must exist |
| `!b branch` | The dispatch base: an existing local or remote branch in the task's repository |
| `!w T202` | Run after T202 ("wait for"). Repeat for several. `!w 202` works too |

- Each token takes one word. Put a bare `!p`, `!t`, or `!a` at the end, or before another token or a `#word`.
- Tokens leave the saved title, and the remaining words join with single spaces. A title is required.
- `!p name` must match one project the board knows: one with tasks, an archived one, or the launch repository. Otherwise the draft stays open and says so, such as `project atlss is not on the board`.
- `!p` creates a project only from an existing absolute directory, `/…` or `~/…`. A relative path or a file keeps the draft open.
- `!b` is checked against the branches on disk, without fetching. An unknown branch refuses on save and keeps the draft open. [Base branch](/docs/dispatch/#base-branch).
- `!w` takes any task number on the board, in any project. A missing or done task refuses and keeps the draft open. [After](/docs/board/#after).

## Thread names

Thread names are lowercased and must:

- Start with an ASCII letter or digit.
- Contain only ASCII letters, digits, `-`, or `.`.
- Be at most 32 characters.

An invalid name keeps the draft open and says which rule failed.

## Herdr quick capture

In Herdr, press `prefix+a` after [setup](/docs/install/#add-to-herdr). A popup opens with the title focused, prefilled with any text selected in the pane you came from.

- `shift+Enter` saves and closes the popup.
- `Esc` discards it, after first closing an open step editor or picker.
- A failed save keeps the draft open to retry or cancel.

To open the popup from a script:

```sh
herdr plugin action invoke quick-capture --plugin herdr-tsk
```

## In the terminal

`tsk capture` opens the same capture flow in your terminal, as does `TSK_MODE=capture tsk`. It exits after a save or a discard.

## From an agent or script

```sh
tsk add -t "Fix login timeout" --thread auth
```

The task starts `open`. [`tsk add`](/docs/cli/#add) lists every option.
