//! One writer owns all data frames, including every continuation and flush.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use futures_util::{FutureExt, SinkExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::{
    protocol::{
        frame::{
            coding::{CloseCode, Data, OpCode},
            Frame,
        },
        CloseFrame,
    },
    Message,
};

use tokio_tungstenite::WebSocketStream;

use crate::{
    execution::session::OutputFence,
    http::listener::{terminal_upgrade::TerminalSocket, OutputAuthority},
};

use super::{
    sink::{DeliveredCursor, Outbound, Reservation},
    terminal_tickets::SNAPSHOT_TIMEOUT_CODE,
    TerminalAdmission,
};

const FRAGMENT_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub(super) struct WriteFailed;

#[derive(Clone, Default)]
pub(super) struct WriterFence {
    active: Arc<Mutex<Option<ActiveWrite>>>,
    writing_frame: Arc<AtomicBool>,
}

struct ActiveWrite {
    fence: OutputFence,
    reservation: Reservation,
}

impl WriterFence {
    pub(super) fn install(&self, output: OutputAuthority, admission: Arc<TerminalAdmission>) {
        let deadline = admission.clone();
        let opening = admission.session().opening().clone();
        let current = self.clone();
        let write = self.clone();
        // Keep admission in the I/O authority until BoundedIo drops.
        // Checks run under the Opening output gate at each TLS write/flush.
        output.bind(
            Arc::new(move || {
                admission.is_live()
                    && current.active.lock().unwrap().as_ref().is_none_or(|write| {
                        write.fence.check().is_ok() && !write.reservation.snapshot_expired()
                    })
            }),
            Arc::new(move || {
                let limit = deadline.deadline();
                write
                    .active
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|write| write.fence.deadline())
                    .map_or(limit, |call| call.min(limit))
            }),
            opening,
        );
    }

    pub(super) async fn write(
        &self,
        socket: &mut TerminalSocket,
        output: Outbound,
    ) -> Result<Option<DeliveredCursor>, WriteFailed> {
        if !output.fence.is_live() {
            return Ok(None);
        }
        let Some(output) = output.into_write() else {
            return Ok(None);
        };
        let expired = output.reservation.snapshot_expired();
        *self.active.lock().unwrap() = Some(ActiveWrite {
            fence: output.fence,
            reservation: output.reservation,
        });
        if expired {
            return Err(WriteFailed);
        }
        self.write_message(socket, &output.bytes).await?;
        drop(output.bytes);
        // Failure or cancellation keeps the permit in the I/O authority
        // until the physical socket releases the staged fragment.
        *self.active.lock().unwrap() = None;
        Ok(output.delivered)
    }
    pub(super) async fn write_message<T: AsyncRead + AsyncWrite + Unpin>(
        &self,
        socket: &mut WebSocketStream<T>,
        bytes: &[u8],
    ) -> Result<(), WriteFailed> {
        if bytes.len() <= FRAGMENT_BYTES {
            return self.send(socket, Message::Binary(bytes.to_vec())).await;
        }
        // Data messages cannot interleave with continuations. The snapshot
        // lease limits how long it can occupy the writer across all fragments.
        let chunks = bytes.chunks(FRAGMENT_BYTES);
        let last = chunks.len() - 1;
        for (index, bytes) in chunks.enumerate() {
            let opcode = if index == 0 {
                Data::Binary
            } else {
                Data::Continue
            };
            self.send(
                socket,
                Message::Frame(Frame::message(
                    bytes.to_vec(),
                    OpCode::Data(opcode),
                    index == last,
                )),
            )
            .await?;
            if index != last {
                tokio::task::yield_now().await;
            }
        }
        Ok(())
    }

    async fn send<T: AsyncRead + AsyncWrite + Unpin>(
        &self,
        socket: &mut WebSocketStream<T>,
        message: Message,
    ) -> Result<(), WriteFailed> {
        self.writing_frame.store(true, Ordering::Release);
        socket.send(message).await.map_err(|_| WriteFailed)?;
        self.writing_frame.store(false, Ordering::Release);
        Ok(())
    }

    pub(super) async fn flush(&self, socket: &mut TerminalSocket) -> Result<(), WriteFailed> {
        self.writing_frame.store(true, Ordering::Release);
        socket.flush().await.map_err(|_| WriteFailed)?;
        self.writing_frame.store(false, Ordering::Release);
        Ok(())
    }

    pub(super) async fn close(
        &self,
        socket: &mut TerminalSocket,
        code: CloseCode,
        reason: &'static str,
    ) {
        let _ = self
            .send(
                socket,
                Message::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                })),
            )
            .await;
    }

    pub(super) fn snapshot_timeout(&self, socket: &mut TerminalSocket) {
        if self.writing_frame.load(Ordering::Acquire) {
            return;
        }
        *self.active.lock().unwrap() = None;
        // A deadline never waits for new write credit to send its Close.
        let _ = self
            .close(socket, CloseCode::Library(4008), SNAPSHOT_TIMEOUT_CODE)
            .now_or_never();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::session::{SnapshotPool, TerminalInterest, TerminalSnapshotLane};
    use crate::http::listener::BoundedIo;
    use tokio::{
        io::AsyncReadExt,
        sync::watch,
        time::{advance, Instant},
    };
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[tokio::test(start_paused = true)]
    async fn trickling_reader_loses_the_snapshot_at_the_absolute_deadline() {
        let pool = Arc::new(SnapshotPool::default());
        let (_stop, stopped) = watch::channel(false);
        let opening = 10;
        let lane = TerminalSnapshotLane::new(opening, pool.clone(), stopped);
        let interest = TerminalInterest::new(Some(lane.clone()));
        let reservation = lane.reserve(&interest).await.unwrap();
        let bytes = vec![b'x'; 12 * 1024 * 1024];
        reservation.encoded(bytes.len());
        let authority = OutputAuthority::default();
        // Each read frees part of a fragment. The full witness crosses
        // continuation frames while leaving most of the message in flight.
        const CHUNK: usize = 32 * 1024;
        let (transport, mut reader) = tokio::io::duplex(CHUNK);
        let mut socket = WebSocketStream::from_raw_socket(
            BoundedIo::new(transport, authority.clone()),
            Role::Server,
            None,
        )
        .await;
        let now = Instant::now();
        let writing = tokio::spawn(async move {
            let writer = WriterFence::default();
            let expired = tokio::select! {
                biased;
                expired = lane.timed_out() => expired,
                result = writer.write_message(&mut socket, &bytes) => panic!("ended early: {result:?}"),
            };
            expired.release();
        });

        let interval = std::time::Duration::from_secs(8);
        let mut chunk = [0; CHUNK];
        for _ in 0..24 {
            reader.read_exact(&mut chunk).await.unwrap();
            while !authority.has_pending_write() {
                assert!(
                    !writing.is_finished(),
                    "writer stalled before the whole-message deadline"
                );
                tokio::task::yield_now().await;
            }
            assert!(pool.holds(opening));
            advance(interval).await;
        }
        writing.await.unwrap();
        assert_eq!(now.elapsed(), std::time::Duration::from_secs(192));
        assert_eq!(pool.available_permits(), 2);
        assert!(!pool.holds(opening));
        drop(reservation);
    }
}
