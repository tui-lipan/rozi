//! The file a pane appends its raw PTY output to while `toggle-pane-logging` is on.
//!
//! The stream stays deliberately raw - escape sequences, CR line endings and all - because that is
//! the only lossless form and the only one that can be replayed. Two things are shaped anyway.
//!
//! **hyprmux's own instrumentation is removed.** The shell-integration scripts emit
//! `OSC 133 ; C ; hyprmux_exe=<basename>` to report the foreground program. That parameter is a
//! private hyprmux extension rather than the pane's output, and someone who asked to log `eza`
//! should not find hyprmux's protocol interleaved with it. Only the parameter is dropped; what
//! remains is the bare `OSC 133 ; C` that every other terminal's shell integration writes, so the
//! log stays a faithful recording of an instrumented shell rather than a doctored one.
//!
//! **The file is bounded.** A logged pane running something chatty would otherwise write until the
//! disk filled. At the limit the log closes itself with a trailer and reports why, taking the same
//! path a write error does.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// hyprmux's private command-start marker, emitted by `assets/shell-integration/*`.
const EXE_MARKER: &[u8] = b"\x1b]133;C;hyprmux_exe=";
/// What replaces it: the standard, parameterless OSC 133 command-start marker.
const BARE_COMMAND_START: &[u8] = b"\x1b]133;C";
/// How far past a matched [`EXE_MARKER`] the filter waits for a string terminator before concluding
/// the match was a coincidence and emitting those bytes verbatim. A real marker carries one
/// url-encoded executable basename and is far shorter; the cap exists only so a stream that happens
/// to contain the prefix can never withhold output indefinitely.
const MAX_MARKER_TAIL: usize = 256;

/// Strips [`EXE_MARKER`] payloads from a byte stream that arrives in arbitrary chunks.
///
/// PTY reads split wherever the kernel happens to return, so a marker is routinely delivered across
/// two calls. Bytes that could still turn out to be the start of one are withheld in `carry` rather
/// than emitted, and reconsidered once the next chunk arrives.
#[derive(Default)]
struct MarkerFilter {
    carry: Vec<u8>,
}

impl MarkerFilter {
    fn push(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        let input = if self.carry.is_empty() {
            bytes.to_vec()
        } else {
            let mut joined = std::mem::take(&mut self.carry);
            joined.extend_from_slice(bytes);
            joined
        };

        let mut index = 0;
        while index < input.len() {
            let rest = &input[index..];
            if rest[0] != 0x1b {
                // Copy the whole run up to the next candidate rather than byte at a time.
                let run = rest
                    .iter()
                    .position(|byte| *byte == 0x1b)
                    .unwrap_or(rest.len());
                out.extend_from_slice(&rest[..run]);
                index += run;
                continue;
            }
            if rest.len() < EXE_MARKER.len() {
                if EXE_MARKER.starts_with(rest) {
                    self.carry.extend_from_slice(rest);
                    return;
                }
                out.push(rest[0]);
                index += 1;
                continue;
            }
            if !rest.starts_with(EXE_MARKER) {
                out.push(rest[0]);
                index += 1;
                continue;
            }
            let payload = &rest[EXE_MARKER.len()..];
            match string_terminator(payload) {
                Some(end) => {
                    // Keep the terminator so the surviving marker is still a well-formed OSC.
                    out.extend_from_slice(BARE_COMMAND_START);
                    out.extend_from_slice(&payload[terminator_start(payload, end)..end]);
                    index += EXE_MARKER.len() + end;
                }
                None if payload.len() > MAX_MARKER_TAIL => {
                    out.extend_from_slice(rest);
                    index = input.len();
                }
                None => {
                    self.carry.extend_from_slice(rest);
                    return;
                }
            }
        }
    }

    /// Release anything still withheld. Only ever a partial hyprmux marker, but emitting it beats
    /// silently swallowing bytes that turned out not to be one.
    fn flush(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.carry);
    }
}

