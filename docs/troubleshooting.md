# Troubleshooting

This page lists common problems by what you see, with the cause, the fix, and a link to the page
that covers the behavior in full. Messages in code formatting are the exact text rozi prints, so
you can search this page for them.

rozi has no diagnostic log file; `[logging]` records the output of a pane, not rozi's own activity.
These commands show what rozi knows:

| Command | Shows |
| --- | --- |
| `rozi --version` | The version, the extension API, and the session protocol range |
| `rozi update --check` | How rozi was installed and whether a newer release exists |
| `rozi sessions list` | The sessions rozi can reach, including restorable ones |
| `rozi --help --advanced` | Advanced options and where control sockets live on this machine |
| `rozi api describe` | The control API version and capabilities of the installed binary |
| `rozi extensions list --verbose` | Each extension's status, paths, and validation errors |

Configuration warnings appear in a toast and are also printed to stderr.

## Installation and updates

### `rozi` is not found after installing

The install scripts do not edit shell startup files, so the directory holding `rozi` may not be on
your `PATH`. The script ends with `Not on PATH` and prints an `Add` snippet in that case.

On Linux and macOS, add the directory in your shell profile:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

On Windows, paste the printed snippet or install with `-AddToPath`. `Not this terminal` means your
user `PATH` is already correct but this terminal started earlier; open a new one. Paste the snippet
rather than running the installer again. See [Add rozi to PATH](installation.md#add-rozi-to-path).

### `rozi update` says another tool owns the install

`rozi update` manages only installs made by the install scripts. For Cargo, mise, Homebrew, Scoop,
WinGet, or a distribution package it declines with a message such as
`this rozi was installed with cargo, which owns its updates - run: cargo install rozi --locked`.

Run the command it prints, or update through your package manager. `rozi update --check` names the
install channel. On NetBSD, update through pkgsrc. See
[Installs rozi does not manage](installation.md#installs-rozi-does-not-manage).

### rozi still behaves like the old version after an update

An update installs the new build for the next launch and restarts nothing. The client you updated
from, and every running session server, keep the old build.

1. Quit rozi and start it again to run the new client.
2. Restart each session that should use the new build: in Sessions (`Ctrl+A`, then `s`), press
   `Ctrl+E` twice on it, or run the `restart-session` action. Restarting ends the programs in its
   panes.
3. On Windows, run the install script again if the update needs to replace the launcher.

See [What an update restarts](installation.md#what-an-update-restarts).

### A prebuilt release does not run on my system

Prebuilt Linux releases need glibc 2.28 or newer. Windows releases need Windows 10 version 1809 or
newer and exist for x86-64 only. There are no prebuilt NetBSD archives.

On an older Linux or an unsupported architecture, install with `cargo install rozi --locked` or
build from source. On NetBSD, use `pkgin install rozi`. See
[Platform support](platform-support.md#requirements).

## Starting rozi and sessions

### `No session or profile named …`

`rozi <NAME>` attaches to a running session with that name, or launches the profile with that name.
It never creates an empty session for an unknown name. A session name belongs to one host, so a
local name does not reach a session on a remote host.

Create the session explicitly with `rozi sessions new <NAME>`, check the spelling with
`rozi sessions list`, or add `--remote <HOST>` for a session on another machine. See
[Open a session from the command line](sessions.md#open-a-session-from-the-command-line).

### A named session disappeared

A named session keeps running until it is killed or its session server stops, for example when the
machine restarts. With `[session] resurrect = true`, the default, a stopped session appears in
Sessions as `restorable`; press `Enter` on it to restore its layout, commands, and history. Killing a
named session also deletes its snapshot.

If a session started under `su`, cron, or Tailscale SSH is missing from your desktop, the two sides
may be using different runtime directories because `XDG_RUNTIME_DIR` was unset. The session then
appears only as restorable. Start such sessions in an environment with the same `XDG_RUNTIME_DIR`;
see [User directories](configuration.md#user-directories).

If the client itself lost its connection, it retries for up to 15 seconds and then returns to the
picker. The server and its panes keep running if the server is still alive. See
[Limits and failure cases](sessions.md#limits-and-failure-cases).

### My temporary session is gone

A temporary session closes when you leave it untouched, when you switch away before using it, or 45
seconds after its last client disconnects. After a crash, it appears as `ephemeral` in Sessions
during those 45 seconds.

To keep a temporary session, name it with `Ctrl+A`, then `S`. The server, panes, and processes keep
running under the new name. See
[Recover a temporary session](sessions.md#recover-a-temporary-session).

### `runs an incompatible rozi version`

The session server was started by a rozi build whose session protocol differs from the client's,
usually after an update. Sessions shows `Attach failed` with `` `dev` runs an incompatible
version ``, and the Sessions sidebar marks the row `incompatible or unavailable`.

To keep working in the old session first, run `rozi update --rollback`, attach, finish, and update
again. Otherwise press `Ctrl+K` twice on the row in Sessions and start the session again. This ends
its programs and deletes its snapshot. For a remote host, update rozi on both ends. See
[Version compatibility](remote.md#version-compatibility).

### A restored session re-ran a command I did not want

When a shell pane was running a command at snapshot time, resurrection types that command back and
runs it. Set `[session] resurrect_foreground = "hold"` to leave it at the prompt for you to confirm,
or `"never"` to restore only the shell.

The session server reads this setting when it starts, so restart the session for it to apply. See
[Commands a pane was running](sessions.md#commands-a-pane-was-running).

## Keys and input

### `Alt` shortcuts don't work in my terminal

The host terminal or desktop keeps some chords for itself, and those never reach rozi. Windows
Terminal and the classic console use `Alt+Enter` for fullscreen and `Alt+Space` for the window menu.
Many desktops reserve `Super`, and Windows intercepts most Windows-key chords.

Use the prefix form instead (`Ctrl+A`, then the command key), unbind the shortcut in your terminal,
or rebind the rozi command under `[keys]`. See
[Platform caveats](keybindings.md#platform-caveats).

### A program in a pane never receives `Ctrl+A` or an `Alt` chord

rozi takes the prefix and the held-modifier chords before the pane sees them.

- Press `Ctrl+A` twice to send a literal `Ctrl+A`.
- Move the prefix, for example `[input] prefix = "ctrl-b"`.
- Set `[input] modifier_shortcuts = false` to send held-modifier chords to the pane. Prefix
  commands keep working.

`Ctrl+C`, by contrast, always goes to the focused pane and never quits rozi. To leave, press
`Ctrl+A`, then `q`; named sessions keep running. See
[Prefix and held modifier](keybindings.md#prefix-and-held-modifier) and
[Leave rozi](sessions.md#leave-rozi).

## Terminal display, shell integration, and clipboard

### Icons show as boxes

`nerd_icons` is on by default and draws [Nerd Font](https://www.nerdfonts.com/) glyphs in pane
titles, badges, and the sidebar. A terminal font without those glyphs shows boxes or blanks.

Set your terminal to a Nerd Font, or turn the icons off:

```toml
nerd_icons = false
```

The same switch is in Settings › General. The `round` and `arrow` tab styles and sidebar file icons
also need it. See [Top-level keys](configuration.md#top-level-keys).

### New panes don't open in my current directory

rozi learns a pane's directory from shell integration, then from process inspection on Linux and
macOS, then falls back to the launch directory. Windows has no process inspection, so it depends on
shell integration.

1. Check that `[shell_integration] mode` is `"auto"`, the default. A change applies to new panes.
2. Use a supported shell: bash, zsh, fish, or PowerShell. `cmd.exe` reports its directory and
   prompts, but not the running command.
3. On a remote host, rozi does not send its shell integration. A remote shell must emit OSC 7 for
   directory tracking.

A directory reported for another host, such as from `ssh` inside a pane, is never used for a new
local pane. Prompt jumps in copy mode and **Copy last command output** need the same integration.
See [Working directories and shell metadata](terminal.md#working-directories-and-shell-metadata).

### A program in a pane cannot set my clipboard

Programs set the clipboard through OSC 52 only while `[clipboard] enable_osc52 = true`, the default.
The change applies when the config reloads.

In a remote session, OSC 52 and mouse selection use your local clipboard. A remote program that
calls a clipboard tool directly uses the remote host's clipboard. See
[Select, copy, and paste](terminal.md#select-copy-and-paste).

## Remote hosts

### Remote connection keeps reconnecting

When an SSH connection drops or stops answering, rozi shows a reconnecting overlay and retries for
up to two minutes. After that the session shows **offline**; if the remote session is gone, it shows
**session lost**. Background reconnects to a connected host never prompt, so a host that needs a
password or host-key answer keeps failing until you reconnect it yourself.

1. Test with `ssh <HOST>`, or run `rozi --remote <HOST>` in a shell to see the SSH error.
2. Select the host in Remote hosts (`Ctrl+R` in Sessions) and press `Enter` or `Ctrl+R`.
3. On a network that drops idle connections, tune `[remote] server_alive_interval_secs` and
   `server_alive_count_max`.

On **session lost**, `Enter` recreates the session from the panes on screen. See
[Reconnection and switching](remote.md#reconnection-and-switching).

### `SSH login rejected`, a host-key error, or no password prompt

SSH runs in batch mode by default, so authentication must finish without a prompt. Load your key
into the SSH agent, or set `identity_file` under `[remote.hosts.<alias>]`.

To answer password, passphrase, and host-key prompts inside rozi, set `[remote] batch_mode = false`.
This needs OpenSSH 8.4 or newer on the client; older versions prompt on the terminal, over the rozi
window.

`Host key not trusted` means OpenSSH could not verify the host's key. Connect once with
`ssh <HOST>` and accept it, or answer the prompt in rozi with `batch_mode = false`.
`Host key changed` means the host no longer matches its recorded key, and OpenSSH refuses to
connect; verify the new key out of band before you remove the old entry.

See [Set up SSH authentication](remote.md#set-up-ssh-authentication), and
[Troubleshooting](remote.md#troubleshooting) in Remote sessions for the other connection messages.

### `No rozi on host`

rozi found no compatible binary on the host and did not install one. `[remote] install = "prompt"`,
the default, asks first in the TUI, but a non-interactive run never installs, and
`install = "never"` always fails here.

Connect from the TUI and choose `Install`, set `install = "always"`, or point `binary_path` at an
existing install. `Rozi update needed` means the host's rozi does not speak this client's session
protocol; install the matching version there. See
[Choose an install policy](remote.md#choose-an-install-policy).

## Configuration

### My config change had no effect

Check each of these:

1. **The file.** `--config` and `ROZI_CONFIG` take precedence over the default location.
   `rozi run-action open-config` opens the file rozi uses.
2. **The timing.** Some settings apply only to new panes, new session servers, new SSH connections,
   or the next launch. For example, `scrollback` and `shell` affect new panes, and
   `session.resurrect` and `logging.*` need a new session server.
3. **Rejection.** If the file is invalid, a live reload keeps the last good configuration and shows
   an error.
4. **Extensions.** rozi does not watch extension directories; run
   `rozi run-action reload-extensions`.

See [When a change takes effect](configuration.md#when-a-change-takes-effect) and
[File location](configuration.md#file-location).

### `Config parse failed for …`

The whole file was rejected because it is unreadable, is not valid TOML, or has a value of the wrong
type. At startup rozi uses the defaults; on a reload it keeps the last good configuration. The text
after the path names the line and problem.

The flat `[hooks]` table is no longer supported and causes this error; convert each entry to a
`[[hooks]]` block. See [Invalid configuration](configuration.md#invalid-configuration) and
[Migrate from `[hooks]`](hooks.md#migrate-from-hooks).

### `Unknown config key …; ignored`

rozi skipped one setting and applied the rest of the file. The key is misspelled, placed in the
wrong table, or no longer exists. A value outside the allowed choices is skipped the same way, and
some numbers are clamped into range. Several warnings share one toast, and each is also printed to
stderr. Compare the key with [Configuration](configuration.md).

## Scripting and extensions

### `no live rozi control socket found`

A control command outside a pane looks for exactly one running rozi window in the runtime directory.
It exits with status `2` when there is none, and with
`multiple live rozi sockets found; pass --socket PATH` when there are several.

- Start rozi, or pass `--socket PATH` before the command. `rozi --help --advanced` prints the
  directory the sockets live in.
- To reach a session with no window, use `rozi --session <NAME> <COMMAND>`.
- Inside a pane, hook, or extension, rozi sets `ROZI_SOCKET` for you.

See [Endpoint discovery](control.md#endpoint-discovery) and
[Output and exit status](control.md#output-and-exit-status).

### `Control socket unavailable` at startup

rozi could not create its control endpoint, and shows this in a `Startup warning` toast; the text
after the colon says why. The UI still
works, but panes, hooks, and extensions receive no `ROZI_SOCKET`, and scripts cannot reach this
window. Check that the runtime directory exists, belongs to you, and is private, then restart rozi.
See [Files and directories](platform-support.md#files-and-directories).

### `rozi dev list-panes` is refused

A bare session name is a launch target, so rozi replies
`` `dev` is a launch target; write `--session dev` before a control command to reach that session ``.
Run `rozi --session dev list-panes` instead.

Against `--session`, commands that need a screen — focus, workspace switching, toasts, pickers,
actions, and `capture-ui` — are refused with a reason. Run those against a running window. See
[Two endpoints](control.md#two-endpoints).

### An extension change does not show up

rozi does not watch extension directories, and a manifest error stops the extension from loading.
`` extension command `…` is unavailable; run `rozi extensions list --verbose` `` means the
command's extension is not loaded.

```sh
rozi extensions check <PATH>
rozi run-action reload-extensions
rozi extensions list --verbose
```

The verbose list shows validation errors, disabled extensions, and the resolved argv of each
command. Extension command output is not shown anywhere, so write diagnostics to a file while
testing. See [Test and debug](extensions.md#test-and-debug).

### My hook runs but I see nothing

rozi discards a hook's output and exit status, does not wait for or retry it, and may skip hooks
once 32 are already running. Hooks run on the client machine, even for a remote session, and not at
all while every client is detached.

Redirect output to a file in the hook command, or run `rozi subscribe` from a `[[services]]` entry
for long-lived handling. See [Lifecycle](hooks.md#lifecycle).

## Getting help

Search or open an issue at [github.com/tui-lipan/rozi/issues](https://github.com/tui-lipan/rozi/issues).
Include:

- the full output of `rozi --version`, including the nightly lines from a nightly build;
- the install method, as reported by `rozi update --check`;
- the operating system and version, terminal emulator, and shell;
- for a remote problem, the versions on both machines;
- the smallest `config.toml` snippet that reproduces the problem;
- the exact error or toast text, and any warnings rozi printed to stderr;
- the steps that lead to the problem.

Pane logs, captures, and scrollback can contain passwords and tokens; remove them before sharing.

Do not report a suspected vulnerability in a public issue. Follow the
[security policy](../SECURITY.md) instead.
