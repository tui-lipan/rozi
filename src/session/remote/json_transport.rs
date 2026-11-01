//! One JSON request and response over the existing SSH/binary discovery path.
//! Transport failures carry data; callers own command-specific diagnostics.

use std::io::{self, Write};
use std::process::{Command, Output, Stdio};

use serde::{Serialize, de::DeserializeOwned};

use super::{
    RemoteTarget, ResolvedRemote, append_remote_rozi_command, append_ssh_destination,
    ssh_base_command, validate_remote_executable_token, validate_remote_target,
};
use crate::config::RemoteConfig;

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
    validate_remote_target(target).map_err(TransportError::Preflight)?;
    let resolved = ResolvedRemote::resolve(target, config);
    let binary = super::binary::resolve(target, config)
        .map_err(|error| TransportError::Preflight(error.message))?;
    validate_remote_executable_token(&binary.path).map_err(TransportError::Preflight)?;
    let payload = serde_json::to_vec(request).map_err(TransportError::Encode)?;
    let mut command = ssh_base_command(&resolved, config);
    append_ssh_destination(&mut command, &resolved);
    append_remote_rozi_command(&mut command, &binary.path, runner_args, binary.family);
    let result = exchange(&mut command, &payload).and_then(|output| {
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
    handle_stream: impl FnOnce(
        Reply,
        io::Lines<io::BufReader<std::process::ChildStdout>>,
    ) -> Result<T, String>,
) -> Result<T, TransportError> {
    use std::io::{BufRead, Read};

    validate_remote_target(target).map_err(TransportError::Preflight)?;
    let resolved = ResolvedRemote::resolve(target, config);
    let binary = super::binary::resolve(target, config)
        .map_err(|error| TransportError::Preflight(error.message))?;
    validate_remote_executable_token(&binary.path).map_err(TransportError::Preflight)?;
    let mut payload = serde_json::to_vec(request).map_err(TransportError::Encode)?;
    payload.push(b'\n');
    let mut command = ssh_base_command(&resolved, config);
    append_ssh_destination(&mut command, &resolved);
    append_remote_rozi_command(&mut command, &binary.path, runner_args, binary.family);

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(TransportError::Ssh)?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    // Read throughout, so a chatty far side can never block on a pipe nobody is draining.
    let (stderr_tx, stderr) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut captured = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut captured);
        let _ = stderr_tx.send(captured);
    });
    let finish = |child: &mut std::process::Child, stderr: std::sync::mpsc::Receiver<Vec<u8>>| {
        let status = child.wait().map_err(TransportError::Ssh)?;
        Ok::<_, TransportError>(Output {
            status,
            stdout: Vec::new(),
            // A connection master this ssh left behind can hold the pipe open for minutes.
            stderr: stderr
                .recv_timeout(std::time::Duration::from_millis(500))
                .unwrap_or_default(),
        })
    };
    if let Err(error) = stdin.write_all(&payload).and_then(|()| stdin.flush()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(TransportError::Send(error));
    }

    let mut lines = io::BufReader::new(stdout).lines();
    let mut skipped = false;
    let reply = loop {
        match lines.next() {
            Some(Ok(line)) => match serde_json::from_str::<Reply>(line.trim()) {
                Ok(reply) => break reply,
                Err(_) => skipped = true,
            },
            Some(Err(error)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TransportError::Ssh(error));
            }
            None => {
                drop(stdin);
                let output = finish(&mut child, stderr)?;
                return Err(if !output.status.success() {
                    TransportError::Failed(output)
                } else if skipped {
                    TransportError::Answer(AnswerError::Malformed)
                } else {
                    TransportError::Answer(AnswerError::Empty)
                });
            }
        }
    };

    let result = handle_stream(reply, lines);
    drop(stdin);
    match result {
        Ok(value) => {
            // The far side is told to stop by stdin closing, and ssh by the far side leaving. Being
            // told is enough when this end stopped reading; only a stream ssh itself cut short ends
            // with a failure status on its own.
            let output = finish(&mut child, stderr)?;
            if output.status.success() {
                Ok(value)
            } else {
                Err(TransportError::Failed(output))
            }
        }
        Err(message) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(TransportError::Preflight(message))
        }
    }
}

fn exchange(command: &mut Command, payload: &[u8]) -> Result<Output, TransportError> {
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
        let sender = scope.spawn(move || {
            stdin
                .write_all(payload)
                .and_then(|()| stdin.write_all(b"\n"))
        });
        let output = child.wait_with_output().map_err(TransportError::Ssh);
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
