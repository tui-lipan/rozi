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

The picker lists the checkouts on the focused pane's session host, including remote hosts. `●`
marks the checkout the focused pane is in. Each row shows the branch and its PR/CI status, with
`primary`, `locked`, `stale lock`, or `prunable` alongside it when applicable. Paths stay searchable
without appearing in rows. Press `Ctrl+C` or click **copy path** to copy the selected checkout's full
path, including a remote host's path, to your clipboard.

The picker refreshes the checkout list when opened. Press `Ctrl+R` to refresh both checkouts and
PR status.

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
| `Ctrl+K` | Remove a linked checkout; press it again to confirm |
| `Ctrl+U` | Unlock a locked checkout |
| `Esc` | Close the picker |

### Read work status

For GitHub repositories, install [GitHub CLI](https://cli.github.com/) and authenticate with
`gh auth login` on the host running the session. Remote sessions use that host's GitHub account.

| Picker | Sidebar detail | Meaning |
| --- | --- | --- |
| `#114 ✓` | `#114 · passed` | CI passed for the current branch commit |
| `#113 ✕` | `#113 · failed` | CI failed for the current branch commit |
| `#115 ◌` | `#115 · running` | CI is pending or running |
| `#116 open` | `#116 · open` | Open PR without checks for the current branch commit |
| `#111 draft` | `#111 · draft` | Draft PR |
| `#112 merged` | `#112 · merged` | Merged PR, including squash merges |
| `#110 closed` | `#110 · closed` | Closed without merging |
| `PR unavailable` | `PR unavailable` | GitHub status could not be read |

Merged, closed, and draft states take precedence over CI. Passing or failing checks on an older
pushed commit are not shown for a newer local branch commit. PR status does not describe uncommitted
changes or imply a checkout is safe to remove. A branch without an associated PR has no work status.
For same-named branches, rozi uses the newest matching PR; a PR from a fork is matched only when
its head commit matches the local branch. Merged and closed PRs also require a matching head commit,
so reusing a branch for new commits clears its old terminal status. Open and draft PRs stay
associated when the local branch is ahead, but CI waits for a matching commit.
Other hosting providers currently have no PR integration.

While the Worktrees sidebar is visible, work status refreshes about once a minute. `Ctrl+R` in the
picker checks immediately.

## Create a worktree

Press `Ctrl+N` in the picker to open the new-worktree form. It has three fields; `Tab` and
`Shift+Tab` move between them.

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

A create or remove that has started finishes even if you close the picker, and rozi reports the
result. If a created checkout is reported in a toast, right-click it to copy the checkout path on
the session host.

## Unlock a worktree

Git can lock a checkout so that it is not removed or pruned. Coding agents such as Claude Code lock
the checkouts they create, and a lock stays behind when an agent exits without cleaning up, even
after its branch is merged.

The picker and the sidebar show a locked checkout as `locked`. When the lock names a process with
`pid <N>` and that process is no longer running on the session host, the checkout shows as
`stale lock`. A lock that names no process is never stale.

Select a locked checkout and press `Ctrl+U` to remove its lock. This works for any lock, so check
that nothing still uses the checkout before unlocking one that is not stale. Unlocking changes
nothing else: the checkout and its branch stay.

## Use worktrees from the command line

```bash
rozi worktrees list                         # checkouts of the repository around this directory
rozi worktrees list --cwd ~/src/rozi --format json
rozi worktrees create feat/login            # new branch from HEAD, at the default path
rozi worktrees create fix/ssh --base origin/main --open
rozi worktrees open ~/src/rozi-worktrees/feat-login
rozi worktrees remove ~/src/rozi-worktrees/feat-login
rozi worktrees unlock ~/src/rozi/.claude/worktrees/fix-ssh
rozi worktrees exclude                      # add the in-repository worktree directory to .git/info/exclude
```

| Command | Behavior |
| --- | --- |
| `list` | Lists checkouts, with `locked` or `stale-lock` in the state column. With `--format json`, prints a `worktrees` array; each entry includes the `sessions` whose recorded origin is that checkout, and a `lock` that is `null` or holds Git's lock `reason` and whether it is `stale`. |
| `create <BRANCH>` | Checks out an existing local branch, or creates it from `--base` (`HEAD` by default). Without `--path`, uses the default location. Prints the new checkout's path, or a `worktree` object with `--format json`. `--open` then opens it like `open`, and cannot be combined with `--format json`. |
| `open <PATH>` | Opens the checkout that contains `PATH`. |
| `remove <PATH>` | Removes a checkout under the same rules as the picker, except that it refuses every locked checkout, stale or not. `--force` only lets Git remove a dirty checkout. |
| `unlock <PATH>` | Removes the lock from the checkout that contains `PATH`, whether or not the lock is stale. |
| `exclude [DIR]` | Adds the relative `[worktrees] directory`, or `DIR`, to `.git/info/exclude`. |

When `create` puts a checkout inside the repository in a directory Git does not ignore, it prints a
warning, and the JSON output marks the checkout as `unignored`.

`open` accepts any path inside a checkout:

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

A relative `directory` must be a single folder name. A nested or escaping value, such as
`tools/.worktrees` or `../worktrees`, is ignored with a warning; use an absolute path for a location
outside the repository.

`directory` is read on the session host: a running session server uses the value it started with,
and a remote session uses the remote host's configuration. See
[Configuration](configuration.md#worktrees) for the full reference.
