#!/usr/bin/env bash
# Usage: frame.sh CLIP OUT TITLE
#
# Shows a clip as a terminal window: a title bar with window controls, rounded corners, a drop
# shadow, and the rozi glow behind it, on the site's background colour. CLIP is a recording's
# frames.ffconcat or a video file. OUT ending in .gif is a 1440-wide GIF for the README; OUT ending
# in .mp4 is a 2560-wide, 60 fps video for the landing page. The backdrop and window are drawn
# once and the frames are composited into the window body.
set -euo pipefail
clip=$1 out=$2 title=$3
input=(-i "$clip")
[[ $clip == *.ffconcat ]] && input=(-f concat -safe 0 -i "$clip")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

case $out in
  *.gif) cw=1440 ;;
  *.mp4) cw=2560 ;;
  *) echo "frame.sh: OUT must end in .gif or .mp4" >&2; exit 2 ;;
esac
# Chrome is laid out for a 1440-wide body and scaled with it.
s() { echo $(($1 * cw / 1440)); }
ch=$((cw * 9 / 16)) bar=$(s 34) radius=$(s 12) line=$(s 1)
ww=$((cw + 2 * line)) wh=$((bar + ch + 2 * line))
side=$(s 56) top=$(s 44) bottom=$(s 72)
w=$((ww + 2 * side)) h=$((wh + top + bottom))
x0=$side y0=$top x1=$((side + ww - 1)) y1=$((top + wh - 1))
cx=$((x0 + line)) cy=$((y0 + line + bar))
glow=$(s 110) shadow=$(s 26) drop=$(s 18) dot=$(s 6) mid=$((y0 + bar / 2))
font=$(fc-match -f '%{file}' monospace 2>/dev/null || true)

magick -size "${w}x${h}" xc:'#06070f' \
  \( -size "${w}x${h}" xc:none -fill 'rgba(253,74,128,0.34)' \
     -draw "circle $((w / 5)),$((h / 4)) $((w / 5 + h / 2)),$((h / 4))" -blur "0x$glow" \) -composite \
  \( -size "${w}x${h}" xc:none -fill 'rgba(152,43,242,0.38)' \
     -draw "circle $((w * 4 / 5)),$((h * 3 / 4)) $((w * 4 / 5 + h / 2)),$((h * 3 / 4))" -blur "0x$glow" \) -composite \
  \( -size "${w}x${h}" xc:none -fill 'rgba(0,0,0,0.8)' \
     -draw "roundrectangle $x0,$((y0 + drop)),$x1,$((y1 + drop)),$radius,$radius" -blur "0x$shadow" \) -composite \
  -fill '#0f0d16' -stroke '#3b3150' -strokewidth "$line" \
  -draw "roundrectangle $x0,$y0,$x1,$y1,$radius,$radius" \
  -stroke none -fill '#1a1724' \
  -draw "roundrectangle $((x0 + line)),$((y0 + line)),$((x1 - line)),$((y0 + bar + radius)),$((radius - line)),$((radius - line))" \
  -fill '#0f0d16' -draw "rectangle $((x0 + line)),$((y0 + bar)),$((x1 - line)),$((y0 + bar + radius + line))" \
  -fill '#2c2639' -draw "rectangle $((x0 + line)),$((y0 + bar - line)),$((x1 - line)),$((y0 + bar - 1))" \
  -fill '#ff5f57' -draw "circle $((x0 + $(s 22))),$mid $((x0 + $(s 22) + dot)),$mid" \
  -fill '#febc2e' -draw "circle $((x0 + $(s 42))),$mid $((x0 + $(s 42) + dot)),$mid" \
  -fill '#28c840' -draw "circle $((x0 + $(s 62))),$mid $((x0 + $(s 62) + dot)),$mid" \
  ${font:+-font "$font"} -pointsize "$(s 15)" -fill '#a59fb6' -gravity north \
  -annotate "+0+$((mid - $(s 10)))" "$title" \
  "$work/back.png"

# The same picture with the window body cut out, laid over the clip to round its lower corners
# and keep the border on top.
magick "$work/back.png" \
  \( -size "${w}x${h}" xc:white -fill black \
     -draw "roundrectangle $((x0 + line)),$((y0 + line)),$((x1 - line)),$((y1 - line)),$((radius - line)),$((radius - line))" \) \
  -alpha off -compose CopyOpacity -composite "$work/over.png"

compose="scale=$cw:$ch:flags=area[c];[1][c]overlay=$cx:$cy:shortest=1[b];[b][2]overlay=shortest=1"
if [[ $out == *.gif ]]; then
  # 25 fps, the smoothest rate GIF delays (in hundredths of a second) hold evenly, and no
  # dither: it would speckle the flat pane backgrounds. Area scaling keeps glyph
  # edges from ringing into colours the palette would then have to spend itself on.
  ffmpeg -loglevel error -y "${input[@]}" -loop 1 -i "$work/back.png" -loop 1 -i "$work/over.png" \
    -filter_complex "[0]fps=25,$compose,split[a][p];[p]palettegen=max_colors=256:stats_mode=full[pal];[a][pal]paletteuse=dither=none:diff_mode=rectangle" \
    "$out"
else
  # Constant 60 fps at level 5.1, as capture.sh writes its clips.
  ffmpeg -loglevel error -y "${input[@]}" -loop 1 -i "$work/back.png" -loop 1 -i "$work/over.png" \
    -filter_complex "[0]fps=60,$compose,pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p" \
    -c:v libx264 -preset veryslow -tune animation -crf 24 -level:v 5.1 -movflags +faststart "$out"
fi
