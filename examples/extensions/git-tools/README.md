# Git tools extension

This third-party-style example uses only Python's standard library, Git, the public extension
environment, and structured Rozi CLI argument arrays. It provides:

- `git-tools.branches` — grouped local branches with refresh, create, switch, and confirmed delete;
- `git-tools.worktrees` — grouped worktrees with refresh, create, safe confirmed remove, and
  activation into a new focused pane.

## Requirements

- Rozi with extension API 1 and the streaming `rozi pick --json` protocol;
- Python 3.10 or newer available as `python`;
- Git 2.36 or newer (`git worktree list --porcelain -z`);
- a focused pane inside a non-bare Git worktree.

No Python packages, shell, `jq`, or Rozi source imports are used. Paths and refs are passed as
individual process arguments; NUL-delimited Git output preserves spaces and Unicode.

## Install

Validate and link the example checkout. Rozi registers it by its manifest identity,
`id = "git-tools"`.

```bash
rozi extensions check ./git-tools
rozi extensions install --link ./git-tools
rozi run-action reload-extensions
```

## Controls and safety

Both pickers use `r` to refresh. Mutations replace the rows in the open picker, so the changed state
is visible without closing and reopening it.

Branch picker:

- `Enter` switches to the selected branch.
- `Ctrl-N` prompts for a branch name and creates it at `HEAD` without switching.
- `Ctrl-D` twice deletes the highlighted branch with Git's non-forcing `-d`.
- The current branch and branches checked out in another worktree cannot be selected or deleted.
- `main`, `master`, `trunk`, and a remote's symbolic default branch are protected from deletion.
- When the current worktree is dirty, branch switching is disabled. Refresh after cleaning it.

Worktree picker:

- `Enter` opens the selected worktree in a new focused pane.
- `Ctrl-N` prompts for a new branch and creates a sibling worktree named
  `<primary-directory>-<branch>`. Slashes in the branch become dashes in the directory name.
- `Ctrl-D` twice removes the highlighted worktree.
- The current, primary, locked, dirty, and missing worktrees cannot be removed. Removal never uses
  `--force`.

## Repeatable manual check

Use a disposable repository so every action can be exercised:

```bash
tmp=$(mktemp -d)
git -C "$tmp" init -b main demo
git -C "$tmp/demo" config user.name "Rozi Example"
git -C "$tmp/demo" config user.email "rozi@example.invalid"
git -C "$tmp/demo" commit --allow-empty -m initial
cd "$tmp/demo"
rozi run-action git-tools.branches
```

In the branch picker:

1. Press `Ctrl-N`, enter `feature/space-ü`, and confirm the new row appears.
2. Select it with `Enter`, reopen the picker, and verify it is grouped under `Current`.
3. Run `touch "dirty ü.txt"`, reopen the picker, and verify other branches say `Dirty tree`.
4. Run `rm "dirty ü.txt" && git switch main`, reopen the picker, highlight the feature branch,
   press `Ctrl-D` once to arm and again to delete, then verify the row disappears.
5. Press `r` after making a branch change in another pane and verify the open list updates.

Then open the worktree picker:

```bash
rozi run-action git-tools.worktrees
```

1. Press `Ctrl-N`, enter `topic/ü`, and verify a sibling `demo-topic-ü` worktree appears.
2. Select it with `Enter` and verify Rozi opens it in a focused pane.
3. Run `touch "$tmp/demo-topic-ü/dirty ü.txt"`, reopen the picker from the original worktree, press
   `r`, and verify the topic row reports `dirty`.
4. Verify confirmed remove is rejected, run `rm "$tmp/demo-topic-ü/dirty ü.txt"`, refresh, then
   press `Ctrl-D` twice and verify the row and directory disappear.
5. Press `Esc` at either picker and verify it closes without an error notification.

Remove the disposable repository and any sibling worktrees when finished:

```bash
rm -rf "$tmp"
```
