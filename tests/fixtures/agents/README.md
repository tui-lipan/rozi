# Agent screen fixtures

One file per agent, named for its `id` in `src/agent_detection/builtin.toml`. Each case is a screen
the agent really draws and the state Rozi must read from it. `screens_read_the_states_their_fixtures_claim`
in `src/agent_detection/fixtures.rs` runs every case; `every_shipped_agent_has_screen_evidence`
fails when an agent ships rules with no evidence behind them.

The point is that a detection rule is a claim about somebody else's UI. Nothing in this repository
can check that claim - only a capture of the real program can.

## Capturing one

Run the agent under Rozi, get it into the state you want, and ask the pane what it looks like:

```bash
rozi capture-pane --target <pane-id>
```

That is the same text the detector sees. Paste it into a `screen` block below. Rules that match the
terminal title need `title` filled in too; `rozi list-panes` reports it.

**Read the capture before committing it.** It is a real screen, so it holds whatever was on it -
paths, branch names, prompt text, and potentially worse. Trim it to the chrome that carries the
state (the footer, the dialog, the title) plus enough context to be recognizable, and drop the
transcript above it.

## Format

```toml
# Where these screens came from. A fixture with no provenance is a guess with a filename.
source = "capture"          # "capture" = seen in a real pane, "derived" = reasoned from elsewhere
captured_at = "2026-08-20"
notes = "pi 0.4.2 under bash"

[[case]]
name = "working-tool-call"  # unique within the file, and describes the moment
state = "working"           # working | blocked | idle | unknown
title = "⠹ pi"              # optional; omit when the title carries nothing
screen = '''
⠹ Working...
'''
```

A case asserting `idle` on a screen that merely *discusses* interrupting or approving is worth as
much as the positive ones - that is the failure mode the scoped rules exist to prevent.
