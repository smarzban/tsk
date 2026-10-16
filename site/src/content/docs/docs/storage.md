---
title: Storage
description: Task data, backups and restore, deleted tasks, and update checks.
---

The board, CLI, and Herdr plugin share one store. The current store format is v13.

## Location

| Setting | Default | Purpose |
| --- | --- | --- |
| `TSK_STATE_DIR` | Platform default | Task data, configuration, backups, trash, and release-check cache. The default is `~/.tsk` on macOS/Linux, `%LOCALAPPDATA%\tsk` on Windows, or `%USERPROFILE%\.tsk` when `LOCALAPPDATA` is unavailable. Without a usable platform home, tsk refuses to run rather than pick a directory |
| `--state-dir <dir>` | State directory | Override storage for a data command |

Use a local disk. NFS and synced folders such as Dropbox, iCloud Drive, or OneDrive are unsupported for the state directory; back up to them instead ([backup targets](#backup-targets)). Directory roots must be real directories, not symlinks or Windows reparse points such as junctions.

Herdr's plugin-specific state/config directories do not override these locations. Removing tsk leaves its task data intact.

## Configuration

Settings are read from `config.toml` in the state directory, beside `tsk.json`. The first full board open creates a starter file when it is missing: it explains profiles and placeholders, quotes the default prompt, and holds commented examples (Claude Code, Codex, Pi, Grok, and any other terminal agent) that define no profiles until you uncomment one, plus a commented [backup target](#backup-targets). Quick capture, CLI commands, and setup do not create it, and tsk never replaces an existing file, even an empty one. Top-level keys and tables tsk does not recognize are ignored, so a file written for a newer tsk still loads; inside an `[agent.<name>]`, `[merge]`, `[board]`, or `[[backup]]` table an unknown key is an error. Each table is read on its own, so one broken table never blocks the others.

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
5. Set the task to review with what you did and what to check: `tsk status {number} review --done "…" --check "…"`, one `--check` per thing to verify, adding `--next` for what comes after. When you need a human, block it with your question: `tsk status {number} blocked --why "…"`, adding `--needs` and one `--option` per choice, then stop.
6. On a relaunch, first read whichever record closed last (`closed_at`) in `tsk list {number} --json`: the `replies` of the last `past_blocks` entry, or the `feedback` and failed `checks` of the last `past_reviews` entry when it was `sent_back`.
7. A message `[tsk T{number} sent back] …` is review feedback, with any failed checks: address it, then set review again.
```

A profile with its own `prompt` replaces the default entirely. The rendered prompt is always appended to the command as its last argument. `env` is an optional table of string values passed to the launched command unchanged. tsk also sets `TSK_AGENT` to the profile name, so the agent's blocks, reviews, and replies carry its name.

A malformed `config.toml` does not block the board or CLI work that does not assign a task. The board opens without profiles and shows the error on its status row. `tsk add` and `tsk edit` read the file only when an assignee is supplied; a configuration error then exits 2 without saving.

### Merge method

`m` on the [cleanup card](/docs/board/#merge-the-pull-request) and `--merge` on [`tsk status … done`](/docs/cli/#status) and [`tsk clean`](/docs/cli/#clean) merge a task's pull request with the GitHub CLI. The `[merge]` table picks how:

```toml
[merge]
method = "squash"   # "merge" (the default), "squash", or "rebase"
```

Without the table, tsk merges with a merge commit. An unknown `method` or key is a configuration error that refuses only the merge (the card says why; the CLI refuses with `merge-config`); it never blocks the board or other commands. The starter file holds this block commented out.

The command and prompt templates support `{number}`, `{title}`, `{notes}`, `{steps}`, `{worktree}`, `{branch}`, and `{base}`. `{branch}` is the dispatched task branch; `{base}` is the short base branch name, for example `dispatch` for `origin/dispatch`, so a prompt can say "open the PR into {base}". tsk replaces only these placeholders. It quotes every argument and renders one command line as `$SHELL -lc '…'`; it never chains commands. On Windows it launches the same argv and `env` through PowerShell instead ([preview](/docs/board/#dispatch-on-windows-preview)). Write Windows paths as TOML literal strings so backslashes stay as typed, for example `command = ['C:\Users\you\.local\bin\claude.exe']`. Profiles are read-only in tsk, edit the file to change them.

### Board defaults

The `[board]` table sets the group and sort a fresh board starts from ([group and sort](/docs/board/#group-and-sort)). `group` and `sort` set the desk; `project_group` and `project_sort` set every project board, including the wide Projects preview.

```toml
[board]
group = "assignee"        # none, project, thread, or assignee
sort = "recent"           # recent, project, thread, title, or assignee
project_group = "thread"  # none, thread, or assignee
project_sort = "title"    # recent, thread, title, or assignee
```

A missing key keeps its default (`none`, `recent`). `g` and `s` still change the layout for the session; nothing is written back. An unknown key or value is a configuration error: the board opens at the default layout and shows the error on its status row (a profile error, if any, shows instead). Profiles and `[merge]` are read separately, so one broken table never blocks the others. The starter file holds this block commented out.

## Backups

| File | Contains |
| --- | --- |
| `tsk.json` | Current tasks and archived-project records |
| `tsk.json.1` | Previous valid task document |
| `tsk.json.v<N>` | Backup made when migrating an older store format, such as `tsk.json.v12` for the v12 → v13 migration |
| `config.toml` | Settings, including agent launch profiles, seeded with commented examples on the first full board open |
| `delivery.json` | Which starter tasks this install has received or dismissed, and the newest release note it has seen |
| `tsk.json.pre-restore-<time>` | The store a [`tsk restore`](#restore) replaced, named by UTC time; the newest five are kept |
| `backup-status.json` | Each backup target's last success, last attempt, and last error |
| `machine-id` | This state directory's machine id, generated once; names its backups |
| `backup-repos/` | Local clones of git backup targets |
| `launchers\`, `cleanups\` | Windows only: dispatch launchers and cleanup progress |

An older binary refuses a newer or unversioned store instead of rewriting it. Use a compatible tsk version to open it. On first save, older stores migrate to v13. v6 added task assignees, dispatch base branches, and dispatch records; v7 added blocks (why, needs, options, what the block waits on, and replies) and the block undo step; v8 added the undo step for a start that dispatched; v9 added review rounds (done, checks, next, who the review is on, feedback, and how each round closed); v10 added who made each change (`by`) and what it changed (`detail`) to every new task event; v11 added `after`, the task numbers a task runs after, the prerequisite that started a task on its event, and the undo step that puts an `after` list back; v12 added `sent` to a reply that `tsk reply --send` delivered on its own, so an unblock or send-back does not send it again; v13 adds the status a done's undo goes back to, and undo steps for status changes, step changes, and archiving. A done from an older store undoes to open, as it did before. A reply from an older store reads as not sent. A task from an older store runs after nothing. Events from an older store keep their kind and time and show on the [paper trail](/docs/task-page/#paper-trail) without who or what. The original document is saved as `tsk.json.v<N>`, for example `tsk.json.v5` from v0.11. A blocked task from an older store keeps its status with no block and reads as blocked on you; a task in review keeps its status with no round and reads as in review on you. Earlier stores still run through each migration in order, including v5 batch undo and the v3 to v4 move from ready to open.

Archived tasks stay in the task document with their existing status. [Archive and restore](/docs/board/#archive).

## Backup targets

`tsk.json.1` and `tsk.json.v<N>` sit on the same disk as the store. A backup target puts copies somewhere else: a folder (an iCloud Drive, Dropbox, OneDrive, or Google Drive folder their desktop apps upload, or a USB disk) or a git repository (for example a private GitHub repository). Each backup is `tsk.json` byte for byte, so it carries its store format, plus a copy of `config.toml` beside it.

```sh
tsk backup add ~/Dropbox/tsk                      # a folder, backed up on every save
tsk backup add git@github.com:me/tsk-backup.git   # a git repository, backed up daily
tsk backup list                                   # targets, last backup, last error
tsk backup                                        # back up to every target now
tsk backup remove 2                               # by number in list, path, or URL
```

On Windows, add a OneDrive folder the same way: `tsk backup add C:\Users\me\OneDrive\tsk`.

`tsk backup add` tests the target straight away: it runs the first backup to it in the foreground and prints where the copy went. When there is nothing to back up yet (a new machine), a git target is still cloned, so a wrong URL or missing access shows here. If the first backup fails, the target stays in `config.toml`, the command exits 1, and it says what to fix; rerun `tsk backup` once it is fixed.

A folder target is only as safe as the app that syncs it: tsk writes the copy into the folder and cannot see whether iCloud, Dropbox, OneDrive, or Google Drive uploaded it, so check the app shows the file. A git target is the one tsk can confirm left the machine: a backup succeeds only once the push does.

The verbs write `[[backup]]` entries to `config.toml`; you can also edit them by hand:

```toml
[[backup]]
kind = "folder"            # "folder" or "git"
path = "/Users/me/Dropbox/tsk"
every = "save"             # "save", "open", "daily", or "twice-daily"
keep_last = 10             # folder only
keep_days = 30             # folder only

[[backup]]
kind = "folder"
path = 'C:\Users\me\OneDrive\tsk'   # Windows: a literal string keeps the backslashes

[[backup]]
kind = "git"
url = "git@github.com:me/tsk-backup.git"
every = "daily"
```

| Key | Default | Meaning |
| --- | --- | --- |
| `kind` | required | `folder` or `git` |
| `path` / `url` | required | The folder (it must exist) or the repository tsk pushes to |
| `every` | `save` (folder), `daily` (git) | `save`: after every save. `open`: on every full board open. `daily` / `twice-daily`: on the first save or board open 24 or 12 hours after the target's last backup (there is no background service; a failed attempt is retried ten minutes later) |
| `keep_last` | 10 | Folder only: the newest copies kept |
| `keep_days` | 30 | Folder only: also the first copy of each of the last this-many UTC days, today included, which is the store as it was before that day's changes |

`tsk backup add` treats a location as git when it is a URL (`https://…`, `ssh://…`, `file://…`), `user@host:path`, ends in `.git`, or is a bare repository; `--git` or `--folder` says so when it cannot tell. Adding a target that exists only updates its `every` (and tests it again).

A folder target holds `tsk-backups/<machine>/tsk-<UTC time>.json` with `config-<UTC time>.toml` beside it (a second backup within one second adds `-1`, `-2`, …). Each file is written privately (owner-only on macOS and Linux) beside its final name and renamed, so a sync app never uploads half a file, and copies past `keep_last` and `keep_days` are deleted with their config copy. A git target holds `machines/<machine>/tsk.json` and `machines/<machine>/config.toml` with one commit per backup, so its history is every version. tsk runs your own `git` with your credentials and stores no tokens, checks files out with links off, and refuses to write through a link in its clone; it never prompts, so use a key or credential helper that works without one. `<machine>` is the host name plus the first eight characters of `machine-id`. Several machines can share one target.

Backups never block or fail a save. When a target is due, the save or board open starts a detached `tsk backup run` and returns; a store and `config.toml` unchanged since a target's last backup are not copied again. A store that does not parse is never copied: the target records `store does not parse, not backed up` instead. A failure is recorded in `backup-status.json`: `tsk backup list` shows it, and after each failed attempt the next full board open shows it on the status row, until the target succeeds. `tsk backup` runs every target in the foreground and exits 1 if any failed.

Once the store holds 10 tasks and no target is configured, one full board open says `No backup yet: tsk backup add <folder|git-url>` on the status row. It is shown once per state directory, never again.

Backups are not encrypted and contain your task notes: use a private folder or a private repository. Trash (`trash.jsonl`) is not backed up. tsk never prunes a `<machine>` directory a machine stopped writing to (after a host name change, say) or the clones under `backup-repos/`; delete them by hand when you no longer want them.

### Restore

`tsk restore` lists the versions every target holds, newest first, with an id, time, machine, task count, and target (a git target shows the newest 20 per machine; `--all` shows every one). In a terminal it asks which to restore, by number or id; `tsk restore <id>` restores one without asking. A task number such as `T12` is not a version: restore suggests `tsk trash restore T12`.

```sh
tsk restore
tsk restore 3f9a1c2e
```

Restore copies the current `tsk.json` to `tsk.json.pre-restore-<UTC time>`, then replaces it with the chosen version under the store lock. It keeps the newest five `tsk.json.pre-restore-*` copies and deletes older ones. A version in an older store format is written as is and migrates on the next open; one newer than this tsk is refused with a request to update tsk, as are files that are not a store.

Restore never changes `config.toml`. It prints where the `config.toml` backed up with that version is (a file in the folder, or the `git show` command that prints it), so you can copy agent profiles and settings over by hand.

Restore refuses while a board or quick capture is open on the store: an open board merges what is on disk into what it holds and would write back, on its next save, every task the restored version does not have. Quit them, then restore.

On a new machine, add the same target and restore from it:

```sh
tsk backup add git@github.com:me/tsk-backup.git
tsk restore
```

## Starter guides and release notes

The four starter tasks (`N1`… on your desk) are seeded once per state directory on the first board open. Mark one done, archive it, or delete it and it never returns. Deleting `delivery.json` seeds any starter catalog id the task document no longer holds.

After an upgrade, the first board open adds one `What's new in tsk` task to your desk (in `review`, under NEEDS YOU) when the new version bundles release notes you have not seen. That includes upgrading from a build that only had starter tasks. Every missed release lands in that one task, newest first, with its changelog link in the notes. The notes ship inside the binary; a blank first install records them as seen and shows none. Clear it like any starter task.

## Deleted tasks

Deleted tasks move to `trash.jsonl` once undo can no longer restore them, or after seven days. A delete already drops the task from every other task's `after`; any link left over is dropped when it moves to trash. They are purged 30 days after deletion.

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
