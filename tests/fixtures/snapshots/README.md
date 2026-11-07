# Snapshot corpus

`v3/integrity` comes from the production `capture_snapshot` and `write_snapshot_job`
paths. The ignored `generate_snapshot_v3_fixture` test regenerates it:

```sh
CARGO_BUILD_JOBS=6 cargo test --lib generate_snapshot_v3_fixture -- --ignored
```

The capture uses three panes with interactive-shell, shell-command, and direct-argv
launch intent. Its timestamp is fixed, and it contains no user paths, hostnames,
usernames, or native agent session identifiers. The shell pane records an observed
foreground command. The normal fixture test compares the production writer's
metadata and replay bytes with this committed snapshot.

`v2/integrity` uses the same replay files and launch intent, with metadata reduced
to the format emitted before commit `bf8d01e2` added format 3. Its version is 2,
`created_from_profile` is null, and it omits `foreground`, `agent_resume`, and
`scrollback`, which that writer did not emit. The schema was checked against
`git show bf8d01e2^:src/session/server/resurrect.rs`. It is a historical-schema
fixture, not a capture made by running an old binary.

Tests read both directories through production completeness, discovery, migration,
and interrupted-backup recovery functions. Reads and recovery preserve the
original metadata bytes. The subprocess test additionally restarts a real session
from format 2 and checks its layout, history, and per-pane scrollback.
