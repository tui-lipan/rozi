//! One JSON request and response over the existing SSH/binary discovery path.
//! Transport failures carry data; callers own command-specific diagnostics.

use std::io::{self, BufRead, Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};

use super::{
    RemoteTarget, ResolvedRemote, append_remote_rozi_command, append_ssh_destination,
    ssh_base_command, validate_remote_executable_token, validate_remote_target,
};
use crate::config::RemoteConfig;

/// How long to wait for ssh's stderr once it has exited.
const STREAM_STDERR_PATIENCE: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub(crate) enum TransportError {
    Preflight(String),
    Encode(serde_json::Error),
    Ssh(io::Error),
    Send(io::Error),
    Failed(Output),
    Answer(AnswerError),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AnswerError {
    Empty,
    Malformed,
}

pub(crate) fn forward<Request: Serialize, Reply: DeserializeOwned>(
    target: &RemoteTarget,
    config: &RemoteConfig,
    runner_args: &[&str],
    request: &Request,
) -> Result<Reply, TransportError> {
    forward_over(target, config, runner_args, request, Stdin::Close)
}

/// [`forward`] for a request that should end with its caller. The runner reads one line, and its
/// standard input then stays open until this process has its answer or has died, however it died:
/// that is how the far side finds out, where the caller's `ssh` would otherwise simply carry on.
pub(crate) fn forward_watched<Request: Serialize, Reply: DeserializeOwned>(
    target: &RemoteTarget,
    config: &RemoteConfig,
    runner_args: &[&str],
    request: &Request,
) -> Result<Reply, TransportError> {
    forward_over(target, config, runner_args, request, Stdin::Hold)
}

#[derive(Clone, Copy, PartialEq)]
enum Stdin {
    Close,
    Hold,
}

fn forward_over<Request: Serialize, Reply: DeserializeOwned>(
    target: &RemoteTarget,
    config: &RemoteConfig,
    runner_args: &[&str],
    request: &Request,
    stdin: Stdin,
) -> Result<Reply, TransportError> {
    let mut command = runner_command(target, config, runner_args)?;
    let payload = serde_json::to_vec(request).map_err(TransportError::Encode)?;
    let result = exchange_over(&mut command, &payload, stdin).and_then(|output| {
        if !output.status.success() {
            return Err(TransportError::Failed(output));
        }
        parse_answer(&output.stdout).map_err(TransportError::Answer)
    });
    if result.is_err() {
        super::binary::invalidate(target, config);
    }
    result
}

/// Send one request, read its JSON answer, and hand that answer and the lines after it to
/// `handle_stream`.
///
/// Stdin stays open until `handle_stream` returns: its closing is how the far side learns nobody
/// wants the stream any more. Lines before the answer that are not one are skipped, since a login
/// shell can print a banner ahead of it.
pub(crate) fn stream_forward<Request: Serialize, Reply: DeserializeOwned, T>(
    target: &RemoteTarget,
    config: &RemoteConfig,
    runner_args: &[&str],
    request: &Request,
    handle_stream: impl FnOnce(Reply, StreamLines) -> Result<T, String>,
) -> Result<T, TransportError> {
    let mut command = runner_command(target, config, runner_args)?;
    let mut payload = serde_json::to_vec(request).map_err(TransportError::Encode)?;
    payload.push(b'\n');
    let (mut runner, mut lines) = StreamRunner::spawn(&mut command)?;
    if let Err(error) = runner.send(&payload) {
        runner.abandon();
        return Err(TransportError::Send(error));
    }
    let result = match read_stream_answer(&mut lines) {
        Ok(Some(reply)) => match handle_stream(reply, lines) {
            Ok(value) => runner.finish().map(|()| value),
            Err(message) => {
                runner.abandon();
                Err(TransportError::Preflight(message))
            }
        },
        Ok(None) => runner.finish_unanswered(),
        Err(error) => {
            runner.abandon();
            Err(TransportError::Ssh(error))
        }
    };
    if matches!(result, Err(TransportError::Failed(_))) {
        super::binary::invalidate(target, config);
    }
    result
}

type StreamLines = io::Lines<io::BufReader<std::process::ChildStdout>>;

/// The first line that parses as an answer, or `None` when the output ended without one.
fn read_stream_answer<Reply: DeserializeOwned>(
    lines: &mut StreamLines,
) -> io::Result<Option<Reply>> {
    for line in lines {
        if let Ok(reply) = serde_json::from_str(line?.trim()) {
            return Ok(Some(reply));
        }
    }
    Ok(None)
}

/// The ssh process behind one stream.
struct StreamRunner {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stderr: mpsc::Receiver<Vec<u8>>,
}

impl StreamRunner {
    fn spawn(command: &mut Command) -> Result<(Self, StreamLines), TransportError> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(TransportError::Ssh)?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr_pipe = child.stderr.take().expect("piped stderr");
        // Read throughout, so a chatty far side can never block on a pipe nobody is draining.
        let (stderr_tx, stderr) = mpsc::channel();
        std::thread::spawn(move || {
            let mut captured = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut captured);
            let _ = stderr_tx.send(captured);
        });
        let lines = io::BufReader::new(stdout).lines();
        Ok((
            Self {
                child,
                stdin,
                stderr,
            },
            lines,
        ))
    }

    fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        self.stdin
            .write_all(payload)
            .and_then(|()| self.stdin.flush())
    }

    /// Close stdin, wait for ssh to leave, and collect what it said.
    fn wait(self) -> Result<Output, TransportError> {
        let Self {
            mut child,
            stdin,
            stderr,
        } = self;
        drop(stdin);
        let status = child.wait().map_err(TransportError::Ssh)?;
        Ok(Output {
            status,
            stdout: Vec::new(),
            // A connection master this ssh left behind can hold the pipe open for minutes.
            stderr: stderr
                .recv_timeout(STREAM_STDERR_PATIENCE)
                .unwrap_or_default(),
        })
    }

    /// End a stream that was answered. Only one ssh itself cut short ends with a failure status.
    fn finish(self) -> Result<(), TransportError> {
        let output = self.wait()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(TransportError::Failed(output))
        }
    }

    /// Say why a stream never got an answer.
    fn finish_unanswered<T>(self) -> Result<T, TransportError> {
        let output = self.wait()?;
        Err(if output.status.success() {
            TransportError::Answer(AnswerError::Empty)
        } else {
            TransportError::Failed(output)
        })
    }

    fn abandon(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The ssh command that runs rozi with `runner_args` on the far host.
fn runner_command(
    target: &RemoteTarget,
    config: &RemoteConfig,
    runner_args: &[&str],
) -> Result<Command, TransportError> {
    validate_remote_target(target).map_err(TransportError::Preflight)?;
    let resolved = ResolvedRemote::resolve(target, config);
    let binary = super::binary::resolve(target, config)
        .map_err(|error| TransportError::Preflight(error.message))?;
    validate_remote_executable_token(&binary.path).map_err(TransportError::Preflight)?;
    let mut command = ssh_base_command(&resolved, config);
    append_ssh_destination(&mut command, &resolved);
    append_remote_rozi_command(&mut command, &binary.path, runner_args, binary.family);
    Ok(command)
}

#[cfg(all(test, unix))]
fn exchange(command: &mut Command, payload: &[u8]) -> Result<Output, TransportError> {
    exchange_over(command, payload, Stdin::Close)
}

fn exchange_over(
    command: &mut Command,
    payload: &[u8],
    after_request: Stdin,
) -> Result<Output, TransportError> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(TransportError::Ssh)?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    // Drain stdout/stderr while sending too: even a login banner can fill a pipe. Closing stdin
    // ends the request. No transport deadline: forwarded waits and checkouts can take a long time.
    std::thread::scope(|scope| {
        let (finished, wait_for_finish) = std::sync::mpsc::channel::<()>();
        let sender = scope.spawn(move || {
            let sent = stdin
                .write_all(payload)
                .and_then(|()| stdin.write_all(b"\n"));
            if after_request == Stdin::Hold {
                let _ = wait_for_finish.recv();
            }
            sent
        });
        let output = child.wait_with_output().map_err(TransportError::Ssh);
        drop(finished);
        let sent = sender.join().expect("request writer panicked");
        let output = output?;
        // A rejecting runner may close stdin before reading. Preserve its status and diagnostics.
        if output.status.success() {
            sent.map_err(TransportError::Send)?;
        }
        Ok(output)
    })
}

