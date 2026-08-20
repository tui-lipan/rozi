---
name: agent-screens
description: >-
  Capture what a coding-agent CLI actually draws, and turn it into detection
  evidence for rozi. Use when adding or changing an agent in
  src/agent_detection/builtin.toml, when a pane reads as the wrong state
  (idle while working, blocked forever, a flapping badge), when
  tests/fixtures/agents/ needs a screen it does not have, or when someone asks
  which agents are still uncovered. Covers the gap report, launching a lab of
  panes, the human-in-the-loop checkpoint loop, scrubbing, and writing a fixture
  that will still mean something in a year.
---

# Agent screens

A detection rule is a claim about somebody else's user interface. Nothing in this repository can
check that claim - only a capture of the real program can. That is what this skill produces:
screens, each paired with the state rozi read from it.

If this skill conflicts with the current workspace docs or source, follow the workspace.

## The rule that matters most

**Never drive the agents. Never answer anything they ask.**

The previous version of this workflow automated the whole loop and the automation is what kept
breaking: it pressed Enter on Claude's trust dialog, then on Cursor's letter-keyed one, and every
guard added afterwards was a guess about a UI nobody had looked at yet. Worse, an answered dialog is
a screen that can no longer be captured - and Cursor leaves its answered trust prompt on screen for
the rest of the session, which is exactly the false positive that had to be found the hard way.

So: the human sets the state, you read it. When you need a pane in some state, ask for it. Use
`AskUserQuestion` at each checkpoint so they can also say "skip that one, it's broken", which is
information too.

## The loop

```bash
python3 .agents/skills/agent-screens/scripts/gaps.py --ready   # what is missing and capturable
python3 .agents/skills/agent-screens/scripts/lab.py launch --all-ready
#   ... ask the human for one checkpoint ...
python3 .agents/skills/agent-screens/scripts/capture.py --expect blocked
#   ... read every screen, write the fixtures, next checkpoint ...
```

1. **`gaps.py`** derives what the corpus has from `builtin.toml`, the fixture files and the ledgers
   in `fixtures.rs`. It replaces a hand-kept checklist, which drifted every time somebody forgot to
   edit it. `--ready` narrows to agents installed on this machine that are still missing something.
2. **`lab.py launch`** spawns one pane per agent in a shared throwaway directory, six to a
   workspace, and switches each workspace to grid. Six is not arbitrary: it gives roughly 88x39
   panes, and every usable capture in this corpus came from that size. Narrower panes wrap the
   status chrome agents draw along the bottom, and a wrapped footer is a screen detection reads
   differently from the one you are looking at.

   Nothing over the control socket *sets* a layout - `toggle-layout` steps to the next one and no
   command reports the current one - so the grid step counts from `[layout] default` through
   `LayoutKind::all()`, both read at runtime. That holds for a workspace created moments ago and
   nowhere else; `--no-layout` skips it when a workspace has already been changed by hand.
3. **A checkpoint.** Pick one state, give the human the exact prompt to type into every pane, and
   wait. `references/checkpoints.md` has the sequence and per-agent wording.
4. **`capture.py`** grabs every agent pane at once, scrubs it, writes candidates to
   `target/agent-screens/`, and prints what detection reads each one as. `--expect working` flags
   the panes that disagree - those are the findings.
5. **Read every screen** before it becomes a fixture. It is a real terminal from someone's machine.
6. **Write the fixture**, then move to the next checkpoint.

## Reading a capture

The reading beside the screen is the point. A capture with no reading says nothing.

`capture.py` also reports where each agent's live signal sits, in non-empty lines from the bottom.
This matters because `scope = "footer"` sees only the last **8** non-empty lines:

- **Signal at 1-8 from the bottom** - a footer-scoped rule can see it. Normal.
- **`<< OUT OF FOOTER`** - it cannot. This is the Cline bug: its spinner rides the line being
  written and drifts upward as text streams, so a whole minute of streaming read as idle while its
  own chrome filled all eight footer rows.
- **No spinner at all** - the signal is either words (Copilot spells out `○ Working`), or an
  *absence* (Goose drops the context meter above its prompt while it works).

A short run can put a spinner in the footer by luck. **Check a long one**: ask for an answer that
streams for a minute, and watch where the signal goes. That check is what found Cline, and what
cleared Grok, Pi, Antigravity, Copilot and Claude.

`capture.py --watch 60` samples over time and keeps one screen per state each pane passes through,
which is how you catch a state that only exists for a moment - or a pane that flaps.

## Writing the fixture

Screens go in `tests/fixtures/agents/<id>.toml`, asserted by `src/agent_detection/fixtures.rs`.
Adding a built-in agent means adding a screen, not just a table.

- **Every case carries a comment saying what it is evidence *of*.** Not what it shows - what would
  break without it. "Kept because Cursor leaves its answered trust dialog on screen, so an unscoped
  needle pins this pane blocked forever" is worth having. "Cursor idle screen" is not.
- **Trim the transcript, never the chrome.** Prose above the live region can be cut to a few lines.
  Everything from the last 8 non-empty lines down must stay byte-exact, because that is the footer.
- **Negative cases earn their place.** A screen that *looks* blocked and is not catches more
  regressions than another positive. So does a finished screen that still carries the working rule's
  positive needle.
- **`source = "capture"`** only if it came from a real pane. Anything reasoned out is a placeholder
  and the corpus tests know the difference.

Validate a new needle against **every** screen in the corpus before shipping it - positives must
match, and no idle or working screen may. Two would-be regressions were caught exactly there.

## Things that will waste your time

- **Detection runs in the session server.** A rule change does nothing until the server restarts.
  A "wrong" reading is a stale server until proven otherwise.
- **Blocked is three different screens.** Trusting a directory, granting permission, and answering a
  question look nothing alike, and an agent covered for one is not covered for the others. `gaps.py`
  tracks them separately.
- **A trust dialog is one-shot per directory.** Once answered, it is gone until a new directory. If
  you want one, ask for a pane in a directory that agent has never seen.
- **`match.names` is what to *recognize*, not what to launch.** `cursor` is the GUI editor; the CLI
  is `cursor-agent`. `lab.py` knows the overrides.
- **Some agents auto-approve.** An agent that deletes a file without asking is not a failed capture,
  it is telling you its policy. Note it in the fixture and move on.
