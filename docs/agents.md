# Agent definitions

Rozi detects coding-agent CLIs in panes and shows their state in the sidebar's
[Activity tab](sidebar.md#activity) and in the [Agents view](sessions.md#go-to-an-agent), which adds
the agents on every connected host. Add a `[[agents]]` entry when Rozi does not recognize a tool, or
override a built-in entry when its screen rules do not match the installed version.

Start with a process match:

```toml
[[agents]]
id = "mycoolagent"
label = "My Cool Agent"
match = { names = ["mca"], paths = ["@acme/mca"] }
```

This is enough to list the process as an agent. Add state rules only when its screen has stable text
that distinguishes working, blocked, idle, or unknown views.

## Match the process

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Lowercase letters, digits, `-`, and `_`. |
| `label` | no | Display name. Defaults to `id`. |
| `base` | no | Use Rozi's common state rules. Defaults to `true`. |
| `match.names` | one match field | Executable basenames. |
| `match.paths` | one match field | Substrings found in executable paths or argument tokens. |
| `states` | no | Screen or title rules. |
| `resume.argv` | no | Native resume argv; `{session}` must be one whole element. |

Names are matched without a directory, without case, and without these launcher suffixes:
`.exe`, `.cmd`, `.bat`, `.ps1`, `.js`, `.mjs`, and `.py`.

Use `paths` for tools launched through Node, Python, a package manager, or another generic
executable. Path matching lowercases the value and normalizes backslashes to slashes:

```toml
match = {
  names = ["mca", "mycoolagent"],
  paths = ["@acme/mca"]
}
```

Rozi inspects the foreground process group and unwraps common shell, language-runtime, and package
launchers. Set `ROZI_AGENT` or `HERDR_AGENT` in the pane environment to provide an explicit name
hint when the launcher cannot otherwise be identified.

At least one name or path is required for a new definition. An override of an existing built-in id
may omit `match` to retain that built-in's process match while replacing its label or state rules.

## Add state rules

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

Each rule accepts:

| Field | Values |
| --- | --- |
| `state` | `blocked`, `working`, `idle`, or `unknown` |
| `scope` | `all`, the default, or `footer` for screen rules |
| `screen` | Pattern group matched against visible terminal text |
| `title` | Pattern group matched against the terminal title |

Set exactly one of `screen` or `title`.

A pattern group accepts:

| Field | Meaning |
| --- | --- |
| `all_of` | Every pattern must match. |
| `any_of` | At least one pattern must match. |
| `none_of` | No listed pattern may match. |
| `regex` | Treat all patterns in this group as regex-lite expressions. Defaults to `false`. |

At least one `all_of` or `any_of` pattern is required. A group containing only `none_of` is
rejected because it would match unrelated screens that merely lack a string.

Matching ignores case. Write literal patterns as the tool displays them. Regex patterns also run
against lowercased text, so they do not need a case-insensitive flag.

`scope = "footer"` reads the last eight non-empty screen lines. Use it for spinners, interrupt
hints, and prompt controls that may also appear in transcript text. It does not apply to title
rules.

## Understand state precedence

When several rules match, Rozi uses this order:

```text
unknown, blocked, working, idle
```

Declaration order does not change that precedence. A blocked approval prompt wins over a working
spinner on the same screen.

`unknown` means the current view does not reveal the run state. Rozi keeps the prior observed state
instead of reporting idle. Use it for a navigator, help page, or subagent view that hides the
tool's normal status area. A held state eventually returns to idle if no confirming evidence appears.

If no rule matches, the detected agent is idle.

With `base = true`, Rozi also applies common rules for approval text, yes/no questions, trust
prompts, choice dialogs, braille spinners, and interrupt hints. These common blocked rules share the
same precedence as your own rules. Set `base = false` when the tool's transcript routinely quotes
that text and creates false states, then define the needed working and blocked rules explicitly.

## Override and extension behavior

Definitions are loaded in this order:

1. `config.toml` entries
2. extension entries
3. built-in entries

A `config.toml` definition with a built-in id replaces that built-in. A config definition with a new
id can claim a process before a built-in definition does.

Extension agent ids are namespaced as `<extension>.<id>`. An extension cannot replace a built-in.
Extension definitions use the same fields in `extension.toml`. See
[Extensions](extensions.md).

Detection runs in the session server. All clients attached to one session therefore see the same
agent label and state. Reloading config re-runs detection, but only the controlling client's reload
updates a shared running server. Restart a long-lived server after rebuilding Rozi with changed
built-in definitions.

## Inspect and wait for agents

The `agents` commands expose the same effective state used by the sidebar:

```bash
rozi --session dev agents list
rozi --session dev agents get --target 3
rozi --session dev agents read --target 3 --scrollback 200
rozi --session dev agents wait --target 3 --until quiescent --timeout 2m
rozi --session dev agents prompt --target 3 --wait idle "Fix the failing test"
```

`rozi agents --help` lists every subcommand and flag, including the integration `report` and
`release` commands that `rozi --help` keeps under `--advanced`. A help flag anywhere after `agents`
prints that help instead of running the command, so it is never submitted as prompt text.

`list` and `get` include an opaque `ref` in JSON output. Numeric pane ids are convenient for
interactive use; automation can pass that exact object back with `--ref '<json>'` to fence the
operation to one agent incarnation.

Wait predicates are `working`, `blocked`, `idle`, `done`, `quiescent` (idle or done), and `gone`.
The wait is registered atomically inside the named session server, so it works while no UI is
attached and cannot miss a transition between reading the agent and subscribing. Failures
distinguish a gone agent, a replacement incarnation, a stale server reference, and a timeout.
Use `--format json` for the stable response and error-code contract.

`agents prompt` resolves one exact incarnation, validates its state, installs any requested
completion observation, and submits the text plus Enter as one server operation. A blocked agent is
never typed into. A working agent is also refused unless `--allow-working` is explicit. This avoids
typing into approval dialogs and closes the race where a fast run could finish between separate
send and wait requests.

Agent hooks can report state that screen detection cannot see:

```bash
rozi --session dev agents report --target "$ROZI_PANE" \
  --agent claude --integration "$AGENT_RUN_ID" \
  --state working --native-session abc123 --seq 1
rozi --session dev agents release --target "$ROZI_PANE" \
  --integration "$AGENT_RUN_ID" --seq 2
```

The caller generates one unique `--integration` token per agent process. Sequence numbers increase
within that token. A released token cannot claim the pane again, so delayed hooks from an old
process fail with `conflict` even after a replacement starts its own sequence at 1. `--agent` binds
the claim to the currently detected agent incarnation.

Against a UI endpoint, an omitted `--target` uses the calling pane's `ROZI_PANE`. With
`--session`, pass `--target` explicitly because an inherited pane number belongs to another session
namespace. A live integration report has authority over screen detection and clears older published
rows. Releasing it returns the pane to current detection or subsequently published rows.

## Publish state instead of reading the screen

Screen matching only sees the view currently drawn in one terminal. It cannot reliably represent a
program with several hidden tabs, parent and child agents, or state kept only in an API.

Use `rozi status` for one pane-level state or `rozi publish` for several activity rows. While a
pane publishes rows, Rozi uses those values instead of screen detection. A published row can also
bring its corresponding in-program activity into view when selected.

See [Control](control.md#published-activity) for fields and lifecycle.

## Declare native resume support

An agent that reports `--native-session` can declare how resurrection resumes that opaque session:

```toml
[[agents]]
id = "mycoolagent"
match = { names = ["mca"] }

[agents.resume]
argv = ["mca", "--resume", "{session}"]
```

`{session}` must appear exactly once and occupy the entire argument. Rozi substitutes it directly
into the argument vector; it never builds a shell command, so spaces and shell metacharacters in an
opaque reference remain data. Invalid resume declarations are ignored with a config warning while
the agent's detection rules continue to work.

An override of a built-in agent inherits its resume capability when `resume` is omitted. Set
`resume = false` on the `[[agents]]` entry to disable native resume for that override.

A snapshot stores only the fact - this agent, this conversation reference - never the command. The
argv is resolved from the definition loaded when the session is restored, so changing `argv` here
also changes how an existing snapshot reopens. See
[Sessions](sessions.md#reopen-an-agent-conversation) for what restore does with it.

## Test a definition

Save the config file and inspect:

```bash
rozi list-panes --format json
```

The `agent` field reports the matched definition. `agent_state` reports screen detection separately
from status published by the program.

Capture the actual screen and title before writing a rule:

```bash
rozi capture-pane --target 3 --format json
```

Use text that belongs to the tool's current controls, not content that can appear in its transcript.
Footer scope is usually safer for live status text.

Config warnings explain invalid ids, missing process matches, unknown states, empty pattern groups,
invalid regular expressions, and rules that set both `screen` and `title`. An invalid rule is
discarded without removing the rest of the definition. An invalid definition is dropped as a whole.

The built-in definitions are in
[`src/agent_detection/builtin.toml`](../src/agent_detection/builtin.toml). Use them as examples, but
verify patterns against the version of the agent CLI you run.
