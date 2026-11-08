# Session wire compatibility

A rozi binary keeps working with session servers, clients, and CLI or hook callers built from other
releases. A missing feature is disabled with a reason; a connection never goes down because the two
builds differ. A running session is restarted only when the user asks to upgrade it, or after a
generation bump.

This guide is the rulebook for every change to the session wire protocol, the shared layout
document, and the resurrection snapshot. The code lives in `src/session/protocol/`.

The builds that have to keep working together:

| Peer A | Peer B | Example |
| --- | --- | --- |
| new client | old server | updated laptop, VPS session started last week |
| old client | new server | second machine not updated yet, attached as a follower |
| new and old client | one server | controller and follower on different releases |
| new CLI or hook | old server | `rozi agents report` from a Claude hook after `rozi` was updated |
| new client | old server over SSH | the proxy is the client's version, the server is not |

## Two tiers: generation and capabilities

`PROTOCOL_VERSION` is the **wire generation**. Two builds can talk when the newest generation both
speak is inside both windows; `negotiate_protocol` picks it, and the handshake reply reports it as
`effective_protocol`.

- A generation bump is a breaking change. It is marked `!` in the commit or PR title, with a
  `### Breaking changes` section, and development builds follow the same rule: there is no bump per
  commit. Expect one or two a year at most.
- `MIN_SUPPORTED_PROTOCOL` trails the current generation by the **support window**: a binary speaks
  the current generation and the previous one for at least six months after a bump ships. Adapters
  for the previous generation live in one module, `src/session/protocol/compat/gen_<n>.rs`, and are
  deleted when the window closes.
- **Everything else is a capability**: a new message variant, a new frame kind, a new value in a
  closed vocabulary, or a new meaning for an existing field. A capability set is an open set of
  strings on the wire. In code every name is a `Capability` variant with `as_str`, so names cannot
  drift. `Capabilities::negotiated` keeps the names both ends know; a peer that sends no set gets
  none of the optional work.
- Capabilities of the control API (`rozi api describe`) are a separate enum, `ControlCapability`.
  They describe what a whole binary accepts from control clients and extensions, not what one
  session connection negotiated.

### The frozen handshake

Every past and future build must parse the handshake, so it never changes shape, even across
generations. Fields may only be added. It consists of:

- `ClientMessage::Attach`, `ClientMessage::Query`, and `ClientMessage::SessionControl`, with
  `protocol_version`, `min_protocol_version`, and `capabilities`;
- `ServerMessage::Attached`, `ServerMessage::SessionInfo`, and `ServerMessage::Error` with
  `protocol-mismatch` (`PROTOCOL_MISMATCH_ERROR_CODE`);
- the remote preamble in `src/session/remote/preamble.rs`.

A `protocol-mismatch` refusal carries the server's `rozi_version`, `protocol_min`, and
`protocol_max` (`PeerRelease`). Every build from generation 52 sends them, so a refusal without
them comes from an older server. Upgrade session relies on this to replace only an older session,
never a newer one it cannot talk to.

`Attached` and `SessionInfo` carry the server's `rozi_version` so a client can name the release a
session runs.

## Evolution rules

Every change to a wire type follows this table. The schema classifier enforces it.

| Change | Mechanism | Generation bump? |
| --- | --- | --- |
| New optional field, receiver works without it | `#[serde(default)]` and skip when unset | no |
| New optional field whose absence changes the outcome | capability; the sender checks it first | no |
| New message variant (either direction) | capability; send gate | no |
| New value in a closed vocabulary (enum, error code, event name) | capability, plus an `Unknown` fallback on receive | no |
| Same shape, new meaning | capability; the receiver branches on it | no |
| New binary frame kind | capability; the ignorable bit if a receiver may skip it | no |
| New field on `SharedLayout` or the snapshot | capability if the server must enforce it; the snapshot gets a migration | no |
| Rename, remove, type change, new required field, framing change | new generation plus an adapter for the previous one | **yes** |

Never add `deny_unknown_fields` to a wire type: an older peer has to ignore fields it does not
know. A field that only makes sense together with a capability is still `#[serde(default)]`, so
the receiving side never depends on the sender having checked.

## The send gate

`ClientMessage::required_capability` and `ServerMessage::required_capability` are exhaustive
matches with no wildcard arm. Adding a variant does not compile until its author decides what it
needs. Every variant in the generation's baseline answers `None`; every variant added later answers
the capability that introduced it.

- The server writes every message through one gate per client, which checks the negotiated
  capabilities. An unsupported broadcast or event is dropped for that client; an unsupported reply
  becomes a typed refusal. It is never written. In code, `encode_control` tags each frame with the
  message's `required_capability`, and a client's outbox (`ClientConn::admits`) refuses a frame its
  client did not negotiate; every server write reaches the socket through that outbox. A reply
  addressed to the sender's own request becomes `Error { code: "unsupported-message" }`.
- The client's writer applies the same gate to requests.
- `SessionClient::supports(Capability)` and the matching method on headless and session-control
  connections are the only feature checks UI and CLI code use. An action whose capability the
  connected session lacks stays visible but is disabled with the reason "needs this session on
  rozi ≥ X".

## Tolerant decoding

The send gate is the main mechanism; decoding is the backstop for bugs and for peers outside the
gate.

