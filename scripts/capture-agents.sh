#!/usr/bin/env bash
#
# Drive every agent CLI installed on this machine through one scenario and keep what it drew.
#
# Detection rules are claims about somebody else's user interface, and the only thing that can
# check such a claim is the tool itself. This runs each agent in a throwaway directory, puts it
# through working / blocked / idle, and writes what the screen looked like at each point together
# with what Rozi's rules currently read it as. The output is a candidate fixture for
# tests/fixtures/agents/ - a human reads it, trims it, and moves it in.
#
# It captures. It never edits a fixture, and it never approves anything an agent asks for.
#
#   scripts/capture-agents.sh --list          # what is installed, and what already has evidence
#   scripts/capture-agents.sh                 # every installed agent
#   scripts/capture-agents.sh pi codex        # just these
#   scripts/capture-agents.sh --blocked-prompt 'Run `sudo id`.' pi
#   scripts/capture-agents.sh --workspace 8 pi     # default is workspace 9
#
# Requires: jq, a running Rozi (ROZI_SOCKET, or --socket PATH).

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILTIN="$REPO/src/agent_detection/builtin.toml"
CORPUS="$REPO/tests/fixtures/agents"
OUT="${ROZI_CAPTURE_OUT:-$REPO/target/agent-captures}"

# One directory, reused across runs, rather than a fresh mktemp each time. Agents ask whether they
# trust a directory the first time they see it, and this script must not answer that question - so
# the directory has to be one a human can approve once and be done with.
WORKDIR="${ROZI_CAPTURE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/rozi-agent-lab}"

# The agent is asked to look at a file and then to do something it should want permission for,
# because those are the two moments worth a fixture: a turn in flight, and a turn stopped on a
# prompt.
PROMPT_WORKING="Read README.md in this directory and describe it in one sentence."

# Which action stops an agent to ask is not universal - it is that agent's policy, and the
# operator's own approval settings on top of it. Running a command is a poor probe: several tools
# treat a plain read-only command as pre-approved and simply run it (Pi runs `date` without a
# word). Deleting a file is gated far more widely, and this file is a seed in a throwaway directory
# the script removes anyway, so approving it costs nothing.
#
# An agent that does this without asking is not a broken capture - it is telling you it
# auto-approves that class of action. Override the probe with --blocked-prompt when you know what
# a particular tool does gate, or start the agent in whatever mode makes it ask.
PROMPT_BLOCKED="Delete the file README.md in this directory."

STARTUP_TIMEOUT=25
WORKING_TIMEOUT=45
BLOCKED_TIMEOUT=60
POLL=0.5

SOCKET=""
# A workspace of the lab's own. Panes spawned where somebody is working re-tile their layout on
# every agent, and a pane that keeps changing size is a pane whose screen cannot be compared with
# the last one's - the geometry is part of what a fixture captures.
WORKSPACE="${ROZI_CAPTURE_WORKSPACE:-9}"
KEEP=0
LIST_ONLY=0
WANTED=()

die() { printf 'capture-agents: %s\n' "$*" >&2; exit 1; }
note() { printf '  %s\n' "$*" >&2; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --socket) SOCKET="${2:-}"; [[ -n "$SOCKET" ]] || die "--socket needs a path"; shift 2 ;;
        --blocked-prompt)
            PROMPT_BLOCKED="${2:-}"
            [[ -n "$PROMPT_BLOCKED" ]] || die "--blocked-prompt needs text"
            shift 2 ;;
        --workspace)
            WORKSPACE="${2:-}"
            [[ "$WORKSPACE" =~ ^[1-9]$ ]] || die "--workspace needs a workspace number"
            shift 2 ;;
        --keep) KEEP=1; shift ;;          # leave the panes open to look at afterwards
        --list) LIST_ONLY=1; shift ;;
        -h|--help) sed -n '3,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) die "unknown flag $1" ;;
        *) WANTED+=("$1"); shift ;;
    esac
done

command -v jq >/dev/null || die "jq is required"
# `--list` only reads the manifest and PATH, so it must work without a Rozi to talk to.
ROZI="${ROZI_BIN:-$(command -v rozi || true)}"
if (( LIST_ONLY == 0 )); then
    [[ -n "$ROZI" && -x "$ROZI" ]] || die "no rozi binary; set ROZI_BIN or put rozi on PATH"
fi

# `send-text` falls back to the source pane, and this script is usually run from inside one. Every
# call below targets a pane id explicitly, and dropping ROZI_PANE keeps a missed one from typing
# into the operator's own shell rather than the agent.
ctl() {
    if [[ -n "$SOCKET" ]]; then
        env -u ROZI_PANE "$ROZI" --socket "$SOCKET" "$@"
    else
        env -u ROZI_PANE "$ROZI" "$@"
    fi
}

