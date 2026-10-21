---
title: Storage
description: Where tsk keeps tasks and settings, config.toml, git backups and restore, deleted tasks, and the update check.
---

Where tsk keeps your tasks and settings, how to configure it, and how to back it up. The board, the CLI, and the Herdr plugin share one store.

## Location

| Platform | State directory |
| --- | --- |
| macOS, Linux | `~/.tsk` |
| Windows | `%LOCALAPPDATA%\tsk`, or `%USERPROFILE%\.tsk` when `LOCALAPPDATA` is not set |

- `TSK_STATE_DIR` moves it. `--state-dir <dir>` moves it for one data command.
- With no home directory and no `TSK_STATE_DIR`, tsk refuses to run rather than pick a directory.
- Inside Herdr, tsk uses the same directory, so the plugin and a bare terminal show the same board.
- Keep it on a local disk. Network drives and synced folders such as Dropbox, iCloud Drive, or OneDrive are not supported; use a [git backup](#git-backups) instead.
- It must be a real directory, not a symlink or a Windows junction.

## Files

| File | Contains |
| --- | --- |
| `tsk.json` | Your tasks, including archived tasks and projects |
| `tsk.json.1` | The previous good copy of `tsk.json` |
| `tsk.json.v<N>` | The store as it was before tsk upgraded its format ([store version](#store-version)) |
| `tsk.json.pre-restore-<time>` | The store a [restore](#restore) replaced; the newest five are kept |
| `config.toml` | Your [settings](#configuration) |
| `trash.jsonl` | [Deleted tasks](#deleted-tasks) |
| `delivery.json` | Which [starter tasks](#starter-tasks) and release notes this directory has received |
| `backup-status.json` | Each backup target's last success, last attempt, and last error |
| `machine-id` | This directory's id, which names its folder in a backup repository |
| `backup-repos/` | Local clones of backup targets |
| `update.json` | The cached [update check](#update-check) |
| `fetch-stamps.json` | When each repository's remote was last fetched. Dispatch, the base picker, and cleanup reuse a fetch from the last 60 seconds. Safe to delete |
| `launchers/` | Dispatch launchers. Each one deletes itself once its agent starts |
| `cleanups/` | Marks of worktree removals under way. After an interrupted removal, the next cleanup refuses and says how to finish by hand |

## Configuration

`config.toml` in the state directory holds agent profiles, the merge method, board defaults, and [backup targets](#settings).

- The first full board open writes a starter file when none exists. It explains each setting and holds commented examples that take effect only when you uncomment them.
- Quick capture, the CLI, and setup never create the file. tsk never replaces an existing file, even an empty one.
- tsk never edits profiles or other settings: edit the file yourself. Only the [backup verbs](#git-backups) write to it, adding and removing `[[backup]]` entries.

### Agent profiles

Each `[agent.<name>]` table is a profile you can [assign](/docs/dispatch/#assign) to a task.

```toml
[agent.implementer]
command = ["pi"]
prompt = "Work on T{number}: {title}\n\n{notes}\n\n{steps}"

[agent.implementer.env]
PI_PROVIDER = "anthropic"
```

| Key | Required | Value |
| --- | --- | --- |
| `command` | Yes | The program and its arguments, as a list of strings |
| `prompt` | No | The agent's first message. Without it, tsk uses its default prompt |
| `env` | No | Environment variables for this agent only, as strings |

- A name follows the thread rules: lowercase letters, digits, `-` and `.`, starting with a letter or digit, up to 32 characters. Quote a name with dots: `[agent."review.strict"]`.
- tsk sets `TSK_AGENT` to the profile name, so the agent's blocks, reviews, and replies carry that name.

### Prompt and placeholders

The prompt goes to the command as its last argument. Both `command` and `prompt` can use these placeholders; tsk replaces nothing else.

| Placeholder | Value |
| --- | --- |
| `{number}` | Task number, without the `T` |
| `{title}` | Title |
| `{notes}` | Notes |
| `{steps}` | Steps, one per line as `[ ]` or `[x]` |
| `{worktree}` | The task's worktree path |
| `{branch}` | The task branch |
| `{base}` | The short base branch name, such as `dispatch` for `origin/dispatch` |

Without `prompt`, tsk sends this default. A profile's own `prompt` replaces it entirely.

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

How tsk launches the command:

- **macOS and Linux:** the command and `env` go into a private launcher file that runs once, then deletes itself. The pane shows only `/bin/sh "…/launchers/dispatch-….sh"`. The command starts with your login shell's environment, so any shell works. No task text or `env` value is typed into your shell or its history.
  - The program must be an executable on your login shell's `PATH`. Shell functions and aliases are not used.
  - `env` names must be variable names: letters, digits, and `_`, not starting with a digit.
  - Dispatch refuses a [state directory](#location) whose path contains `$`, `` ` ``, `\`, `"`, or `!`.
- **Windows ([preview](/docs/dispatch/#windows-preview)):** the same command and `env` run through Windows PowerShell. Write paths as TOML literal strings so backslashes stay as typed: `command = ['C:\Users\you\.local\bin\claude.exe']`.

### Merge method

`m` on the [cleanup card](/docs/dispatch/#merge-the-pull-request), and `--merge` on [`tsk status … done`](/docs/cli/#status) and [`tsk clean`](/docs/cli/#clean), merge the task's pull request with the GitHub CLI. The `[merge]` table picks how:

```toml
[merge]
method = "squash"   # "merge" (the default), "squash", or "rebase"
```

### Board defaults

The `[board]` table sets the [group and sort](/docs/board/#group-and-sort) a fresh board starts with, whether it shows the [live lines](/docs/board/#rows) under rows, and the name the board calls you:

```toml
[board]
group = "assignee"        # desk: none, project, thread, or assignee
sort = "recent"           # desk: recent, project, thread, title, or assignee
project_group = "thread"  # project boards: none, thread, or assignee
project_sort = "title"    # project boards: recent, thread, title, or assignee
messages = "hidden"       # live lines under rows: shown or hidden
me = "sam"                # your name on the board; the default is you
```

- A missing key keeps its default: `none`, `recent`, and `shown`.
- The project keys also set the wide Projects preview.
- `g`, `s`, and `z` change the layout for the session only. Nothing is written back.

`me` replaces `@you` wherever the board names you. That covers assignee labels, pickers, filter, group, and sort, who a block or review waits on, your replies, and the paper trail.

- Sentences addressed to you keep "you". `tsk list --json` still says `you`.
- The CLI takes `you`, `me`, or your name for yourself, in `--assignee` and `--on`.
- The name follows the [thread name](/docs/capture/#thread-names) rules.
- A name that is also an agent profile is an error: the board says so on its status row and calls you `you`.

### Errors

An unknown key or a bad value in one table never blocks the others. A file that is not valid TOML breaks them all.

| Mistake | What happens |
| --- | --- |
| Unknown top-level key or table | Ignored, so a file written for a newer tsk still loads |
| Unknown key inside `[agent.<name>]`, `[merge]`, `[board]`, or `[[backup]]` | That table is an error |
| Broken profile | The board opens without profiles and shows the error on its status row. `tsk add` and `tsk edit` with an assignee exit 2 without saving |
| Broken `[merge]` | Only merging refuses: the cleanup card says why, and the CLI refuses with `merge-config` |
| Broken `[board]` | The board opens at the default layout and shows the error on its status row. A bad `me` leaves the layout alone and names you `you` |

## Git backups

`tsk.json.1` and `tsk.json.v<N>` sit on the same disk as the store. A backup target keeps copies in a git repository somewhere else, such as a private GitHub repository.

```sh
tsk backup setup                                  # make (or reuse) a private GitHub repository and add it
tsk backup add git@gitlab.com:me/tsk-backup.git   # any other git repository
tsk backup list                                   # targets, last backup, last error
tsk backup                                        # back up to every target now
tsk backup remove 2                               # by number in list, URL, or path
```

Every flag: [`tsk backup`](/docs/cli/#backup).

### Set up with GitHub

`tsk backup setup` needs the [GitHub CLI](https://cli.github.com) (`gh`), installed and logged in with `gh auth login`. On Windows, install it with `winget install GitHub.cli`.

It uses `<your GitHub user>/tsk-backup`, or the repository you name with `--repo`: `name`, `owner/name`, or `host/owner/name` for GitHub Enterprise.

| The repository | Setup |
| --- | --- |
| Does not exist | Creates it, private and empty |
| Is private and empty, or already holds tsk backups | Uses it |
| Is private and holds other content | Refuses |
| Is internal (enterprise) | Uses it, and notes that everyone in the enterprise can read it |
| Is public | Refuses: backups hold your task notes |

Setup then adds the repository like `tsk backup add`, using the clone URL that matches your `gh` git protocol. Running it again changes nothing. On a new machine with an empty store, it says when the repository holds other machines' backups and suggests `tsk restore`. It never restores by itself.

### Add any repository

`tsk backup add` takes a git URL (`git@host:me/repo.git`, `https://…`, `ssh://…`, `file://…`) or the path of a local repository.

- It runs the first backup straight away and prints where it went. With nothing to back up yet, it still clones the repository, so a wrong URL shows at once.
- If the first backup fails, the target stays in `config.toml` and the command exits 1. Fix the problem, then run `tsk backup`.
- Adding a target that already exists only updates its `every`.

A local bare repository inside a Dropbox, iCloud Drive, OneDrive, or Google Drive folder works for **one machine only**. Two machines pushing to the same synced folder can corrupt it. For several machines, use a hosted repository.

### Settings

The backup verbs write `[[backup]]` entries to `config.toml`. You can also edit them by hand:

```toml
[[backup]]
url = "git@github.com:me/tsk-backup.git"
every = "save"             # "save" (the default) or "daily"
```

| Key | Default | Meaning |
| --- | --- | --- |
| `url` | Required | A git URL or a local path |
| `every` | `save` | `save`: after every save. `daily`: on the first save or full board open 24 hours after the last backup; a failed attempt is retried ten minutes later |
| `kind` | `git` | Optional. `git` is the only kind |

### What is pushed

Each machine writes its own folder, `machines/<machine-id>/`:

- `tsk.json`, byte for byte.
- `config.toml`, with every value under `[agent.<name>.env]` replaced by `"<not backed up>"` and credentials removed from backup URLs. Fill the values in by hand after copying a profile over.
- `machine.toml`: the host name and tsk version.

Each backup is one commit on the repository's default branch, so its history holds every version. Several machines can share one repository.

- Backups are not encrypted and contain your task notes: keep the repository private.
- Trash is not backed up.
- A store or `config.toml` that does not parse is not pushed. A store unchanged since the last backup is not pushed again.
- tsk never deletes a machine folder that stopped receiving backups, or a clone under `backup-repos/`. Delete them by hand when you no longer need them.

Once the store holds 10 tasks and no target is set, one board open says `No backup yet: run tsk backup setup` on the status row. It says this once per state directory.

### Troubleshooting

Saves and board opens never wait for a backup: when a target is due, tsk backs it up in the background.

- A failure shows in `tsk backup list`, and on the status row at every full board open until the target succeeds.
- `tsk backup` runs every target in the foreground and exits 1 if any failed.
- Background backups have no terminal, so git must reach the repository without a prompt. After a first backup that worked, `add` and `setup` check that a background backup can reach it too, and say so when it cannot.
- The usual cause is an ssh key with a passphrase and no ssh-agent the board can see, for example a board started in Herdr from a shell without `SSH_AUTH_SOCK`. Load the key into an agent every shell sees (on macOS, `UseKeychain yes` and `AddKeysToAgent yes` in `~/.ssh/config`), or use https with a credential helper.
- tsk runs your own `git` with your credentials. Background backups commit as `tsk`, without signing or hooks.

### Credentials

Prefer ssh or a credential helper to a token in a backup URL. A URL such as `https://me:TOKEN@github.com/me/tsk-backup.git` works, with these limits:

- `config.toml` keeps the URL as you wrote it, token included.
- Backups never push the token: the pushed `config.toml` has it removed.
- Every command prints the URL with its credentials as `***`, such as `https://***@github.com/me/tsk-backup.git`. `tsk backup remove` takes that form too, unless two targets share it: then give the number or the full URL.

## Restore

```sh
tsk restore             # list versions, then choose one
tsk restore 3f9a1c2e    # restore one version without asking
```

`tsk restore` lists the versions every target holds, newest first, with an id, time, host name, task count, and target. It shows the newest 20 per machine; `--all` shows every one. An id restores the version it names, shown or not. In a terminal it asks which to restore.

- Quit every board and quick capture on the store first: restore refuses while one is open.
- The store it replaces is kept as `tsk.json.pre-restore-<time>`, in UTC. The newest five copies are kept.
- A version from an older tsk migrates on the next open. One from a newer tsk is refused until you update.
- Task numbers never go backwards: new tasks continue after the highest number the replaced store or trash held.
- Restore never changes `config.toml`. It prints the `git show` command for the `config.toml` saved with that version, so you can copy settings over by hand.
- A task number such as `T12` is not a version. To bring back a deleted task, use `tsk trash restore T12`.

On a new machine, add the same repository, then restore:

```sh
tsk backup setup        # or: tsk backup add <git-url>
tsk restore
```

## Store version

This version writes store format 6. tsk 0.12 and older refuse a format 6 store, as older tsk refuses any newer store. tsk keeps a `tsk.json.v<N>` copy when it upgrades the file.

To roll back: quit every board, then copy `tsk.json.v<N>` over `tsk.json`. Changes made since the upgrade are lost.

## Deleted tasks

A deleted task moves to `trash.jsonl` once undo can no longer bring it back, or after seven days. Trash is purged 30 days after deletion.

```sh
tsk list --deleted --all
tsk trash restore T12
```

`tsk list --deleted` shows recent deletions and trash together. `tsk trash restore` reads trash only: use `ctrl+u` on the board for a deletion that has not moved there yet.

## Starter tasks

The first full board open puts four starter tasks on your desk, painted `N1`…. They arrive once per state directory: mark one done, archive it, or delete it, and it never returns.

After an upgrade, the first board open adds one `What's new in tsk` task to your desk, in review under **NEEDS YOU**, when the new version has release notes you have not seen. Every missed release lands in that one task, newest first, with a link to the changelog. A new install shows none. Clear it like a starter task.

## Update check

When the board opens, tsk asks GitHub for the latest release if its last check is more than 24 hours old. A newer version shows on the status row as `vX.Y.Z available, run tsk update`. The check sends no task data, and a failed check stays silent.

| Setting | Effect |
| --- | --- |
| `TSK_NO_UPDATE_CHECK` | Set it to turn off the check and the notice |
| `TSK_UPDATE_CURL=/path/to/curl` | macOS and Linux: the curl the check and `tsk update` use, as an absolute path. The default is `/usr/bin/curl`. Windows uses a built-in HTTPS client |

[Upgrade tsk](/docs/install/#upgrade).
