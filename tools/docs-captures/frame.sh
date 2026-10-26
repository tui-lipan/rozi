#!/usr/bin/env bash
# Usage: frame.sh CLIP OUT.gif TITLE
#
# CLIP is a recording's frames.ffconcat or a video file.
#
# Encodes a clip's frames as a GIF of a terminal window: a title bar with window controls, rounded
# corners, a drop shadow, and the rozi glow behind it. The backdrop and window are drawn once, the
# frames are composited into the window body, and one palette serves the whole GIF.
set -euo pipefail
clip=$1 out=$2 title=$3
input=(-i "$clip")
[[ $clip == *.ffconcat ]] && input=(-f concat -safe 0 -i "$clip")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Content size, then the window and the canvas around it.
cw=1440 ch=810 bar=34 radius=12
ww=$((cw + 2)) wh=$((bar + ch + 2))
side=56 top=44 bottom=72
w=$((ww + 2 * side)) h=$((wh + top + bottom))
x0=$side y0=$top x1=$((side + ww - 1)) y1=$((top + wh - 1))
cx=$((x0 + 1)) cy=$((y0 + 1 + bar))
font=$(fc-match -f '%{file}' monospace 2>/dev/null || true)

magick -size "${w}x${h}" xc:'#0b0910' \
  \( -size "${w}x${h}" xc:none -fill 'rgba(253,74,128,0.34)' \
     -draw "circle $((w / 5)),$((h / 4)) $((w / 5 + h / 2)),$((h / 4))" -blur 0x110 \) -composite \
  \( -size "${w}x${h}" xc:none -fill 'rgba(152,43,242,0.38)' \
     -draw "circle $((w * 4 / 5)),$((h * 3 / 4)) $((w * 4 / 5 + h / 2)),$((h * 3 / 4))" -blur 0x110 \) -composite \
  \( -size "${w}x${h}" xc:none -fill 'rgba(0,0,0,0.8)' \
     -draw "roundrectangle $x0,$((y0 + 18)),$x1,$((y1 + 18)),$radius,$radius" -blur 0x26 \) -composite \
  -fill '#0f0d16' -stroke '#3b3150' -strokewidth 1 \
  -draw "roundrectangle $x0,$y0,$x1,$y1,$radius,$radius" \
  -stroke none -fill '#1a1724' \
  -draw "roundrectangle $((x0 + 1)),$((y0 + 1)),$((x1 - 1)),$((y0 + bar + radius)),$((radius - 1)),$((radius - 1))" \
  -fill '#0f0d16' -draw "rectangle $((x0 + 1)),$((y0 + bar)),$((x1 - 1)),$((y0 + bar + radius + 1))" \
  -fill '#2c2639' -draw "line $((x0 + 1)),$((y0 + bar)) $((x1 - 1)),$((y0 + bar))" \
  -fill '#ff5f57' -draw "circle $((x0 + 22)),$((y0 + bar / 2)) $((x0 + 28)),$((y0 + bar / 2))" \
  -fill '#febc2e' -draw "circle $((x0 + 42)),$((y0 + bar / 2)) $((x0 + 48)),$((y0 + bar / 2))" \
  -fill '#28c840' -draw "circle $((x0 + 62)),$((y0 + bar / 2)) $((x0 + 68)),$((y0 + bar / 2))" \
  ${font:+-font "$font"} -pointsize 15 -fill '#a59fb6' -gravity north \
  -annotate "+0+$((y0 + bar / 2 - 10))" "$title" \
  "$work/back.png"

# The same picture with the window body cut out, laid over the clip to round its lower corners
# and keep the border on top.
magick "$work/back.png" \
  \( -size "${w}x${h}" xc:white -fill black \
     -draw "roundrectangle $((x0 + 1)),$((y0 + 1)),$((x1 - 1)),$((y1 - 1)),$((radius - 1)),$((radius - 1))" \) \
  -alpha off -compose CopyOpacity -composite "$work/over.png"

# 16 fps, and no dither: it would speckle the flat pane backgrounds. Area scaling keeps glyph
# edges from ringing into colours the palette would then have to spend itself on.
ffmpeg -loglevel error -y "${input[@]}" -loop 1 -i "$work/back.png" -loop 1 -i "$work/over.png" \
  -filter_complex "[0]fps=16,scale=$cw:$ch:flags=area[c];[1][c]overlay=$cx:$cy:shortest=1[b];[b][2]overlay=shortest=1,split[a][p];[p]palettegen=max_colors=256:stats_mode=full[pal];[a][pal]paletteuse=dither=none:diff_mode=rectangle" \
  "$out"
