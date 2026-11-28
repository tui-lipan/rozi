#!/bin/sh
# Write a looping full-viewport video page for sustained browser graphics benchmarks.
#
# Usage: graphics-video.sh DIRECTORY [plain|noise]
#   plain  FFmpeg testsrc2: moving shapes and gradients, which zlib compresses well
#   noise  the same scene under per-frame noise, which it barely compresses
#
# Writes DIRECTORY/index.html and DIRECTORY/scene.webm: eight seconds of 1920x1080 VP9 at 60
# frames per second, muted and autoplaying, stretched to fill the browser viewport. Generate it
# before a measurement, then serve it over loopback, for example with
# `python3 -m http.server --bind 127.0.0.1 --directory DIRECTORY PORT`. Requires ffmpeg with
# libvpx-vp9. Encoding uses the realtime preset, so output bytes can differ between FFmpeg builds;
# record the scene's SHA-256 with each result.
set -eu
directory=$1 variant=${2:-plain}
case $variant in
  plain) filter="testsrc2=size=1920x1080:rate=60" bitrate=3M ;;
  noise) filter="testsrc2=size=1920x1080:rate=60,noise=alls=40:allf=t+u" bitrate=8M ;;
  *) echo "unknown variant: $variant" >&2; exit 2 ;;
esac
mkdir -p "$directory"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$filter" -t 8 -c:v libvpx-vp9 \
  -deadline realtime -cpu-used 8 -b:v "$bitrate" -an "$directory/scene.webm"
cat > "$directory/index.html" <<'HTML'
<!doctype html><meta charset="utf-8"><title>Graphics benchmark video</title>
<style>html,body{margin:0;background:#111;width:100%;height:100%;overflow:hidden}
video{width:100%;height:100%;object-fit:fill}</style>
<video autoplay muted loop playsinline src="scene.webm"></video>
HTML
