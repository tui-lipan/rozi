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

It requires Python 3 available as `python`. Validate and link the example checkout, then reload:

```bash
rozi extensions check ./activity-dashboard
rozi extensions install --link ./activity-dashboard
rozi run-action reload-extensions
rozi extensions list --verbose
```

Disabling or invalidating the extension terminates its service and withdraws its published rows.
