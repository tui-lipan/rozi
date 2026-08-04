# Performance

This directory is the durable record of hyprmux performance investigations. It separates:

- [the audit playbook](audit-playbook.md), which explains how to reproduce measurements;
- [benchmark documentation](../benchmarks.md), which describes the permanent Criterion and memory
  harnesses;
- dated audit reports in [`audits/`](audits/), which record evidence and conclusions for one
  revision and environment.

## Current assessment

The latest audit found no release-blocking performance problems and classified hyprmux as
**ready with minor improvements**. The confirmed follow-up areas were broad-scope scrollback search,
optional workbar command poller lifecycle, and fixed idle polling cost. See the report for measured
limits and the distinction between confirmed findings and unverified risks.

## Audit ledger

| Date | Revision | Verdict | Report |
| --- | --- | --- | --- |
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
