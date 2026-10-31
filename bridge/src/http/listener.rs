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
use crate::execution::session::Opening;

const AUTHORITY_RECHECK: std::time::Duration = std::time::Duration::from_millis(25);

#[derive(Clone, Copy)]
pub struct PeerAddress(pub IpAddr);

type AuthorityCheck = Arc<dyn Fn() -> bool + Send + Sync>;
type AuthorityDeadline = Arc<dyn Fn() -> tokio::time::Instant + Send + Sync>;

struct Authority {
    id: uuid::Uuid,
    check: AuthorityCheck,
    deadline: Option<AuthorityDeadline>,
    opening: Option<Opening>,
}

enum AuthorityPoll<R> {
    Retry,
    Denied,
    Complete(Poll<io::Result<R>>, Option<tokio::time::Instant>),
}

#[derive(Default)]
struct AuthorityState {
    current: Mutex<Option<Authority>>,
    releases: Mutex<Vec<(uuid::Uuid, Box<dyn Send>)>>,
    waker: AtomicWaker,
    pending_write: AtomicBool,
    #[cfg(test)]
    stall_cut: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

type StalledWriteDeadline = Pin<Box<dyn Future<Output = ()> + Send>>;

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
    #[cfg(any(test, feature = "testing"))]
    pub fn has_pending_write(&self) -> bool {
        self.0.pending_write.load(Ordering::Acquire)
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Tests only: this connection's stalled writes are cut when `cut` is
    /// notified, and never by a clock, so a loaded machine cannot let one go
    /// before the test has looked at it. Neither the stalled-write timer, the
    /// authority recheck nor the SSE heartbeat runs: each would let a stalled
    /// or retired stream go. A retired binding's stalled write ends at the cut.
    #[cfg(test)]
    pub fn cut_stalled_writes_on(&self, cut: Arc<tokio::sync::Notify>) {
        *self.0.stall_cut.lock().unwrap() = Some(cut);
    }

    fn stalled_write_deadline(&self) -> StalledWriteDeadline {
        #[cfg(test)]
        if let Some(cut) = self.0.stall_cut.lock().unwrap().clone() {
            return Box::pin(cut.notified_owned());
        }
        Box::pin(tokio::time::sleep(STALLED_WRITE_TIMEOUT))
    }

    /// How often a stream on this connection sends a heartbeat.
    pub fn heartbeat_interval(&self) -> std::time::Duration {
        #[cfg(test)]
        if !self.rechecks_by_clock() {
            return std::time::Duration::from_secs(365 * 24 * 60 * 60);
        }
        super::sse::HEARTBEAT_INTERVAL
    }

    fn rechecks_by_clock(&self) -> bool {
        #[cfg(test)]
        if self.0.stall_cut.lock().unwrap().is_some() {
            return false;
        }
        true
    }
    pub fn set(&self, check: Option<AuthorityCheck>) {
        *self.0.current.lock().unwrap() = check.map(|check| Authority {
            id: uuid::Uuid::new_v4(),
            check,
            deadline: None,
            opening: None,
        });
        self.0.waker.wake();
    }

    pub fn bind(
        &self,
        check: AuthorityCheck,
        deadline: AuthorityDeadline,
        opening: Opening,
    ) -> OutputTicket {
        let id = uuid::Uuid::new_v4();
        *self.0.current.lock().unwrap() = Some(Authority {
            id,
            check,
            deadline: Some(deadline),
            opening: Some(opening),
        });
        self.0.waker.wake();
        OutputTicket {
            authority: self.clone(),
            id,
        }
    }

    fn poll_authorized<R>(
        &self,
        waker: &std::task::Waker,
        mut poll: impl FnMut() -> Poll<io::Result<R>>,
    ) -> io::Result<(Poll<io::Result<R>>, Option<tokio::time::Instant>)> {
        self.0.waker.register(waker);
        loop {
            let (id, opening) = {
                let current = self.0.current.lock().unwrap();
                let Some(current) = current.as_ref() else {
                    return Ok((poll(), None));
                };
                (current.id, current.opening.clone())
            };
            let attempt = if let Some(opening) = opening {
                opening
                    .while_open(|| {
                        let current = self.0.current.lock().unwrap();
                        let Some(current) = current.as_ref().filter(|current| current.id == id)
                        else {
                            return AuthorityPoll::Retry;
                        };
                        let deadline = current.deadline.as_ref().map(|deadline| deadline());
                        if !(current.check)()
                            || deadline
                                .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
                        {
                            AuthorityPoll::Denied
                        } else {
                            AuthorityPoll::Complete(poll(), deadline)
                        }
                    })
                    .unwrap_or(AuthorityPoll::Denied)
            } else {
                let current = self.0.current.lock().unwrap();
                let Some(current) = current.as_ref().filter(|current| current.id == id) else {
                    continue;
                };
                let deadline = current.deadline.as_ref().map(|deadline| deadline());
                if !(current.check)()
                    || deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
                {
                    AuthorityPoll::Denied
                } else {
                    AuthorityPoll::Complete(poll(), deadline)
                }
            };
            match attempt {
                AuthorityPoll::Retry => continue,
                AuthorityPoll::Denied => {
                    self.0.pending_write.store(false, Ordering::Release);
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "HTTP output binding ended",
                    ));
                }
                AuthorityPoll::Complete(result, deadline) => return Ok((result, deadline)),
            }
        }
    }

    fn finish_flush(&self) {
        let released = {
            let mut releases = self.0.releases.lock().unwrap();
            std::mem::take(&mut *releases)
        };
        if released.is_empty() {
            return;
        }
        let mut current = self.0.current.lock().unwrap();
        if current
            .as_ref()
            .is_some_and(|current| released.iter().any(|(released, _)| *released == current.id))
        {
            *current = None;
        }
        drop(current);
        drop(released);
        self.0.waker.wake();
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

    /// Keep this exact response's owners until Hyper flushes the bytes it
    /// accepted from the body into the production socket. A successor binding
    /// is never cleared by an older response's delayed flush.
    pub fn release(self) {
        self.release_with(());
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn release_with_for_testing(self, owner: impl Send + 'static) {
        self.release_with(owner);
    }

    pub(crate) fn release_with(self, owner: impl Send + 'static) {
        self.authority
            .0
            .releases
            .lock()
            .unwrap()
            .push((self.id, Box::new(owner)));
        self.authority.0.waker.wake();
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
    write_deadline: Option<StalledWriteDeadline>,
    authority_deadline: Option<Pin<Box<Sleep>>>,
    authority_recheck: Option<Pin<Box<Sleep>>>,
}

impl<T> BoundedIo<T> {
    pub fn new(inner: T, authority: OutputAuthority) -> Self {
        Self {
            inner,
            authority,
            write_deadline: None,
            authority_deadline: None,
            authority_recheck: None,
        }
    }

    fn arm_authority_wakes(
        &mut self,
        cx: &mut Context<'_>,
        deadline: Option<tokio::time::Instant>,
    ) {
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
        if !self.authority.rechecks_by_clock() {
            return;
        }
        let recheck_at = tokio::time::Instant::now() + AUTHORITY_RECHECK;
        let recheck = self
            .authority_recheck
            .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(recheck_at)));
        recheck.as_mut().reset(recheck_at);
        let _ = recheck.as_mut().poll(cx);
    }

    fn authority_error<R>(&mut self, error: io::Error) -> Poll<io::Result<R>> {
        self.authority
            .0
            .pending_write
            .store(false, Ordering::Release);
        self.write_deadline = None;
        self.authority_deadline = None;
        self.authority_recheck = None;
        Poll::Ready(Err(error))
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
            self.authority_deadline = None;
            self.authority_recheck = None;
            return result;
        }
        let authority = self.authority.clone();
        let timer = self
            .write_deadline
            .get_or_insert_with(|| authority.stalled_write_deadline());
        if timer.as_mut().poll(cx).is_ready() {
            self.authority
                .0
                .pending_write
                .store(false, Ordering::Release);
            self.authority_deadline = None;
            self.authority_recheck = None;
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
        let authority = self.authority.clone();
        let waker = cx.waker().clone();
        let (result, deadline) = match authority
            .poll_authorized(&waker, || Pin::new(&mut self.inner).poll_write(cx, buf))
        {
            Ok(result) => result,
            Err(error) => return self.authority_error(error),
        };
        self.arm_authority_wakes(cx, deadline);
        self.checked_write(cx, result)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let authority = self.authority.clone();
        let waker = cx.waker().clone();
        let (result, deadline) =
            match authority.poll_authorized(&waker, || Pin::new(&mut self.inner).poll_flush(cx)) {
                Ok(result) => result,
                Err(error) => return self.authority_error(error),
            };
        self.arm_authority_wakes(cx, deadline);
        let result = self.checked_write(cx, result);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.authority.finish_flush();
        }
        result
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let authority = self.authority.clone();
        let waker = cx.waker().clone();
        let (result, deadline) = match authority
            .poll_authorized(&waker, || Pin::new(&mut self.inner).poll_shutdown(cx))
        {
            Ok(result) => result,
            Err(error) => return self.authority_error(error),
        };
        self.arm_authority_wakes(cx, deadline);
        let result = self.checked_write(cx, result);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.authority.finish_flush();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// A socket whose peer never reads: writes stay pending and nothing wakes them.
    struct NeverDrains;

    impl AsyncWrite for NeverDrains {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    /// A check going false wakes nothing: no `invalidate()`, no socket progress,
    /// no grant deadline. Only the recheck clock notices, so a retired stream's
    /// stalled write is denied within `AUTHORITY_RECHECK` rather than at the
    /// ten-second stalled-write timeout. Time is paused, so the runtime jumps
    /// straight to whichever timer fires first.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_write_is_denied_within_a_recheck_once_its_check_goes_false() {
        let live = Arc::new(AtomicBool::new(true));
        let authority = OutputAuthority::default();
        let check = live.clone();
        authority.set(Some(Arc::new(move || check.load(Ordering::SeqCst))));
        let mut socket = BoundedIo::new(NeverDrains, authority.clone());
        let write = tokio::spawn(async move { socket.write_all(b"event").await });
        while !authority.has_pending_write() {
            tokio::task::yield_now().await;
        }

        let retired = tokio::time::Instant::now();
        live.store(false, Ordering::SeqCst);
        let denied = write.await.unwrap().unwrap_err();

        assert_eq!(denied.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            retired.elapsed() <= AUTHORITY_RECHECK,
            "denied {:?} after the check went false",
            retired.elapsed()
        );
        assert!(!authority.has_pending_write());
    }
}
