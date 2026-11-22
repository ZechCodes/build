use std::{
    io::{self, IoSlice},
    pin::Pin,
    sync::atomic::Ordering,
    task::{ready, Context, Poll},
};

use socket2::SockRef;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    task::coop,
};

use super::OutputAuthority;

/// Measures output below TLS, which can write ciphertext and still return
/// Pending from a plaintext write or flush.
pub(in crate::http) struct TcpWriteProgress {
    stream: TcpStream,
    authority: OutputAuthority,
}

impl TcpWriteProgress {
    pub(in crate::http) fn new(stream: TcpStream, authority: OutputAuthority) -> Self {
        Self { stream, authority }
    }

    fn observe_write(
        &self,
        cx: &mut Context<'_>,
        result: Poll<io::Result<usize>>,
        try_send: impl FnOnce(SockRef<'_>) -> io::Result<usize>,
    ) -> Poll<io::Result<usize>> {
        let progress = &self.authority.0.write_progress;
        if progress.load(Ordering::Relaxed) == 0 {
            return result;
        }
        let result = if result.is_pending() {
            // A TCP send buffer can accept bytes before Tokio reports it writable.
            // On every pending poll of a tracked connection, try the socket directly
            // if the cooperative budget permits. That includes readiness wakes and
            // the listener's periodic authority rechecks.
            let budget = ready!(coop::poll_proceed(cx));
            match try_send(SockRef::from(&self.stream)) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
                result => {
                    budget.made_progress();
                    Poll::Ready(result)
                }
            }
        } else {
            result
        };
        if let Poll::Ready(Ok(bytes)) = result {
            progress.fetch_add(bytes as u64, Ordering::Relaxed);
        }
        result
    }
}

impl AsyncRead for TcpWriteProgress {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for TcpWriteProgress {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write(cx, buf);
        self.observe_write(cx, result, |socket| {
            socket.send_with_flags(buf, libc::MSG_NOSIGNAL)
        })
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write_vectored(cx, bufs);
        self.observe_write(cx, result, |socket| {
            socket.send_vectored_with_flags(bufs, libc::MSG_NOSIGNAL)
        })
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
