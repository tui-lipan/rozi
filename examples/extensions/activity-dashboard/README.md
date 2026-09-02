# Activity dashboard extension

This example exercises the long-running extension contract without importing Rozi internals:

- `[[services]]` supervises the dashboard process;
- `rozi subscribe` supplies live events;
- `rozi publish` maintains an actionable sidebar row;
- `rozi notify` reports blocked work;
- activating the published row raises `rozi pick`;
- `activity-dashboard.open` opens the same picker directly.

Recent events are written atomically beside the installed extension, so a supervised restart or
config reload retains the dashboard history. Rozi intentionally does not watch extension trees.

It requires Python 3 available as `python`. Install the directory manually, validate it, and reload:

```bash
rozi extensions check ./activity-dashboard
cp -R ./activity-dashboard \
  "${XDG_DATA_HOME:-$HOME/.local/share}/rozi/extensions/activity-dashboard"
rozi run-action reload-extensions
rozi extensions list --verbose
```

Disabling or invalidating the extension terminates its service and withdraws its published rows.
