#!/usr/bin/env bash
set -euo pipefail

export DISPLAY=:1

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)
ALBUM="$HOME/album"
VAULT="$HOME/vault"
NOTES="$HOME/notes.txt"
MP4="$SCRIPT_DIR/demo.mp4"
GIF="$SCRIPT_DIR/demo.gif"
FFMPEG_LOG="/tmp/albumfs-demo-ffmpeg.log"
PALETTE=$(mktemp /tmp/albumfs-demo-palette.XXXXXX.png)
FFMPEG_PID=""
TERM_PID=""
DEMO_CREATED=0

cleanup() {
    set +e
    if [[ -n "$TERM_PID" ]]; then
        kill "$TERM_PID" 2>/dev/null || true
        wait "$TERM_PID" 2>/dev/null || true
    fi
    pkill -TERM -f "feh --fullscreen $ALBUM/cover.jpg" 2>/dev/null || true
    if mountpoint -q "$VAULT"; then
        "$REPO_ROOT/target/debug/albumfs" umount "$VAULT" >/dev/null 2>&1 || true
    fi
    if [[ -n "$FFMPEG_PID" ]]; then
        kill -INT "$FFMPEG_PID" 2>/dev/null || true
        wait "$FFMPEG_PID" 2>/dev/null || true
    fi
    rm -f "$PALETTE"
    if [[ "$DEMO_CREATED" == 1 ]]; then
        rm -f \
            "$ALBUM/cover.jpg" \
            "$ALBUM/2019-beach.jpg" \
            "$ALBUM/birthday.jpg" \
            "$ALBUM/hiking-trip.jpg" \
            "$ALBUM/weekend-lake.jpg" \
            "$ALBUM/city-lights.jpg" \
            "$ALBUM/garden.jpg" \
            "$ALBUM/road-trip.jpg" \
            "$NOTES"
        rmdir "$ALBUM" "$VAULT" 2>/dev/null || true
    fi
}
trap cleanup EXIT

for tool in xdpyinfo ffmpeg feh xterm magick mountpoint; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        printf 'required tool is missing: %s\n' "$tool" >&2
        exit 1
    fi
done

if ! xdpyinfo >/dev/null 2>&1; then
    printf 'cannot reach X display %s\n' "$DISPLAY" >&2
    exit 1
fi

GEO=$(xdpyinfo | awk '/dimensions:/{print $2; exit}')
if [[ ! "$GEO" =~ ^[0-9]+x[0-9]+$ ]]; then
    printf 'could not determine display geometry\n' >&2
    exit 1
fi

for path in "$ALBUM" "$VAULT" "$NOTES"; do
    if [[ -e "$path" ]]; then
        printf 'refusing to replace existing demo path: %s\n' "$path" >&2
        exit 1
    fi
done

cd "$REPO_ROOT"
cargo build
export PATH="$REPO_ROOT/target/debug:$PATH"

mkdir "$ALBUM" "$VAULT"
DEMO_CREATED=1
printf '%s\n' \
    'Weekend cabin access' \
    'Gate code: 4821' \
    'Bring the blue folder.' >"$NOTES"

magick rose: -filter Lanczos -resize '1024x768^' -gravity center \
    -extent 1024x768 -seed 41 -attenuate 0.06 +noise Gaussian \
    -quality 92 "$ALBUM/cover.jpg"

make_photo() {
    local name="$1"
    local seed="$2"
    local dark="$3"
    local light="$4"
    magick -seed "$seed" -size 1024x768 plasma:fractal \
        -colorspace sRGB +level-colors "$dark,$light" \
        -attenuate 0.025 +noise Gaussian -quality 92 "$ALBUM/$name"
}

make_photo 2019-beach.jpg 101 '#176b87' '#f5d790'
make_photo birthday.jpg 102 '#7a2033' '#ffd36a'
make_photo hiking-trip.jpg 103 '#244b36' '#c7a66b'
make_photo weekend-lake.jpg 104 '#193f63' '#b9e3e8'
make_photo city-lights.jpg 105 '#251342' '#ef9d5f'
make_photo garden.jpg 106 '#315b35' '#e9a8b6'
make_photo road-trip.jpg 107 '#704024' '#8fc5d4'

# Display :1 has no XFixes cursor support, so cursor drawing must stay disabled.
ffmpeg -y -f x11grab -draw_mouse 0 -framerate 30 \
    -video_size "$GEO" -i :1.0 -c:v libx264 -pix_fmt yuv420p \
    -preset veryfast "$MP4" >"$FFMPEG_LOG" 2>&1 &
FFMPEG_PID=$!
sleep 1

xterm -title 'AlbumFS encrypted photo vault' -geometry 108x29+25+35 \
    -fa Monospace -fs 13 -e bash "$SCRIPT_DIR/demo_session.sh" &
TERM_PID=$!
wait "$TERM_PID"
TERM_PID=""

sleep 1
kill -INT "$FFMPEG_PID"
wait "$FFMPEG_PID" 2>/dev/null || true
FFMPEG_PID=""

ffmpeg -y -i "$MP4" \
    -vf 'fps=14,scale=900:-1:flags=lanczos,palettegen=stats_mode=diff' \
    -frames:v 1 -update 1 \
    "$PALETTE"
ffmpeg -y -i "$MP4" -i "$PALETTE" \
    -lavfi 'fps=14,scale=900:-1:flags=lanczos [v];[v][1:v]paletteuse=dither=bayer:bayer_scale=3' \
    "$GIF"

printf 'recorded %s at %s\n' "$DISPLAY" "$GEO"
ffprobe -v error -show_entries format=duration,size \
    -of default=noprint_wrappers=1 "$MP4"
printf 'gif_bytes=%s\n' "$(stat -c %s "$GIF")"
