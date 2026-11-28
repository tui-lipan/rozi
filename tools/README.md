# Developer tools

Maintained tooling for working on rozi. None of it ships with rozi. Put a new tool in the folder
for its purpose, with its tests beside it.

| Folder | Holds |
| --- | --- |
| `bench/` | Graphics, memory, and CPU benchmark harnesses; see [Benchmarks and profiling](../dev/benchmarks.md) |
| `conformance/` | Cross-checks against other rozi repositories |
| `docs-captures/` | The sandbox and scenes behind the website's screenshots and clips |
| `release/` | Release notes, third-party notices, and the pinned Zig install used by CI |
| `ui-sketches/` | Headless renders of rozi chrome for visual review (`--example ui_sketches`) |

Archived performance experiments live in [`dev/performance/experiments/`](../dev/performance/experiments/),
and the usage counter Worker in [`services/usage-counter/`](../services/usage-counter/).
