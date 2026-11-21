//! One writer owns all data frames, including every continuation and flush.

use std::sync::{Arc, Mutex};

use futures_util::SinkExt;
use tokio::sync::OwnedSemaphorePermit;
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

use crate::{
    execution::session::OutputFence,
    http::listener::{terminal_upgrade::TerminalSocket, OutputAuthority},
};

use super::{sink::Outbound, TerminalAdmission};

const FRAGMENT_BYTES: usize = 256 * 1024;

pub(super) struct WriteFailed;

#[derive(Clone, Default)]
pub(super) struct WriterFence(Arc<Mutex<Option<ActiveWrite>>>);

struct ActiveWrite {
    fence: OutputFence,
    _reservation: OwnedSemaphorePermit,
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
                    && current
                        .0
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_none_or(|write| write.fence.check().is_ok())
            }),
            Arc::new(move || {
                let limit = deadline.deadline();
                write
                    .0
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
    ) -> Result<(), WriteFailed> {
        if !output.fence.is_live() {
            return Ok(());
        }
        let Some(output) = output.into_write() else {
            return Ok(());
        };
        *self.0.lock().unwrap() = Some(ActiveWrite {
            fence: output.fence,
            _reservation: output.reservation,
        });
        write_message(socket, &output.bytes).await?;
        // Failure or cancellation keeps the permit in the I/O authority
        // until the physical socket releases the staged fragment.
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

pub(super) async fn write_message(
    socket: &mut TerminalSocket,
    bytes: &[u8],
) -> Result<(), WriteFailed> {
    if bytes.len() <= FRAGMENT_BYTES {
        return send(socket, Message::Binary(bytes.to_vec())).await;
    }
    let chunks = bytes.chunks(FRAGMENT_BYTES);
    let last = chunks.len() - 1;
    for (index, bytes) in chunks.enumerate() {
        let opcode = if index == 0 {
            Data::Binary
        } else {
            Data::Continue
        };
        // SinkExt::send awaits flush, including on continuation frames.
        send(
            socket,
            Message::Frame(Frame::message(
                bytes.to_vec(),
                OpCode::Data(opcode),
                index == last,
            )),
        )
        .await?;
    }
    Ok(())
}

async fn send(socket: &mut TerminalSocket, message: Message) -> Result<(), WriteFailed> {
    socket.send(message).await.map_err(|_| WriteFailed)
}

pub(super) async fn close(socket: &mut TerminalSocket, code: CloseCode) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: "".into(),
        })))
        .await;
}
