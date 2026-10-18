# Isolated roots for one demo clip. Sourced by render.sh (bash), never run directly.
#
# Every clip gets /tmp/tsk-vid-<clip>/ with its own HOME, tsk state, Herdr server, config and
# plugin registry, and git repos with a local bare origin. Nothing here reads or writes the
# caller's ~/.tsk, ~/.config/herdr, Herdr server, or real repositories.

VIDEOS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$VIDEOS_DIR/../.." && pwd)"

# The isolated environment, as `env -i` arguments: nothing from the caller's shell leaks in
# (a dispatched agent's TSK_AGENT would turn every dispatch into a plain start, and its
# HERDR_* would reach the owner's server).
vid_env() {
  local clip="$1"
  VID_ROOT="/tmp/tsk-vid-$clip"
  VID_HOME="$VID_ROOT/home"
  VID_BIN="$VID_ROOT/bin"
  VID_PATH="$VID_BIN:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
  VID_ENV=(
    HOME="$VID_HOME"
    USER=demo
    LOGNAME=demo
    SHELL=/bin/zsh
    ZDOTDIR="$VID_HOME"
    TERM=xterm-256color
    COLORTERM=truecolor
    LANG=en_US.UTF-8
    LC_ALL=en_US.UTF-8
    PATH="$VID_PATH"
    TMPDIR="$VID_ROOT/tmp"
    XDG_CONFIG_HOME="$VID_HOME/.config"
    XDG_STATE_HOME="$VID_HOME/.local/state"
    XDG_DATA_HOME="$VID_HOME/.local/share"
    XDG_CACHE_HOME="$VID_HOME/.cache"
    HERDR_SOCKET_PATH="$VID_ROOT/herdr.sock"
    TSK_STATE_DIR="$VID_ROOT/state"
    TSK_NO_UPDATE_CHECK=1
    GIT_CONFIG_GLOBAL="$VID_HOME/.gitconfig"
    GIT_CONFIG_NOSYSTEM=1
    VID_ROOT="$VID_ROOT"
  )
}

# Run a command inside the clip's environment.
vid_run() {
  env -i "${VID_ENV[@]}" "$@"
}

# Fresh root: HOME with a neutral prompt, PATH with the fake tools first, git identity.
# vid_reset [square]: the square cut starts Herdr with its sidebar folded to the status rail,
# leaving the 1080-wide frame to the panes.
vid_reset() {
  local variant="${1:-wide}"
  case "$VID_ROOT" in
    /tmp/tsk-vid-?*) ;;
    *) echo "refusing to reset $VID_ROOT" >&2; return 1 ;;
  esac
  vid_stop_herdr
  rm -rf -- "$VID_ROOT"
  mkdir -p "$VID_HOME/.config/herdr" "$VID_HOME/.local/state" "$VID_BIN" "$VID_ROOT/tmp" \
    "$VID_ROOT/state" "$VID_ROOT/gh"

  # Login shells (Herdr panes, the agent's `$SHELL -lc`) run /etc/zprofile's path_helper,
  # which reorders PATH: set it again after, so the fake gh and agent win.
  cat >"$VID_HOME/.zprofile" <<EOF
export PATH="$VID_PATH"
EOF
  cat >"$VID_HOME/.zshrc" <<EOF
export PATH="$VID_PATH"
PROMPT='%F{8}~/%1~%f %F{4}❯%f '
RPROMPT=''
unsetopt PROMPT_SP
setopt NO_BEEP
EOF
  cat >"$VID_HOME/.gitconfig" <<'EOF'
[user]
	name = Demo
	email = demo@example.com
[init]
	defaultBranch = main
[advice]
	detachedHead = false
EOF

  ln -s "$REPO_DIR/target/release/tsk" "$VID_BIN/tsk"
  ln -s "$VIDEOS_DIR/lib/fake-gh" "$VID_BIN/gh"
  ln -s "$VIDEOS_DIR/lib/fake-claude" "$VID_BIN/claude"
  ln -s "$VIDEOS_DIR/lib/caption-pane" "$VID_BIN/caption-pane"

  cat >"$VID_HOME/.config/herdr/config.toml" <<'EOF'
onboarding = false

[theme]
name = "terminal"

[update]
version_check = false
manifest_check = false

# Unbound by default. The tapes use last_pane to step back from the caption strip to the
# pane they left, whichever side of a split that is, and the workspace pair to visit an
# agent's workspace.
[keys]
last_pane = "prefix+;"
next_workspace = "prefix+]"
previous_workspace = "prefix+["
EOF
  if [ "$variant" = square ]; then
    printf '\n[ui]\nsidebar_start_collapsed = true\n' >>"$VID_HOME/.config/herdr/config.toml"
  fi
}

# A project repository at ~/code/<name> with a local bare origin, both under the clip root.
# Prints the repository path.
vid_repo() {
  local name="$1"
  local origin="$VID_ROOT/origin/$name.git"
  local repo="$VID_HOME/code/$name"
  vid_run git init -q --bare "$origin"
  vid_run git init -q "$repo"
  printf '# %s\n' "$name" >"$repo/README.md"
  vid_run git -C "$repo" add -A
  vid_run git -C "$repo" commit -q -m "Initial commit"
  vid_run git -C "$repo" remote add origin "$origin"
  vid_run git -C "$repo" push -q -u origin main
  vid_run git -C "$repo" remote set-head origin main
  echo "$repo"
}

# The clip's own Herdr server, its status checked against the temp socket before anything
# else touches it, then tsk registered into its config by `tsk setup herdr`.
vid_start_herdr() {
  (cd "$VID_HOME" && env -i "${VID_ENV[@]}" nohup herdr server >"$VID_ROOT/herdr-server.out" 2>&1 &)
  for _ in $(seq 1 50); do
    [ -S "$VID_ROOT/herdr.sock" ] && break
    sleep 0.1
  done
  local status
  status="$(vid_run herdr status server 2>&1)" || {
    echo "herdr server did not start: $status" >&2
    return 1
  }
  case "$status" in
    *"$VID_ROOT/herdr.sock"*) ;;
    *) echo "herdr status does not name $VID_ROOT/herdr.sock:" >&2; echo "$status" >&2; return 1 ;;
  esac
  vid_run "$VID_BIN/tsk" setup herdr </dev/null >"$VID_ROOT/setup.out" 2>&1 || {
    cat "$VID_ROOT/setup.out" >&2
    return 1
  }
  vid_run herdr server reload-config >/dev/null
}

vid_stop_herdr() {
  [ -n "${VID_ROOT:-}" ] && [ -S "$VID_ROOT/herdr.sock" ] || return 0
  vid_run herdr server stop >/dev/null 2>&1 || true
}

# The opening layout: one workspace on <repo> holding a shell pane over the caption strip.
# Prints the shell pane's id.
vid_workspace() {
  local repo="$1" label="$2" out shell caption
  out="$(vid_run herdr workspace create --cwd "$repo" --label "$label" --focus)"
  shell="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["root_pane"]["pane_id"])')"
  out="$(vid_run herdr pane split "$shell" --direction down --ratio 0.92 --cwd "$repo" --no-focus)"
  caption="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["pane"]["pane_id"])')"
  vid_run herdr pane run "$caption" "clear; exec caption-pane" >/dev/null
  vid_run herdr pane run "$shell" "clear" >/dev/null
  echo "$shell"
}
