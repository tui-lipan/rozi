# Platform support

rozi supports Linux, macOS, and Windows with native PTYs and local session servers. It also
builds and runs on NetBSD, which is community-supported rather than released; see
[NetBSD](#netbsd) below.

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Terminal backend | Unix PTY | Unix PTY | ConPTY |
| Local IPC | Unix-domain sockets | Unix-domain sockets | Named pipes |
| Shell integration | bash, zsh, fish | bash, zsh, fish | PowerShell, limited cmd.exe support |
| Foreground program detection | Shell metadata, then `/proc` | Shell metadata, then system process information | Shell metadata |
| Release architectures | x86-64, ARM64 | x86-64, ARM64 | x86-64 |

## Requirements

All platforms need a terminal emulator that can run a full-screen terminal application.

The prebuilt Linux releases require glibc 2.28 or newer. Building from source may inherit a newer
glibc requirement from the build host unless an equivalent compatibility environment is used.

Windows requires Windows 10 version 1809, build 17763, or newer because rozi uses ConPTY. Windows
Terminal is recommended but not required.

Building from source requires Rust 1.90 or newer. See [Installation](installation.md).

## NetBSD

rozi builds and runs on NetBSD on x86-64. The pkgsrc maintainer confirmed a working session on
bare metal, and rozi has been packaged in pkgsrc since v0.0.18, so `pkgin install rozi` is the
usual way to get it there. The install scripts and `rozi update` do not cover NetBSD: there are no
prebuilt NetBSD archives, so a pkgsrc install is updated through pkgsrc, and anything else is a
build from source.

NetBSD is community-supported, which is a narrower promise than the three platforms above:

- Every push is cross-compiled for `x86_64-unknown-netbsd`, and pushes to `master` also build and
  run the test suite in a NetBSD VM. Neither job blocks a merge.
- Nobody developing rozi runs NetBSD day to day, so a regression is likely to be found by a NetBSD
  user rather than by CI.
- Nothing on the platform is special-cased. NetBSD gets whatever the Unix paths do, and where a
  platform needs its own behavior it belongs in `src/platform/` like any other.

Bug reports are welcome and patches more so. Other BSDs are not packaged or tested, though the
same Unix code paths cover them and `platform::errno` already names FreeBSD and DragonFly.

## Files and directories

| Purpose | Linux and macOS | Windows |
| --- | --- | --- |
| Configuration | `$XDG_CONFIG_HOME/rozi`, or `~/.config/rozi` | `%APPDATA%\rozi` |
| State | `$XDG_STATE_HOME/rozi`, or `~/.local/state/rozi` | `%LOCALAPPDATA%\rozi` |
| Cache | `$XDG_CACHE_HOME/rozi`, or `~/.cache/rozi` | `%LOCALAPPDATA%\rozi\cache` |
| Runtime data | `$XDG_RUNTIME_DIR/rozi`, or a private temporary directory | `%LOCALAPPDATA%\rozi\run` |

Set `ROZI_CONFIG` to use a different configuration file on any platform.

rozi restricts local runtime endpoints and session data to the current operating system user.
Remote sessions use SSH authentication rather than exposing local session endpoints over the
network.

## Shell integration

On Linux and macOS, rozi supports bash, zsh, and fish integration. On Windows, PowerShell provides
the full integration. cmd.exe provides prompt markers but cannot report every piece of metadata.

Shell integration helps rozi track prompt boundaries, the current working directory, and the
foreground program. Features that depend on this information may have less context when the active
shell does not report it.

rozi injects shell integration when it starts a pane. It does not edit shell startup files,
PowerShell profiles, or registry startup commands.

See [Terminal features](terminal.md#working-directories-and-shell-metadata).

## Input differences

The default direct modifier is `Alt`. It works on all supported platforms. Many Windows terminals
do not deliver `Super` or Windows-key combinations to terminal applications, so prefix mode is the
reliable choice there.

Windows Terminal and the classic console also keep some `Alt` chords for themselves, including
`Alt+Enter` for fullscreen. Those never reach rozi. Prefer the `Ctrl+A` prefix, unbind the host
shortcut, or rebind the command in rozi.

`Ctrl+C` goes to the program in the focused pane on every platform. It does not quit rozi.

See [Keybindings](keybindings.md#platform-caveats).

## Platform-dependent features

- Foreground process inspection is available on Linux and macOS. Windows relies on shell-reported
  metadata.
- Desktop notifications use the operating system's notification tools and may depend on a desktop
  session being available.
- Clipboard behavior depends on the host clipboard and terminal. OSC 52 can carry copied text
  through SSH when enabled.
- File paths, command shells, and executable lookup follow the conventions of the host operating
  system.

The pane, workspace, session, profile, configuration, and automation interfaces use the same
commands across supported platforms. See [Remote sessions](remote.md) for differences between local
and SSH-attached sessions.
