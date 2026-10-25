# Extensions

An extension adds features to `rozi` without changing `rozi` itself: commands, background services,
link handlers, sidebar tabs, coding-agent definitions, and suggested keybindings. This page covers
finding, installing, and managing extensions. To make your own, see
[Write an extension](writing-extensions.md).

An extension is a directory containing an `extension.toml` manifest and, usually, the programs that
manifest launches. Those programs talk to `rozi` through the same
[`rozi` control commands](control.md) that scripts use.

**Extensions are trusted programs, not sandboxed ones.** An extension's programs run as you, with
your user account's permissions: they can read and change your files, use your credentials, reach
the network, and start other programs. Installing an extension is equivalent to installing any
other software from that source. "Trusted" in **Discover** says who maintains an extension; it
grants it nothing extra.

`rozi` does limit which `rozi` requests an extension may make. A
[placed](writing-extensions.md#run-on-the-session-host) process can only act on panes in its own
session or host, and it cannot run other actions or read the whole client. Those limits apply only
to what it asks `rozi` to do. It still runs as you on its host, with your account's access there.

The install report in **Discover**, the install prompt, and `rozi extensions install` all repeat
this.

## Review an extension before installing

Before installing an extension:

1. Get the source into a temporary or project directory.
2. Review `extension.toml` and every executable it references.
3. Check its direct dependencies and any network access.
4. Validate the unpacked directory:

   ```sh
   rozi extensions check ./rozi-git-tools
   ```

Validation checks the manifest, API version, IDs, launch declarations, environment, and executable
paths. It does not make untrusted code safe.

`rozi` never loads extensions from a project-local directory such as `.rozi/extensions`, so opening
a checkout does not run its code.

## Open links through an extension

An extension may supply link handlers that take effect when it loads. The
[terminal-browser extension](https://github.com/tui-lipan/rozi-terminal-browser) opens HTTP and HTTPS
links in a browser pane and can reuse browsers in the current session. Install it through the CLI
or the extension install prompt; no link-handler configuration is required.

`Ctrl`-clicks and uppercase hint labels use the same handlers. Your explicit
[`[[links.handlers]]`](configuration.md#links-handlers) take precedence. Disabling or removing an
extension removes its handlers along with its commands. The extension report lists its schemes
and command IDs so you can review which links it handles before installation.

## Discover public extensions

Open the command palette and choose **Extensions…**, then switch to the **Discover** tab. It lists
public GitHub repositories that carry the `rozi-extension` topic.

Discover reads an index of those repositories, published at
[`tui-lipan/rozi-extension-index`](https://github.com/tui-lipan/rozi-extension-index). The index
records each repository's metadata at an exact default-branch commit. It is not a package registry:
it hosts no packages and resolves no dependencies. rozi marks its author's Claude Code sessions,
Codex threads, Terminal browser, and Vim and Neovim navigator extensions as **trusted**. Their
reports show "Trusted · Maintained by the rozi author". Other entries show "Not audited; inspect
source before installing". Installing from Discover clones the repository at the indexed commit;
from then on it is an ordinary Git installation that updates from its remote.

To install from Discover:

1. Press `Enter` on a row to open its installation report. The report names the repository and
   commit, and shows compatibility, contribution counts, trust status, and that the extension
   will run without a sandbox.
2. Optionally press `Ctrl+L` to open the repository in your browser at that exact commit. This is
   the source `rozi` installs, even if the default branch has moved since the index was built.
3. Press `Enter` again to install that commit.

`rozi` validates the complete extension before moving it into its private data directory. While it
installs, a small progress card with a spinner, the extension, and the commit sits over the report,
which stays open beneath it. When installation finishes, the report shows the extension as
installed, and `Esc` returns to the **Discover** list with your place kept. `Esc` while installing
closes the report without cancelling; `rozi` still loads the extension and reports the result, even
if the manager was closed. Reopening that entry's report shows the progress again. Only one
installation runs at a time, whether it started from a report or from the install prompt.

Extensions you already have stay listed, marked `installed`, and their report says so instead of
offering to install them again.

`rozi` fetches the index in the background when **Extensions…** opens, so the manager stays
responsive while the request runs or when you are offline. It keeps the last fetched index in its
cache directory and lists it immediately. A cached index up to ten minutes old is used as is; an
older one is refreshed in the background, and a spinner above the hints shows the fetch. If a fetch
fails, the listed entries stay and a row reports the failure. `Ctrl+R` on this tab always refetches.

An index entry that `rozi` cannot validate is left out of **Discover**. Only an unreadable index or
an unsupported index schema version makes discovery unavailable. For a repository that is not in the
index, use the `Ctrl+I` install prompt or the CLI.

To list your own extension, see [Publish to Discover](writing-extensions.md#publish-to-discover).

## Install an extension

Install a reviewed local directory, HTTPS Git remote, or SSH Git remote:

```sh
rozi extensions install ./rozi-git-tools
rozi extensions install https://github.com/user/rozi-git-tools.git
rozi extensions install git@github.com:user/rozi-git-tools.git
```

Then run `rozi run-action reload-extensions` in each running client that should load it.

`rozi` validates the extension before installing it. It copies a local directory and clones a Git
repository into its own storage. In both cases it:

- uses the manifest ID as the installation name;
- refuses to overwrite an existing installation or a conflicting ID;
- removes the ID from the disabled list.

A Git installation remembers its remote and installed commit, which
[`rozi extensions update`](#update-an-extension) uses later. Installing never enables background
updates. When installation completes, the report summarizes the extension's
[navigation targets](writing-extensions.md#navigation-targets) and whether each
[suggested keybinding](writing-extensions.md#suggested-keybindings) is active, conflicting, or suppressed.

The installation directory is private `rozi` data; you never need to create or edit it. `rozi` also
never installs editor plugins or other external integrations on an extension's behalf.

To develop an extension, link your checkout instead of copying it:

```sh
rozi extensions install --link ./rozi-git-tools
```

`rozi` stores only a symlink to a linked extension, and the checkout stays yours. Changes in the
checkout take effect after an extension reload.

## Manage installed extensions

Open the command palette and choose **Extensions…**. The **Installed** tab groups your extensions by
status; the **Discover** tab lists the public index. `Tab`, `Shift+Tab`, `←`, and `→` switch tabs
while the search field keeps focus, and typing filters the active tab.

On the **Installed** tab:

| Key | Action |
| --- | --- |
| `Enter` | Enable or disable the selected extension |
| `Ctrl+D` | Open the full report |
| `Ctrl+I` | Open the install prompt |
| `Ctrl+U` | Apply the selected extension's update, or check every Git-managed extension for one |
| `Ctrl+Shift+U` | Update every extension with a known update (shown when there is more than one) |
| `Ctrl+R` | Rescan extension manifests |
| `Ctrl+O` | Open `extension.toml` |
| `Ctrl+K` | Remove the installation (press twice) |

In the report, `Ctrl+Y` copies it, `Ctrl+O` opens `extension.toml`, `Ctrl+U` updates a Git-managed
installation, and `Ctrl+L` opens the manifest's `homepage` when it declares one. The report is
read-only and wraps long command, path, and diagnostic lines.

Rows show where an extension came from: a linked checkout shows `linked`, and a Git installation
with an update shows both versions, such as `0.2.1 → 0.2.2 · git`.

The install prompt accepts the same local paths and Git HTTPS or SSH URLs as
`rozi extensions install <SOURCE>`, and shows the same progress card over the prompt. A finished
installation opens **Installed** on the new row. Use the CLI's `--link` option when the checkout
must stay yours.

Scripts and other tools can read the same information from the CLI:

```sh
rozi extensions list
rozi extensions list --verbose
rozi extensions list --json
rozi extensions check ./git-tools --json
```

The list includes loaded, disabled, invalid, incompatible, and duplicate extensions. `--verbose`
adds installation paths; public command, service, agent, and sidebar tab IDs; navigation targets;
resolved executables; and validation errors.

## Disable or remove an extension

Press `Enter` on a loaded extension in **Extensions…** to disable it, and again to enable it. `rozi`
writes the ID to `config.toml`, reloads extension contributions, and keeps the overlay open. You can
also edit the list yourself:

```toml
[extensions]
disabled = ["git-tools"]
```

When the config is saved, `rozi` removes the extension's commands, agents, sidebar tabs, and
navigation targets, stops its services, and closes any pickers, activity rows, and event
subscriptions it opened. Its sidebar tab placement is remembered, so re-enabling it puts the tab
back where you had it.

To remove an installation, press `Ctrl+K` twice in **Extensions…**, or use the CLI with the
manifest ID:

```sh
rozi extensions remove git-tools
```

Removal deletes a copied extension or a `rozi`-owned Git clone. For a linked extension, it deletes
only the symlink, never the checkout it points to. After a CLI removal, run
`rozi run-action reload-extensions` in running clients.

Removal also deletes the extension's output logs. It keeps the files the extension saved in its
[state directory](writing-extensions.md#keep-state), so reinstalling it picks up where it left
off. To delete them too:

```sh
rozi extensions remove git-tools --purge
```

`--purge` deletes the state on the machine where you run it. An extension that ran placed
contributions on a remote host keeps its state there; run the command on that host to delete it.
`--purge` also deletes the state of an extension that is no longer installed.

## See why an extension failed

When an extension's command or service fails, the notification ends with the command to run. It
prints what the extension's processes wrote on this machine:

```sh
rozi extensions logs git-tools
rozi extensions logs git-tools --unit service.watch --lines 200
```

Without `--unit`, it prints the last 50 lines of each of the extension's logs; `--lines` changes
how many. A process placed on a remote host logs there, so run the command on that host. The
[extension author guide](writing-extensions.md#read-an-extensions-logs) lists what each log holds.

## Bind a key to an extension command

An extension command's public ID is `<extension-id>.<command-id>`. Bind it in `[keys]` like any
other action:

```toml
[keys]
"git-tools.branches" = "ctrl-a b"
```

A binding may name an extension that is not loaded. It becomes active when that extension loads and
inactive while it is disabled or absent, and `rozi` warns that the binding is preserved but
inactive. See [Keybindings](keybindings.md) for key notation.

## Update an extension

`rozi` never updates extensions automatically. Update one Git-managed installation, or all of them,
explicitly:

```sh
rozi extensions update git-tools
rozi extensions update --all
rozi run-action reload-extensions
rozi extensions list --verbose
```

`--all` updates each Git-managed installation in turn, including one whose manifest is missing or
invalid. One that fails does not stop the rest, and the command exits with an error after
reporting every failure. An installation whose source metadata cannot be read is reported the
same way.

A reload after an update restarts the extension's services, so they run the new files.

The update clones the recorded remote into a staging area, validates it, and replaces the old
checkout only if the new version is valid. It refuses to replace a checkout with local changes.
Copied local extensions and linked checkouts cannot be updated this way.

**Extensions…** checks Git remotes in the background when it opens and when `Ctrl+R` reloads the
**Installed** tab. Enabling, configuring, or installing an extension keeps the results already
found and checks only installations that have none yet. A muted spinner beside a row's version
shows its check running, and the **Installed** tab counts the updates found.
An outdated row shows `installed → latest`, using the version from the remote `extension.toml`, or a
short commit hash when the remote changed without changing its version. The report's **Update** row
shows the same result, or the error when a remote could not be checked.

On a row with a known update, `Ctrl+U` runs the same update as the CLI and reloads the current
client when it succeeds. On any other row, `Ctrl+U` checks every Git-managed installation's remote
again. When more than one update is known, `Ctrl+Shift+U` updates them all in turn and reloads once
at the end. It needs a terminal that reports `Shift` with `Ctrl`, such as one with the Kitty
keyboard protocol; elsewhere, use `rozi extensions update --all`. Closing **Extensions** does not
stop an update. Opening it again shows the rows that are still updating, and another update or a
removal waits until that run finishes. After a CLI update, reload explicitly, because `rozi` does
not watch extension directories.
