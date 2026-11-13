# Developer guides

This directory is for people working on rozi itself. User documentation lives in
[`docs/`](../docs/index.md) and is published at [rozi.tui-lipan.dev](https://rozi.tui-lipan.dev);
nothing here is part of that site. Start with [CONTRIBUTING.md](../CONTRIBUTING.md) for setup,
checks, and sign-off.

| Guide | Covers |
| --- | --- |
| [Benchmarks and profiling](benchmarks.md) | Benchmark targets, comparing runs, the memory harness, and profiling |
| [Performance archive](performance/README.md) | Dated performance audits and their recorded verdicts |
| [Performance audit playbook](performance/audit-playbook.md) | Running and recording a new audit |
| [Session wire compatibility](wire-compatibility.md) | Wire generations, capabilities, the send gate, and the rules for changing the session protocol, layout document, or snapshot |
| [Terminal engines](terminal-engines.md) | The experimental Ghostty engine: building it, selecting it, testing on it, and its licence notices |
| [Release process](release-process.md) | Preparing, signing, publishing, and checking a release |
| [Image and file paste over SSH](ssh-clipboard-investigation.md) | Clipboard failure trace, Herdr comparison, and a proposed attachment transfer design |
