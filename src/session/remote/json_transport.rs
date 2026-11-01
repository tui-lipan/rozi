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
    validate_remote_target(target).map_err(TransportError::Preflight)?;
    let resolved = ResolvedRemote::resolve(target, config);
    let binary = super::binary::resolve(target, config)
        .map_err(|error| TransportError::Preflight(error.message))?;
    validate_remote_executable_token(&binary.path).map_err(TransportError::Preflight)?;
    let payload = serde_json::to_vec(request).map_err(TransportError::Encode)?;
    let mut command = ssh_base_command(&resolved, config);
    append_ssh_destination(&mut command, &resolved);
    append_remote_rozi_command(&mut command, &binary.path, runner_args, binary.family);
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

pub(crate) fn stream_forward<Request: Serialize>(
    target: &RemoteTarget,
    config: &RemoteConfig,
    runner_args: &[&str],
    request: &Request,
    handle_stream: impl FnOnce(
        &str,
        io::Lines<io::BufReader<std::process::ChildStdout>>,
    ) -> Result<(), String>,
) -> Result<(), TransportError> {
    validate_remote_target(target).map_err(TransportError::Preflight)?;
    let resolved = ResolvedRemote::resolve(target, config);
    let binary = super::binary::resolve(target, config)
        .map_err(|error| TransportError::Preflight(error.message))?;
    validate_remote_executable_token(&binary.path).map_err(TransportError::Preflight)?;
    let payload = serde_json::to_vec(request).map_err(TransportError::Encode)?;
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
    stdin.write_all(&payload).map_err(TransportError::Send)?;
    stdin.write_all(b"\n").map_err(TransportError::Send)?;
    drop(stdin);

    let stdout = child.stdout.take().expect("piped stdout");
    use std::io::BufRead;
    let mut reader = io::BufReader::new(stdout);
    let mut first_line = String::new();
    reader
        .read_line(&mut first_line)
        .map_err(TransportError::Ssh)?;
    if first_line.is_empty() {
        let output = child.wait_with_output().map_err(TransportError::Ssh)?;
        if !output.status.success() {
            return Err(TransportError::Failed(output));
        }
        return Err(TransportError::Answer(AnswerError::Empty));
    }

    if let Err(err) = handle_stream(first_line.trim(), reader.lines()) {
        return Err(TransportError::Preflight(err));
    }

    let _ = child.wait();
    Ok(())
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
