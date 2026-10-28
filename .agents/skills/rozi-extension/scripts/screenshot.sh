#!/usr/bin/env bash
# Capture a gallery screenshot of an extension in a real rozi UI, at an exact terminal size.
#
#   screenshot.sh --extension DIR --scene SCENE.sh --output docs/screenshot.webp
#                 [--size 160x50] [--scale 2] [--config EXTRA.toml] [--session NAME] [--keep]
#
# The extension is linked into a throwaway lab: HOME, every XDG directory, and TMPDIR live in a
# temporary directory that is removed afterwards, so your config, sessions, and history are never
# read or written. The UI runs in a pane of a second, detached rozi session that has no UI of its
# own, started at exactly --size cells; the shot is `rozi capture-ui --render png --scale N` of it.
#
# SCENE.sh is sourced once the UI is attached, with the lab environment, `ROZI=1`, and the UI's
# `ROZI_SOCKET` exported, so plain `rozi split`, `rozi run-action`, and `rozi send-keys` drive it.
# It may also call:
#   keys KEY...    type into the UI, as `rozi send-keys` reads keys (`C-a`, `Enter`, `Escape`, text)
#   settle MS      wait for the UI to finish drawing
#   bin NAME BODY  write an executable stub NAME on the lab PATH with BODY as its script
# and read $LAB, $DEMO (a project directory to start panes in), $EXTENSION, and $ROZI_BIN. A scene
# that starts a daemon defines a `teardown` function, which runs before the lab is removed.
#
# ROZI_BIN selects the rozi binary (default: `rozi` on PATH). Inside the lab, `rozi` is that binary
# for the scene and the extension alike. It must accept every manifest key the extension uses.
set -euo pipefail

# Start from an empty environment: a nested rozi would otherwise talk to the outer UI, programs
# would detect your terminal, and the shot would show your prompt, paths, and settings.
if [ -z "${ROZI_SHOT_CLEAN:-}" ]; then
    exec env -i ROZI_SHOT_CLEAN=1 PATH="$PATH" LANG="${LANG:-C.UTF-8}" \
        ROZI_BIN="$(command -v "${ROZI_BIN:-rozi}")" "$0" "$@"
fi

size=160x50
scale=2
session=demo
extension=
scene=
output=
extra_config=
keep=0

