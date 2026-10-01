---
title: Capture
description: Add a task without leaving your work.
---

Press `+` or click **+ add**. Type a title, then press `Enter`. On the website demo, the capture footer actions are clickable. New captures land in the **inbox** with status `open`.

## Quick-add

| Key | Action |
| --- | --- |
| `Enter` | Save and close |
| `Shift+Enter` | Save and add another |
| `Tab` | Open details |
| `Esc` | Cancel |

The board stays visible. A saved task flashes and becomes selected in the inbox. Invalid input stays open with an explanation.

Clicking a task while the quick-add line is open discards the draft and selects that task.

## Details

Press `Tab` from quick-add to add notes, steps, an assignee, a thread, or a scope. The draft uses the full pane and opens in Notes. Its forward field ring is Title → Notes → steps → **+ step** → Assignee → Thread → Scope → Title; `Shift+Tab` reverses it. The footer uses that same left-to-right order. In a project preview, its title stays in the right-column header while Notes is active.

- `Shift+Enter` saves.
- `Esc` returns to the quick-add line.
- `Tab` from the line restores your draft details.

Scroll to reach steps below long notes. Typing brings the notes cursor back into view.

## Destination

| Where you add | Default destination |
| --- | --- |
| Project board | That project |
| Desk or Projects | The board's launch repository inside Git; desk outside Git |
| Herdr quick-capture popup | The focused pane's repository inside Git; desk outside Git |

Use title tokens or the draft's scope control to change the destination. Archived projects cannot receive new tasks.

Keeping an archived launch project archived changes the session's default to desk.

## Title tokens

Add a project, thread, or assignee while typing the title:

```text
Fix login timeout !p atlas !t auth !a reviewer
Buy coffee !p !a
```

| Token | Destination, thread, or assignee |
| --- | --- |
| `!p` | Desk |
| `!p name` | Existing project uniquely matching that basename, ignoring case |
| `!p /path` | Existing absolute directory, creating a project there if needed |
| `!t` | No thread |
| `!t name` | Named thread |
| `!a` | Unassigned |
| `!a name` | Exact configured agent profile name, normalized to lowercase |

Each token takes one whitespace-separated argument. Put bare `!p`, `!t`, or `!a` at the end, or before another token. A following `#word` also leaves the token bare.

Tokens are removed from the saved title. Remaining words are joined with single spaces. A title is required.

A bare project name must match exactly one project the board already knows: a project with tasks, an archived project, or the repository you launched from. A missing or ambiguous name leaves the draft open and names the problem, for example `project atlss is not on the board`.

Use an absolute existing directory (`/…` or `~/…`) to create a new project destination. Relative paths and paths that are not directories leave the draft open; the destination row keeps showing the last valid destination.

## Thread names

Thread names are lowercased and must:

- Start with an ASCII letter or digit.
- Contain only ASCII letters, digits, `-`, or `.`.
- Be at most 32 characters.

Invalid names leave the draft open with the rule that failed.

## Quick capture

In Herdr, press **prefix+a** after [setup](/docs/install/#add-to-herdr). A popup opens with Title focused.

| Action | Result |
| --- | --- |
| Type or edit the title | Name the task |
| Add notes, a thread, an assignee, or steps | Include details before saving |
| `Shift+Enter` | Save and close the popup |
| `Esc` | Discard and close |

If a step editor or scope picker is open, `Esc` closes that first. During save recovery, it cancels the pending save.

Selected text from the invoking pane prefills the title. An archived launch project falls back to desk. A failed save keeps the draft open for retry or cancel.

You can also invoke the popup explicitly:

```sh
herdr plugin action invoke quick-capture --plugin herdr-tsk
```

For the same capture flow in your terminal, run `tsk capture`. `TSK_MODE=capture tsk` is equivalent. Captured tasks start `open`.

## From an agent or script

```sh
tsk add -t "Fix login timeout" --thread auth
# the new task starts open; use tsk status T<n> ready when you pick it
```

[CLI options](/docs/cli/#add) · [Editing tasks](/docs/task-page/)
