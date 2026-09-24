# Platform support

This page lists the operating systems rozi runs on, what each one requires, and where behavior
differs between them. Commands, configuration, and automation work the same everywhere unless a
section below says otherwise.

| Platform | Support | Prebuilt releases | Install |
| --- | --- | --- | --- |
| Linux | Supported | x86-64, ARM64 (glibc 2.28 or newer) | [Install script](installation.md#install-a-release) |
| macOS | Supported | x86-64, ARM64 | [Install script](installation.md#install-a-release) |
| Windows | Supported | x86-64 (Windows 10 1809 or newer) | [Install script](installation.md#install-a-release) |
| NetBSD | Community-supported | None | `pkgin install rozi`, or build from source |

Supported platforms are tested and released with every version. Community support is a narrower
promise; see [NetBSD](#netbsd).

## Requirements

All platforms need a terminal emulator that can run a full-screen terminal application.

- **Linux:** prebuilt releases need glibc 2.28 or newer. A build from source may need a newer glibc,
  inherited from the build host, unless you build in an equivalent compatibility environment.
- **Windows:** Windows 10 version 1809 (build 17763) or newer, because rozi uses ConPTY. Windows
  Terminal is recommended but not required. There is no ARM64 Windows release.
- **Building from source:** Rust 1.90 or newer. See [Installation](installation.md#build-from-source).

## Platform differences

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Terminal backend | Unix PTY | Unix PTY | ConPTY |
| Local IPC | Unix-domain sockets | Unix-domain sockets | Named pipes |
| Shell integration | bash, zsh, fish | bash, zsh, fish | PowerShell; limited cmd.exe |
| Foreground program detection | Shell metadata, then `/proc` | Shell metadata, then system process information | Shell metadata only |

Other platform-dependent behavior:

- Desktop notifications use the operating system's notification tools and may need a desktop
  session.
- Clipboard behavior depends on the host clipboard and terminal. When enabled, OSC 52 can carry
  copied text through SSH.
- File paths, command shells, and executable lookup follow the host operating system's conventions.

See [Remote sessions](remote.md) for differences between local and SSH-attached sessions.

## Files and directories

| Purpose | Linux and macOS | Windows |
| --- | --- | --- |
| Configuration | `$XDG_CONFIG_HOME/rozi`, or `~/.config/rozi` | `%APPDATA%\rozi` |
| State | `$XDG_STATE_HOME/rozi`, or `~/.local/state/rozi` | `%LOCALAPPDATA%\rozi` |
| Cache | `$XDG_CACHE_HOME/rozi`, or `~/.cache/rozi` | `%LOCALAPPDATA%\rozi\cache` |
| Runtime data | `$XDG_RUNTIME_DIR/rozi`, or `/run/user/<uid>/rozi`, or a private temporary directory | `%LOCALAPPDATA%\rozi\run` |

Set `ROZI_CONFIG` to use a different configuration file on any platform.

rozi restricts local session data and the local connections between clients and session servers to
the current operating system user. Remote sessions use SSH authentication; rozi does not expose
local sessions over the network.

## Shell integration

Shell integration lets rozi track prompt boundaries, the current working directory, and the
foreground program. On Linux and macOS, rozi integrates with bash, zsh, and fish. On Windows,
PowerShell has full integration; cmd.exe provides prompt markers but cannot report every piece of
metadata. Features that rely on this information have less context when a shell does not report it.

rozi injects shell integration when it starts a pane. It does not edit shell startup files,
PowerShell profiles, or registry startup commands.

See [Terminal features](terminal.md#working-directories-and-shell-metadata).

## Input differences

The default direct modifier, `Alt`, works on every supported platform. Many Windows terminals do not
pass `Super` or Windows-key combinations to terminal applications, so the prefix is the reliable
choice there.

Windows Terminal and the classic console also keep some `Alt` chords for themselves, such as
`Alt+Enter` for fullscreen, and those never reach rozi. Use the `Ctrl+A` prefix, unbind the host
shortcut, or rebind the command in rozi.

`Ctrl+C` goes to the program in the focused pane on every platform. It does not quit rozi.

See [Keybindings](keybindings.md#platform-caveats).

## NetBSD

rozi builds and runs on NetBSD on x86-64. The pkgsrc maintainer has confirmed a working session on
bare metal, and rozi has been in pkgsrc since v0.0.18, so `pkgin install rozi` is the usual way to
get it. There are no prebuilt NetBSD archives, so the install scripts and `rozi update` do not cover
NetBSD: update a pkgsrc install through pkgsrc, or build from source.

Community support means:

- CI cross-compiles `x86_64-unknown-netbsd` for every pull request and every push to `master`. A
  separate portability workflow, run weekly and on demand, builds rozi and runs its tests in a
  NetBSD 10.1 VM. Neither check blocks a merge or a release.
- Nobody developing rozi runs NetBSD day to day, so a NetBSD user is likely to find a regression
  before CI does.
- NetBSD has no special-cased code. It uses the same Unix code paths as the other platforms.

Bug reports are welcome, and patches more so. Other BSDs are not packaged or tested, though the same
Unix code paths cover them.