/// Find the last typed JSON line, ignoring login banners and trailing logout messages.
pub(crate) fn parse_answer<Reply: DeserializeOwned>(stdout: &[u8]) -> Result<Reply, AnswerError> {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(AnswerError::Empty);
    }
    stdout
        .split(|byte| *byte == b'\n')
        .rev()
        .find_map(|line| serde_json::from_slice(line).ok())
        .ok_or(AnswerError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Reply {
        answer: u32,
    }

    #[test]
    fn last_typed_answer_survives_banners_and_logout() {
        assert_eq!(
            parse_answer::<Reply>(
                b"Welcome\n{\"answer\":1}\n{\"unrelated\":2}\n{\"answer\":3}\nlogout\n"
            ),
            Ok(Reply { answer: 3 })
        );
    }

    #[test]
    fn empty_and_malformed_answers_are_distinct() {
        for empty in [b"".as_slice(), b" \r\n\t"] {
            assert_eq!(parse_answer::<Reply>(empty), Err(AnswerError::Empty));
        }
        for malformed in [
            b"Welcome\n".as_slice(),
            b"{bad}",
            b"{\"unrelated\":2}",
            b"\xff",
        ] {
            assert_eq!(
                parse_answer::<Reply>(malformed),
                Err(AnswerError::Malformed)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_held_stdin_stays_open_after_the_request_and_never_outlives_the_answer() {
        // Reads the request, then looks in the background for the end of file a closed stdin gives.
        let probe = r#"IFS= read -r line; echo "$line"
exec 3<&0
(IFS= read -r more <&3; : > "$0") > /dev/null 2>&1 &
sleep 0.3
if [ -e "$0" ]; then echo closed; else echo held; fi"#;
        for (mode, expected) in [(Stdin::Hold, "held"), (Stdin::Close, "closed")] {
            let directory = tempfile::tempdir().unwrap();
            let mut command = Command::new("sh");
            command
                .args(["-c", probe])
                .arg(directory.path().join("eof"));
            let output = exchange_over(&mut command, b"request", mode).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                format!("request\n{expected}\n")
            );
        }
    }

    #[test]
    fn ssh_failure_invalidates_the_resolved_binary() {
        let target = RemoteTarget::Alias("json-transport-invalidation-fixture".into());
        let mut config = RemoteConfig::default();
        config.hosts.insert(
            target.display_label(),
            crate::config::RemoteHostConfig {
                // OpenSSH rejects this option before connecting, without relying on any real host.
                ssh_args: vec!["-o".into(), "RoziInvalidOption=yes".into()],
                ..Default::default()
            },
        );
        super::super::binary::remember(
            &target,
            &config,
            "rozi".into(),
            super::super::bootstrap::RemoteFamily::Posix,
        )
        .unwrap();
        let result: Result<Reply, _> = forward(
            &target,
            &config,
            &["--remote-control", "dev"],
            &serde_json::json!({}),
        );
        assert!(
            matches!(
                result,
                Err(TransportError::Failed(_) | TransportError::Ssh(_))
            ),
            "{result:?}"
        );
        assert!(super::super::binary::last_known(&target, &config).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_large_banner_and_request_can_flow_in_both_directions() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 262144 /dev/zero; cat"]);
        let request = vec![b'x'; 262144];
        let output = exchange(&mut command, &request).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 524289);
        assert_eq!(&output.stdout[262144..524288], request);
        assert_eq!(output.stdout.last(), Some(&b'\n'));
    }

    #[cfg(unix)]
    #[test]
    fn rejecting_runner_keeps_its_diagnostic_when_it_closes_stdin_early() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'unknown runner' >&2; exit 17"]);
        let output = exchange(&mut command, &vec![b'x'; 262144]).unwrap();
        assert_eq!(output.status.code(), Some(17));
        assert_eq!(output.stderr, b"unknown runner");
    }

    #[cfg(unix)]
    #[test]
    fn exchange_closes_input_and_preserves_output_status_and_stderr() {
        let mut command = Command::new("sh");
        command.args(["-c", "cat; printf 'diagnostic' >&2; exit 23"]);
        let output = exchange(&mut command, b"request").unwrap();
        assert_eq!(output.stdout, b"request\n");
        assert_eq!(output.stderr, b"diagnostic");
        assert_eq!(output.status.code(), Some(23));
    }

    #[cfg(unix)]
    #[test]
    fn exchange_preserves_signal_termination() {
        let mut command = Command::new("sh");
        command.args(["-c", "cat >/dev/null; kill -TERM $$"]);
        assert_eq!(exchange(&mut command, b"{}").unwrap().status.code(), None);
    }
}
