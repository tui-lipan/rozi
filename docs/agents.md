# Agent definitions

rozi recognizes coding-agent CLIs running in panes and shows whether each one is working, blocked,
done, or idle. This page covers adding or overriding an agent definition, the full rule reference,
and the `rozi agents` commands for scripts.

## What agent detection shows

Detected agents appear in the sidebar's [Activity tab](sidebar.md#activity) and in the
[Agents view](sessions.md#go-to-an-agent), which also lists agents in your other local sessions and
on connected hosts.

rozi ships definitions for common agent CLIs in
[`src/agent_detection/builtin.toml`](../src/agent_detection/builtin.toml). Add an `[[agents]]` entry
to `config.toml` when rozi does not recognize a tool, or override a built-in when its screen rules do
not match the version you run.

Detection runs in the session server, so every client attached to a session sees the same agent
label and state.

## Add an agent

Start with a process match:

```toml
[[agents]]
id = "mycoolagent"
label = "My Cool Agent"
match = { names = ["mca"], paths = ["@acme/mca"] }
```

This is enough to list the process as an agent. rozi's [common state rules](#common-rules) then
detect typical approval prompts, spinners, and interrupt hints. Add your own state rules only when
the tool's screen has stable text that shows it is working, blocked, idle, or in a view that hides
its state:

```toml
[[agents.states]]
state = "working"
scope = "footer"
screen = { any_of = ["esc to interrupt"] }

[[agents.states]]
state = "blocked"
screen = {
  all_of = ["esc dismiss"],
  any_of = ["enter submit", "enter toggle"]
}
```

## Override a built-in agent

Give your entry the same `id` as a built-in to replace that built-in. You can omit `match` to keep
the built-in's process match while replacing its label or state rules. The override also keeps the
built-in's [native resume](#declare-native-resume-support) setting unless you set `resume`.

## Test a definition

1. Save `config.toml`. rozi [reloads it](configuration.md#reloading) and re-runs detection.
2. Check which definition matched:

   ```bash
   rozi list-panes --format json
   ```

   The `agent` field names the matched definition. `agent_state` reports screen detection, separate
   from any status the program publishes itself.
3. Capture the actual screen and title before writing a rule:

   ```bash
   rozi capture-pane --target 3 --format json
   ```

Match text that belongs to the tool's live controls, not text that can also appear in its
transcript. Footer scope is usually safer for live status text.

rozi reports config warnings for invalid ids, definitions without a process match, unknown states,
empty pattern groups, invalid regular expressions, rules that set both `screen` and `title`, and
invalid `scope` or `resume` values. An invalid rule is dropped and the rest of the definition still
loads. An invalid definition is dropped as a whole.

In a [shared session](shared-sessions.md), only the controlling client's reload updates the running
server. After rebuilding rozi with changed built-in definitions, restart long-lived session servers
to pick them up.

## Match the process

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Lowercase letters, digits, `-`, and `_`. |
| `label` | no | Display name. Defaults to `id`. |
| `base` | no | Apply rozi's [common rules](#common-rules). Defaults to `true`. |
| `match.names` | one match field | Executable basenames. |
| `match.paths` | one match field | Substrings of executable paths or argument tokens. |
| `states` | no | Screen or title rules. |
| `resume.argv` | no | Native resume command; see [Declare native resume support](#declare-native-resume-support). |

A new definition needs at least one name or path. Only an override of a built-in may omit `match`.
A second definition with the same `id` in the same file is ignored.

Names are compared without a directory, without case, and without these launcher suffixes: `.exe`,
`.cmd`, `.bat`, `.ps1`, `.js`, `.mjs`, and `.py`.

Use `paths` for tools launched through Node, Python, a package manager, or another generic
executable. Paths are compared in lowercase, with backslashes treated as slashes:

```toml
match = {
  names = ["mca", "mycoolagent"],
  paths = ["@acme/mca"]
}
```

rozi inspects the pane's foreground process and looks through common shell, language-runtime, and
package launchers. If the launcher still hides the tool, set `ROZI_AGENT` (or `HERDR_AGENT`) in the
pane's environment to the agent's name.

## Add state rules

Each `[[agents.states]]` rule accepts:

| Field | Values |
| --- | --- |
| `state` | `blocked`, `working`, `idle`, or `unknown` |
| `scope` | `all`, the default, or `footer`; screen rules only |
| `screen` | Pattern group matched against visible terminal text |
| `title` | Pattern group matched against the terminal title |

Set exactly one of `screen` or `title`.

`scope = "footer"` reads only the last eight non-empty lines of the screen. Use it for spinners,
interrupt hints, and prompt controls that may also appear in transcript text. An unrecognized
`scope` value falls back to `all` with a warning, and `scope` on a `title` rule is ignored with a
warning.

A pattern group accepts:

| Field | Meaning |
| --- | --- |
| `all_of` | Every pattern must match. |
| `any_of` | At least one pattern must match. |
| `none_of` | No listed pattern may match. |
| `regex` | Treat all patterns in this group as regex-lite expressions. Defaults to `false`. |

A group needs at least one `all_of` or `any_of` pattern. A group with only `none_of` is rejected,
because it would match any unrelated screen that lacks the text. Empty patterns are rejected.

Matching ignores case, so write literal patterns as the tool displays them. Regex patterns run
against lowercased text and do not need a case-insensitive flag.

## State precedence

When several rules match, the first state in this order wins:

```text
unknown, blocked, working, idle
```

Declaration order does not matter. For example, an approval prompt (`blocked`) wins over a spinner
(`working`) on the same screen. If no rule matches, the agent is idle.

`unknown` means the current view does not show the run state, such as a navigator, help page, or
subagent view that hides the tool's usual status area. rozi keeps the last observed state instead of
reporting idle. If nothing confirms that state for a while, it eventually returns to idle.

### Common rules

With `base = true`, rozi also applies common rules for approval text, yes/no questions, trust
prompts, choice dialogs, braille spinners, and interrupt hints. They follow the same precedence as
your own rules, so adding a `working` rule does not override a common `blocked` rule.

Set `base = false` when the tool's transcript often quotes that kind of text and causes false
states. Then define the working and blocked rules the tool needs yourself.

## Load order and extensions

Definitions are consulted in this order:

1. `config.toml` entries
2. extension entries
3. built-in entries

A `config.toml` entry with a built-in's id replaces that built-in. An entry with a new id is checked
before the built-ins, so it can claim a process that a built-in would also match.

Extensions declare agents in `extension.toml` with the same fields. Their ids become
`<extension>.<id>`, so an extension cannot replace a built-in. See [Extensions](extensions.md).

## Declare native resume support

An agent that reports a native session (with `rozi agents report --native-session`) can declare how
[resurrection](sessions.md#reopen-an-agent-conversation) reopens that conversation:

```toml
[[agents]]
id = "mycoolagent"
match = { names = ["mca"] }

[agents.resume]
argv = ["mca", "--resume", "{session}"]
```

`argv` must not be empty or contain empty strings. `{session}` must appear exactly once, as a whole
argument, and not as the program. rozi puts the session reference directly into the argument list
without a shell, so spaces and shell characters in it stay literal. An invalid `resume` is ignored
with a warning, and the agent's detection rules still work.

An override of a built-in inherits the built-in's resume command when `resume` is omitted. Set
`resume = false` on the `[[agents]]` entry to turn native resume off. `resume = true` is not valid.

A snapshot stores only which agent and which conversation — never the command. rozi reads `argv`
from the definition loaded when the session is restored, so changing `argv` also changes how
existing snapshots reopen. See [Sessions](sessions.md#reopen-an-agent-conversation) for what restore
does.

## Inspect and wait for agents

The `rozi agents` commands use the same state as the sidebar:

```bash
rozi --session dev agents list
rozi --session dev agents get --target 3
rozi --session dev agents read --target 3 --scrollback 200
rozi --session dev agents wait --target 3 --until quiescent --timeout 2m
rozi --session dev agents prompt --target 3 --wait idle "Fix the failing test"
```

`rozi agents --help` lists every subcommand and flag, including `report` and `release`, which
`rozi --help` shows only with `--advanced`. A help flag anywhere after `agents` prints help instead
of running the command, so it is never sent as prompt text.

Target an agent by pane id with `--target`, or by the exact agent with `--ref '<json>'`. `list` and
`get` include an opaque `ref` in JSON output. Passing it back limits the command to that one run of
the agent, so a replacement started in the same pane does not receive it.

### Wait for a state

`agents wait` and `agents prompt` need `--session`, because they run in the session server. That
lets a wait work while no client is attached, without missing a change that happens between reading
the agent and starting to wait.

Wait states are `working`, `blocked`, `idle`, `done`, `quiescent` (idle or done), and `gone`. Errors
distinguish a gone agent, a replacement agent, a stale reference, and a timeout. Use
`--format json` for stable responses and error codes.

### Send a prompt safely

`agents prompt` checks the agent's state, sets up any `--wait`, and submits the text followed by
`Enter` as one operation, so a fast run cannot finish unobserved between sending and waiting. It
never types into a blocked agent, which could answer an approval dialog. It also refuses a working
agent unless you pass `--allow-working`.

### Report state from agent hooks

Agent hooks can report state that screen detection cannot see:

```bash
rozi --session dev agents report --target "$ROZI_PANE" \
  --agent claude --integration "$AGENT_RUN_ID" \
  --state working --native-session abc123 --seq 1
rozi --session dev agents release --target "$ROZI_PANE" \
  --integration "$AGENT_RUN_ID" --seq 2
```

- Generate one unique `--integration` token per agent process. Sequence numbers increase within a
  token.
- A released token cannot claim the pane again. Late hooks from an old process fail with `conflict`,
  even after a new process starts its own sequence at 1.
- `--agent` ties the report to the agent currently detected in the pane.
- A live report overrides screen detection and clears older published rows. Releasing it returns
  the pane to screen detection or rows published later.

Inside a rozi pane without `--session`, an omitted `--target` means the calling pane
(`ROZI_PANE`). With `--session`, always pass `--target`, because an inherited pane number may belong
to a different session.

## Publish state instead of reading the screen

Screen matching only sees what one terminal is currently drawing. It cannot reliably follow a
program with several hidden tabs, parent and child agents, or state that exists only in an API.

For those programs, use `rozi status` to report one state for the pane, or `rozi publish` for
several activity rows. While a pane publishes rows, rozi shows them instead of screen detection.
Selecting a published row can also bring that activity into view inside the program. See
[Control](control.md#published-activity) for fields and lifecycle.