ok_or_die() {
    local reply="$1" what="$2"
    [[ "$(jq -r '.ok' <<<"$reply")" == "true" ]] ||
        die "$what failed: $(jq -r '.error // .' <<<"$reply")"
}

# Every `[[agents]]` id in builtin.toml, with the executable names that identify it. The manifest is
# the source of truth for what Rozi claims to support, so the lab reads it rather than keeping its
# own list to fall out of date.
agent_table() {
    awk '
        /^id = "/ { id = $0; sub(/^id = "/, "", id); sub(/".*$/, "", id); next }
        /^match = / && id != "" {
            names = $0
            sub(/^.*names = \[/, "", names)
            sub(/\].*$/, "", names)
            gsub(/[",]/, "", names)
            print id "\t" names
            id = ""
        }
    ' "$BUILTIN"
}

# The first executable of an agent that is actually installed, if any.
program_for() {
    local names="$1" name
    for name in $names; do
        if command -v "$name" >/dev/null 2>&1; then
            printf '%s' "$name"
            return 0
        fi
    done
    return 1
}

capture_json() { ctl capture-pane --target "$1"; }
screen_of() { jq -r '.data.text' <<<"$1"; }
title_of() { jq -r '.data.title // ""' <<<"$1"; }

# What the rules make of the pane right now. This is the number the lab exists to produce: a screen
# with no reading beside it says nothing about whether detection works.
#
# Empty means the pane is gone, which is different from `none` (a pane no definition matched).
read_state() {
    ctl list-panes | jq -r --argjson id "$1" \
        '.data[] | select(.id == $id) | .agent_state // "none"'
}

pane_alive() { [[ -n "$(read_state "$1")" ]]; }

# Never press Enter at a pane that is asking a question.
#
# Enter is submit at a prompt box and *confirm* at a dialog, and an agent's first screen in a new
# directory is often "do you trust this folder?" with the yes option already selected. Typing a
# prompt and hitting Enter there does not ask the agent anything - it grants the trust, which this
# script has no business doing on anyone's behalf.
#
# The test cannot be "does detection say blocked". Detection being wrong about a dialog is the
# whole reason this lab exists, and Codex proves it: its trust prompt reads as idle, so a guard
# that asked detection would have typed straight through the one screen it most needed to stop at.
# So the guard reads the screen itself, with a shape crude enough to be independent of any rule -
# two numbered lines. A false positive costs one skipped capture. Being wrong the other way means
# answering somebody's trust dialog.
looks_like_a_dialog() {
    local screen="$1"
    (( $(grep -cP '^\s*\S{0,2}\s*[0-9]+\.\s' <<<"$screen" || true) >= 2 )) ||
        [[ "$(read_state "$2")" == "blocked" ]]
}

read_agent() {
    ctl list-panes | jq -r --argjson id "$1" \
        '.data[] | select(.id == $id) | .agent // "none"'
}

# End the agent by talking to its own pane, never by closing "the focused pane".
#
# `run-action close` acts on whatever is focused, which is not necessarily what this script
# spawned - focus moves, a spawn can already have died, and a focus call can fail quietly. Getting
# that wrong closes the operator's own pane. Sending the program an interrupt and then EOF is
# addressed to a pane id and cannot land anywhere else; the pane closes itself when its command
# exits.
close_pane() {
    local id="$1" deadline
    # Two interrupts in one write, because a TUI that treats the first as "clear the line" wants
    # the second promptly to mean "quit"; then EOF for the ones that read it as exit.
    ctl send-keys --target "$id" C-c C-c >/dev/null 2>&1 || true
    sleep 1
    ctl send-keys --target "$id" C-d C-d >/dev/null 2>&1 || true
    deadline=$((SECONDS + 10))
    while (( SECONDS < deadline )); do
        pane_alive "$id" || return 0
        sleep "$POLL"
    done
    # Deliberately not typing `/exit` or the like: that is a guess at another program's command
    # vocabulary, and typing a guess into a pane that turned out to be showing a dialog is how
    # this script would answer one by accident.
    note "pane $id would not exit - close it yourself"
}

# Wait until the pane stops changing, so a capture is a settled screen rather than a half-drawn one.
wait_quiet() {
    local id="$1" deadline=$((SECONDS + $2)) last="" now
    while (( SECONDS < deadline )); do
        now="$(screen_of "$(capture_json "$id")")"
        [[ -n "$now" && "$now" == "$last" ]] && return 0
        last="$now"
        sleep "$POLL"
    done
    return 1
}

# Poll until the rules report `want`, and answer with the screen at that moment.
#
# When they never do - the case this whole lab exists for - the answer must still be the screen
# from *during* the turn, not the one left on the pane at the timeout. A turn that takes four
# seconds under a forty-five second timeout is over long before the polling is, and returning the
# final screen would hand back the finished transcript: the exact screen that says nothing about
# what working looks like. So the first screen that differs from the pre-prompt baseline is kept as
# the fallback, because that difference *is* the turn starting.
wait_for_state() {
    local id="$1" want="$2" deadline=$((SECONDS + $3)) baseline="$4" reply changed="" now
    while (( SECONDS < deadline )); do
        reply="$(capture_json "$id")"
        if [[ "$(read_state "$id")" == "$want" ]]; then
            printf '%s' "$reply"
            return 0
        fi
        if [[ -z "$changed" ]]; then
            now="$(screen_of "$reply")"
            [[ -n "$now" && "$now" != "$baseline" ]] && changed="$reply"
        fi
        sleep "$POLL"
    done
    printf '%s' "${changed:-$(capture_json "$id")}"
    return 1
}

emit_case() {
    local file="$1" name="$2" state="$3" reply="$4" reading="$5"
    local screen title
    screen="$(screen_of "$reply")"
    title="$(title_of "$reply")"
    {
        printf '\n[[case]]\n'
        printf '# rozi read this as: %s\n' "$reading"
        printf 'name = "%s"\n' "$name"
        printf 'state = "%s"\n' "$state"
        [[ -n "$title" ]] && printf 'title = %s\n' "$(jq -Rn --arg t "$title" '$t')"
        if [[ "$screen" == *"'''"* ]]; then
            # A literal TOML string cannot hold its own delimiter; fall back to the escaped form.
            printf 'screen = %s\n' "$(jq -Rn --arg s "$screen" '$s')"
        else
            printf "screen = '''\n%s\n'''\n" "$screen"
        fi
    } >>"$file"
}

run_agent() {
    local id="$1" program="$2"
    local file pane reply reading baseline
    file="$OUT/$id.toml"

    printf '%s (%s)\n' "$id" "$program" >&2
    reply="$(ctl new-pane --workspace "$WORKSPACE" --cwd "$WORKDIR" --argv "$program")"
    ok_or_die "$reply" "spawning $program"
    pane="$(jq -r '.data.id' <<<"$reply")"

    if ! wait_quiet "$pane" "$STARTUP_TIMEOUT"; then
        note "never settled after $STARTUP_TIMEOUT s - capturing anyway"
    fi
    if ! pane_alive "$pane"; then
        note "exited during startup - nothing to capture"
        return 1
    fi

    {
        printf '# Captured by scripts/capture-agents.sh. Read every screen before moving this into\n'
        printf '# tests/fixtures/agents/ - it is a real pane and holds whatever was on it. Trim each\n'
        printf '# case to the chrome that carries the state plus enough context to recognize it.\n'
        printf 'source = "capture"\n'
        printf 'captured_at = "%s"\n' "$(date -u +%Y-%m-%d)"
        printf 'notes = "%s via %s"\n' "$id" "$program"
    } >"$file"

    reply="$(capture_json "$pane")"
    reading="$(read_state "$pane")"
    note "startup: detection says $(read_agent "$pane")/$reading"
    if looks_like_a_dialog "$(screen_of "$reply")" "$pane"; then
        # A first-run question - "do you trust this folder?" and its relatives. Real blocked
        # evidence, and the end of this agent's run: answering it is the operator's call.
        emit_case "$file" "startup-dialog" "blocked" "$reply" "$reading"
        note "starts on a question. Captured it, and stopping here rather than answering it."
        note "run \`$program\` in $WORKDIR once by hand, answer it, then capture again"
        finish_agent "$pane"
        return 0
    fi
    emit_case "$file" "idle-startup" "idle" "$reply" "$reading"

    ctl send-text --target "$pane" "$PROMPT_WORKING" >/dev/null
    sleep 1  # a TUI that watches for pasted input needs the newline as its own event
    # Baseline *after* the text is typed and before it is submitted: typing into the prompt box is
    # itself a screen change, and a baseline taken before it makes the very first poll look like
    # the turn starting. What we want is the first change Enter causes.
    baseline="$(screen_of "$(capture_json "$pane")")"
    ctl send-keys --target "$pane" Enter >/dev/null
    reply="$(wait_for_state "$pane" "working" "$WORKING_TIMEOUT" "$baseline")" && reading="working" || {
        reading="$(read_state "$pane")"
        note "never read as working - kept the screen the turn drew instead ($reading)"
    }
    emit_case "$file" "working" "working" "$reply" "$reading"
    pane_alive "$pane" || { note "exited mid-turn"; return 1; }

    wait_quiet "$pane" "$WORKING_TIMEOUT" || note "still moving; capturing idle anyway"
    reply="$(capture_json "$pane")"
    reading="$(read_state "$pane")"
    emit_case "$file" "idle-after-turn" "idle" "$reply" "$reading"

    if looks_like_a_dialog "$(screen_of "$reply")" "$pane"; then
        note "still asking something after the first turn - leaving it for a human"
        finish_agent "$pane"
        return 0
    fi
    ctl send-text --target "$pane" "$PROMPT_BLOCKED" >/dev/null
    sleep 1
    baseline="$(screen_of "$(capture_json "$pane")")"
    ctl send-keys --target "$pane" Enter >/dev/null
    reply="$(wait_for_state "$pane" "blocked" "$BLOCKED_TIMEOUT" "$baseline")" && reading="blocked" || {
        reading="$(read_state "$pane")"
        note "never read as blocked - kept the screen the turn drew instead ($reading)"
        note "if it never asked at all, this agent auto-approves; see --blocked-prompt"
    }
    emit_case "$file" "blocked" "blocked" "$reply" "$reading"

    finish_agent "$pane"
    printf '  -> %s\n' "$file" >&2
}

# Leave nothing running. Escape declines whatever was asked rather than leaving a live prompt
# behind, and it is the one key that answers a dialog with "no".
finish_agent() {
    local pane="$1"
    ctl send-keys --target "$pane" Escape >/dev/null 2>&1 || true
    if (( KEEP == 0 )); then
        close_pane "$pane"
    else
        note "pane $pane left open"
    fi
}

if (( LIST_ONLY )); then
    # The two agents whose screens are asserted inline instead of in the corpus; read from the test
    # that owns that list so this cannot drift from it.
    inline="$(sed -n 's/^const EVIDENCE_IN_UNIT_TESTS.*&\[\(.*\)\];$/\1/p' \
        "$REPO/src/agent_detection/fixtures.rs" | tr -d '" ' | tr ',' ' ')"
    printf '%-16s %-14s %-10s %s\n' AGENT PROGRAM INSTALLED EVIDENCE
    while IFS=$'\t' read -r id names; do
        program="$(program_for "$names" || true)"
        evidence="none"
        [[ " $inline " == *" $id "* ]] && evidence="unit tests"
        if [[ -f "$CORPUS/$id.toml" ]]; then
            # A derived fixture is a placeholder for a capture, so it is not evidence yet.
            if grep -q '^source = "derived"' "$CORPUS/$id.toml"; then
                evidence="derived (recapture)"
            else
                evidence="fixture"
            fi
        fi
        printf '%-16s %-14s %-10s %s\n' \
            "$id" "${program:--}" "$([[ -n "$program" ]] && echo yes || echo no)" "$evidence"
    done < <(agent_table)
    exit 0
fi

mkdir -p "$OUT" "$WORKDIR"
cat >"$WORKDIR/README.md" <<'SEED'
# Sample project

One file, so an agent asked to read it has something short and harmless to read.
SEED
ok_or_die "$(ctl list-panes)" "talking to rozi"

captured=0
failed=()
while IFS=$'\t' read -r id names; do
    if (( ${#WANTED[@]} )); then
        [[ " ${WANTED[*]} " == *" $id "* ]] || continue
    fi
    if ! program="$(program_for "$names")"; then
        (( ${#WANTED[@]} )) && note "$id: none of [$names] is installed"
        continue
    fi
    # One agent's bad day is not the batch's. A tool that exits at startup, never comes up, or
    # stops on a question it will not get past must cost its own slot and nothing more - a run of
    # fourteen that aborts on the second is worth less than one that reports twelve and two.
    if run_agent "$id" "$program"; then
        captured=$((captured + 1))
    else
        failed+=("$id")
        note "$id: incomplete, moving on"
    fi
done < <(agent_table)

(( ${#failed[@]} )) && printf '\nincomplete: %s\n' "${failed[*]}" >&2
(( captured )) || die "nothing captured - is anything installed? try --list"
printf '\n%d captured in %s. Read them, trim them, then move them into %s.\n' \
    "$captured" "$OUT" "$CORPUS" >&2
