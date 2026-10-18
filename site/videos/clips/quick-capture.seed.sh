# Clip 1 seed, sourced by render.sh after vid_reset: the api project with a few tasks, a
# Herdr workspace on it with an idle shell over the caption strip.

repo="$(vid_repo api)"
tsk_() { vid_run "$VID_BIN/tsk" "$@" >/dev/null; }
tsk_ add -p "$repo" -t "Rate-limit the password reset endpoint" --thread auth
tsk_ add -p "$repo" -t "Add request IDs to access logs" --thread observability
tsk_ add -p "$repo" -t "Document the webhook retry policy" --thread docs
tsk_ add -p "$repo" -t "Upgrade the Postgres driver" --thread infra
tsk_ status 2 ready
tsk_ status 3 started
tsk_ status 4 ready

vid_workspace "$repo" api >/dev/null
