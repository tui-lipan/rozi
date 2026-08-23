# Agent definitions

Rozi detects coding-agent CLIs in panes and shows their state in the sidebar's
[Activity tab](sidebar.md#activity). Add a `[[agents]]` entry when Rozi does not recognize a tool,
or override a built-in entry when its screen rules do not match the installed version.

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

## Publish state instead of reading the screen

Screen matching only sees the view currently drawn in one terminal. It cannot reliably represent a
program with several hidden tabs, parent and child agents, or state kept only in an API.

Use `rozi status` for one pane-level state or `rozi publish` for several activity rows. While a
pane publishes rows, Rozi uses those values instead of screen detection. A published row can also
bring its corresponding in-program activity into view when selected.

See [Control](control.md#published-activity) for fields and lifecycle.

## Test a definition

Reload config and inspect:

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
