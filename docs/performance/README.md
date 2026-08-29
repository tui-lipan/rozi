# Performance archive

This directory records performance audits for specific revisions and machines. Dated reports are
historical evidence. They are not current benchmark guidance and their source, dependency, or
architecture descriptions may no longer match the repository.

The latest report is [Idle server wakeup evidence, 2026-08-29](audits/2026-08-29.md).

Use the [benchmark guide](../benchmarks.md) for permanent harness commands and definitions. Use the
[audit playbook](audit-playbook.md) to run and record a new audit.

## Audit ledger

| Date | Measured revision | Recorded verdict | Report |
| --- | --- | --- | --- |
| 2026-08-29 | `4dedfc8` before and `0638f41` after, plus recorded benchmark and wait tuning | ready | [Idle server wakeup evidence](audits/2026-08-29.md) |
| 2026-08-04 | `2b85924` plus recorded audit worktree changes, with follow-ups at `9f66a6a` and `4d45cf4` | ready with minor improvements | [Performance follow-up evidence](audits/2026-08-04.md) |
| 2026-08-03 | `c07b6be` plus recorded worktree changes | ready with minor improvements | [Performance and resource-efficiency audit](audits/2026-08-03.md) |

Create a new `audits/YYYY-MM-DD.md` for measurements from another revision, machine, or workload.
Do not overwrite old results to make them describe the current tree. Small corrections may clarify
the original record if they do not change its measured meaning.

Keep raw Criterion data, profiles, memory-matrix output, logs, sockets, and captures under ignored
`target/` paths. Commit only the summarized report.