while [ $# -gt 0 ]; do
    case "$1" in
        --extension) extension=$2; shift 2 ;;
        --scene) scene=$2; shift 2 ;;
        --output) output=$2; shift 2 ;;
        --size) size=$2; shift 2 ;;
        --scale) scale=$2; shift 2 ;;
        --config) extra_config=$2; shift 2 ;;
        --session) session=$2; shift 2 ;;
        --keep) keep=1; shift ;;
        -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ -n "$extension" ] && [ -n "$scene" ] && [ -n "$output" ] || {
    echo "usage: screenshot.sh --extension DIR --scene SCENE.sh --output FILE" >&2
    exit 2
}
extension=$(cd "$extension" && pwd)
export EXTENSION=$extension
scene=$(cd "$(dirname "$scene")" && pwd)/$(basename "$scene")
case "$output" in /*) ;; *) output="$PWD/$output" ;; esac
export ROZI_BIN

LAB=$(mktemp -d "${TMPDIR:-/tmp}/rozi-shot.XXXXXX")
export LAB
# The session that holds the UI's terminal: a pane of exactly the requested size.
terminal="$session-terminal"
cleanup() {
    trap - EXIT HUP INT TERM
    if declare -F teardown >/dev/null; then teardown || true; fi
    "$ROZI_BIN" sessions kill "$terminal" >/dev/null 2>&1 || true
    "$ROZI_BIN" sessions kill "$session" >/dev/null 2>&1 || true
    if [ "$keep" = 1 ]; then echo "lab kept at $LAB" >&2; return; fi
    # A daemon the scene started may still be writing as it exits, and rozi keeps unpacked
    # extension bundles read-only.
    for _ in 1 2 3 4 5; do
        chmod -R u+w "$LAB" 2>/dev/null
        rm -rf "$LAB" 2>/dev/null
        # The session server writes its state once more as it exits.
        sleep 1
        [ -e "$LAB" ] || return
    done
    rm -rf "$LAB"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

export HOME="$LAB/home"
export XDG_CONFIG_HOME="$LAB/config"
export XDG_DATA_HOME="$LAB/data"
export XDG_STATE_HOME="$LAB/state"
export XDG_CACHE_HOME="$LAB/cache"
export XDG_RUNTIME_DIR="$LAB/runtime"
export TMPDIR="$LAB/tmp"
export PATH="$LAB/bin:$PATH"
export SHELL=/bin/bash
export TERM=xterm-256color COLORTERM=truecolor
export DEMO="$HOME/src/demo"
mkdir -p "$HOME" "$XDG_CONFIG_HOME/rozi" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME" \
    "$XDG_RUNTIME_DIR" "$TMPDIR" "$LAB/bin" "$DEMO"
chmod 700 "$XDG_RUNTIME_DIR"
ln -s "$ROZI_BIN" "$LAB/bin/rozi"
cat > "$HOME/.bashrc" <<'EOF'
# The system bashrc titles panes user@host:dir. Report only the directory, so titles follow `cd`.
PROMPT_COMMAND='printf "\033]7;file://localhost%s\033\\" "$PWD"'
PS1='\[\e[35m\]\W\[\e[0m\] ❯ '
unset HISTFILE
EOF
{
    # A still frame must not catch an animation halfway.
    printf '[animations]\nsession = "off"\n'
    [ -n "$extra_config" ] && cat "$extra_config"
} > "$XDG_CONFIG_HOME/rozi/config.toml"

bin() {
    printf '#!/usr/bin/env bash\n%s\n' "$2" > "$LAB/bin/$1"
    chmod +x "$LAB/bin/$1"
}
keys() { "$ROZI_BIN" --session "$terminal" send-keys --target "$terminal_pane" "$@" >/dev/null; }
settle() { sleep "$(awk "BEGIN { print $1 / 1000 }")"; }

"$ROZI_BIN" extensions check "$extension" >/dev/null || "$ROZI_BIN" extensions check "$extension"
"$ROZI_BIN" extensions install --link "$extension" >/dev/null

# Both sessions start detached. The UI attaches from a pane of the terminal session, which rozi
# sizes exactly, and opens the scene session's first pane in $DEMO. That pane's own ROZI variables
# would make the UI a client of the terminal session, so they are cleared.
"$ROZI_BIN" sessions new "$session" --detach
"$ROZI_BIN" sessions new "$terminal" --detach
terminal_pane=$("$ROZI_BIN" --session "$terminal" split --size "$size" --cwd "$DEMO" --argv \
    bash -c 'unset "${!ROZI@}"; exec "$0" sessions attach "$1"' "$ROZI_BIN" "$session" |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["data"]["id"])')
for _ in $(seq 100); do
    ROZI_SOCKET=$(find "$XDG_RUNTIME_DIR" -name 'control-*.sock' -print -quit 2>/dev/null)
    [ -n "$ROZI_SOCKET" ] && break
    sleep 0.1
done
if [ -z "$ROZI_SOCKET" ]; then
    echo "the rozi UI did not start; its terminal shows:" >&2
    "$ROZI_BIN" --session "$terminal" capture-pane --target "$terminal_pane" --format text >&2 || true
    exit 1
fi
export ROZI=1 ROZI_SOCKET
settle 800

cd "$DEMO"
# shellcheck source=/dev/null
. "$scene"

mkdir -p "$(dirname "$output")"
png="$LAB/shot.png"
"$ROZI_BIN" capture-ui --render png --scale "$scale" --output "$png" >/dev/null
case "$output" in
    *.png) cp "$png" "$output" ;;
    *.webp) magick "$png" -quality 92 -define webp:method=6 "$output" ;;
    *.jpg|*.jpeg) magick "$png" -quality 90 "$output" ;;
    *) echo "unsupported output type: $output" >&2; exit 2 ;;
esac
bytes=$(wc -c < "$output" | tr -d ' ')
echo "$output: $(magick identify -format '%wx%h' "$output" 2>/dev/null || echo ?), $bytes bytes"
if [ "$bytes" -gt $((2 * 1024 * 1024)) ]; then
    echo "over the index's 2 MiB limit; use .webp or a smaller --scale" >&2
    exit 1
fi
