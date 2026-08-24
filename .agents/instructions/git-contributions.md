# Git and contributions

## Worktree safety

- Preserve unrelated changes. Never revert files you did not change.
- Do not sweep untracked local scratch files or unrelated `.agents/` material into commits.
- Before committing, inspect `git status --short`, `git diff --stat`, and `git log --oneline -10`.
- Stage explicit paths and run `git diff --check`.
- Do not force-push or run destructive Git commands without explicit approval.

## Commits and DCO

Use concise conventional prefixes, for example `fix: improve toast confirmation behavior` or
`feat: live reload config changes`.

Every commit needs a `Signed-off-by` trailer matching its author identity. The tracked
`.githooks/prepare-commit-msg` hook adds the repository maintainer's trailer automatically. The
tracked `.githooks/pre-commit` hook additionally refuses a `Cargo.lock` whose entries have lost
their registry `source`, which is what a local `[patch.crates-io]` in a gitignored
`.cargo/config.toml` does to it. Regenerate the lockfile with that override moved aside rather than
committing through with `--no-verify`; `--locked` CI fails on such an entry immediately. Other
contributors should use `git commit -s` with their own identity. See `CONTRIBUTING.md` and `DCO`.

Repair a missing sign-off automatically when that is the only history change:

```bash
git commit --amend -s --no-edit
git rebase --signoff <base>
```

This permission covers DCO repair, not rewriting content or authorship. If repaired commits are
already pushed, do not force-push without explicit approval.

Contributions use inbound equals outbound under MPL-2.0. The DCO records provenance; it does not
assign copyright or grant relicensing rights.

## Pushing and releases

Commit, push, tag, publish, or create a release only when the user asks. Before any of those actions,
inspect the exact commits and refs involved. Release archives include `README.md`, `LICENSE`, and
`examples/`; `.github/workflows/release.yml` is the source of truth for the current matrix and
signing flow.
