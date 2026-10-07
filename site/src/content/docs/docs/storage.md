---
title: Storage
description: Task data, backups, deleted tasks, and update checks.
---

The board, CLI, and Herdr plugin share one store. The current store format is v6.

## Location

| Setting | Default | Purpose |
| --- | --- | --- |
| `TSK_STATE_DIR` | Platform default | Task data, configuration, backups, trash, and release-check cache. The default is `~/.tsk` on macOS/Linux, `%LOCALAPPDATA%\tsk` on Windows, or `%USERPROFILE%\.tsk` when `LOCALAPPDATA` is unavailable. Without a usable platform home, tsk refuses to run rather than pick a directory |
| `--state-dir <dir>` | State directory | Override storage for a data command |

Use a local disk. NFS and synced folders such as Dropbox or iCloud Drive are unsupported. Directory roots must be real directories, not symlinks or Windows reparse points such as junctions.

Herdr's plugin-specific state/config directories do not override these locations. Removing tsk leaves its task data intact.

## Configuration

Settings are read from `config.toml` in the state directory, beside `tsk.json`. The first full board open creates a starter file when it is missing: it explains profiles and placeholders, quotes the default prompt, and holds commented examples (Claude Code, Codex, Pi, Grok, and any other terminal agent) that define no profiles until you uncomment one. Quick capture, CLI commands, and setup do not create it, and tsk never replaces an existing file, even an empty one. Top-level keys and tables tsk does not recognize are ignored, so a file written for a newer tsk still loads; inside an `[agent.<name>]` table an unknown key is an error.

### Agent profiles

Each `[agent.<name>]` table is an agent launch profile. Each profile name must already be lowercase and use the same shape as a thread name: start with a letter or number, then use only letters, numbers, hyphens, and dots, up to 32 characters. Quote a name that contains dots, for example `[agent."review.strict"]`.

```toml
[agent.implementer]
command = ["pi"]
prompt = "Work on T{number}: {title}\n\n{notes}\n\n{steps}"

[agent.implementer.env]
PI_PROVIDER = "anthropic"
```

`command` is a required, non-empty argv template. `prompt` is optional; without it, tsk supplies this default:

```text
You were dispatched to T{number} ({title}) in worktree {worktree} on branch {branch}, based on {base}.

1. Run `tsk guide`, then `tsk list {number} --json`. The task notes are your brief.
2. Read the repo's agent instructions (AGENTS.md or CLAUDE.md) if present.
3. Work only on {branch}. Run the project's checks before saying you are done.
4. Push and open a pull request into {base}. Never merge it.
5. Set the task to review with one line on what to look at, or to blocked with your question when you need a human.
```

A profile with its own `prompt` replaces the default entirely. The rendered prompt is always appended to the command as its last argument. `env` is an optional table of string values passed to the launched command unchanged.

A malformed `config.toml` does not block the board or CLI work that does not assign a task. The board opens without profiles and shows the error on its status row. `tsk add` and `tsk edit` read the file only when an assignee is supplied; a configuration error then exits 2 without saving.

The command and prompt templates support `{number}`, `{title}`, `{notes}`, `{steps}`, `{worktree}`, `{branch}`, and `{base}`. `{branch}` is the dispatched task branch; `{base}` is the short base branch name, for example `dispatch` for `origin/dispatch`, so a prompt can say "open the PR into {base}". tsk replaces only these placeholders. It quotes every argument and renders one command line as `$SHELL -lc '…'`; it never chains commands. On Windows it writes the same argv and `env` into a one-time PowerShell launcher under `launchers\` in this directory instead ([preview](/docs/board/#dispatch-on-windows-preview)). Write Windows paths as TOML literal strings so backslashes stay as typed, for example `command = ['C:\Users\you\.local\bin\claude.exe']`. Profiles are read-only in tsk, edit the file to change them.

## Backups

| File | Contains |
| --- | --- |
| `tsk.json` | Current tasks and archived-project records |
| `tsk.json.1` | Previous valid task document |
| `tsk.json.v<N>` | Backup made when migrating an older store format, such as `tsk.json.v5` for the v5 → v6 migration |
| `config.toml` | Settings, including agent launch profiles, seeded with commented examples on the first full board open |
| `delivery.json` | Which starter tasks this install has received or dismissed, and the newest release note it has seen |

An older binary refuses a newer or unversioned store instead of rewriting it. Use a compatible tsk version to open it. On first save, v5 stores migrate to v6, which adds task assignees, dispatch base branches, and dispatch records; the original document is saved as `tsk.json.v5`. Earlier stores still run through each migration in order, including v5 batch undo and the v3 to v4 move from ready to open.

Archived tasks stay in the task document with their existing status. [Archive and restore](/docs/board/#archive).

## Starter guides and release notes

The four starter tasks (`N1`… on your desk) are seeded once per state directory on the first board open. Mark one done, archive it, or delete it and it never returns. Deleting `delivery.json` seeds any starter catalog id the task document no longer holds.

After an upgrade, the first board open adds one `What's new in tsk` task to your desk (in `review`, under NEEDS YOU) when the new version bundles release notes you have not seen. That includes upgrading from a build that only had starter tasks. Every missed release lands in that one task, newest first, with its changelog link in the notes. The notes ship inside the binary; a blank first install records them as seen and shows none. Clear it like any starter task.

## Deleted tasks

Deleted tasks move to `trash.jsonl` once undo can no longer restore them, or after seven days. They are purged 30 days after deletion.

```sh
tsk list --deleted --all
tsk trash restore T12
```

The list includes both recently deleted tasks still in the main store and tasks in trash. Restore reads trash; use board undo for a recent deletion that has not moved there yet.

## Fetch window

Dispatch, the branch picker, and cleanup fetch a base's remote before reading its branches. A remote tsk fetched successfully in the last 60 seconds is not fetched again, so opening the picker and then dispatching costs one round trip. Each fetch also refreshes the remote's default branch (`origin/HEAD`). A surface that needs a remote another is already fetching waits for that fetch and shares its result. Failed fetches are not remembered, and cleanup decides merged status from the refs on disk only inside this window: when its own fetch fails, it keeps the branch.

| File | Purpose |
| --- | --- |
| `fetch-stamps.json` | When CLI verbs and the board last fetched each repository's remote, so the window carries across `tsk` processes. Entries older than the window are dropped on the next write; deleting the file only costs one fetch |

## Update check

On launch, tsk checks for a newer release if its cached check is older than 24 hours. A newer version appears on the board's idle status row as `vX.Y.Z available, run tsk update`.

| Setting or file | Purpose |
| --- | --- |
| `TSK_NO_UPDATE_CHECK` | Set to disable the check and notice |
| `TSK_UPDATE_CURL=/absolute/path/to/curl` | macOS/Linux only: use a nonstandard, explicit curl path for the check and for `tsk update` (default `/usr/bin/curl`). Windows uses the binary's native HTTPS client |
| `update.json` | Cached release check in the state directory |

The check requests the latest release tag from GitHub over HTTPS only. It does not upload task data. Failures are silent.

[Upgrade tsk](/docs/install/#upgrade).
