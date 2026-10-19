//! The stdio MCP adapter's connection to the daemon control socket.

use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::Value;

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const REPLY_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Send one action once and wait on that connection for its tool result.
/// A read deadline is not a failed action: the daemon may already have applied
/// it. Keep the connection and partial bytes until a complete reply, a terminal
/// error, or the overall reply deadline. Never reconnect or resend the action.
pub fn request_daemon(path: &str, request: &str) -> Result<Value, String> {
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("could not reach Build daemon: {error}"))?;
    request_on_stream(&mut stream, request, READ_TIMEOUT, REPLY_TIMEOUT)
}

fn request_on_stream(
    stream: &mut UnixStream,
    request: &str,
    read_timeout: Duration,
    reply_timeout: Duration,
) -> Result<Value, String> {
    write_request(stream, request)?;
    let started = Instant::now();
    let response = read_response(
        |buffer, timeout| {
            stream.set_read_timeout(Some(timeout))?;
            stream.read(buffer)
        },
        || started.elapsed(),
        read_timeout,
        reply_timeout,
    )?;
    super::daemon_action_result(response)
}

fn write_request(stream: &mut impl Write, request: &str) -> Result<(), String> {
    writeln!(stream, "{request}")
        .map_err(|error| unknown_outcome(format!("could not send request: {error}")))
}

fn read_response(
    mut read: impl FnMut(&mut [u8], Duration) -> io::Result<usize>,
    mut elapsed: impl FnMut() -> Duration,
    read_timeout: Duration,
    reply_timeout: Duration,
) -> Result<Value, String> {
    let mut response = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let timeout = remaining_read_timeout(elapsed(), read_timeout, reply_timeout)?;
        match read(&mut buffer, timeout) {
            Ok(0) => {
                return Err(unknown_outcome(
                    "Build daemon closed the connection before a complete response",
                ));
            }
            Ok(count) => {
                if let Some(end) = buffer[..count].iter().position(|byte| *byte == b'\n') {
                    response.extend_from_slice(&buffer[..end]);
                    return serde_json::from_slice(&response).map_err(|error| {
                        unknown_outcome(format!("invalid daemon response: {error}"))
                    });
                }
                response.extend_from_slice(&buffer[..count]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(error) => {
                return Err(unknown_outcome(format!("could not read response: {error}")));
            }
        }
    }
}

fn remaining_read_timeout(
    elapsed: Duration,
    read_timeout: Duration,
    reply_timeout: Duration,
) -> Result<Duration, String> {
    let remaining = reply_timeout
        .checked_sub(elapsed)
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            unknown_outcome(format!(
                "Build daemon exceeded the overall reply deadline ({reply_timeout:?})"
            ))
        })?;
    Ok(read_timeout.min(remaining))
}

fn unknown_outcome(reason: impl std::fmt::Display) -> String {
    format!("{reason}. The call may have been applied; check its outcome before retrying.")
}

#[cfg(test)]
mod tests;
