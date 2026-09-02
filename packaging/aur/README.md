# AUR packaging

Two packages, one product. They `conflict` with each other, so exactly one can be installed.

| Package | Source | For |
| --- | --- | --- |
| `rozi-bin` | the signed release archive | most people: no Rust toolchain, no compile |
| `rozi` | the repository tarball at the tag | people who want it built on their machine |

Neither is a *managed* installation. pacman owns `/usr/bin/rozi`, so `rozi update` declines and says
so rather than fighting your package manager — see "Installs rozi does not manage" in
`docs/installation.md`. That is the intended behaviour for a distribution package.

## Why `rozi` builds from the repository tarball, not the crate

The tag tarball is the exact tree upstream CI builds, tests, and releases from, so a source build
here matches what upstream verified.

This used to be a correctness requirement rather than a preference: rozi carried a
`[patch.crates-io]` for `termina`, whose mouse decoder panicked the input worker on a report at
column or row 0, and **cargo strips `[patch]` when publishing** — a build from the crate would have
silently contained that panic. tui-lipan 0.4.1 requires a fixed `termina` on its own, so the patch
is gone and `cargo install rozi` is a supported install path again.

## Publishing

You need an AUR account with an SSH key registered on it. Each package is its own git repository.

```bash
git clone ssh://aur@aur.archlinux.org/rozi-bin.git
cd rozi-bin
cp /path/to/rozi/packaging/aur/rozi-bin/{PKGBUILD,.SRCINFO} .
git add PKGBUILD .SRCINFO
git commit -m 'Update to 0.0.2'
git push
```

Same for `rozi`. Only `PKGBUILD` and `.SRCINFO` belong in an AUR repository — never build output,
`src/`, `pkg/`, or a tarball.

## Updating for a new release

1. Set `pkgver`, reset `pkgrel=1` in both `PKGBUILD`s.
2. Refresh the checksums:
   - `rozi-bin`: the published `.sha256` sidecars, which are release assets —
     `curl -sL https://github.com/tui-lipan/rozi/releases/download/v<VER>/rozi-<VER>-<TARGET>.tar.gz.sha256`
   - `rozi`: `makepkg -g` in the `rozi/` directory, or hash the tag tarball directly.
3. Regenerate both `.SRCINFO` files — the AUR reads *these*, not the `PKGBUILD`, so a stale one
   publishes the wrong version:
   ```bash
   cd packaging/aur/rozi     && makepkg --printsrcinfo > .SRCINFO
   cd ../rozi-bin            && makepkg --printsrcinfo > .SRCINFO
   ```
4. Build both in a clean chroot before pushing. `makepkg` on a developer machine reuses whatever is
   already installed and will not catch a missing `depends`:
   ```bash
   extra-x86_64-build          # from devtools
   ```
5. `namcap` the resulting packages, then push.

## Notes on the build flags

`options=('!lto')` on `rozi` is load-bearing. makepkg's LTO adds `-flto` to the C flags, and `ring`
— reached through the release verification path — then compiles its C sources to LLVM bitcode the
Rust link cannot resolve, failing on every `ring_core_*` symbol. The release profile already applies
thin LTO across the Rust side, which is where rozi's own code is.

`options=('!strip')` on `rozi-bin` is likewise deliberate: the release profile already stripped the
binary, and there is no source tree to produce a debug package from.
