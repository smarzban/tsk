# Demo videos

Short clips of tsk inside Herdr, recorded with [VHS](https://github.com/charmbracelet/vhs) from
checked-in tapes. Everything a clip needs (board state, git repos, a Herdr server, the agent)
is scripted, so a render is repeatable and never touches your own board, Herdr, or repos.

| Clip | What it shows |
| --- | --- |
| `quick-capture` | `prefix+t` opens the board beside a shell, `+` quick-adds `Fix login timeout !p api !t auth`, the row lands in the inbox with its thread, `ctrl+n` makes it ready |
| `dispatch-loop` | `@` assigns the task to `@claude`, `ctrl+s` dispatches it (worktree, Herdr workspace, agent), the agent hands back for review, the task page's checks are ticked, `ctrl+d` then `m` merges the PR and cleans up |

## Render

Needs `vhs` (brings `ttyd`; `brew install vhs`), `ffmpeg`, Herdr 0.9+, Google Chrome (VHS
drives it headless), the JetBrainsMono Nerd Font, and a release build of this repo:

```sh
cargo build --release
site/videos/render.sh                    # every clip
site/videos/render.sh dispatch-loop      # one clip
VARIANTS=wide site/videos/render.sh      # skip the square cut while iterating
```

Output lands in `site/videos/out/` (gitignored), per clip:

| File | For |
| --- | --- |
| `<clip>.mp4` | 1920x1080 H.264, the site |
| `<clip>.webm` | the same in VP9, the site |
| `<clip>.gif` | 1280 wide, 12 fps, the README |
| `<clip>-square.mp4` | 1080x1080, X and Discord |

The square cut is recorded at that size rather than cropped from the 16:9 one, so the board
lays itself out for the narrower frame and the text stays readable.

## How a clip runs

`render.sh` does this for each clip and size:

1. `lib/env.sh` builds a fresh root at `/tmp/tsk-vid-<clip>/`: a neutral `HOME` (prompt
   `~/api ❯`, no hostname or user), `TSK_STATE_DIR`, `XDG_*`, and `HERDR_SOCKET_PATH`, all
   inside it. Every process runs under `env -i` with only those variables, so nothing from the
   calling shell leaks in (a dispatched agent's `TSK_AGENT` would turn the clip's dispatch into
   a plain start, and its `HERDR_*` would reach your Herdr server).
2. It starts the clip's own headless Herdr server, checks that `herdr status` names the temp
   socket, then runs `tsk setup herdr` against those roots.
3. `clips/<clip>.seed.sh` creates the repos (each with a local bare `origin`), the tasks, the
   agent profile, and the opening Herdr layout.
4. VHS records `clips/<clip>.tape` behind a generated header (size, font, theme) and attaches
   a Herdr client to the clip's server.
5. The server is stopped. The root stays for inspection until the next render of that clip.

### Fakes

- `lib/fake-claude` is the agent: the `claude` profile's command. It reads the clip's plan
  (`$VID_ROOT/agent/T<n>.sh`, written by the seed), prints a few believable tool calls, edits a
  file in its worktree, commits, pushes, opens a PR with the fake `gh`, and hands back with
  `tsk status <n> review --done … --check …`. Offline, deterministic, free.
- `lib/fake-gh` is first on the clip's `PATH` (the board finds `gh` on `PATH`). It answers
  `gh pr view` and `gh pr merge` from the local bare origin: a pushed branch has an open PR;
  merging squashes it onto `main`, deletes the remote branch as GitHub would, and records the
  PR as merged.

### Captions

Captions sit in a one-line strip under the work: a Herdr pane running `lib/caption-pane`.
While recording is hidden, the tape focuses the strip (`prefix+j`), types the next caption,
and steps back with `prefix+;` (`last_pane`, bound only in the clip's Herdr config), so each
caption change lands as a clean cut in the same font and palette as everything else.

The alternative, burning captions in with ffmpeg `drawtext`, was ruled out: Homebrew's ffmpeg
has no `drawtext` (or `subtitles`) filter, and a post-pass would need caption timestamps kept
in sync by hand with tapes whose waits (worktree creation, the agent) vary from run to run.

### Look

The owner's setup: Ghostty with gruvbox-material-dark (`theme.json`), JetBrainsMono Nerd
Font (the installed family name; plain "JetBrains Mono" falls back to a serif in VHS's
browser), Herdr with theme `terminal`. tsk itself is monochrome, so the terminal palette is
the whole look.

## Adding a clip

Write `clips/<name>.seed.sh` (sourced by `render.sh` with `lib/env.sh` loaded) and
`clips/<name>.tape` (no `Output` or `Set` lines; the header is generated), add the name to
the default list in `render.sh`, render, then pull frames and look at them:

```sh
ffmpeg -i site/videos/out/<name>.mp4 -vf "fps=1/2,scale=800:-1,tile=3x3" -frames:v 1 sheet.png
```

Check for a stray shell prompt, cut-off text, and anything from the real machine.
