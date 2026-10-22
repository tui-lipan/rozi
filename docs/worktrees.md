# Worktrees

A Git worktree is an extra checkout of the same repository, with its own branch and working
directory. rozi can open each worktree in its own named [session](sessions.md), so work on two
branches keeps separate shells, layouts, and running processes, and you can switch between them
without stashing.

This page covers the **Worktrees** picker, creating, removing, and unlocking checkouts, the
`rozi worktrees` command, and the `[worktrees]` settings.

## Open a worktree

Open **Worktrees** from the command palette (`Ctrl+A`, then `p`) while a pane is focused in a Git
repository, or use the sidebar's [Worktrees tab](sidebar.md#worktrees), which lists the same
checkouts. The palette lists **Worktrees** only while the focused pane is in a Git repository.
**Worktrees** has no default command key.

The picker title names the repository, and the host when it is a remote one.

The picker lists the checkouts on the focused pane's session host, including remote hosts. `●`
marks the checkout the focused pane is in. Each row shows the branch, then `Primary`, `Locked`,
`Stale lock`, or `Prunable` when it applies, then its PR/CI status. Paths stay searchable without
appearing in rows. Press `Ctrl+C` or click **copy path** to copy the selected checkout's full
path, including a remote host's path, to your clipboard.

The list is current when the picker opens, and PR status refreshes about once a minute while it
stays open. Press `Ctrl+R` to refresh both now. If a lookup fails, the last results stay visible
marked `Stale`, and a missing tool or an authentication problem is explained in a
notification; fix it and press `Ctrl+R`.

Press `Enter` on a checkout to open it:

- With one associated session, rozi switches to it.
- With several, rozi lists them so you can choose.
- With none, rozi creates a named session, `wt-<branch>`, whose first shell starts in the checkout.
  A number is added when that name is taken.

A session is associated with a checkout when the checkout is its recorded origin — the checkout it
was created for. A pane that later changes directory into a checkout does not associate its session
with it.

Press `Ctrl+Enter` or click **pane** to open a shell in the selected worktree as a new pane in the
current session's workspace. Normal pane placement rules apply. This action requires a writable
client with layout control and is unavailable while the scratchpad is open.

| Key | Action |
| --- | --- |
| `Enter` | Open the checkout's session, or create one |
| `Ctrl+Enter` | Open a pane in the current session at the selected checkout |
| `Ctrl+C` | Copy the selected checkout's full path |
| `Ctrl+N` | Create a checkout from a branch and base revision, then open it in a new session |
| `Ctrl+R` | Refresh checkouts and PR/CI status |
| `Ctrl+K` twice | Remove a linked checkout |
| `Ctrl+U` | Unlock a locked checkout |
| `Esc` | Close the picker |

### Read work status

