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

Use a local disk. NFS and synced folders such as Dropbox, iCloud Drive, or OneDrive are unsupported for the state directory; use a [backup target](#backup-targets) instead. Directory roots must be real directories, not symlinks or Windows reparse points such as junctions.

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
| `backup-repos/` | Local clones of backup targets |
| `launchers\`, `cleanups\` | Windows only: dispatch launchers and cleanup progress |

An older binary refuses a newer or unversioned store instead of rewriting it. Use a compatible tsk version to open it. On first save, older stores migrate to v13. v6 added task assignees, dispatch base branches, and dispatch records; v7 added blocks (why, needs, options, what the block waits on, and replies) and the block undo step; v8 added the undo step for a start that dispatched; v9 added review rounds (done, checks, next, who the review is on, feedback, and how each round closed); v10 added who made each change (`by`) and what it changed (`detail`) to every new task event; v11 added `after`, the task numbers a task runs after, the prerequisite that started a task on its event, and the undo step that puts an `after` list back; v12 added `sent` to a reply that `tsk reply --send` delivered on its own, so an unblock or send-back does not send it again; v13 adds the status a done's undo goes back to, and undo steps for status changes, step changes, and archiving. A done from an older store undoes to open, as it did before. A reply from an older store reads as not sent. A task from an older store runs after nothing. Events from an older store keep their kind and time and show on the [paper trail](/docs/task-page/#paper-trail) without who or what. The original document is saved as `tsk.json.v<N>`, for example `tsk.json.v5` from v0.11. A blocked task from an older store keeps its status with no block and reads as blocked on you; a task in review keeps its status with no round and reads as in review on you. Earlier stores still run through each migration in order, including v5 batch undo and the v3 to v4 move from ready to open.

Archived tasks stay in the task document with their existing status. [Archive and restore](/docs/board/#archive).

## Backup targets

`tsk.json.1` and `tsk.json.v<N>` sit on the same disk as the store. A backup target puts copies in a git repository somewhere else, such as a private GitHub repository. Each backup is `tsk.json` byte for byte, so it carries its store format, plus `config.toml` with secrets removed.

```sh
tsk backup setup                                  # make (or reuse) a private GitHub repository and add it
tsk backup add git@gitlab.com:me/tsk-backup.git   # any other git repository
tsk backup list                                   # targets, last backup, last error
tsk backup                                        # back up to every target now
tsk backup remove 2                               # by number in list, URL, or path
```

### Set up with GitHub

`tsk backup setup` needs the [GitHub CLI](https://cli.github.com) (`gh`; on Windows `winget install GitHub.cli`), installed and logged in (`gh auth login`); without it, it says how to get it and writes nothing. When `gh` refuses, its own message is shown. It uses `<your GitHub user>/tsk-backup`, or the repository `--repo` names: `name`, `owner/name`, or `host/owner/name` for GitHub Enterprise (every `gh` call then runs with `GH_HOST` set to that host; otherwise your own `GH_HOST` or `gh`'s default host applies).

- A repository that does not exist is created private and empty.
- An existing private repository is used when it is empty or already holds tsk backups (`machines/`); one with other content is refused.
- An internal (enterprise) repository is used, with a note that everyone in the enterprise can read it.
- A public repository is refused: backups hold your task notes.

It then adds the repository like `tsk backup add`, with the clone URL that matches your `gh` git protocol (ssh or https), and prints it. Run again with the target already added, it says so and changes nothing. On a new machine whose store is still empty, it says when the repository already holds other machines' backups and suggests `tsk restore`; it never restores by itself.

### Add any repository

`tsk backup add <git-url>` takes a URL (`git@host:me/repo.git`, `https://…`, `ssh://…`, `file://…`) or the path of a local repository, stored as an absolute path. It tests the target straight away: it runs the first backup in the foreground and prints where it went. With nothing to back up yet (a new machine), it still clones the repository, so a wrong URL or missing access shows here. If the first backup fails, the target stays in `config.toml`, the command exits 1, and it says what to fix; rerun `tsk backup` once it is fixed. Adding a target that exists only updates its `every` (and tests it again).

After a first backup that worked, `add` and `setup` also start one detached `tsk backup run`, exactly as a save does, and wait up to five seconds for it to reach the repository. If only that run fails, they say so (the exit code stays 0): background runs have no terminal, so git must reach the repository without a prompt. The usual cause is an ssh key with a passphrase and no ssh-agent the board's environment can see, for example a board started in Herdr from a shell that has no `SSH_AUTH_SOCK`. Load the key into an agent that every shell sees (on macOS, `UseKeychain yes` and `AddKeysToAgent yes` in `~/.ssh/config`), or use https with a credential helper.

Advanced: a local bare repository works as a target too, including one inside a Dropbox, iCloud Drive, OneDrive, or Google Drive folder (`git init --bare ~/Dropbox/tsk-backup.git`), for **one machine only**. tsk can only confirm the push into the folder; the cloud app still has to finish syncing it, and two machines pushing to the same synced folder can corrupt the repository. For more than one machine, use `tsk backup setup` or another hosted repository.

### config.toml

The verbs write `[[backup]]` entries to `config.toml`; you can also edit them by hand:

```toml
[[backup]]
url = "git@github.com:me/tsk-backup.git"
every = "save"             # "save" (the default) or "daily"
```

| Key | Default | Meaning |
| --- | --- | --- |
| `url` | required | The repository tsk pushes to: a URL or a local path |
| `every` | `save` | `save`: after every save. `daily`: on the first save or full board open 24 hours after the target's last backup (there is no background service; a failed attempt is retried ten minutes later) |
| `kind` | `git` | Optional; `git` is the only kind |

### How a backup runs

Backups never block or fail a save. When a target is due, the save or board open starts a detached `tsk backup run` and returns; that child commits and pushes. Runs are coalesced: one runs and at most one waits, so a burst of saves pushes at most twice, and a save that finds a run already waiting leaves its trigger to it instead of starting another child. A background run never stops to repack its clone (git's automatic `gc` is off there); `tsk backup`, `add`, and `setup` let git tidy it when it wants to. A store and `config.toml` unchanged since a target's last backup are not pushed again. A store that does not parse is never pushed: the target records `store does not parse, not backed up` instead.

A failure is recorded in `backup-status.json`: `tsk backup list` shows it, and every full board open shows it on the status row until the target succeeds (a configuration error that needs the row takes it first; the backup failure shows on a later open). A target that cannot be reached is recorded even before there is a store to back up. `tsk backup` runs every target in the foreground and exits 1 if any failed.

The repository holds `machines/<machine-id>/tsk.json`, `config.toml`, and `machine.toml` (host name and tsk version), one commit per backup on the repository's default branch (the branch its `HEAD` names, even in an empty repository), so its history is every version. Each commit message records the host name, task count, tsk version, and store format. `<machine-id>` is this state directory's `machine-id`, so a host name change keeps the same directory; several machines can share one repository. The repository root is left free on purpose: a later sync across machines puts its shared file there.

tsk runs your own `git` with your credentials and stores no tokens. The detached child never prompts and does not depend on your global git setup: it commits as `tsk`, without signing or hooks, with interactive credential helpers off and ssh in batch mode (unless you set `GIT_SSH_COMMAND` yourself), so use a key or credential helper that works without a prompt. It checks files out with links off and refuses to write through a link in its clone. A push another machine raced is retried up to three times (the machines write different directories, so there is never a conflict); any other refusal is recorded at once.

### Secrets

The pushed `config.toml` keeps every key but replaces each value under `[agent.<name>.env]` with `"<not backed up>"`, under a comment saying so; fill them in by hand after copying a profile over. Credentials written into a `[[backup]]` URL are dropped too (`https://user:token@host/…` is pushed as `https://host/…`; an ssh user name such as `git@` stays). A `config.toml` that does not parse is not pushed at all. Backups are not encrypted and contain your task notes: keep the repository private. Trash (`trash.jsonl`) is not backed up.

Once the store holds 10 tasks and no target is configured, one full board open says `No backup yet: run tsk backup setup` on the status row. It is shown once per state directory, never again.

tsk never deletes `machines/<machine-id>/` directories a machine stopped writing to, or the clones under `backup-repos/`; delete them by hand when you no longer want them.

### Restore

`tsk restore` lists the versions every target holds, newest first, with an id, time, host name, task count, and target (the newest 20 per machine; `--all` shows every one). In a terminal it asks which to restore, by number or id; `tsk restore <id>` restores one without asking. A task number such as `T12` is not a version: restore suggests `tsk trash restore T12`.

```sh
tsk restore
tsk restore 3f9a1c2e
```

Restore copies the current `tsk.json` to `tsk.json.pre-restore-<UTC time>`, then replaces it with the chosen version under the store lock. It keeps the newest five `tsk.json.pre-restore-*` copies and deletes older ones. A version in an older store format is written as is and migrates on the next open; one newer than this tsk is refused with a request to update tsk, as are files that are not a store.

Only `machines/` directories shaped like a machine id are listed; anything else another writer put there is skipped. Restore never changes `config.toml`. It prints the `git show` command that prints the `config.toml` backed up with that version, so you can copy agent profiles and settings over by hand.

Restore refuses while a board or quick capture is open on the store: an open board merges what is on disk into what it holds and would write back, on its next save, every task the restored version does not have. Quit them, then restore.

On a new machine, set up the same repository and restore from it:

```sh
tsk backup setup        # or: tsk backup add <git-url>
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
