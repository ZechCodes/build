//! Real Unix-socket bytes and native origins; no handler/effect success claim.
use super::emit;
use crate::timing::write_wait::{WriteCohort, WriteSnapshot, WriteWaitRegistry};
use crate::timing::FrameClock;
use serde_json::{json, Value};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWrite};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

const ORIGINAL: &str = "qual/ack-emission/mcp/set-topic";
const SAFETY: Duration = Duration::from_secs(5);

/// Writes use the real socket. Only return from the actual flush is held,
/// allowing the client to decode before the server commits its send outcome.
struct HeldFlush {
    stream: UnixStream,
    release: oneshot::Receiver<bool>,
    first_write_at: Option<Instant>,
}
impl AsyncWrite for HeldFlush {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.first_write_at.get_or_insert_with(Instant::now);
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.stream).poll_flush(cx) {
            Poll::Ready(Ok(())) => {}
            other => return other,
        }
        match Pin::new(&mut self.release).poll(cx) {
            Poll::Ready(Ok(true)) => Poll::Ready(Ok(())),
            Poll::Ready(_) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "controlled failed flush after actual response bytes",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

struct Evidence {
    before_commit: WriteSnapshot,
    after_root: WriteSnapshot,
    after_child: WriteSnapshot,
    reply: Value,
    decoded_at: Instant,
    released_at: Instant,
    emission_entered_at: Instant,
    first_write_at: Instant,
}
fn exact_row(registry: &WriteWaitRegistry) -> WriteSnapshot {
    let rows = registry.snapshot();
    assert_eq!(rows.len(), 1, "exact native original only: {rows:?}");
    rows.into_iter().next().unwrap()
}

async fn emit_after_client_decode(success: bool) -> Evidence {
    let clock = FrameClock::new();
    let registry = clock.capture_write_waits();
    let frame =
        json!({"id":ORIGINAL,"request":{"action":"set_topic","topic":"Exact reply boundary"}});
    let origin = clock.mcp_write(&frame, Instant::now()).unwrap();
    let child = origin.context().child();
    let child_id = child.id();
    let (stream, client) = UnixStream::pair().unwrap();
    let (release, released) = oneshot::channel();
    let mut emitter = tokio::spawn(async move {
        let mut writer = HeldFlush {
            stream,
            release: released,
            first_write_at: None,
        };
        let emission_entered_at = Instant::now();
        emit(
            &mut writer,
            json!({"ok":true,"result":{"topic":"Exact reply boundary"}}),
            Some(&origin),
        )
        .await;
        (
            emission_entered_at,
            writer
                .first_write_at
                .expect("native content write occurred"),
        )
    });
    let mut client = tokio::io::BufReader::new(client);
    let mut line = String::new();
    let read = tokio::time::timeout(SAFETY, client.read_line(&mut line)).await;
    let parsed = serde_json::from_str::<Value>(&line);
    // Actual parse has finished before either the release or native root finish.
    let decoded_at = Instant::now();
    let before_commit = exact_row(&registry);
    let released_at = Instant::now();
    let sent_release = release.send(success);
    let joined = match tokio::time::timeout(SAFETY, &mut emitter).await {
        Ok(joined) => joined,
        Err(error) => {
            emitter.abort();
            let _ = emitter.await;
            drop(client);
            child.complete();
            panic!("owned native emitter did not join: {error}");
        }
    };
    drop(client);
    let after_root = exact_row(&registry);
    child.complete();
    let after_child = exact_row(&registry);
    // All owned I/O and required descendants are settled before assertions.
    let (emission_entered_at, first_write_at) = joined.expect("owned emitter did not panic");
    assert!(
        read.is_ok_and(|read| read.is_ok_and(|bytes| bytes > 0)),
        "actual newline reply: {line:?}"
    );
    assert!(
        sent_release.is_ok(),
        "native flush remained held until client decode"
    );
    assert_eq!(after_root.outstanding, vec![child_id]);
    assert!(
        after_root.completed_at.is_none(),
        "required child remains owed"
    );
    assert!(after_child.outstanding.is_empty() && after_child.completed_at.is_some());
    assert!(registry.active_snapshot().is_empty());
    Evidence {
        before_commit,
        after_root,
        after_child,
        reply: parsed.unwrap(),
        decoded_at,
        released_at,
        emission_entered_at,
        first_write_at,
    }
}

fn assert_exact_native_identity(evidence: &Evidence) {
    for row in [
        &evidence.before_commit,
        &evidence.after_root,
        &evidence.after_child,
    ] {
        assert_eq!(row.original_id, ORIGINAL);
        assert_eq!(row.method, "mcp.control:set_topic");
        assert_eq!(row.cohort, WriteCohort::SemanticWrite);
        assert_eq!(row.id, evidence.before_commit.id);
        assert!(!row.abandoned);
    }
    assert_eq!(
        evidence.reply,
        json!({"ok":true,"result":{"topic":"Exact reply boundary"}})
    );
    assert!(!evidence.before_commit.root_finished);
    assert!(
        evidence.before_commit.acknowledged_at.is_none(),
        "emission attempt is not committed before successful flush"
    );
    assert!(evidence.after_root.root_finished);
    assert!(evidence.after_child.completed_at.unwrap() >= evidence.released_at);
}

#[tokio::test]
async fn successful_native_mcp_ack_keeps_emission_time_before_client_decode() {
    let evidence = emit_after_client_decode(true).await;
    assert_exact_native_identity(&evidence);
    assert!(!evidence.after_root.cancelled);
    let acknowledged = evidence
        .after_root
        .acknowledged_at
        .expect("successful native emission has an ACK");
    assert!(acknowledged >= evidence.after_root.admitted_at.unwrap());
    assert!(
        acknowledged >= evidence.emission_entered_at,
        "reply ACK cannot be backdated to intake/admission or another earlier phase"
    );
    assert!(
        acknowledged <= evidence.first_write_at,
        "successful ACK must retain the native emission sample before its first write"
    );
    assert!(acknowledged <= evidence.decoded_at,
        "ACK must retain the successful emission's pre-decode timestamp; ack={acknowledged:?}, decoded={:?}, committed only after={:?}",
        evidence.decoded_at, evidence.released_at);
}

#[tokio::test]
async fn failed_native_mcp_flush_after_client_decode_cannot_publish_ack() {
    let evidence = emit_after_client_decode(false).await;
    assert_exact_native_identity(&evidence);
    assert!(evidence.after_root.cancelled);
    assert!(
        evidence.after_root.acknowledged_at.is_none(),
        "decoded bytes do not turn a failed native flush into an ACK"
    );
    assert!(evidence.after_child.cancelled && evidence.after_child.acknowledged_at.is_none());
}