/// Offset just past the OSC string terminator ending `payload`, or `None` while it has not arrived.
fn string_terminator(payload: &[u8]) -> Option<usize> {
    for (index, byte) in payload.iter().enumerate() {
        match byte {
            0x07 => return Some(index + 1),
            0x1b => {
                return payload.get(index + 1).and_then(|next| match next {
                    b'\\' => Some(index + 2),
                    // ESC starting something else: this was never a complete marker. Waiting for a
                    // terminator that cannot come is what `MAX_MARKER_TAIL` bounds.
                    _ => None,
                });
            }
            _ => {}
        }
    }
    None
}

/// Where the terminator found at `end` begins, so it can be copied without its payload.
fn terminator_start(payload: &[u8], end: usize) -> usize {
    if payload.get(end - 1) == Some(&0x07) {
        end - 1
    } else {
        end - 2
    }
}

pub struct PaneLog {
    file: File,
    path: PathBuf,
    /// Bytes this file already holds, including anything a previous run appended.
    written: u64,
    /// Size ceiling in bytes; `0` disables the cap.
    limit: u64,
    filter: MarkerFilter,
    /// Reused across writes so a busy pane does not allocate per PTY chunk.
    scratch: Vec<u8>,
}

/// What a log's opening line records, so a file that spans several logging runs is self-describing.
pub(super) struct LogHeader<'a> {
    pub session: &'a str,
    pub pane_id: crate::state::PaneId,
    pub generation: u64,
    pub cols: u16,
    pub rows: u16,
}

impl LogHeader<'_> {
    fn render(&self) -> String {
        let Self {
            session,
            pane_id,
            generation,
            cols,
            rows,
        } = self;
        let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%z");
        // CRLF because the surrounding stream is raw terminal output: a bare LF would leave the
        // next line indented by the column the header ended on when the file is `cat`ed.
        format!(
            "\r\n=== hyprmux log · session {session} · pane {pane_id}-{generation} · \
             {cols}x{rows} · started {started} ===\r\n"
        )
    }
}

impl PaneLog {
    pub(super) fn open(path: &Path, limit: u64, header: &LogHeader<'_>) -> io::Result<Self> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let existing = file.metadata()?.len();
        let mut opening = header.render();
        if existing == 0 {
            // The leading blank line separates appended runs; there is nothing to separate yet.
            opening.remove(0);
            opening.remove(0);
        }
        file.write_all(opening.as_bytes())?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            written: existing + opening.len() as u64,
            limit,
            filter: MarkerFilter::default(),
            scratch: Vec::new(),
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Append `bytes`, minus hyprmux's own markers. `Err` means logging must stop, and carries the
    /// reason to report.
    pub(super) fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.scratch.clear();
        let mut scratch = std::mem::take(&mut self.scratch);
        self.filter.push(bytes, &mut scratch);
        let result = self.write_filtered(&scratch);
        self.scratch = scratch;
        result
    }

    fn write_filtered(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.limit > 0 && self.written.saturating_add(bytes.len() as u64) > self.limit {
            // Stop before the limit rather than after it: a partial chunk would cut a raw log
            // mid-escape-sequence, which is worse to read back than a clean early end.
            let limit = self.limit;
            self.filter.carry.clear();
            let trailer = format!("\r\n=== hyprmux log stopped · size limit {limit} bytes ===\r\n");
            let _ = self.file.write_all(trailer.as_bytes());
            return Err(format!("pane log reached its {limit} byte limit"));
        }
        self.file
            .write_all(bytes)
            .map_err(|error| format!("pane log write failed: {error}"))?;
        self.written += bytes.len() as u64;
        Ok(())
    }
}

