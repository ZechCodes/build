use super::*;
use std::cell::Cell;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};

enum ReadStep {
    Bytes(&'static [u8]),
    Error(ErrorKind),
}

struct ScriptedReader(VecDeque<ReadStep>);

impl Read for ScriptedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.pop_front() {
            Some(ReadStep::Bytes(bytes)) => {
                buffer[..bytes.len()].copy_from_slice(bytes);
                Ok(bytes.len())
            }
            Some(ReadStep::Error(kind)) => Err(kind.into()),
            None => Ok(0),
        }
    }
}

fn scripted_response(steps: Vec<ReadStep>) -> Result<Value, String> {
    let mut reader = ScriptedReader(steps.into());
    let elapsed = Cell::new(Duration::ZERO);
    read_response(
        |buffer, _timeout| {
            // Each scripted read costs ten virtual seconds, without sleeping.
            elapsed.set(elapsed.get() + READ_TIMEOUT);
            reader.read(buffer)
        },
        || elapsed.get(),
        READ_TIMEOUT,
        REPLY_TIMEOUT,
    )
}

#[test]
fn late_reply_survives_read_deadlines() {
    let response = scripted_response(vec![
        ReadStep::Error(ErrorKind::WouldBlock),
        ReadStep::Error(ErrorKind::TimedOut),
        ReadStep::Error(ErrorKind::Interrupted),
        ReadStep::Bytes(b"{\"ok\":true,\"result\":42}\n"),
    ]);
    assert_eq!(response, Ok(serde_json::json!({"ok":true,"result":42})));
}

#[test]
fn partial_json_and_split_utf8_survive_read_deadlines() {
    let response = scripted_response(vec![
        ReadStep::Bytes(b"{\"ok\":true,"),
        ReadStep::Error(ErrorKind::WouldBlock),
        ReadStep::Bytes(b"\"result\":\"caf\xc3"),
        ReadStep::Error(ErrorKind::TimedOut),
        ReadStep::Bytes(b"\xa9\"}"),
        ReadStep::Error(ErrorKind::Interrupted),
        ReadStep::Bytes(b"\n"),
    ]);
    assert_eq!(response, Ok(serde_json::json!({"ok":true,"result":"café"})));
}

#[test]
fn disconnect_before_a_complete_reply_reports_unknown_outcome() {
    for steps in [vec![], vec![ReadStep::Bytes(b"{\"ok\":true}")]] {
        let error = scripted_response(steps).unwrap_err();
        assert!(error.contains("closed"), "{error}");
        assert_unknown_outcome(&error);
    }
}

#[test]
fn terminal_read_error_reports_unknown_outcome() {
    let error = scripted_response(vec![ReadStep::Error(ErrorKind::ConnectionReset)]).unwrap_err();
    assert!(error.contains("could not read response"), "{error}");
    assert_unknown_outcome(&error);
}

#[test]
fn invalid_reply_reports_unknown_outcome() {
    let error = scripted_response(vec![ReadStep::Bytes(b"invalid\n")]).unwrap_err();
    assert!(error.contains("invalid daemon response"), "{error}");
    assert_unknown_outcome(&error);
}

#[test]
fn missing_reply_reaches_overall_deadline_with_unknown_outcome() {
    let steps = (0..40)
        .map(|_| ReadStep::Error(ErrorKind::TimedOut))
        .collect();
    let error = scripted_response(steps).unwrap_err();
    assert!(error.contains("overall reply deadline"), "{error}");
    assert_unknown_outcome(&error);
}

#[test]
fn trickling_bytes_cannot_reset_or_evade_overall_deadline() {
    let mut steps = vec![ReadStep::Bytes(b"{")];
    steps.extend((0..40).map(|_| ReadStep::Bytes(b" ")));
    steps.push(ReadStep::Bytes(b"\"ok\":true}\n"));
    let error = scripted_response(steps).unwrap_err();
    assert!(error.contains("overall reply deadline"), "{error}");
    assert_unknown_outcome(&error);
}

#[test]
fn final_socket_read_is_limited_to_the_remaining_overall_budget() {
    let elapsed = Cell::new(Duration::ZERO);
    let mut timeouts = Vec::new();
    let error = read_response(
        |_buffer, timeout| {
            timeouts.push(timeout);
            elapsed.set(elapsed.get() + timeout);
            Err(ErrorKind::TimedOut.into())
        },
        || elapsed.get(),
        Duration::from_millis(10),
        Duration::from_millis(25),
    )
    .unwrap_err();
    assert_eq!(timeouts, [10, 10, 5].map(Duration::from_millis));
    assert!(error.contains("overall reply deadline"), "{error}");
    assert_unknown_outcome(&error);
}

#[test]
fn late_socket_reply_arrives_after_one_request_on_the_original_connection() {
    let (mut client, mut daemon) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        daemon
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(daemon.try_clone().unwrap());
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        assert_eq!(request, "{\"request\":\"apply_once\"}\n");
        std::thread::sleep(Duration::from_millis(40));
        daemon
            .write_all(b"{\"ok\":true,\"result\":{\"applied\":true}}\n")
            .unwrap();
        let mut extra = Vec::new();
        reader.read_to_end(&mut extra).unwrap();
        assert!(extra.is_empty(), "the action must never be resent");
    });
    let result = request_on_stream(
        &mut client,
        "{\"request\":\"apply_once\"}",
        Duration::from_millis(2),
        Duration::from_secs(5),
    );
    drop(client);
    server.join().unwrap();
    assert_eq!(result, Ok(serde_json::json!({"applied":true})));
}

struct PartialWrite(Vec<u8>);

impl Write for PartialWrite {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if !self.0.is_empty() {
            return Err(ErrorKind::BrokenPipe.into());
        }
        self.0.extend_from_slice(&buffer[..2]);
        Ok(2)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn partial_send_failure_reports_unknown_outcome() {
    let mut writer = PartialWrite(Vec::new());
    let error = write_request(&mut writer, "{\"request\":\"apply_once\"}").unwrap_err();
    assert_eq!(writer.0, b"{\"");
    assert!(error.contains("could not send request"), "{error}");
    assert_unknown_outcome(&error);
}

fn assert_unknown_outcome(error: &str) {
    assert!(error.contains("may have been applied"), "{error}");
    assert!(
        error.contains("check its outcome before retrying"),
        "{error}"
    );
}
