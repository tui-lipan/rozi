#!/usr/bin/env bash
# Install the Zig that builds libghostty-vt into DEST (default /opt/zig) on Linux and print the
# executable's path. For the manylinux release containers, which cannot run the setup-zig action.
# The tarball must match its pinned SHA-256; keep the version in step with
# .github/actions/libghostty-vt.
set -euo pipefail

version=0.17.1
dest=${1:-/opt/zig}
case "$(uname -m)" in
  x86_64) sha256=3581863e3cbcfd0fce0efc682ce7e9e93da96668a977791d356a26d2a7e9b17a ;;
  aarch64) sha256=a1546d5646eb184437918d6635ca6b762a0e889eac09cef720ee7a1ed3fe7795 ;;
  *) echo "install-zig.sh: no pinned Zig for $(uname -m)" >&2; exit 1 ;;
esac
# Zig ships .tar.xz; a minimal manylinux image may lack xz.
command -v xz >/dev/null || dnf install -y -q xz >&2
name="zig-$(uname -m)-linux-$version"
tarball=$(mktemp)
trap 'rm -f "$tarball"' EXIT
curl --proto "=https" --tlsv1.2 -fsSL -o "$tarball" "https://ziglang.org/download/$version/$name.tar.xz"
echo "$sha256  $tarball" | sha256sum -c - >&2
mkdir -p "$dest"
tar -xJf "$tarball" -C "$dest" --strip-components=1
"$dest/zig" version >&2
echo "$dest/zig"
