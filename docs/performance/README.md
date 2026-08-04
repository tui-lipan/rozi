# Performance

This directory is the durable record of hyprmux performance investigations. It separates:

- [the audit playbook](audit-playbook.md), which explains how to reproduce measurements;
- [benchmark documentation](../benchmarks.md), which describes the permanent Criterion and memory
  harnesses;
- dated audit reports in [`audits/`](audits/), which record evidence and conclusions for one
  revision and environment.

## Current assessment

The latest audit found no release-blocking performance problems and classified hyprmux as
**ready with minor improvements**. Broad-scope scrollback search now runs in bounded cooperative
slices, epoch-gated workers resolve the workbar command lifecycle concern, and an isolated
framework comparison confirms a roughly 38-39% snapshot-rebuild reduction. Saturation and durable
resurrection now have permanent measured harnesses. Durable snapshots write off the server loop
and reuse unchanged panes' replay files, cutting maximum 16-pane 5,000-row server-loop blocking from
207.7 ms to 12.0 ms. Remaining work is idle server backoff/readiness design, gated image-budget work,
bounding persisted history for sessions where every pane is busy, slow-storage evidence, and a full
memory soak.

## Audit ledger

| Date | Revision | Verdict | Report |
| --- | --- | --- | --- |
| 2026-08-04 | `2b85924` plus recorded audit worktree changes, with resurrection follow-ups at `9f66a6a` and `4d45cf4` | ready with minor improvements | [Performance follow-up evidence](audits/2026-08-04.md) |
| 2026-08-03 | `c07b6be` plus recorded worktree changes | ready with minor improvements | [Performance and resource-efficiency audit](audits/2026-08-03.md) |

## Recording future audits

Create a new `audits/YYYY-MM-DD.md` rather than overwriting an older result. Every report should
record:

1. exact Git revision and whether the worktree was dirty;
2. OS, CPU, Rust version, release profile, and unavailable tools;
3. exact commands, dimensions, durations, sample counts, and statistics;
4. application memory separately from child-process memory;
5. confirmed findings, deliberate trade-offs, harmless details, and unverified risks;
6. prioritized follow-up work and the measurement that would prove each improvement;
7. one of the verdicts `ready`, `ready with minor improvements`,
   `needs optimization before release`, or `insufficient evidence`.

Small factual corrections may update an existing report. New measurements, changed workloads, or a
different revision require a new dated report so historical comparisons remain meaningful.

Raw Criterion data, Samply profiles, memory-matrix output, logs, sockets, and terminal captures stay
under ignored `target/` paths and are never committed. Only the summarized report belongs here.
