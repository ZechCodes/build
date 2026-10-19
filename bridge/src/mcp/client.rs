//! The stdio MCP adapter's connection to the daemon control socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::Value;

/// Send one action to the daemon and return its tool result.
pub fn request_daemon(path: &str, request: &str) -> Result<Value, String> {
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("could not reach Build daemon: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("could not set daemon timeout: {error}"))?;
    writeln!(stream, "{request}").map_err(|error| format!("could not send request: {error}"))?;
    super::daemon_action_result(read_response(BufReader::new(stream))?)
}

fn read_response(mut reader: impl BufRead) -> Result<Value, String> {
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .map_err(|error| format!("could not read response: {error}"))?;
    serde_json::from_str(&response).map_err(|error| format!("invalid daemon response: {error}"))
}