For GitHub repositories, install [GitHub CLI](https://cli.github.com/) and authenticate with
`gh auth login` on the host running the session. Remote sessions use that host's GitHub account.

The picker and the sidebar show the same status:

| Status | Meaning |
| --- | --- |
| `#114 · Passed` | CI passed for the current branch commit |
| `#113 · Failed` | CI failed for the current branch commit |
| `#115 · Running` | CI is pending or running |
| `#116 · Open` | Open PR without checks for the current branch commit |
| `#111 · Draft` | Draft PR |
| `#112 · Merged` | Merged PR, including squash merges |
| `#110 · Closed` | Closed without merging |
| `PR unavailable` | GitHub status could not be read |

Merged, closed, and draft states take precedence over CI. Passing or failing checks on an older
pushed commit are not shown for a newer local branch commit. PR status does not describe uncommitted
changes or imply a checkout is safe to remove. A branch without an associated PR has no work status.
Rozi matches a branch to the remote branch most likely to hold its PR, using the branch's push
remote, upstream, and push mode. When a branch pushes to the remote it tracks, a differently named
upstream branch is used unless `push.default` is `current` or `matching`. Otherwise rozi uses the
branch's own name on its push remote.
For same-named branches, rozi uses the newest matching PR; a PR from a repository other than the
push remote is matched only when its head commit matches the local branch. Merged and closed PRs also require a matching head commit,
so reusing a branch for new commits clears its old terminal status. Open and draft PRs stay
associated when the local branch is ahead, but CI waits for a matching commit.
Other hosting providers currently have no PR integration.

While the Worktrees sidebar is visible, work status refreshes about once a minute. `Ctrl+R` in the
picker checks immediately.

## Create a worktree

Press `Ctrl+N` in the picker to open the new-worktree form. Press `Ctrl+B` or click **branches**
to search local branches on the session host. Branches already checked out are dimmed and cannot
be chosen. Choose an available branch, or type a new name and choose its **create branch** row,
to fill the form. `Esc` returns to the form without changing the branch. Selecting a branch does
not create the checkout until you submit the form.

The form has three fields; `Tab` and `Shift+Tab` move between them.

| Field | Meaning |
| --- | --- |
| **Branch** | An existing local branch to check out, or a new branch to create |
| **Base** | The revision a new branch starts from. Defaults to `HEAD`. |
| **Path** | Where the checkout goes on the session host. Prefilled with the default location. |

The default path depends on `[worktrees] directory`:

| `directory` | Default path |
| --- | --- |
| Not set | `<repo>-worktrees/<branch>`, beside the repository |
| An absolute path | `<directory>/<repo>/<branch>` |
| A single folder name, such as `.worktrees` | `<repo>/<directory>/<branch>`, inside the repository |

Edit **Path** to use any other absolute path on the host.

### Keep Git from seeing an in-repository checkout

A checkout inside the repository appears in `git status`, and `git add -A` picks it up, unless Git
ignores its directory. When Git does not ignore it, the form shows a warning under the path. Press
`Ctrl+E` to add the directory to `.git/info/exclude`, which applies only to your clone.

rozi never edits the committed `.gitignore`, and creating a checkout does not change ignore rules on
its own. A checkout created without the rule shows the same warning afterwards.

### Seed new sessions with a profile

A new worktree session starts with one plain shell in the checkout; `[profile] default` does not
apply. To start with a layout instead, set `[worktrees] profile` to a [profile](profiles.md) name.

Pane directories in that profile that are inside any checkout of the repository are rebased onto
the new checkout. For example, a pane saved at `~/src/rozi/frontend` opens at
`~/src/rozi-worktrees/feat-login/frontend`. Directories outside the repository are kept, and panes
without a directory start in the checkout. The session records both the profile and the worktree as
its origin.

Profiles are local files. For a worktree on a remote host, the profile applies only when all of its
pane directories are inside the repository. Otherwise, rozi says so and starts the session as one
shell in the checkout.

## Remove a worktree

Select a linked checkout and press `Ctrl+K`, then press it again within three seconds to confirm.
Removal follows these rules:

- It never deletes a branch.
- It refuses the primary checkout and any checkout owned by a running or restorable rozi session.
  Stop or forget that session first.
- It refuses a locked checkout unless the lock is stale. On a stale lock, the confirming press
  unlocks and removes the checkout. To remove a checkout with any other lock,
  [unlock it](#unlock-a-worktree) first.
- If Git refuses because the checkout has uncommitted changes, press `Ctrl+K` again to force the
  removal. Forcing affects only that Git check, never a lock.

In the sidebar's Worktrees tab, the ✕ on a checkout follows the same rules.
During removal, the checkout shows an animated spinner and `Removing…` in the picker and sidebar
until the host finishes or reports an error.

### Delete the branch too

Removing a checkout keeps its branch unless `[worktrees] delete_branch` lists what to delete:
`["local"]`, `["remote"]`, or `["local", "remote"]`. Press `Ctrl+E` on Worktrees in **Sidebar tabs…**
to set it without editing the file. Nothing is ever force-deleted, and forcing a
dirty checkout does not change that.

- **`local`** deletes the local branch with `git branch -d`, so Git's own merge rule applies: the
  branch must be merged into its upstream or into the primary checkout's `HEAD`.
- **`remote`** deletes the branch on the remote it tracks, only when the branch tracks a branch of
  the same name, the fetched remote tip is merged into the primary checkout's `HEAD`, and nobody has
  pushed to it since you last fetched. A branch merged by squash or rebase is not an ancestor, so
  its remote copy is kept; delete it on the host or let the host do it when the pull request merges.

A branch that another checkout has checked out, and a detached checkout, are left alone. Rozi
reports each copy it kept and why. Remote deletion needs access to the remote without a prompt, and
gives up after 30 seconds.

A create or remove that has started finishes even if you close the picker, and rozi reports the
result. If a created checkout is reported in a toast, right-click it to copy the checkout path on
the session host.

## Unlock a worktree

Git can lock a checkout so that it is not removed or pruned. Coding agents such as Claude Code lock
the checkouts they create, and a lock stays behind when an agent exits without cleaning up, even
after its branch is merged.

The picker and the sidebar show a locked checkout as `Locked`. When the lock names a process with
`pid <N>` and that process is no longer running on the session host, the checkout shows as
`Stale lock`. A lock that names no process is never stale.

Select a locked checkout and press `Ctrl+U` to remove its lock. This works for any lock, so check
that nothing still uses the checkout before unlocking one that is not stale. Unlocking changes
nothing else: the checkout and its branch stay.

## Use worktrees from the command line

```bash
rozi worktrees list                         # checkouts of the repository around this directory
rozi worktrees list --cwd ~/src/rozi --format json
rozi worktrees create feat/login            # new branch from HEAD, at the default path
rozi worktrees create fix/ssh --base origin/main --open
rozi worktrees open feat/login
rozi worktrees remove feat/login --cwd ~/src/rozi
rozi worktrees unlock ~/src/rozi/.claude/worktrees/fix-ssh
rozi worktrees exclude                      # add the in-repository worktree directory to .git/info/exclude
```

| Command | Behavior |
| --- | --- |
| `list` | Lists checkouts, with `locked` or `stale-lock` in the state column. With `--format json`, prints a `worktrees` array; each entry includes the `sessions` whose recorded origin is that checkout, and a `lock` that is `null` or holds Git's lock `reason` and whether it is `stale`. |
| `create <BRANCH>` | Checks out an existing local branch, or creates it from `--base` (`HEAD` by default). Without `--path`, uses the default location. Prints the new checkout's path, or a `worktree` object with `--format json`. `--open` then opens it like `open`, and cannot be combined with `--format json`. |
| `open <CHECKOUT>` | Opens the checkout's session, or creates one. |
| `remove <CHECKOUT>` | Removes a checkout under the same rules as the picker, except that it refuses every locked checkout, stale or not. `--force` only lets Git remove a dirty checkout. `--delete-branch local`, `remote`, or `local,remote` overrides `[worktrees] delete_branch` for this removal, and `--keep-branch` keeps every branch. Deleted branches are printed; a kept one is reported on stderr. |
| `unlock <CHECKOUT>` | Removes the checkout's lock, whether or not the lock is stale. |
| `exclude [DIR]` | Adds the relative `[worktrees] directory`, or `DIR`, to `.git/info/exclude`. |

When `create` puts a checkout inside the repository in a directory Git does not ignore, it prints a
warning, and the JSON output marks the checkout as `unignored`.

### Name a checkout

`open`, `remove`, and `unlock` take a checkout as its branch or as any path inside it. A detached
checkout has no branch, so name it by path.

- An argument that starts with `./`, `../`, `/`, or `~` is always a path.
- Any other argument, such as `feat/login`, is looked up both as a path and as a branch. If only one
  matches, or both lead to the same checkout, that checkout is used. If they lead to different
  checkouts — for example, a `feat/login` directory in the primary checkout and a branch
  `feat/login` checked out elsewhere — the command fails and lists both. Write `./feat/login` for
  the path or `--branch feat/login` for the branch.
- `--branch <NAME>` matches only the branch, and replaces the argument.
- `--cwd <DIR>` sets the repository that branches are looked up in and that relative paths start
  from. Without it, both use the current directory. An absolute path is not limited to that
  repository.

`open` then chooses a session:

- With exactly one associated session, it attaches to that session.
- Otherwise, it creates a session named `wt-<branch>` whose first shell starts in the checkout and
  which records the checkout as its origin.
- When several sessions use the checkout, choose one with `--name <SESSION>`. A `--name` that is not
  yet associated with the checkout creates another session for it.

### Worktrees on a remote host

Add `--remote <HOST>` to run a `worktrees` command against a remote host. Paths resolve on the host
that owns the repository, so `~` and relative paths mean that host's home and working directory.
Quote `~` so your local shell does not expand it:

```bash
rozi --remote workbox worktrees list --cwd '~/src/rozi'
rozi --remote workbox worktrees create feat/login --cwd '~/src/rozi' --open
```

See [Remote sessions](remote.md).

## Configuration

| Key | Effect |
| --- | --- |
| `[worktrees] directory` | Default location for new checkouts, as described in [Create a worktree](#create-a-worktree) |
| `[worktrees] profile` | Profile that seeds new worktree sessions |
| `[worktrees] sidebar_open` | Whether a click on a checkout in the sidebar opens its session or a pane, as described in [Sidebar](sidebar.md#worktrees) |
| `[worktrees] delete_branch` | Which copies of a removed checkout's branch to delete too, as described in [Delete the branch too](#delete-the-branch-too) |

A relative `directory` must be a single folder name. A nested or escaping value, such as
`tools/.worktrees` or `../worktrees`, is ignored with a warning; use an absolute path for a location
outside the repository.

`directory` is read on the session host: a running session server uses the value it started with,
and a remote session uses the remote host's configuration. See
[Configuration](configuration.md#worktrees) for the full reference.
