# Image and file paste over SSH

Investigation against rozi `ffbfa781`, using released `tui-lipan 0.27.3` sources.
Herdr source reference: `143bf5fd4781d164260c63fe28e7dc08054ad32f`.
Codex source reference: `e4568a3538c9cab6627d64ff8cdfc7d98b79b7db`.
These are the inspected source revisions, not a claim that the installed Codex
binary, which reports `0.163.0`, was built from that revision.

rozi lacks a local-to-session attachment transfer path. Image rendering and
OSC 52 clipboard writes do not provide that path. Herdr reads an image on the
client, sends its bytes through its client/server protocol, stages a file on the
server, and pastes the server-side path into the agent. The same design fits
rozi's server-owned panes.

## Why the reported error happens

1. rozi builds its terminal widget with
   `TerminalPasteShortcutBehavior::Performable` in `src/view/pane.rs`.
2. In `tui-lipan`'s `src/clipboard/provider.rs`, `read_terminal_paste` classifies
   file lists and image data as `ClipboardPasteContent::Rich`. A rich format
   takes precedence over any accompanying text representation.
3. In `src/app/input/handlers/terminal.rs`, `preflight_key` pastes text itself,
   but returns `Forward` for rich content. rozi's normal key routing then sends
   Ctrl+V to the pane through the session connection.
4. Codex handles that key as an image paste. Its
   `codex-rs/tui/src/clipboard_paste.rs::paste_image_as_png` constructs an
   `arboard::Clipboard` in the process running Codex. For a remote pane, that
   process runs on the remote host. Clipboard initialization failure becomes
   `ClipboardUnavailable`; the application adds `Failed to paste image:`.

The supplied X11 timeout is consistent with this exact code path. It is the
remote Codex process trying to open a desktop clipboard. The source trace does
not establish whether that particular host has an unset, stale, or forwarded
`DISPLAY` value. Changing `DISPLAY` does not add attachment transfer to rozi.

The explicit rozi paste command also needs attention. In `src/actions.rs`,
`paste_from_focused_pane` calls only `ctx.clipboard().read()` and sends encoded
text. It does not read image bytes or file lists.

Existing framework tests explicitly cover forwarding images and file lists,
including rich clipboards that also advertise text. See
`terminal_performable_ctrl_v_forwards_image_without_text`,
`terminal_performable_ctrl_v_forwards_file_list_without_text`, and
`terminal_performable_ctrl_v_prefers_image_over_text_fallback` in
`src/app/runner/run_tests.rs`. These are source evidence; they were not executed
during this investigation.

## Which SSH workflows can support this

| Workflow | Clipboard reader | What attachment transfer requires |
| --- | --- | --- |
| Local rozi attached to a remote rozi session | Local desktop | Add image/file transfer to the existing session connection. |
| Local rozi with an ordinary `ssh host` process inside a local pane | Local desktop, but pane ownership is local | A separate remote helper or explicit upload integration. The local session server cannot make its temporary paths exist on the SSH host. |
| `ssh host`, followed by running rozi there | Remote host | A local companion or terminal-supported transfer mechanism. Remote rozi cannot directly read the local desktop clipboard. |
| Local rozi and a local agent | Local desktop | Existing native agent clipboard access can work; a new bridge must preserve it unless the pane opts in. |

Containers and nested SSH add another filesystem boundary. A path staged beside
the owning server is useful only if the consuming agent can read that filesystem.
Do not claim nested SSH or container support from success with a managed remote
session.

## What Herdr actually implements

The relevant implementation is in
[client/clipboard_images.rs](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/client/clipboard_images.rs),
[client/mod.rs](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/client/mod.rs),
[server/clipboard_image.rs](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/server/clipboard_image.rs),
and [server/headless.rs](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/server/headless.rs).

- The local client intercepts its configurable image-paste key, Ctrl+V by
  default, and an empty bracketed paste when a remote image bridge is active.
  It reads the local clipboard and otherwise leaves normal input handling in
  place. Overlay and pane targeting determine whether it can bridge the input.
- A separate file-drop parser recognizes a single absolute image path. It
  accepts PNG, JPEG, GIF, WebP, and BMP extensions, handles quotes and Unix
  backslash escaping, checks for a regular file, and reads with a size limit.
- The client sends `ClientMessage::ClipboardImage` containing the target,
  extension, and bytes. Both ends enforce a 16 MiB payload limit. This uses the
  existing session stream, rather than spawning an `scp` command for each paste.
- The server checks the target, writes a unique file in a staging directory,
  and routes its path as a paste. On Unix it requests a mode-0700 directory and
  mode-0600 files. It tracks files per client, removes them when that client is
  removed, and sweeps files older than 24 hours when staging another image.

