# Session wire fixtures

These fixtures check that rozi builds from different releases can keep talking to each other. The
rules they enforce are in [`dev/wire-compatibility.md`](../../../dev/wire-compatibility.md).

- `schema/gen-<N>.schema.json` is the session wire schema that generation `N` was introduced with,
  copied from `schemas/session-wire.schema.json` when the generation was introduced and never
  edited afterwards. `tests/suites/contracts/wire_compat.rs` compares the current schema with it.
- `current/client/<type>.json` and `current/server/<type>.json` hold one example of every message
  variant this build has, named by its `type`. A new variant needs a new file here.
- `<release>/` is a copy of `current/` taken at that release, with a `manifest.json` naming its
  generation (see `dev/release-process.md`). Every build decodes every release directory whose
  generation is still in its support window, so delete a directory once its generation leaves it.

The examples are hand-written JSON documents. `tests/suites/contracts/wire_corpus.rs` decodes each
one through the production types, re-encodes it, and validates what a build sends to a peer with
no optional capabilities against every frozen schema in the window.
