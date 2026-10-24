# Shared layout and pane authority

`SharedLayout` is the semantic document for shared placement. Its edits define layout kind,
master share, floating placement, fullscreen exclusivity, split share, Scrollable width,
workspace transfers, swaps, and removal. Both control endpoints invoke those edits. The UI
resolves interactive geometry and applies focus and animation policy locally.

`reconcile_controller_layout` updates existing panes and workspace structure after a controller
edit. It preserves parser instances, focus, active workspace, viewport anchors, gestures, and
animation state. Callers apply the operation's local consequences explicitly. Followers and
attach use `apply_shared_layout`, which also handles incoming pane lifecycles, orphan output,
and authoritative revision bookkeeping. A controller edit does not simulate a foreign commit.

Interactive workspace adapters use the same document edits without imposing shared-session
validation on local scratch panes. Their unit canvas keeps explicit cell rectangles exact.
Pointer hit testing, split-axis selection, and whole-cell resize calculations remain UI policy.

## Atomic shared creation

Protocol 38 carries `SpawnPlacementCommit` with each shared `SpawnPane`: an optimistic base
revision and the complete resulting document. Local owner-scoped panes carry no placement.
The server checks controller authority and read-only state, then revision, document structure,
and the prospective pane set before starting a PTY. The requested id and generation must appear
exactly once. Every other live shared pane must remain placed with its current generation;
unknown additional panes are rejected. A known exited pane may remain placed with its recorded
generation; omission is allowed only for exited panes. Attach preserves retained exit status.

The event loop executes spawn and publication together. Spawn failure publishes no revision.
Success installs the pane and document before returning any messages or processing PTY events.
Followers receive pane metadata, including initial grid and cell geometry, before the single
layout revision. A lost response does not undo an accepted transaction, and disconnect cannot
leave a successful pane unplaced.

Headless `new-pane` resolves its explicit append policy and invokes the same transaction with
server authorship. It cannot bypass a controller. Initial UI and profile panes are published
serially, with only confirmed server panes and the current creation in each document. Only one
shared spawn is in flight per attachment. Ordinary layout publications wait behind that
transaction and the remaining creation queue, so they cannot publish unstarted placeholders.

A successful `LayoutCommitted` confirms the new base before the next request starts. An
attached client's failed shared spawn returns `SpawnResult` followed by `LayoutRejected`,
including PTY startup and authority failures. The client removes the failed placeholder and rebases from the reported
revision before attempting the next independent pane. Losing control pauses the queue until
control returns. Foreign layout corrections preserve unsent placeholders while applying the
authoritative structure to existing panes. Fresh-session seeding replaces older deferred
requests with the newly assigned generations. Server-reported identities remain
available when a control handoff clears the client's optimistic document cache.

A populated session without a committed layout is invalid for current peers. Attach refuses
that state instead of reconstructing workspace 0. Older snapshot capacity metadata remains
readable; protocol negotiation accepts only this build's exact wire version.

## History capacity

`ServerPane.scrollback` records the capacity selected at creation. Rebuilding an empty screen
for a resize uses that recorded value. Server spawn-policy reload reads the server's own config
and affects future panes only; headless creation refreshes policy at its existing point of use.
No client sends a replacement capacity for an existing pane.

`PaneMeta.scrollback` accompanies attach and `PaneMetadata`. Clients retain capacities by pane
id and generation before applying a layout that creates parsers. A local placeholder adopts
server capacity before output; a new generation adopts its own capacity before rebinding.
Attach/reconnect metadata precedes replay. Snapshot metadata stores the capacity per pane and
restore creates each screen with that value, falling back to server config for older snapshots
that have no capacity field.

Regression coverage exercises both control dispatchers, authority and revision rejection,
invalid placement, spawn failure, disconnect boundaries, first and later panes, metadata order,
failed middle startup panes, stale and authority rejection recovery, retained exited panes,
client configuration differences, new generations, attach/reconnect, config reload, and
resurrection. Session protocol, multi-client, headless control, and real PTY suites also run
through the atomic request shape.
