# Leaves a small uncommitted change in the demo checkout so Git views have something to show.
set -e
cd "$1"
python3 - <<'PY'
p = "src/layout/mod.rs"
s = open(p).read()
s = s.replace("//! Window-manager layout: tile trees and their geometry.",
              "//! Window-manager layout: tile trees, their geometry, and how panes share space.", 1)
open(p, "w").write(s)
PY
printf '\n- Captured with tools/docs-captures.\n' >> docs/overview.md
