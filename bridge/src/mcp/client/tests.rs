use super::*;
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read};

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
    read_response(BufReader::new(ScriptedReader(steps.into())))
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

fn assert_unknown_outcome(error: &str) {
    assert!(error.contains("may have been applied"), "{error}");
    assert!(
        error.contains("check its outcome before retrying"),
        "{error}"
    );
}
