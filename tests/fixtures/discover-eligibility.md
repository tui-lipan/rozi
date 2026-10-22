# Discover eligibility conformance

`discover-eligibility.json` contains complete TOML manifests and their expected catalog eligibility.
Each case assumes a root `extension.toml` in a public, active GitHub repository with the
`rozi-extension` topic. `install_valid`, when present, also asserts the CLI exit status independently
of Discover eligibility.

Run the Rust preflight through the real CLI:

```sh
cargo test --locked --test discover_conformance
```

Run the same cases through an existing checkout of
[tui-lipan/rozi-extension-index](https://github.com/tui-lipan/rozi-extension-index):

```sh
python scripts/check_discover_conformance.py ../rozi-extension-index/scripts/build_index.py
```

The Python runner imports the supplied generator and calls `catalog_entry` with synthetic repository
metadata. It does not fetch GitHub data or execute extension code. The corpus and runner can also
be used by the index repository's tests via the runner's `--corpus` option. Add eligibility edge
cases here so both implementations can check the same inputs.
