# Codex activity plugin

This Codex plugin reports its activity directly to Rozi's Activity sidebar and Agents view.
It also reports the native thread ID so Rozi can reopen that thread with `codex resume` when you
restore a session.

## Install

Use a Rozi version with `rozi agents report` and `rozi agents release`. You also need Codex 0.160
or newer with hooks enabled, which is the default, and Python 3.10 or newer available as `python3`
on `PATH`. The plugin validates with Codex 0.160.0.

Add this repository as a Codex plugin marketplace and install the plugin:

```sh
codex plugin marketplace add tui-lipan/rozi
codex plugin add rozi@rozi
```

Restart Codex inside a local Rozi pane. Codex asks you to review hooks that are new or changed:
choose **Trust all and continue**, or review the ten `rozi` hooks with `/hooks` and trust them
there. Until you trust them, they do not run. Codex asks again after an update changes them.
The plugin does not require a Rozi extension or edits to Codex's `config.toml` or `hooks.json`.

To try a checkout before installing it, add the checkout as a local marketplace from the
repository root:

```sh
codex plugin marketplace add .
codex plugin add rozi@rozi
```

Remove an installed copy with `codex plugin remove rozi@rozi`, then restart Codex.
For pane and session control, install Rozi's separate [agent skill](../../docs/agent-skill.md).

For a single Codex client running several threads at once, also install the
[codex-rozi-sessions extension](https://github.com/tui-lipan/codex-rozi-sessions). It lists every
thread loaded in Codex's background server as its own row, and the hook report supplies the live
state of the row with the same thread ID. The plugin works on its own when you only need activity
for the thread on screen.

## Activity states

| Codex event | Rozi state |
| --- | --- |
| First prompt of a new or resumed thread | `idle`, with the native thread ID, then `working` |
| Prompt submitted or tool running | `working` |
| Approval requested | `blocked` |
| `request_user_input` question asked | `blocked` |
| Tool finishes | `working` unless another approval or question remains |
| Compaction | `working`, keeping the same integration token |
| Turn finishes | `done` |
| Turn interrupted, including by declining an approval with `Esc` | `idle` |
| Codex exits | Releases the report; Rozi returns to screen detection |

Codex starts a thread's hooks at its first prompt in a client, not when the client opens or when
`/resume` switches to the thread. Until you send that prompt, the pane keeps reporting the thread
it showed before, or uses screen detection when it has shown none.

Switching threads with `/resume`, `/new`, `/clear`, or `/fork` releases the previous thread's
report and claims the pane for the new one at its first prompt. A prompt sent to a thread this
client opened earlier claims the pane again, since Codex does not repeat its start event.
Sub-agent events arrive with an agent ID and are ignored, so a child's completion cannot mark its
parent done. The plugin reports one activity for the thread on screen; it does not create separate
child rows.

Codex's approval event carries no tool call ID. The plugin matches an approval to the call that
completes it by tool name and input, and counts identical parallel calls, so the completion of an
unrelated call keeps the thread blocked. An approval you decline without interrupting the turn stays
blocked until the turn finishes.

Hooks call the matching `ROZI_BIN` executable through structured arguments. A unique integration
token and increasing sequence numbers fence each reporting lifecycle. Events from a different
thread or an already-ended lifecycle cannot update that lifecycle. Hooks record events and
outstanding waits in short SQLite transactions before delivering them. A separate sender lock
serializes delivery without preventing parallel hooks from recording their state. Pending snapshots
may be coalesced; the newest state keeps all outstanding waits.

Thread changes persist the new lifecycle and the old token's pending release together. The sender
retries that release before claiming the new token. CLI errors and timeouts leave the transition
pending, so later hooks can recover without restarting Codex. A release already accepted by rozi is
recognised on retry. If no later hook runs, pending delivery waits until one does; the plugin does
not run a background retry service.

The plugin keeps tokens, thread IDs, sequence numbers, pending states and releases, and hashed
approval identities under Codex's `PLUGIN_DATA` directory, in `activity`. It does not read
transcripts or store prompts, tool input, or assistant messages. Individual rozi calls time out
after 750 ms, with a 900 ms delivery budget per hook. Hooks produce no output or permission
decisions, and failures never prevent Codex from continuing.

State persistence gets priority over delivery. Schema initialization and the event transaction
retry SQLite lock contention for up to three seconds, with short waits and random delays between
attempts; an interrupt hook retries for up to 1.2 seconds, because Codex stops it after three.
Failed transactions roll back before retrying, so an event is recorded once. The configured hook
timeout is five seconds, leaving time for startup and delivery after persistence. Delivery stays
best effort; a failed delivery leaves committed state pending.

Reporting requires the pane's `ROZI_SOCKET`, `ROZI_PANE`, and `ROZI_SESSION_INSTANCE`. Codex runs
hooks in the client process, so this holds whether the thread runs in that process or in Codex's
shared background server. Remote and headless panes without a local UI endpoint keep using Rozi's
screen detection. If the UI endpoint goes away, hook reports cannot reach it. Restart Codex in an
attached pane to begin a new reporting lifecycle. Removing the plugin while Codex is running also
requires exiting that Codex client to release its last report.

## Verify

From the repository root:

```sh
python3 -m unittest discover -s plugins/codex/tests -v
```

For a live check, launch Codex with the plugin inside a Rozi pane. From another pane run
`rozi agents list --format json`. Confirm these transitions:

1. Submit a prompt. The pane reports the native thread ID and its state becomes `working`.
2. Request a command that needs approval. Its state becomes `blocked` until you answer.
3. Let the response finish. Its state becomes `done`.
4. Run `/compact`. Reporting continues with the same token.
5. Run `/new` and submit a prompt. The pane reports the new thread ID.
6. Exit Codex. Its report is released and its activity disappears when the process exits.

The hook and plugin formats follow the Codex [hooks reference](https://developers.openai.com/codex/hooks)
and [plugin guide](https://developers.openai.com/codex/plugins/build). This plugin uses Rozi's public
CLI rather than a socket protocol.
