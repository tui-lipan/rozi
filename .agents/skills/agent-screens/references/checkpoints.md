# Checkpoints

One checkpoint puts every pane into one state, so a single `capture.py` run collects the whole row.
Ask for them in this order: the cheap ones first, and the destructive one last, because answering a
prompt destroys the screen that prompt was.

Give the human the **exact text to type**. "Make it ask a question" is different words in every
tool, and a checkpoint that half the panes miss is a wasted round.

---

## 1. Trust — before anything else

Only available in a directory the agent has never seen, and gone forever once answered. If the lab
directory is already trusted, this checkpoint needs a fresh one:

```bash
python3 .agents/skills/agent-screens/scripts/lab.py launch --cwd /tmp/lab-$(date +%s) pi codex
```

Ask for: **panes launched, trust prompt left unanswered.** Then capture with `--expect blocked`.

Watch for: not every agent asks. Goose opens straight into its session. Note that rather than
retrying.

---

## 2. Idle

Ask for: **the trust prompt answered, nothing typed yet.** Capture with `--expect idle`.

The cheapest checkpoint and the most undervalued: an idle screen that stays idle is the assertion
that catches a new shared rule reading attention or work into ordinary chrome.

---

## 3. Working, short

> Read README.md in this directory and describe it in one sentence.

Capture with `--expect working` **while it runs**, which is a few seconds. `--watch 30` is easier
than timing it by hand.

---

## 4. Working, long — do not skip this

> Explain 20 common data structures, one short paragraph each. No tools, just write.

Runs about a minute, which is long enough for the transcript to push the live region around. This is
the checkpoint that found Cline: a short run can leave a spinner in the footer by luck, and a long
one shows where the signal really lives. Capture with `--expect working` and read the
`from bottom` column, not just the state.

---

## 5. Question

Agents word this differently, and some only offer it in a particular mode:

| Agent | Wording that works |
| --- | --- |
| claude | Use your question tool to ask me which of two approaches I'd prefer for a config format. |
| codex | Plan mode only — its question tool is unavailable otherwise. |
| cline, cursor, maki, antigravity | Use the question tool and ask me any question. |
| pi | Ask me what I would like to test next, with a few options. |

Ask for: **the chooser open, unanswered.** Capture with `--expect blocked`.

Then, without answering, ask for a second capture *after* it is answered — an answered chooser still
on screen is the false positive that nearly pinned Cursor blocked forever.

---

## 6. Approval — last

> Delete the file README.md in this directory.

Deleting a seed file in a throwaway directory is gated far more widely than running a command;
several tools treat a read-only command as pre-approved and simply run it. Pi runs `date` without a
word.

An agent that deletes without asking is not a failed capture — it is telling you it auto-approves
that class of action. Record that in the fixture. Cline needs "Auto-approve all" switched off first,
which is a thing to ask for rather than assume.

Capture with `--expect blocked`, then let the human answer it however they like; the run is over.

---

## Negative screens

Worth their own round, because nobody captures them by accident:

- **Claude with a message whose first line is `1. Yes, go ahead`.** Claude prefixes your own messages
  with `❯`, rendering `❯ 1. …` — byte-for-byte its approval dialog's shape.
- **An agent discussing approvals or questions in prose**, no dialog open.
- **A finished answer full of prose**, for any agent whose working rule is unscoped.
