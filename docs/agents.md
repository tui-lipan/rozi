# Agent definitions

Rozi recognizes coding-agent CLIs running inside panes, and reads their screens to tell a run in
flight from one waiting on you. Both halves are data: an **agent definition** says which foreground
process is a given agent, and what its screen looks like in each state.

Nothing about the agents Rozi ships is privileged. The built-in catalog is
[`src/agent_detection/builtin.toml`](../src/agent_detection/builtin.toml), written in the format
below and parsed by the same validator your `config.toml` goes through. Teaching Rozi about a tool
it has never heard of is a table, not a plugin process.

```toml
[[agents]]
id = "mycoolagent"
label = "My Cool Agent"
match = { names = ["mca"], paths = ["@acme/mca"] }

[[agents.states]]
state = "working"
scope = "footer"
screen = { any_of = ["esc to interrupt"] }
```

The rows appear in the sidebar's [Activity tab](sidebar.md); what a detected state means for
elapsed time, alerts, and grouping is documented there.

## Where definitions come from

| Source | Ids | Can replace a built-in |
| --- | --- | --- |
| Built-in catalog | `claude`, `opencode`, … | — |
| `config.toml` `[[agents]]` | as declared | yes |
| An extension's `extension.toml` `[[agents]]` | `<extension>.<id>` | no |

Config entries are consulted first, then extension entries, then the built-ins. A config entry
declaring an id a built-in already uses **replaces** it rather than competing with it; an entry
with a new id is simply added ahead of them, so it can claim an executable name a built-in also
lists. An extension's ids are namespaced the way its commands and services are, which is what
stops an installed extension from quietly taking over `claude`.

Detection runs in the session server, which owns the PTYs, so one session has one answer that every
attached client sees. A follower with different `[[agents]]` in its own config does not get a
different reading. Editing definitions and running `reload-config` re-reads them and re-detects
every pane; only the controller's reload does this.

## Recognizing the process

```toml
match = { names = ["mca", "mycoolagent"], paths = ["@acme/mca"] }
```

`names` are executable basenames. They match after the directory is stripped and a launcher suffix
(`.exe`, `.cmd`, `.bat`, `.ps1`, `.js`, `.mjs`, `.py`) is removed, so `names = ["mca"]` covers
`/usr/local/bin/mca` and `mca.exe`.

