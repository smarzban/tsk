# Clip 5 seed, sourced by render.sh after vid_reset: the api project with a few tasks, a
# `claude` profile whose command is the scripted fake agent, and the agent's plan for T2.

repo="$(vid_repo api)"
mkdir -p "$repo/src"
cat >"$repo/src/session.ts" <<'EOF'
import { Store } from "./store";

// Sessions expire after this long without a request.
export const LOGIN_TIMEOUT_MS = 5_000;

export function expired(lastSeen: number, now: number): boolean {
  return now - lastSeen > LOGIN_TIMEOUT_MS;
}
EOF
vid_run git -C "$repo" add -A
vid_run git -C "$repo" commit -q -m "Add session expiry"
vid_run git -C "$repo" push -q origin main

cat >"$VID_ROOT/state/config.toml" <<EOF
[agent.claude]
command = ["$VID_BIN/claude"]

[merge]
method = "squash"
EOF

tsk_() { vid_run "$VID_BIN/tsk" "$@" >/dev/null; }
tsk_ add -p "$repo" -t "Rate-limit the password reset endpoint" --thread auth
tsk_ add -p "$repo" -t "Fix login timeout" --thread auth \
  --notes "Users get logged out mid-form. The session timeout is 5s; it should be 30s."
tsk_ add -p "$repo" -t "Add request IDs to access logs" --thread observability
tsk_ add -p "$repo" -t "Document the webhook retry policy" --thread docs
tsk_ status 2 ready
tsk_ status 3 ready
tsk_ status 4 started

mkdir -p "$VID_ROOT/agent"
cat >"$VID_ROOT/agent/T2.sh" <<'EOF'
EDIT_FILE=src/session.ts
EDIT_FROM="export const LOGIN_TIMEOUT_MS = 5_000;"
EDIT_TO="export const LOGIN_TIMEOUT_MS = 30_000;"
TEST_LINE="14 passed · 0 failed"
COMMIT="Raise the login timeout to 30s"
DONE="Login timeout raised from 5s to 30s"
CHECKS=("Stay idle 20s on the login form, then submit: still signed in" "npm test passes")
EOF

# The board alone over the caption strip, run in the workspace's own pane rather than as a
# plugin split: the workspace keeps a pane in the repo, so Herdr files the agent's worktree
# workspace under it instead of opening a second "api" workspace for the repo.
shell="$(vid_workspace "$repo" api)"
vid_run herdr pane rename "$shell" tsk >/dev/null
vid_run herdr pane run "$shell" "clear; exec tsk" >/dev/null
