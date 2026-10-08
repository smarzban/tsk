#!/usr/bin/env bash
# CI probe: does this PR need the Rust verify? Writes rust=true|false to $GITHUB_OUTPUT.
# Only a PR whose every changed path is under site/ or is the site workflow skips; a push,
# a failed or empty diff, or any other path runs the full verify. Expects the PR merge
# commit checked out with its first parent (fetch-depth: 2). --no-renames lists a move's
# source too, so moving code under site/ still counts. No pipes: an early-exiting reader
# under pipefail must never turn into a skip.
set -uo pipefail

rust=true
if [ "${GITHUB_EVENT_NAME:-}" = pull_request ] &&
  changed=$(git diff --no-renames --name-only HEAD^1 HEAD) && [ -n "$changed" ]; then
  printf '%s\n' "$changed"
  rust=false
  while IFS= read -r path; do
    case $path in
      site/* | .github/workflows/site.yml) ;;
      *) rust=true; break ;;
    esac
  done <<<"$changed"
  [ "$rust" = false ] && echo "::notice::Site-only change; skipping the Rust verify steps."
fi
echo "rust=$rust" >>"${GITHUB_OUTPUT:-/dev/stdout}"
