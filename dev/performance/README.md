# Performance archive

This directory records performance audits for specific revisions and machines. Dated reports are
historical evidence. They are not current benchmark guidance and their source, dependency, or
architecture descriptions may no longer match the repository.

The latest measurements are [immutable media adoption, 2026-10-25](audits/2026-10-24-browser-media-regression.md#final-build-adoption-v2).
The preceding [graphics pipeline acceptance](audits/2026-10-14-graphics-pipeline.md) records the
broader rendering work.

Use the [benchmark guide](../benchmarks.md) for permanent harness commands and definitions. Use the
[audit playbook](audit-playbook.md) to run and record a new audit.

## Audit ledger

| Date | Measured revision | Recorded verdict | Report |
| --- | --- | --- | --- |
| 2026-10-25 | `9f914739` and framework `a9a1005` with recorded adoption-v2 changes and source hashes | about 60 frames/s; combined plain/noise CPU 9.9–10.4% / 11.1–11.75%, below pre-#241; client pixel reads removed, shared snapshot storage lower; local raw-media fairness improves, inline tails vary; strict RSS parity remains unmet in some cases; framework release pending | [Immutable adoption and acquisition watermarks](audits/2026-10-24-browser-media-regression.md#final-build-adoption-v2) |
| 2026-10-24 | `3a07e529` versus `9f914739`, both with framework 0.29.0; fix on `9f914739` with an unreleased framework change | automatic shared-memory negotiation raised combined Rozi video CPU from 12–13% to 110% of one core and forced files dropped to 49.6 frames/s; snapshot relay and worker wakeup restore 59.8 frames/s at 16.4–16.9%, and 59.6 instead of 34.7 frames/s on incompressible video | [Browser consuming-media regression](audits/2026-10-24-browser-media-regression.md) |
| 2026-10-14 | `a6ba03fc` with framework `113ac1b` plus local resize fixes | resize upload gap falls from 183.5 ms to 17.0 ms at 60 Hz and 6.9 ms at 240 Hz; static native pictures survive resize and match composed pixels | [Graphics resize latency](audits/2026-10-14-graphics-resize.md) |
| 2026-10-14 | `c31f5499` with framework `49ae792`; composed fix `746eaa2` | every client frame presented at 60–240 Hz (baseline dropped 3–9%, browser patches up to 64%); host traffic 11–133× lower; five pipeline defects fixed during the audit; TuiOS capped near 60 frames/s in comparison | [Graphics pipeline acceptance](audits/2026-10-14-graphics-pipeline.md) |
| 2026-10-11 | `60742a5a` with framework `f1e6085b` and `81392dec` | direct placement reduces generated-workload composition; Raffles lookup fix reduces client CPU by 55%; shipped in framework 0.22.2; generated file-reuse limitation recorded in audit | [Native Kitty patch forwarding](audits/2026-10-11-native-kitty-patches.md) |
| 2026-10-08 | `b5996d8d`; selected comparison with `v0.0.29` | 139 timing cases, 109 memory scenarios, allocation/effect probes, replay-memory checks, and saturation recovery passed; no material regression in eight compared cases | [Full benchmark baseline on Ryzen 7 9700X](audits/2026-10-08.md) |
| 2026-10-08 | `8d7c82a8` plus local framework and rozi changes | 475×116 body + title client CPU drops 2.3% to 0.2% of one core; sparse snapshot time drops 87–96%; framework release pending | [Active TUI improvements](audits/2026-10-08-active-tui-improvements.md) |
| 2026-10-07 | `8d7c82a8` plus benchmark and documentation additions | investigation: animated titles force full UI rebuilds; sparse snapshots still scale with viewport area; runtime unchanged | [Active TUI panes and animated titles](audits/2026-10-07-active-tui.md) |
| 2026-09-15 | Alacritty `d692748d` plus archived port `e8e5aee6` | opt-in port halves Alacritty PSS with short-line history and is faster on short lines; other vtebench within 3%; upstream closed the proposal; not adopted | [Compact scrollback in Alacritty](audits/2026-09-15-alacritty-port.md) |
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
