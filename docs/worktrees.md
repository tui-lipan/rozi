# Worktrees

A Git worktree is an extra checkout of the same repository, with its own branch and working
directory. rozi can open each worktree in its own named [session](sessions.md), so work on two
branches keeps separate shells, layouts, and running processes, and you can switch between them
without stashing.

This page covers the **Worktrees** picker, creating and removing checkouts, the `rozi worktrees`
command, and the `[worktrees]` settings.

## Open a worktree

Open **Worktrees** from the command palette (`Ctrl+A`, then `p`) while a pane is focused in a Git
repository, or use the sidebar's [Worktrees tab](sidebar.md#worktrees), which lists the same
checkouts. **Worktrees** has no default command key.

The picker lists the checkouts on the focused pane's session host, including remote hosts. `●`
marks the checkout the focused pane is in, and each row shows that checkout's sessions on the
right. The picker opens with the list it last showed for the repository and refreshes it in place.

Press `Enter` on a checkout to open it:

- With one associated session, rozi switches to it.
- With several, rozi lists them so you can choose.
- With none, rozi creates a named session, `wt-<branch>`, whose first shell starts in the checkout.
  A number is added when that name is taken.

A session is associated with a checkout when the checkout is its recorded origin — the checkout it
was created for. A pane that later changes directory into a checkout does not associate its session
with it.

| Key | Action |
| --- | --- |
| `Enter` | Open the checkout's session, or create one |
| `Ctrl+N` | Create a checkout from a branch and base revision, then open it in a new session |
| `Ctrl+R` | Refresh the list |
| `Ctrl+K` | Remove a linked checkout; press again to force only if Git refused a dirty checkout |
| `Esc` | Close the picker |

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

Select a linked checkout and press `Ctrl+K`. Removal follows these rules:

- It never deletes a branch.
- It refuses the primary checkout, a locked checkout, and any checkout owned by a running or
  restorable rozi session. Stop or forget that session first.
- If Git refuses because the checkout has uncommitted changes, press `Ctrl+K` again to force the
  removal. Forcing affects only that Git check.

A create or remove that has started finishes even if you close the picker, and rozi reports the
result.

## Use worktrees from the command line

```bash
rozi worktrees list                         # checkouts of the repository around this directory
rozi worktrees list --cwd ~/src/rozi --format json
rozi worktrees create feat/login            # new branch from HEAD, at the default path
rozi worktrees create fix/ssh --base origin/main --open
rozi worktrees open ~/src/rozi-worktrees/feat-login
rozi worktrees remove ~/src/rozi-worktrees/feat-login
rozi worktrees exclude                      # add the in-repository worktree directory to .git/info/exclude
```

| Command | Behavior |
| --- | --- |
| `list` | Lists checkouts. With `--format json`, prints a `worktrees` array; each entry includes the `sessions` whose recorded origin is that checkout. |
| `create <BRANCH>` | Checks out an existing local branch, or creates it from `--base` (`HEAD` by default). Without `--path`, uses the default location. Prints the new checkout's path, or a `worktree` object with `--format json`. `--open` then opens it like `open`, and cannot be combined with `--format json`. |
| `open <PATH>` | Opens the checkout that contains `PATH`. |
| `remove <PATH>` | Removes a checkout under the same rules as the picker. `--force` only lets Git remove a dirty checkout. |
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
