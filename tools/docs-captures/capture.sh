#!/usr/bin/env bash
# Regenerates the documentation screenshots in docs/assets/captures/.
#
# Usage: tools/docs-captures/capture.sh [scene-name ...]
#
# Every scene runs the real rozi binary headlessly in a throwaway sandbox with its own HOME and XDG
# directories, so nothing touches your config, sessions, or shell history. See README.md.
set -euo pipefail

repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
here=$repo/tools/docs-captures
out=${OUT:-$repo/docs/assets/captures}
# Full-quality PNGs, kept outside the repository for reviewing a run.
preview=${PREVIEW:-/tmp/rzc-preview}
bin=$repo/target/debug/rozi
# Unix socket paths are short-limited, so the sandbox lives directly under /tmp.
sandbox=/tmp/rzc
base=/tmp/rzc-base
only=" $* "

for tool in nvim lazygit btop eza bat bash git magick ffmpeg python3 tmux; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done

if [[ -z ${SKIP_BUILD:-} ]]; then
  cargo build --quiet --features ui-snapshot --manifest-path "$repo/Cargo.toml"
fi
mkdir -p "$out"

if [[ ! -d $base/.git ]]; then
  rm -rf "$base"
  git clone --quiet --depth 40 "file://$repo" "$base"
  git -C "$base" config user.name "rozi docs"
  git -C "$base" config user.email docs@example.invalid
fi

sandbox_env() {
  env -i \
    PATH="$sandbox/bin:$PATH" LANG=C.UTF-8 TERM=xterm-256color COLORTERM=truecolor \
    HOME="$sandbox/home" XDG_CONFIG_HOME="$sandbox/config" XDG_STATE_HOME="$sandbox/state" \
    XDG_CACHE_HOME="$sandbox/cache" XDG_DATA_HOME="$sandbox/data" XDG_RUNTIME_DIR="$sandbox/run" \
    "$@"
}

stop_sessions() {
  local names session
  names=$(sandbox_env "$bin" sessions list --format json 2>/dev/null |
    python3 -c 'import json, sys; print(" ".join(s["name"] for s in json.load(sys.stdin)))' || true)
  for session in $names; do
    sandbox_env "$bin" sessions kill "$session" >/dev/null 2>&1 || true
  done
}

prepare_sandbox() {
  stop_sessions
  rm -rf "$sandbox"
  mkdir -p "$sandbox"/{bin,config/rozi,state,cache,data,run,home/src}
  chmod 700 "$sandbox/run"
  # A `rozi` typed in a pane runs this build, not an installed one.
  ln -s "$bin" "$sandbox/bin/rozi"
  cp "$here/sandbox/bin/"* "$sandbox/bin/"
  cp -a "$base" "$sandbox/home/src/rozi"
  cp -r "$here/sandbox/nvim" "$here/sandbox/btop" "$here/sandbox/lazygit" "$sandbox/config/"
  cp -r "$here/sandbox/profiles" "$sandbox/config/rozi/"
  cp "$here/sandbox/home/.bashrc" "$sandbox/home/"
  bash "$here/sandbox/demo-change.sh" "$sandbox/home/src/rozi"
}

# [VAR=value ...] scene NAME VIEWPORT SETTLE_MS SCRIPT [EXTRA_CONFIG]
#
# SCRIPT is a TUI_LIPAN_SNAPSHOT_SCRIPT run after the workspace has settled; use `sleep:` for steps
# that wait on a program and `wait:` for rozi's own animations. EXTRA_CONFIG is appended to the
# generated config.toml, so it must not repeat `[session]` or `[profile]`.
#
# Optional variables:
#   PROFILE  profile launched at startup (default `dev`); STARTUP overrides `[session] startup`
#   LAYOUT   replaces the layout of the profile's first workspace
#   SETUP    shell code run before launch; its `rozi` function runs the sandboxed binary
scene() {
  local name=$1 viewport=$2 settle=$3 script=$4 extra=${5:-}
  [[ $only == "  " || $only == *" $name "* ]] || return 0
  prepare_scene "$extra"

  sandbox_env \
    TUI_LIPAN_SNAPSHOT="$sandbox/shot.png" \
    TUI_LIPAN_SNAPSHOT_VIEWPORT="$viewport" \
    TUI_LIPAN_SNAPSHOT_SETTLE_MS="$settle" \
    TUI_LIPAN_SNAPSHOT_SCRIPT="$script" \
    TUI_LIPAN_SNAPSHOT_ADVANCE_MS=1000 \
    timeout 120 "$bin" >"$sandbox/run.log" 2>&1 || { cat "$sandbox/run.log" >&2; exit 1; }
  stop_sessions

  magick "$sandbox/shot.png" -resize 1920x -quality 90 -define webp:method=6 "$out/$name.webp"
  mkdir -p "$preview"
  cp "$sandbox/shot.png" "$preview/$name.png"
  echo "captured $name"
}

