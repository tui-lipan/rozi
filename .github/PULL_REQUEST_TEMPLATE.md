## Summary

<!-- Explain what changed, why it is needed, and the user-visible effect. Release notes are written
from this section. Put breaking changes under `### Breaking changes` here, with migration steps. A
`!` title requires that section. -->

## Verification

<!-- List exact commands and manual checks. State any check you could not run. -->

## Checklist

<!-- Keep this heading separate from Verification. Keep checklist items consecutive, with no
blank lines between them. Put detailed test results and limitations in Verification above. -->

- [ ] Focused tests cover the affected behavior.
- [ ] `cargo fmt --all -- --check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, and
      `git diff --check` pass.
- [ ] User documentation is updated, or this change has no documentation effect.
- [ ] Affected platform behavior was tested on Linux, macOS, or Windows, or the untested risk is
      described above.
- [ ] `cargo deny check licenses sources advisories bans` and `cargo audit` pass if dependencies
      changed.
- [ ] Every commit includes a DCO `Signed-off-by` trailer.
