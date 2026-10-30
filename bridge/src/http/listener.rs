//! Supervised native HTTPS. Connection failures cannot stop shared services.
use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use axum::{Extension, Router};
use futures_util::task::AtomicWaker;
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;
use tokio::time::{timeout, Sleep};
use tokio_rustls::TlsAcceptor;

use super::{limits, sse::STALLED_WRITE_TIMEOUT};

#[derive(Clone, Copy)]
pub struct PeerAddress(pub IpAddr);

type AuthorityCheck = Arc<dyn Fn() -> bool + Send + Sync>;
type AuthorityDeadline = Arc<dyn Fn() -> tokio::time::Instant + Send + Sync>;

struct Authority {
    id: uuid::Uuid,
    check: AuthorityCheck,
    deadline: Option<AuthorityDeadline>,
}

#[derive(Default)]
struct AuthorityState {
    current: Mutex<Option<Authority>>,
    waker: AtomicWaker,
    pending_write: AtomicBool,
}

/// HTTP/1 has one response writer. Checks apply at each socket write, including
/// buffered output, and a separate timer wakes pending writes at grant expiry.
#[derive(Clone, Default)]
pub struct OutputAuthority(Arc<AuthorityState>);

#[derive(Clone)]
pub struct OutputTicket {
    authority: OutputAuthority,
    id: uuid::Uuid,
}

impl OutputAuthority {
    #[cfg(test)]
    pub fn has_pending_write(&self) -> bool {
        self.0.pending_write.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn set(&self, check: Option<AuthorityCheck>) {
        *self.0.current.lock().unwrap() = check.map(|check| Authority {
            id: uuid::Uuid::new_v4(),
            check,
            deadline: None,
        });
        self.0.waker.wake();
    }

    pub fn bind(&self, check: AuthorityCheck, deadline: AuthorityDeadline) -> OutputTicket {
        let id = uuid::Uuid::new_v4();
        *self.0.current.lock().unwrap() = Some(Authority {
            id,
            check,
            deadline: Some(deadline),
        });
        self.0.waker.wake();
        OutputTicket {
            authority: self.clone(),
            id,
        }
    }

    fn check(&self, cx: &Context<'_>) -> io::Result<Option<tokio::time::Instant>> {
        self.0.waker.register(cx.waker());
        let current = self.0.current.lock().unwrap();
        let Some(current) = current.as_ref() else {
            return Ok(None);
        };
        let deadline = current.deadline.as_ref().map(|deadline| deadline());
        if !(current.check)()
            || deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "HTTP output binding ended",
            ));
        }
        Ok(deadline)
    }
}

impl OutputTicket {
    pub fn invalidate(&self) {
        let mut current = self.authority.0.current.lock().unwrap();
        if let Some(current) = current.as_mut().filter(|current| current.id == self.id) {
            current.check = Arc::new(|| false);
            self.authority.0.waker.wake();
        }
    }
}

pub async fn serve(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    router: Router,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let sockets = Arc::new(Semaphore::new(limits::MAX_SOCKETS));
    let (stop, stopped) = watch::channel(false);
    let acceptor = TlsAcceptor::from(tls);
    let mut connections = JoinSet::new();
    let result = loop {
        if *shutdown.borrow() {
            break Ok(());
        }
        tokio::select! {
            _ = shutdown.changed() => break Ok(()),
            accepted = listener.accept() => {
                let (socket, peer) = match accepted { Ok(value) => value, Err(error) => break Err(error) };
                let Ok(permit) = sockets.clone().try_acquire_owned() else { continue; };
                let service = router.clone();
                let acceptor = acceptor.clone();
                let stop = stopped.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    let _ = connection(socket, peer.ip(), acceptor, service, stop).await;
                });
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    };
    // Stop accepting and fence each HTTP socket before bounded teardown.
    drop(listener);
    let _ = stop.send(true);
    if timeout(limits::SOCKET_TEARDOWN_TIMEOUT, async {
        while connections.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    result
}

async fn connection(
    socket: TcpStream,
    peer: IpAddr,
    acceptor: TlsAcceptor,
    router: Router,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    socket.set_nodelay(true)?;
    let stream = tokio::select! {
        accepted = timeout(limits::HEADER_TIMEOUT, acceptor.accept(socket)) => {
            accepted.map_err(|_| io::Error::new(io::ErrorKind::TimedOut,"TLS handshake deadline"))??
        }
        _ = stop.changed() => return Ok(()),
    };
    let authority = OutputAuthority::default();
    let io = TokioIo::new(BoundedIo::new(stream, authority.clone()));
    let service = TowerToHyperService::new(
        router
            .layer(Extension(PeerAddress(peer)))
            .layer(Extension(authority.clone())),
    );
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits::HEADER_TIMEOUT)
        .max_buf_size(limits::MAX_HEADER_BYTES)
        .max_headers(64)
        .writev(false);
    let connection = builder.serve_connection(io, service);
    tokio::pin!(connection);
    tokio::select! {
        result = &mut connection => result.map_err(io::Error::other),
        _ = stop.changed() => {
            authority.set(Some(Arc::new(|| false)));
            connection.as_mut().graceful_shutdown();
            let _ = timeout(limits::SOCKET_TEARDOWN_TIMEOUT, connection).await;
            Ok(())
        }
    }
}

/// Finite socket stalled-write time, independent of heartbeats and auth refresh.
pub struct BoundedIo<T> {
    inner: T,
    authority: OutputAuthority,
    write_deadline: Option<Pin<Box<Sleep>>>,
    authority_deadline: Option<Pin<Box<Sleep>>>,
}

impl<T> BoundedIo<T> {
    pub fn new(inner: T, authority: OutputAuthority) -> Self {
        Self {
            inner,
            authority,
            write_deadline: None,
            authority_deadline: None,
        }
    }

    fn check_authority(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        let deadline = self.authority.check(cx).inspect_err(|_| {
            self.authority
                .0
                .pending_write
                .store(false, Ordering::Release);
        })?;
        if let Some(deadline) = deadline {
            let timer = self
                .authority_deadline
                .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(deadline)));
            timer.as_mut().reset(deadline);
            // Register a wake independently from socket progress or heartbeat.
            let _ = timer.as_mut().poll(cx);
        } else {
            self.authority_deadline = None;
        }
        Ok(())
    }

    fn checked_write<R>(
        &mut self,
        cx: &mut Context<'_>,
        result: Poll<io::Result<R>>,
    ) -> Poll<io::Result<R>> {
        self.authority
            .0
            .pending_write
            .store(result.is_pending(), Ordering::Release);
        if result.is_ready() {
            self.write_deadline = None;
            return result;
        }
        let timer = self
            .write_deadline
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(STALLED_WRITE_TIMEOUT)));
        if timer.as_mut().poll(cx).is_ready() {
            self.authority
                .0
                .pending_write
                .store(false, Ordering::Release);
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HTTP stalled write",
            )));
        }
        Poll::Pending
    }
}

impl<T> Drop for BoundedIo<T> {
    fn drop(&mut self) {
        self.authority
            .0
            .pending_write
            .store(false, Ordering::Release);
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for BoundedIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for BoundedIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Err(error) = self.check_authority(cx) {
            return Poll::Ready(Err(error));
        }
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.checked_write(cx, result)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Err(error) = self.check_authority(cx) {
            return Poll::Ready(Err(error));
        }
        let result = Pin::new(&mut self.inner).poll_flush(cx);
        self.checked_write(cx, result)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
