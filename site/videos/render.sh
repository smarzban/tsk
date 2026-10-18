#!/usr/bin/env bash
# Render the demo clips. Usage: site/videos/render.sh [clip ...]   (default: every clip)
#
# Per clip, into site/videos/out/ (gitignored):
#   <clip>.mp4         1920x1080 H.264, the site's 16:9 cut
#   <clip>.webm        the same in VP9, for the site
#   <clip>.gif         1280 wide, for the README
#   <clip>-square.mp4  1080x1080, recorded at that size (the board's narrow layout), for X/Discord
#
# Needs vhs (brings ttyd), ffmpeg, herdr 0.9+, and `cargo build --release` in this repo. Each
# recording runs against fresh throwaway roots under /tmp/tsk-vid-<clip>/ (see lib/env.sh).
set -euo pipefail
export LC_ALL=C

cd "$(dirname "$0")"
# shellcheck source=lib/env.sh
. lib/env.sh
# A failed or interrupted recording still stops its Herdr server (and the agent in it).
trap 'vid_stop_herdr' EXIT
OUT="$VIDEOS_DIR/out"
mkdir -p "$OUT"

[ -x "$REPO_DIR/target/release/tsk" ] || { echo "build first: cargo build --release" >&2; exit 1; }
for tool in vhs ttyd ffmpeg herdr; do
  command -v "$tool" >/dev/null || { echo "missing $tool" >&2; exit 1; }
done

# VARIANTS=wide renders only the 16:9 cut, for quicker iteration.
VARIANTS="${VARIANTS:-wide square}"
clips=("$@")
[ ${#clips[@]} -gt 0 ] || clips=(quick-capture dispatch-loop)

# The tape header: size, font, the owner's palette. The clip body follows.
tape_header() {
  local output="$1" width="$2" height="$3" font_size="$4"
  cat <<TAPE
Output "$output"
Set Shell "zsh"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize $font_size
Set Width $width
Set Height $height
Set Padding 28
Set Framerate 30
Set TypingSpeed 75ms
Set CursorBlink false
Set Theme $(tr -d '\n' <theme.json)
TAPE
}

record() {
  local clip="$1" variant="$2" width="$3" height="$4" font_size="$5"
  local name="$clip"
  [ "$variant" = wide ] || name="$clip-$variant"
  vid_env "$clip"
  vid_reset "$variant"
  vid_start_herdr
  # shellcheck source=/dev/null
  . "clips/$clip.seed.sh"
  local tape="$OUT/$name.tape"
  { tape_header "$OUT/$name.mp4" "$width" "$height" "$font_size"; cat "clips/$clip.tape"; } >"$tape"
  echo "recording $name"
  (cd "$VID_HOME" && env -i "${VID_ENV[@]}" vhs -q "$tape")
  vid_stop_herdr
}

for clip in "${clips[@]}"; do
  [ -f "clips/$clip.tape" ] || { echo "no clip $clip" >&2; exit 1; }
  case " $VARIANTS " in *" wide "*) record "$clip" wide 1920 1080 24 ;; esac
  case " $VARIANTS " in *" square "*) record "$clip" square 1080 1080 20 ;; esac
  [ -f "$OUT/$clip.mp4" ] || continue
  ffmpeg -loglevel error -y -i "$OUT/$clip.mp4" -c:v libvpx-vp9 -b:v 0 -crf 34 -row-mt 1 \
    "$OUT/$clip.webm"
  ffmpeg -loglevel error -y -i "$OUT/$clip.mp4" -filter_complex \
    "fps=12,scale=1280:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=64:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" \
    "$OUT/$clip.gif"
done

for f in "$OUT"/*.mp4 "$OUT"/*.webm "$OUT"/*.gif; do
  [ -f "$f" ] || continue
  printf '%-28s %6.1fs %8s\n' "$(basename "$f")" \
    "$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$f")" \
    "$(du -h "$f" | cut -f1)"
done
