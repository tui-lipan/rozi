# Terminal engines

rozi's panes run on tui-lipan's `TerminalScreen`, which has two emulator engines:
`alacritty_terminal` (the default) and the experimental libghostty-vt engine. The Ghostty engine is
compiled in only with the non-default `ghostty-engine` feature; release and nightly builds enable
it. Users select it with `[experimental] terminal_engine = "ghostty"` (see
[configuration](../docs/configuration.md#experimental)).

## Building

The feature builds libghostty-vt from the vendored `tui-lipan-ghostty-vt-src` crate with **Zig
0.17.x**, on `PATH` or named by `ZIG`. The first build takes about 1.5 minutes on Linux and is
cached in `target/ghostty-vt/`. tui-lipan's `docs/widgets/terminal.md` lists the build variables.

```bash
cargo build --features ghostty-engine
```

Release and nightly builds set `GHOSTTY_VT_GLIBC=2.28` inside the manylinux containers and
`MACOSX_DEPLOYMENT_TARGET=13.0` on macOS, and check both floors on the result. The Linux containers
install Zig through `scripts/install-zig.sh`, which pins its checksum.

## How the engine is chosen

`src/pane/engine.rs` owns the choice. The session server and each UI client read the key once at
startup and keep that engine for the life of the process. They need not agree: output, attach
replays, resurrection seeds and upgrade handoffs cross between processes as VT bytes. `rozi doctor`
prints the engine the config selects, and `rozi --version` lists the engines a build contains.

On the Ghostty engine, a pane whose output has been quiet for two seconds has its history
compressed in small steps, in the server and in each client (`src/pane/idle_compression.rs`).

## Tests

Unit tests build their screens through the same switch. Run the lib suite on Ghostty with:

```bash
ROZI_TEST_TERMINAL_ENGINE=ghostty cargo test --features ghostty-engine --lib
```

CI's `ghostty engine` job does this on Linux. Integration tests under `tests/` start real
processes, which read the scratch config and so run on Alacritty.

## Third-party notices

Builds with the feature link libghostty-vt and its bundled dependencies. `THIRD_PARTY_NOTICES.md`,
shipped in every release archive, is generated from the locked `-src` crate:

```bash
python3 scripts/third_party_notices.py
```

CI fails when it is out of date, so rerun it after updating tui-lipan.
