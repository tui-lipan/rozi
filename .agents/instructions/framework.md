# tui-lipan and dependency sources

Rozi normally uses the released `tui-lipan` crate. `Cargo.lock` must record the registry source and
checksum expected by the manifest.

Use the sibling `../tui-lipan` repository when a missing capability is reusable framework behavior,
not Rozi policy. Inspect its status separately and preserve work in both repositories.

## Unreleased framework work

A committed git dependency must point to a pushed revision. A commit that exists only in the sibling
working tree is unreachable to CI. Prefer a revision on the framework's default branch, and let a
dependency-resolving Cargo command update the lockfile. Never leave the manifest revision and lock
source disagreeing.

For local iteration only, `.cargo/config.toml` may contain the ignored override:

```toml
[patch.crates-io]
tui-lipan = { path = "../tui-lipan" }
```

The override rewrites the tui-lipan lock entries into source-less path form. Do not stage those
changes. If `Cargo.lock` was clean before your work and only the override changed it, restore your
own lockfile change before handoff. Never discard pre-existing lockfile changes.

At framework release time, publish the framework, update Rozi's version requirement, remove any
committed git patch, and confirm the lockfile again has registry sources and checksums. Preserve
unrelated dependency patches such as the documented `termina` revision.

## Cross-repository checks

For framework terminal changes, run in `../tui-lipan`:

```bash
cargo check --features terminal
cargo clippy --features terminal
```

Then rerun the relevant Rozi tests and lints. CI checks out only Rozi, so no pushed change may depend
on an uncommitted sibling checkout.