impl Drop for PaneLog {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        self.filter.flush(&mut pending);
        if !pending.is_empty() {
            let _ = self.file.write_all(&pending);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filtered(chunks: &[&[u8]]) -> Vec<u8> {
        let mut filter = MarkerFilter::default();
        let mut out = Vec::new();
        for chunk in chunks {
            filter.push(chunk, &mut out);
        }
        filter.flush(&mut out);
        out
    }

    #[test]
    fn exe_payload_is_replaced_by_the_standard_bare_marker() {
        assert_eq!(
            filtered(&[b"before\x1b]133;C;hyprmux_exe=eza\x1b\\after"]),
            b"before\x1b]133;C\x1b\\after"
        );
    }

    #[test]
    fn bel_terminated_markers_keep_their_own_terminator() {
        assert_eq!(
            filtered(&[b"\x1b]133;C;hyprmux_exe=ls\x07rest"]),
            b"\x1b]133;C\x07rest"
        );
    }

    #[test]
    fn a_marker_split_across_pty_reads_is_still_stripped() {
        // Every split point, since the kernel picks it, not us.
        let whole: &[u8] = b"a\x1b]133;C;hyprmux_exe=cargo\x1b\\b";
        for split in 0..whole.len() {
            let (head, tail) = whole.split_at(split);
            assert_eq!(
                filtered(&[head, tail]),
                b"a\x1b]133;C\x1b\\b",
                "split at {split}"
            );
        }
    }

    #[test]
    fn standard_osc_133_markers_survive_untouched() {
        // Only hyprmux's private parameter is ours to remove; A/B/D and a bare C are the open
        // standard other terminals emit too.
        let stream: &[u8] = b"\x1b]133;A\x1b\\\x1b]133;B\x1b\\\x1b]133;C\x1b\\\x1b]133;D;0\x1b\\";
        assert_eq!(filtered(&[stream]), stream);
    }

    #[test]
    fn unrelated_escapes_and_osc_sequences_are_untouched() {
        let stream: &[u8] = b"\x1b[31mred\x1b[0m\x1b]0;title\x07\x1b]7;file://host/tmp\x1b\\";
        assert_eq!(filtered(&[stream]), stream);
    }

    #[test]
    fn an_unterminated_marker_is_emitted_verbatim_once_it_outgrows_the_cap() {
        let mut stream = b"\x1b]133;C;hyprmux_exe=".to_vec();
        stream.extend(std::iter::repeat_n(b'x', MAX_MARKER_TAIL + 1));
        assert_eq!(filtered(&[&stream]), stream);
    }

    #[test]
    fn a_trailing_partial_prefix_is_released_on_close() {
        assert_eq!(filtered(&[b"tail\x1b]133"]), b"tail\x1b]133");
    }

    #[test]
    fn header_is_written_and_the_limit_stops_logging_before_it_is_exceeded() {
        let dir = std::env::temp_dir().join(format!("hyprmux-pane-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("limit.log");
        let _ = std::fs::remove_file(&path);
        let header = LogHeader {
            session: "dev",
            pane_id: 1,
            generation: 2,
            cols: 80,
            rows: 24,
        };
        let mut log = PaneLog::open(&path, 4096, &header).unwrap();
        let opening = std::fs::read_to_string(&path).unwrap();
        assert!(opening.starts_with("=== hyprmux log · session dev · pane 1-2 · 80x24 · started "));
        assert!(opening.ends_with("===\r\n"));

        log.write(b"kept").unwrap();
        let error = log.write(&vec![b'x'; 8192]).unwrap_err();
        assert!(error.contains("4096 byte limit"), "{error}");
        drop(log);

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("kept"));
        // None of the over-limit payload landed: the chunk is refused whole, never half-written.
        assert!(!written.contains("xxxx"));
        assert!(written.ends_with("=== hyprmux log stopped · size limit 4096 bytes ===\r\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopening_separates_runs_with_a_blank_line() {
        let dir =
            std::env::temp_dir().join(format!("hyprmux-pane-log-reopen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reopen.log");
        let _ = std::fs::remove_file(&path);
        let header = LogHeader {
            session: "dev",
            pane_id: 1,
            generation: 2,
            cols: 80,
            rows: 24,
        };
        PaneLog::open(&path, 0, &header)
            .unwrap()
            .write(b"first")
            .unwrap();
        PaneLog::open(&path, 0, &header)
            .unwrap()
            .write(b"second")
            .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written.matches("=== hyprmux log ·").count(), 2);
        assert!(written.contains("first\r\n=== hyprmux log ·"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
