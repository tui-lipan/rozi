# OpenCode activity in rozi

This package connects OpenCode to rozi. OpenCode V2 publishes each open session tab as a separate
activity row. The V1 event plugin reports one aggregate status for the pane.

## Install for OpenCode V2

Add the absolute path to this directory to `plugins` in your OpenCode `cli.json`:

```json
{
  "plugins": ["/absolute/path/to/rozi/plugins/opencode"],
  "tabs": { "mode": "on" }
}
```

Keep any other entries already in that file. A global configuration normally lives at
`~/.config/opencode/cli.json`; a project configuration lives at `.opencode/cli.json`.
See [OpenCode's CLI plugin documentation](https://opencode.ai/v2/docs/build/plugins/cli/).
This package has no npm dependencies. The root `index.js` and `tui.js` entrypoints support
OpenCode's local-directory loader as well as the package exports.

Start OpenCode inside a rozi pane. Open sessions with OpenCode's **Switch session** command.
Each open root-session tab gets a row, including background tabs. Selecting a row in rozi focuses
that tab through OpenCode's UI API. With tabs disabled, the plugin publishes only the currently
viewed root session.

Each row carries its native session ID and its session's directory. rozi uses that directory to
group the row under the repository and branch it works in, including linked Git worktrees. The
plugin does not create or remove worktrees. OpenCode remains responsible for those operations.

A pending permission or input form blocks its session's row. Child-session requests and running
children contribute to their root session's state. Global MCP forms block rows at the form's
location. A background session that completes stays done until its tab is viewed. Closing a tab
withdraws its row. OpenCode's own tab persistence may synchronize tabs between clients using the
same state directory; this plugin always follows the tab list of the client it runs in.

The plugin launches the matching `ROZI_BIN publish` bridge, passes the pane's environment, and
keeps reading activation requests. It sends changed snapshots and coalesces them if the bridge
cannot keep up. Plugin unload, bridge failure, or client exit closes the stream and withdraws its
rows. Outside a pane with the current rozi environment contract, the plugin does nothing.

The CLI process owns the bridge, so a shared OpenCode server never chooses a rozi pane for other
clients. A CLI plugin can also connect to a remote OpenCode server, but worktree grouping requires
the reported session directories to exist on the rozi pane's host.

The V2 integration targets the public CLI plugin contract tested with OpenCode `2.0.22`.
Restart the rozi session server after upgrading from a version that does not stamp
`ROZI_SESSION_INSTANCE` into panes.

## Install for OpenCode V1

Copy `rozi-agent-state.js` into your project's `.opencode/plugins/` directory or the global
`~/.config/opencode/plugins/` directory. OpenCode V1 loads the exported event plugin automatically.

The V1 plugin aggregates sessions into one pane status, with blocked sessions taking precedence
over working sessions. It does not publish session rows, directories, native IDs, or activation.
Its implementation uses the legacy Unix control socket; use the V2 CLI plugin for portable
publishing. Do not copy the V1 file into a V2 plugin directory.

## Verify

```sh
node --test plugins/opencode/*.test.mjs
```

Tests cover client ownership, per-session directories and IDs, child activity, concurrent waits,
background completion, activation, withdrawal, bridge failure, and backpressure. The Cargo
contracts suite runs these tests when Node is available, including on the primary CI platforms.