`paths` are substrings of an executable path or an argv token, lowercased with `\` normalized to
`/`. They are what recognizes an agent whose binary name says nothing — an npm package launched
through `node`, where the only evidence is the script path. This is why `claude` lists
`@anthropic-ai/claude-code`.

Rozi looks at the pane's whole foreground process group, not just the leader, and unwraps common
launchers (`node`, `python`, `bun`, `deno`, `sh`/`bash`/`zsh`/`fish`, `cmd`/`pwsh`, `npx`, `uvx`,
`env`, and friends). An explicit `ROZI_AGENT` environment variable in the pane overrides all of it
and is matched against `names`, which is the escape hatch for a launcher nothing else can see
through.

At least one of `names` or `paths` is required — a definition nothing can match is dropped with a
warning. The exception is an entry replacing a built-in: omit `match` entirely there and it keeps
the built-in's process vocabulary, so you can retune only the screen rules.

## Reading the screen

A definition's `[[agents.states]]` rules each say what a match concludes.

```toml
[[agents.states]]
state = "blocked"
screen = { all_of = ["esc dismiss"], any_of = ["enter submit", "enter toggle"] }
```

| Key | Notes |
| --- | --- |
| `state` | `blocked`, `working`, `idle`, or `unknown`. Required. |
| `scope` | `all` (default) or `footer`. `screen` rules only. |
| `screen` / `title` | The needles, over one observation. Exactly one per rule. |

A pattern group takes `all_of` (every needle matches), `any_of` (at least one), and `none_of`
(nothing matches, a veto). At least one of `all_of` or `any_of` is required. Set `regex = true` to
read the group as `regex-lite` regular expressions rather than literals; mix the two by writing two
rules with the same `state`.

Needles are matched against text Rozi has already lowercased, so write a literal however the agent
draws it and a regex without a case-insensitive flag.

### Precedence, not declaration order

Rules are evaluated by outcome, highest first:

```text
blocked  →  working  →  idle  →  unknown  →  (nothing matched: idle)
```

An agent drawing a spinner *and* an approval dialog is waiting on you either way, so a `blocked`
rule outranks any `working` evidence beneath it no matter where the two sit in the file. This is
also why adding a `working` rule to an agent never demotes the shared `blocked` vocabulary.

`unknown` is not idle. It means *recognized the agent, learned nothing about its state*, and Rozi
holds the pane's previous state instead of ending the run. It exists for a view that replaces the
agent's own status chrome — OpenCode's subagent navigator covers the composer and the status line,
the only two places the parent run reports progress. Without it, opening a subagent would end the
run on screen, restart its elapsed clock, and announce a completion that never happened. A held
state that goes fifteen minutes with no confirming evidence falls back to `idle`.

A screen that matches nothing at all *is* idle: a definition whose rules all fail has observed its
agent sitting at a prompt.

### `scope = "footer"`

Footer scope reads only the last eight non-empty lines — where live status chrome sits. It matters
more than it sounds: an agent's transcript quotes its own footer hints constantly, writing *about*
interrupting a run while no run is in flight. An unscoped `"esc to interrupt"` rule reads such a
pane as working forever.

### The shared vocabulary

Every definition also gets Rozi's base rules unless it opts out, which is why most agents need no
rules of their own:

- `blocked` on `permission required`, `action required`, `do you want to proceed?`,
  `waiting for permission`, `allow command?`, `[y/n]`, `yes (y)` anywhere on screen, or
  `action required` in the title;
- `working` on `esc to interrupt`, `esc again to interrupt`, `press esc to interrupt`,
  `ctrl+c to interrupt`, or `esc interrupt` **in the footer**.

Set `base = false` to drop it. The built-in `claude` definition does: Claude Code transcripts quote
approval prose verbatim while a run is still going, so only the dialog's own structure — a
selection cursor on a numbered option — counts as blocked there. Turning the base off drops its
`working` rule too, which is why `claude` restates the interrupt hints.

Base rules are deliberately not user-extensible. One bad needle there would misread every pane at
once; a definition wanting a shared vocabulary of its own restates it.

## When a definition is not enough

Screen scraping is a guess, and there are things it structurally cannot do. A program running
several agents behind one terminal — a client with its own tab bar, a parent session and its
subagents — can only ever be seen one at a time, and a state that lives in a log file or an API
rather than on screen is not there to read.

Such a program reports for itself instead, through
[`rozi status`](control.md) or [`rozi publish`](control.md#agent-slots). While a pane publishes
rows, Rozi stops scraping it entirely and takes the pane's state from them. That path needs no
definition and no extension — any program that can run a command can use it — and an extension can
publish on another pane's behalf by setting `ROZI_PANE`, which is what the
[agent-activity example](../examples/extensions/agent-activity/) does.

Reach for a definition when a tool you did not write draws its state on screen. Reach for
publishing when the program is yours, or when one pane holds more than one run.

## Validating a definition

Config warnings surface as toasts on load and reload, and a rejected definition always says why:

```text
Ignored agent `mytool` with no `match.names` or `match.paths`: nothing could ever match it
Ignored agent `mytool` `working` rule with invalid regex `(`: unclosed group
```

A bad *rule* costs only that rule; a bad definition is dropped whole rather than loaded in a
surprising partial form. In an `extension.toml` the same errors invalidate the whole extension,
matching how a bad command or service is treated there — check it with
`rozi check-extension .` before installing.

To see what Rozi currently makes of a pane, `rozi list-panes` reports each pane's reported status,
and the Activity sidebar shows the resolved label and state.