This inspected path is an image bridge. It does not handle arbitrary documents,
directories, or a batch of dropped files. Herdr's
[remote guide](https://herdr.dev/docs/persistence-remote/#remote-attach-over-ssh)
also explicitly distinguishes a local client from running Herdr entirely inside
an ordinary SSH connection.

The transfer is separate from agent attachment semantics. Codex's
[composer](https://github.com/openai/codex/blob/e4568a3538c9cab6627d64ff8cdfc7d98b79b7db/codex-rs/tui/src/bottom_pane/chat_composer.rs)
opens a pasted path as an image and creates an attachment if it can read its
dimensions. Its `pasting_filepath_attaches_image` test documents this behavior.
Claude Code's [image workflow](https://code.claude.com/docs/en/common-workflows#work-with-images)
supports providing an image path. Both can therefore consume a transferred
image without accessing a remote desktop clipboard. Their handling of ordinary
document paths is a separate contract; uploading a PDF does not itself turn it
into an image attachment.

## Recommended implementation

### Framework input support

Add the reusable terminal paste capability in `../tui-lipan`, then consume a
released framework version in rozi. The current terminal input event contains
encoded bytes and an input kind, not image content or file-list content.

The framework already has image and file-list readers on `ClipboardProvider`
and `ClipboardService`, and public `ClipboardHandle::read_files`. Its image
paste support for editable widgets does not expose an equivalent terminal
callback. Add an opt-in terminal callback that can deliver typed image/file
content before the existing rich-content key forwarding. Preserve current
forwarding when the terminal has no attachment handler. Do not implement
another native clipboard provider inside rozi.

Keep native clipboard gestures distinct from terminal-emitted text pastes.
The latter may contain a dropped path but carry no OS drag metadata. An ordinary
pasted path can refer to a remote file, so do not automatically upload every
string that happens to name a local file. Prefer real clipboard file lists and
an explicit file-paste action; define a narrow image-drop policy separately.
An empty bracketed paste is a terminal-specific signal to investigate, not a
universal indication that an image was pasted.

### Session transfer and staging

Add a typed attachment transfer to rozi's existing authenticated session
connection. Bind each transfer to the selected connection, pane ID, PTY
generation, and locality at initiation. Completion must not resolve the newly
focused pane and accidentally paste into it.

Use bounded chunks and an explicit begin/finish/cancel exchange. rozi's maximum
frame is 8 MiB, so Herdr's 16 MiB one-message image design cannot be copied into
the existing framing unchanged. Set file, total-transfer, and pending-transfer
limits independently of frame size. Stream file bytes off the UI thread and
provide completion or failure feedback. Never inject file bytes as PTY input.

The session server should enforce the same pane ownership and input permissions
as ordinary typing. It stages the completed file, then inserts the server path
with the target PTY's current bracketed-paste mode. Do not send Enter. A rejected
or partially transferred attachment must not also forward the original Ctrl+V
and trigger the remote clipboard error.

Put platform-specific staging under `src/platform/`, using the existing
`platform::fs_security` policy. Generate paths on the server; do not accept an
arbitrary destination path from the client. Preserve a safe basename for
documents, use exclusive file creation, and reject directories and special
files. Decide how symlinks are handled before reading local file-list entries.

Make staged files session-owned rather than client-owned. A detached client can
leave an agent with an unsubmitted prompt referring to a staged file. Herdr's
disconnect cleanup would be too early for that case. Define retention, stale
file cleanup, and disk limits without silently deleting a file an active draft
still needs. Persistence across server restart needs its own explicit policy.

### Images and other files

Implement the transfer for regular files, with image clipboard data represented
as a generated PNG file. This avoids separate network transports for screenshots
and documents. Initially paste one image path per paste event, since Codex's
inspected image-path parser recognizes a single path.

For non-image files, transfer first and insert readable paths. Leave interpretation
to the target TUI. Claude Code also documents `@` file references, but rozi should
not inject agent-specific syntax into arbitrary programs. Filenames with spaces,
quotes, newlines, Windows drive prefixes, and multiple selections require a
defined path-encoding policy and application-level tests.

Keep OSC 52's existing enable/disable enforcement intact. It controls clipboard
writes from pane output and does not implement this local-to-remote file transfer.

## Verification required for implementation

- Framework tests with injected clipboard providers for text, raw images,
  file lists, mixed formats, unavailable clipboard, and provider errors.
  Exercise preflight handling as well as app-command paste so an app handler
  cannot be bypassed by the terminal widget consuming Ctrl+V first.
- Session tests for exact transferred bytes, size limits, partial transfers,
  cancellation, disconnects, stale PTY generations, read-only clients,
  invalid targets, cleanup, and a focus change during transfer. Isolate all
  user directories and endpoints.
- Isolated live local-client/remote-server tests through SSH with the remote
  desktop clipboard unavailable. Verify both Codex and Claude Code attach a
  screenshot and a copied image file without an API submission. Test document
  paths separately. Check that text paste and empty-clipboard Ctrl+V retain
  their expected behavior, and inspect the actual inserted path and remote file.
- Live checks for the outer terminal's paste and drag behavior, plus macOS and
  Windows clipboard and staging coverage. A headless test alone cannot prove
  what key sequence a user's terminal sends.

This investigation changed no runtime code or dependencies. It inspected rozi,
the released framework source and its existing tests, Herdr, Codex, and Claude
Code documentation. It did not reproduce the user's SSH session, read or replace
their clipboard, or run agent prompts. No Rust checks were run because the only
repository change is this investigation and its index entry.
