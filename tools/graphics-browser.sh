#!/bin/sh
# Open terminal-browser as a benchmark producer in the current pane.
#
# Usage: graphics-browser.sh BROWSER FPS PRESENTER URL
#   BROWSER    terminal-browser launcher (bin/terminal-browser of an installation)
#   FPS        render.fps for the browser, matching the client's frame rate
#   PRESENTER  render.presenter: auto (normal patches) or full
#   URL        a page URL, or "generated" for tools/graphics-page.html over loopback
#
# Settings are written to the browser's configuration in the caller's XDG_CONFIG_HOME, so run it
# only with the benchmark's isolated directories. GPU rendering is disabled for repeatable runs.
#
# GRAPHICS_BROWSER_PROFILE=SOURCE:DESTINATION first copies an existing profile, such as one that
# already dismissed a page's cookie notice. SOURCE is the install suffix of the profile under the
# user's ~/.config, ~/.local/share and ~/.local/state (terminal-browser-SOURCE), DESTINATION that of
# the browser being run. The copy is private to this run; delete it afterwards.
set -eu
browser=$1 fps=$2 presenter=$3 url=$4
if [ -n "${GRAPHICS_BROWSER_PROFILE:-}" ]; then
  source=${GRAPHICS_BROWSER_PROFILE%%:*} destination=${GRAPHICS_BROWSER_PROFILE#*:}
  for isolated in "$XDG_CONFIG_HOME:$HOME/.config" "$XDG_DATA_HOME:$HOME/.local/share" \
    "$XDG_STATE_HOME:$HOME/.local/state"; do
    if [ "${isolated%%:*}" = "${isolated#*:}" ]; then
      echo "refusing to copy a profile without isolated XDG directories" >&2
      exit 1
    fi
  done
  mkdir -p "$XDG_CONFIG_HOME/terminal-browser"
  if [ -f "$HOME/.config/terminal-browser/settings.json" ]; then
    cp "$HOME/.config/terminal-browser/settings.json" "$XDG_CONFIG_HOME/terminal-browser/"
  fi
  for pair in "$HOME/.config:$XDG_CONFIG_HOME" "$HOME/.local/share:$XDG_DATA_HOME" \
    "$HOME/.local/state:$XDG_STATE_HOME"; do
    from="${pair%%:*}/terminal-browser-$source" to="${pair#*:}/terminal-browser-$destination"
    if [ -d "$from" ]; then
      rm -rf "$to"
      cp -a "$from" "$to"
      rm -f "$to"/Singleton*
    fi
  done
fi
for setting in "render.fps $fps" "render.presenter $presenter" "telemetry.usage off" \
  "telemetry.crashReports off" "updates.check off"; do
  # shellcheck disable=SC2086 # each setting is a key and a value
  "$browser" config set $setting >/dev/null
done
if [ "$url" = generated ]; then
  port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  python3 -m http.server --bind 127.0.0.1 --directory "$(dirname "$0")" "$port" >/dev/null 2>&1 &
  sleep 0.5
  url="http://127.0.0.1:$port/graphics-page.html"
fi
TERMINAL_BROWSER_DISABLE_GPU=1 exec "$browser" open "$url" --no-merge
