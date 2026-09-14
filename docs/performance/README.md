# Performance archive

This directory records performance audits for specific revisions and machines. Dated reports are
historical evidence. They are not current benchmark guidance and their source, dependency, or
architecture descriptions may no longer match the repository.

The latest report is [Compact history ingest decision, 2026-09-14](audits/2026-09-14-ingest.md).

Use the [benchmark guide](../benchmarks.md) for permanent harness commands and definitions. Use the
[audit playbook](audit-playbook.md) to run and record a new audit.

## Audit ledger

| Date | Measured revision | Recorded verdict | Report |
| --- | --- | --- | --- |
| 2026-09-14 | `52cccdc` plus archived experiment | application ingest cost within noise for short lines, +7% for full-width styled logs; 55-122 MiB application PSS saved; technical go, adoption waits on an engine-fork decision; found two tui-lipan hang-up exit bugs | [Compact history ingest decision](audits/2026-09-14-ingest.md) |
| 2026-09-14 | `cedd4a6` plus sibling tui-lipan worktree | attr-keyed runs cut realistic snapshots ~46-49%; live compact gap gone at the new baseline; ready to ship in tui-lipan, compact still blocked on ingest | [Attr-keyed terminal spans](audits/2026-09-14-attr.md) |
| 2026-09-14 | `cedd4a6` plus archived experiment | Style-per-cell mapping and PartialEq dominate released snapshots; attr-keyed runs cut assembly ~43%; not adopted | [Dense span-assembly profiling](audits/2026-09-14-span.md) |
| 2026-09-14 | `cedd4a6` plus archived experiment | bulk reads recover scrollback and cut the live snapshot gap in half; live path still about 15% slower; not adopted | [Render-oriented history reads](audits/2026-09-14-render.md) |
| 2026-09-14 | `cedd4a6` plus archived experiment | batching is slower and retains more memory; active snapshot regression confirmed; not adopted | [Bounded batch history compaction](audits/2026-09-14.md) |
| 2026-09-13 | `824d925` plus archived experiment | 86% wide short-line allocation saving and zero steady allocation calls; short-line ingestion still slower; not adopted | [Dense viewport and compact history](audits/2026-09-13.md) |
| 2026-09-10 | `824d925` | compact-history prototype saves 86% on wide short-line allocations, but fails the ingest gate; not adopted | [Terminal history storage investigation](audits/2026-09-10.md) |
| 2026-09-05 | `1e8f4a0` plus recorded worktree changes | fixed: default-history 253×64 pane with one client uses 14.1% less application PSS | [Pane retained-memory audit](audits/2026-09-05.md) |
| 2026-09-04 | `ca5f34b`, fix measured at `151f175` plus the framework change | confirmed defect, fixed: one changed character repainted the whole window; damaged-row repaint cuts the reported case from 8.3% to 2.7% of a core | [Idle CPU under a non-fullscreen agent spinner](audits/2026-09-04-idle-repaint.md) |
| 2026-09-04 | `4b8f076` plus recorded framework changes | five changes land a 32% cut | [Element-tree allocation in view and layout](audits/2026-09-04.md) |
| 2026-09-03 | `c9e9881`, clean worktree | ready with minor improvements | [Render and ingest CPU attribution](audits/2026-09-03.md) |
| 2026-08-29 | `4dedfc8` before and `0638f41` after, plus recorded benchmark and wait tuning | ready | [Idle server wakeup evidence](audits/2026-08-29.md) |
| 2026-08-04 | `2b85924` plus recorded audit worktree changes, with follow-ups at `9f66a6a` and `4d45cf4` | ready with minor improvements | [Performance follow-up evidence](audits/2026-08-04.md) |
| 2026-08-03 | `c07b6be` plus recorded worktree changes | ready with minor improvements | [Performance and resource-efficiency audit](audits/2026-08-03.md) |

Create a new `audits/YYYY-MM-DD.md` for measurements from another revision, machine, or workload.
Do not overwrite old results to make them describe the current tree. Small corrections may clarify
the original record if they do not change its measured meaning.

Keep raw Criterion data, profiles, memory-matrix output, logs, sockets, and captures under ignored
`target/` paths. Commit only the summarized report.