# [VAR=value ...] clip NAME VIEWPORT SETTLE_MS STEPS [EXTRA_CONFIG]
#
# Runs rozi in a real terminal of VIEWPORT, records the whole UI with `rozi record start ui` while
# clip.py plays STEPS in real time, and encodes the recording as NAME.mp4 with a NAME-poster.webp.
# STEPS use clip.py's syntax. The variables are those of `scene`, plus POSTER, the second of the
# clip shown before it plays (default: its last frame).
clip() {
  local name=$1 viewport=$2 settle=$3 steps=$4 extra=${5:-}
  [[ $only == "  " || $only == *" $name "* ]] || return 0
  prepare_scene "$extra"

  local rec=$sandbox/clip.rozirec frames=$sandbox/frames
  sandbox_env timeout 300 python3 "$here/clip.py" "$sandbox/tmux.sock" \
    "${viewport%x*}" "${viewport#*x}" "$settle" \
    "run:rozi record start ui --output $rec >/dev/null
$steps
run:rozi record stop --ui >/dev/null" -- "$bin" \
    >"$sandbox/run.log" 2>&1 || { cat "$sandbox/run.log" >&2; exit 1; }
  stop_sessions

  sandbox_env "$bin" record export "$rec" --to png-frames "$frames" --scale 2 >/dev/null 2>&1
  ffmpeg -loglevel error -y -f concat -safe 0 -i "$frames/frames.ffconcat" \
    -vf "pad=ceil(iw/2)*2:ceil(ih/2)*2" -c:v libx264 -preset veryslow -tune animation -crf 24 \
    -pix_fmt yuv420p -fps_mode vfr -movflags +faststart "$out/$name.mp4"
  if [[ -n ${POSTER:-} ]]; then
    ffmpeg -loglevel error -y -ss "$POSTER" -i "$out/$name.mp4" -frames:v 1 "$sandbox/poster.png"
  else
    ffmpeg -loglevel error -y -sseof -0.1 -i "$out/$name.mp4" -frames:v 1 -update 1 "$sandbox/poster.png"
  fi
  magick "$sandbox/poster.png" -quality 90 -define webp:method=6 "$out/$name-poster.webp"
  mkdir -p "$preview"
  cp "$out/$name.mp4" "$sandbox/poster.png" "$preview/" && mv "$preview/poster.png" "$preview/$name-poster.png"
  echo "recorded $name"
}

# Fresh sandbox, the profile, config.toml with EXTRA_CONFIG, and SETUP, for `scene` and `clip`.
prepare_scene() {
  local extra=$1
  local profile=${PROFILE:-dev} startup=${STARTUP:-profile}
  prepare_sandbox
  if [[ -n ${LAYOUT:-} ]]; then
    sed -i "0,/^layout = .*/s//layout = \"$LAYOUT\"/" "$sandbox/config/rozi/profiles/$profile.toml"
  fi
  cat >"$sandbox/config/rozi/config.toml" <<EOF
cwd = "~/src/rozi"
shell = "bash"

[session]
startup = "$startup"
resurrect = false

[profile]
default = "$profile"

[updates]
check = false

$extra
EOF
  if [[ -n ${SETUP:-} ]]; then
    bash -c "$(declare -f sandbox_env); bin=$bin; sandbox=$sandbox; rozi() { sandbox_env \"\$bin\" \"\$@\"; }; $SETUP"
  fi
}

source "$here/scenes.sh"
stop_sessions
rm -rf "$sandbox"