- `ClientMessage`, `ServerMessage`, `ControlCommand`, and the vocabulary enums a peer receives end
  in a `#[serde(other)] Unknown` unit variant.
  - A server answers an unknown request with `Error { code: "unsupported-message" }`
    (`UNSUPPORTED_MESSAGE_ERROR_CODE`) and keeps the connection open.
  - A client ignores an unknown server message.
  - An unknown vocabulary value is shown as neutral, never as an error.
- A vocabulary that is also part of the public control API (`AgentState`, `AgentAuthority`,
  `PublisherKind`) stays closed, so the control schema does not change. So does one only the
  server produces and matches on widely (`DetectedAgentState`). The session wire fields that carry
  one decode through `protocol::tolerant`: an unknown value becomes the type's neutral reading (an
  agent state reads as a run still going, the rule a custom status follows). `EventKind` has an
  `Unknown` variant outside `EventKind::ALL` and a hand-written schema, so the public list of event
  names stays as it is; a client ignores an event of a kind it does not know.
- `Unknown` is never sent. `required_capability` panics in debug builds when asked about it.
- Binary frames: the high bit of the kind byte (`0x80`) means "a receiver may skip this frame". An
  unknown kind with the bit set is skipped; an unknown kind without it is still fatal.

## Documents: shared layout and snapshot

The layout document and the resurrection snapshot have versions of their own, and follow the same
rules: an additive change does not bump the version.

- `SharedLayout::validate` accepts every document version in its window, from
  `MIN_SHARED_LAYOUT_VERSION` to `SHARED_LAYOUT_VERSION`. A layout feature that the server must
  enforce or keep is gated on the server's capabilities: an older server re-encodes the document
  through its own types and drops fields it does not know before followers see them. A controller
  does not offer such a feature when the server lacks it.
- A snapshot is read through migration functions, `vN → vN+1`, applied in sequence across the
  window, and always written in the current format. A damaged snapshot is kept, and a snapshot from
  a newer format is refused. This is what makes Upgrade session (restart from snapshot) safe across
  a format change.

To change the snapshot format, raise `SNAPSHOT_VERSION` in `src/session/server/resurrect.rs` and
append a `vN_to_vN+1` migration to the `STEPS` table in `resurrect/migrate.rs`. A compile-time
assertion requires one migration per format step from `OLDEST_READABLE_SNAPSHOT_VERSION`. Raise
that constant, and delete the oldest migration, only when the format leaves the window. Migrations
run on the decoded metadata in memory; the files on disk are left as they are until the next write.
Add a fixture under `tests/fixtures/snapshots/v<N>/` produced by the production writer (see the
README there) and keep the older fixtures, which the tests read through discovery, restore, and
interrupted-backup recovery.

## Verification

- **Schema baseline.** `schemas/session-wire.schema.json` is generated from the wire types by
  `cargo run --features schema-gen --bin rozi-api-schema`. CI fails when it is out of date, so a
  wire change shows up as a diff in the same pull request. `tui_lipan::Color` and
  `tui_lipan::ProgramStatusSnapshot` are opaque in the schema; the golden corpus pins them.
- **Compatibility classifier.** `tests/suites/contracts/wire_compat.rs` compares the generated
  schema with `tests/fixtures/wire/schema/gen-<N>.schema.json`, the frozen schema of the current
  generation, and flags anything the evolution rules forbid: a removed or renamed field or
  variant, a type or nullability change, a new required field, a closed vocabulary that grew, a
  value added to a tolerant vocabulary that `GATED_VOCABULARY` does not list with its capability,
  a removed capability, or a new message variant whose `required_capability` is `None`. A flagged
  change needs a generation bump. Every generation in the window must have a frozen schema.
- **Golden corpus.** `tests/fixtures/wire/current/` holds one JSON example per message variant,
  and each release copies it to `tests/fixtures/wire/<release>/`. The current build decodes every
  fixture of every release in the window, and every message it sends without optional capabilities
  validates against the frozen schema of every generation in the window
  (`tests/suites/contracts/wire_corpus.rs`).
- **Cross-version end-to-end tests** run real release binaries against the current build in the
  nightly and release-health workflows.

## Checklist for a wire change

1. Find the row in the evolution rules. If it needs a capability, add a `Capability` variant (and
   to `Capability::ALL`), offer it in `Capabilities::current`, and return it from
   `required_capability` for a new variant, or list a new vocabulary value in `GATED_VOCABULARY`.
2. Gate the sender: check `supports` before sending, and degrade or disable the feature with a
   reason when the peer lacks it.
3. Regenerate the schema and commit it with the change.
4. Add a golden fixture for any new variant in `tests/fixtures/wire/current/`.
5. If the classifier says a generation bump is needed, read the next section first.

## Bumping the generation

1. Raise `PROTOCOL_VERSION`. Leave `MIN_SUPPORTED_PROTOCOL` on the previous generation.
2. Add `src/session/protocol/compat/gen_<previous>.rs` with the adapters that translate between the
   two shapes, and route the previous generation's connections through it.
3. Regenerate the schema and freeze it as `tests/fixtures/wire/schema/gen-<new>.schema.json`. Keep
   the previous generation's frozen schema until the window closes.
4. Mark the change `!` and describe what breaks in `### Breaking changes`.
5. When the window closes, raise `MIN_SUPPORTED_PROTOCOL` and delete the adapter module.
