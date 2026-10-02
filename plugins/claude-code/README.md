# Claude Code activity plugin

This Claude Code plugin reports its activity directly to Rozi's Activity sidebar and Agents view.
It also reports the native conversation ID so Rozi can reopen that conversation with
`claude --resume` when you restore a session.

## Install

Use a Rozi version with `rozi agents report` and `rozi agents release`. You also need a current
Claude Code with exec-form command hooks and `CLAUDE_PLUGIN_DATA`, and Python 3.10 or newer
available as `python` on `PATH`. The plugin validates with Claude Code 2.1.286.

In Claude Code, add this repository as a marketplace and install the plugin:

```text
/plugin marketplace add tui-lipan/rozi
/plugin install rozi@rozi
```

Restart Claude Code inside a local Rozi pane. The plugin starts reporting automatically.
It does not require a Rozi extension or edits to Claude's `settings.json`.

To try a checkout before installing it, run this from the repository root inside a Rozi pane:

```sh
claude --plugin-dir ./plugins/claude-code
```

Remove an installed copy with `/plugin uninstall rozi@rozi`, then restart Claude Code.
For pane and session control, install Rozi's separate [agent skill](../../docs/agent-skill.md).

For multiple conversations and switching between them, also install the
[claude-rozi-sessions extension](https://github.com/tui-lipan/claude-rozi-sessions).
With rozi 0.0.28 or newer, the hook report supplies the live state of the row
with the same conversation ID, while the extension lists the other conversations and handles row
selection. The plugin works on its own when you only need activity for the main conversation.

## Activity states

| Claude Code event | Rozi state |
| --- | --- |
| Session starts or resumes | `idle`, with the native conversation ID |
| Prompt submitted or tool running | `working` |
| Permission requested | `blocked` |
| `AskUserQuestion` or MCP input requested | `blocked` |
| Tool finishes, fails, or MCP input returns | `working` unless another input wait remains |
| Parallel tool batch resolves | `working`, clearing unresolved batch waits |
| Response finishes without background tasks or scheduled wakeups | `done` |
| Response finishes with background tasks or scheduled wakeups | `working` |
| Compaction | `working`, keeping the same integration token |
| Turn fails because of an API error | `idle`, with an error reason |
| Session ends | Releases the report; Rozi returns to screen detection |

Permission and MCP dialog notifications also report `blocked`. Authentication, delayed idle
notifications, and notifications about hidden sessions do not change the parent pane's state.
Subagent-local hooks are ignored so a child's completion cannot mark its parent done. The plugin
reports one activity for the main conversation; it does not create separate child rows.

Parallel tool calls cannot clear each other's input waits. The plugin tracks questions by
`tool_use_id` and MCP requests by server and elicitation ID where provided. Permission hooks
normally omit the tool ID, so those waits remain blocked until `PostToolBatch` confirms the whole
batch resolved, or the turn ends. A completion of an unrelated tool keeps the conversation blocked.

Hooks call the matching `ROZI_BIN` executable through structured arguments. A unique integration
token and increasing sequence numbers fence each reporting lifecycle. Switching conversations
releases the previous token before claiming a new one. Events from a different conversation or an
already-ended lifecycle cannot update that lifecycle. Hooks record events and outstanding waits
in short SQLite transactions before delivering them. A separate sender lock serializes delivery
without preventing parallel hooks from recording their state. Pending snapshots may be coalesced;
the newest state keeps all outstanding waits.

Conversation changes persist the new lifecycle and the old token's pending release together.
The sender retries that release before claiming the new token. CLI errors and timeouts leave the
transition pending, so later hooks can recover without restarting Claude. A release already
accepted by rozi is recognised on retry. If no later hook runs, pending delivery waits until one
does; the plugin does not run a background retry service.

The plugin keeps tokens, conversation IDs, sequence numbers, pending states and releases, and
input-wait identifiers and reasons under
`CLAUDE_PLUGIN_DATA/activity`. It does not read transcripts or store prompts, tool arguments, or
notification text. Individual rozi calls time out after 750 ms, with a 900 ms delivery budget per
hook. Hooks produce no output or permission
decisions, and failures never prevent Claude from continuing.

Reporting requires the pane's `ROZI_SOCKET`, `ROZI_PANE`, and `ROZI_SESSION_INSTANCE`. Remote and
headless panes without a local UI endpoint keep using Rozi's screen detection. If the UI endpoint
goes away, hook reports cannot reach it. Restart Claude Code in an attached pane to begin a new
reporting lifecycle. Removing the plugin while Claude is running also requires ending that
Claude session to release its last report.

Use the Rozi build containing this plugin for session switching: it routes reports to the
caller's session even when the UI shows another attached session. Earlier versions may route a
report to the session on screen instead.

## Verify

From the repository root:

```sh
claude plugin validate --strict ./plugins/claude-code
claude plugin validate --strict ./.claude-plugin/marketplace.json
python -m unittest discover -s plugins/claude-code/tests -v
```

For a live check, launch Claude with the plugin inside a Rozi pane. From another pane run
`rozi agents list --format json`. Confirm these transitions:

1. The new session reports `idle` and its native conversation ID.
2. Submit a prompt. Its state becomes `working`.
3. Request a tool that needs permission. Its state becomes `blocked` until you answer.
4. Let the response finish. Its state becomes `done`, unless background work remains.
5. Run `/compact`, `/clear`, and `/resume`. Compaction preserves reporting; conversation changes
   report the new native ID.
6. Exit Claude. Its report is released and its activity disappears when the process exits.

The hook and plugin formats follow the [Claude Code hooks reference](https://code.claude.com/docs/en/hooks)
and [plugin reference](https://code.claude.com/docs/en/plugins-reference). Herdr's
[Claude integration](https://github.com/hdosys/herdr-ext/tree/master/src/integration/assets/claude)
informed native identity reporting; the separate
[Claude lifecycle plugin](https://github.com/daocoding/herdr-claude-lifecycle) informed the activity
mapping. This plugin uses Rozi's public CLI rather than Herdr's socket protocol.
