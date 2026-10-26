#!/usr/bin/env bash
# Usage: frame.sh CLIP OUT TITLE
#
# Shows a clip as a terminal window with a title bar and window controls. CLIP is a recording's
# frames.ffconcat or a video file.
#
# OUT ending in .mp4 is the window alone, 2560 wide at 60 fps, for the landing page, which draws
# the corners, border, shadow, and glow itself so they can spread past the video's edges. OUT
# ending in .gif is a 1440-wide GIF for the README, which has no styling of its own, so the GIF
# carries all of it: rounded corners, a drop shadow, and the rozi glow on the site's background.
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
ch=$((cw * 9 / 16)) bar=$(s 34) dot=$(s 6)
font=$(fc-match -f '%{file}' monospace 2>/dev/null || true)

# The title bar across WIDTH, with its left edge at X and top at Y, drawn onto the image in $1.
title_bar() {
  local image=$1 x=$2 y=$3 width=$4 mid=$(($3 + bar / 2))
  magick "$image" -fill '#1a1724' -draw "rectangle $x,$y,$((x + width - 1)),$((y + bar - 1))" \
    -fill '#2c2639' -draw "rectangle $x,$((y + bar - $(s 1))),$((x + width - 1)),$((y + bar - 1))" \
    -fill '#ff5f57' -draw "circle $((x + $(s 22))),$mid $((x + $(s 22) + dot)),$mid" \
    -fill '#febc2e' -draw "circle $((x + $(s 42))),$mid $((x + $(s 42) + dot)),$mid" \
    -fill '#28c840' -draw "circle $((x + $(s 62))),$mid $((x + $(s 62) + dot)),$mid" \
    ${font:+-font "$font"} -pointsize "$(s 15)" -fill '#a59fb6' -gravity north \
    -annotate "+$((x + width / 2 - $(magick identify -format %w "$image") / 2))+$((mid - $(s 10)))" "$title" \
    "$image"
}

if [[ $out == *.mp4 ]]; then
  magick -size "${cw}x$((bar + ch))" xc:'#0f0d16' "$work/back.png"
  title_bar "$work/back.png" 0 0 "$cw"
  # Constant 60 fps at level 5.1, as capture.sh writes its clips.
  ffmpeg -loglevel error -y "${input[@]}" -loop 1 -i "$work/back.png" \
    -filter_complex "[0]fps=60,scale=$cw:$ch:flags=area[c];[1][c]overlay=0:$bar:shortest=1,format=yuv420p" \
    -c:v libx264 -preset veryslow -tune animation -crf 24 -level:v 5.1 -movflags +faststart "$out"
  exit
fi

radius=$(s 12) line=$(s 1)
ww=$((cw + 2 * line)) wh=$((bar + ch + 2 * line))
side=$(s 56) top=$(s 44) bottom=$(s 72)
w=$((ww + 2 * side)) h=$((wh + top + bottom))
x0=$side y0=$top x1=$((side + ww - 1)) y1=$((top + wh - 1))
glow=$(s 110) shadow=$(s 26) drop=$(s 18)

magick -size "${w}x${h}" xc:'#06070f' \
  \( -size "${w}x${h}" xc:none -fill 'rgba(253,74,128,0.34)' \
     -draw "circle $((w / 5)),$((h / 4)) $((w / 5 + h / 2)),$((h / 4))" -blur "0x$glow" \) -composite \
  \( -size "${w}x${h}" xc:none -fill 'rgba(152,43,242,0.38)' \
     -draw "circle $((w * 4 / 5)),$((h * 3 / 4)) $((w * 4 / 5 + h / 2)),$((h * 3 / 4))" -blur "0x$glow" \) -composite \
  \( -size "${w}x${h}" xc:none -fill 'rgba(0,0,0,0.8)' \
     -draw "roundrectangle $x0,$((y0 + drop)),$x1,$((y1 + drop)),$radius,$radius" -blur "0x$shadow" \) -composite \
  -fill '#0f0d16' -stroke '#3b3150' -strokewidth "$line" \
  -draw "roundrectangle $x0,$y0,$x1,$y1,$radius,$radius" "$work/base.png"

# The base with the window body cut out, laid over the clip: it rounds the window's corners,
# including the title bar's, and keeps the border on top.
magick "$work/base.png" \
  \( -size "${w}x${h}" xc:white -fill black \
     -draw "roundrectangle $((x0 + line)),$((y0 + line)),$((x1 - line)),$((y1 - line)),$((radius - line)),$((radius - line))" \) \
  -alpha off -compose CopyOpacity -composite "$work/over.png"
cp "$work/base.png" "$work/back.png"
title_bar "$work/back.png" $((x0 + line)) $((y0 + line)) "$cw"

# 25 fps, the smoothest rate GIF delays (in hundredths of a second) hold evenly, and no dither:
# it would speckle the flat pane backgrounds. Area scaling keeps glyph edges from ringing into
# colours the palette would then have to spend itself on.
ffmpeg -loglevel error -y "${input[@]}" -loop 1 -i "$work/back.png" -loop 1 -i "$work/over.png" \
  -filter_complex "[0]fps=25,scale=$cw:$ch:flags=area[c];[1][c]overlay=$((x0 + line)):$((y0 + line + bar)):shortest=1[b];[b][2]overlay=shortest=1,split[a][p];[p]palettegen=max_colors=256:stats_mode=full[pal];[a][pal]paletteuse=dither=none:diff_mode=rectangle" \
  "$out"
