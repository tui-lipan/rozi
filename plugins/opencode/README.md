# OpenCode activity in rozi

This package connects OpenCode V2 to rozi: it publishes each open session tab as a separate agent
row.

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

The plugin launches the matching `ROZI_BIN publish --kind agent --name opencode` bridge, passes
the pane's environment, and keeps reading activation requests. Its rows are agents: they appear in
rozi's Agents view and `rozi agents`, and replace screen detection for the pane while listed. It sends changed snapshots and coalesces them if the bridge
cannot keep up. Plugin unload, bridge failure, or client exit closes the stream and withdraws its
rows. Outside a pane with the current rozi environment contract, the plugin does nothing.

The CLI process owns the bridge, so a shared OpenCode server never chooses a rozi pane for other
clients. A CLI plugin can also connect to a remote OpenCode server, but worktree grouping requires
the reported session directories to exist on the rozi pane's host.

The V2 integration targets the public CLI plugin contract tested with OpenCode `2.0.22`.
Restart the rozi session server after upgrading from a version that does not stamp
`ROZI_SESSION_INSTANCE` into panes.

## Read responses

`rozi agents prompt --ref <JSON> "Review this diff" --wait turn --timeout 10m --result` returns
completed assistant text through OpenCode's message API, including background tabs. Use a
reference from `rozi agents list` to address a particular session. Markdown and Unicode do not
depend on the pane dimensions. JSON reports `source: "native-api"` and `fidelity: "semantic"`.
A failed message fetch or interrupted turn may have no response. Text over 1 MiB has
`truncated: true` and `complete: false`.

## Verify

```sh
node --test plugins/opencode/*.test.mjs
```

Tests cover client ownership, per-session directories and IDs, child activity, concurrent waits,
background completion, activation, withdrawal, bridge failure, and backpressure. The Cargo
contracts suite runs these tests when Node is available, including on the primary CI platforms.
