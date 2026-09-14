# Compact history ingest decision

This is the last compact-history experiment. It is not enabled in Rozi. See the
[measurement report](../../../docs/performance/audits/2026-09-14-ingest.md).

The candidate is the render-history engine, unchanged, with its framework changes ported to
released `tui-lipan` 0.9.1. The bulk `RowParts` builder uses the released attr-keyed run helpers.
The dense-iterator builder is left byte-for-byte as released, so `ROZI_EXPERIMENT_RENDER=dense`
still means the shipped snapshot path.

Unlike the earlier experiments, the matrix measures the application, not the engine: a session
server, attached `rozi sessions attach` clients under `script`, and 1, 4, or 8 panes that each
`cat` a fixed corpus once. It records server and client CPU time from `/proc/PID/stat`, wall time
until both go quiet, and PSS and RSS once they have. Dense and compact runs alternate, and the
order flips each repetition.

## Reproduce

Linux only. Run Cargo commands one at a time, and keep the lockfile out of any commit: the
`--config` overrides rewrite the tui-lipan and engine entries to path sources.

```bash
exp=target/ingest-decision
mkdir -p "$exp"
python3 tools/experiments/ingest-decision/prepare.py --output "$exp/src"
python3 tools/experiments/ingest-decision/make-corpus.py "$exp/corpus"

cp Cargo.lock "$exp/Cargo.lock.before"
cargo build --release --locked
cp target/release/rozi "$exp/rozi-dense"
CARGO_TARGET_DIR=target/ingest-compact cargo \
  --config "patch.crates-io.tui-lipan.path=\"$PWD/$exp/src/framework\"" \
  --config "patch.crates-io.alacritty_terminal.path=\"$PWD/$exp/src/engine\"" \
  build --release
cp target/ingest-compact/release/rozi "$exp/rozi-compact"
cp "$exp/Cargo.lock.before" Cargo.lock

EXP_DIR="$PWD/$exp" REPS=5 tools/experiments/ingest-decision/run-matrix.sh "$exp/results.jsonl"
python3 tools/experiments/ingest-decision/summarize.py "$exp/results.jsonl"
```

`run-matrix.sh` takes optional `"PANES CONTENT CLIENTS"` arguments to rerun a subset. The report's
styled-log rows added ten more repetitions of `"1 log 1"` and `"8 log 1"` this way.

## Process safety

Developers run their own Rozi on the machine they measure on. `owned.sh` signals only a PID the run
started, whose executable is an `rozi-*` binary in `EXP_DIR` (or the `script` wrapper), and whose
start time still matches. It never looks a process up by name or by socket.

Cleanup reaps clients before their `script` wrappers. Killing a wrapper first leaves an orphaned
client whose terminal dies part-way through its exit. On `tui-lipan` 0.9.1 that client spins a core
forever (see the report), and each leaked one contaminates every later run. A run refuses to start,
and fails, if any experiment binary is still alive.

`prepare.py` applies `framework.patch` to the cached 0.9.1 crate and `../render-history/engine.patch`
to cached `alacritty_terminal` 0.26.0. Apache-2.0 attribution for the engine is in
`../render-history/LICENSE-APACHE`.
