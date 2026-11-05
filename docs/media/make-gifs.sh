#!/usr/bin/env bash
# Convert the clips from record.js into README GIFs and copy them into docs/media.
#   docs/media/make-gifs.sh /tmp/rvwasm-media
set -euo pipefail
IN="${1:-/tmp/rvwasm-media}"
OUT="$(cd "$(dirname "$0")" && pwd)"
# Small palettes and no dithering keep terminal text crisp and files small.
gif() { # name start-seconds fps width colors
  ffmpeg -loglevel error -y -ss "$2" -i "$IN/$1.webm" \
    -vf "fps=$3,scale=$4:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=$5:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" \
    "$OUT/$1.gif"
}
gif xv6 0.4 8 860 48
gif linux 0.4 7 800 40
cp "$IN/timeline.png" "$OUT/timeline.png"
ls -la "$OUT"
