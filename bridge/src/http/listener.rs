//! Supervised native HTTPS. Connection failures cannot stop shared services.
use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

#[path = "terminal_upgrade.rs"]
pub mod terminal_upgrade;

use terminal_upgrade::{wait_for_stop, TerminalUpgrade};

mod write_progress;

pub(in crate::http) use write_progress::TcpWriteProgress;

pub const AUTHORITY_RECHECK: std::time::Duration = std::time::Duration::from_millis(25);

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
    // Zero until the terminal upgrade. Then counts TCP bytes plus one.
    write_progress: AtomicU64,
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
    pub(in crate::http) fn track_write_progress(&self) {
        self.0.write_progress.store(1, Ordering::Relaxed);
    }

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

    /// Healthy output cadence and independent idle-authority wakeups.
    pub(crate) fn stream_timing(&self) -> super::sse_stream::StreamTiming {
        #[cfg(test)]
        if !self.rechecks_by_clock() {
            let manual = std::time::Duration::from_secs(365 * 24 * 60 * 60);
            return super::sse_stream::StreamTiming {
                heartbeat: manual,
                authority_recheck: manual,
            };
        }
        super::sse_stream::StreamTiming::default()
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

/// One log window per listener, preserved across successful accepts as well as
/// retry backoffs so intermittent resource pressure cannot flood stderr.
#[derive(Default)]
struct AcceptRetryLog(Mutex<Option<tokio::time::Instant>>);

impl AcceptRetryLog {
    fn report(&self, error: &io::Error, write: impl FnOnce(&io::Error)) {
        let mut next = self.0.lock().unwrap();
        let now = tokio::time::Instant::now();
        if next.is_some_and(|next| now < next) {
            return;
        }
        *next = Some(now + std::time::Duration::from_secs(1));
        drop(next);
        write(error);
    }
}

/// Retry resource pressure and per-connection failures without terminating the
/// daemon. The enclosing select keeps shutdown and connection reaping live
/// during this backoff; fatal listener errors still reach daemon supervision.
async fn accept_with_retry<T, F, A>(mut accept: F, retry_log: &AcceptRetryLog) -> io::Result<T>
where
    F: FnMut() -> A,
    A: Future<Output = io::Result<T>>,
{
    loop {
        match accept().await {
            Err(error) if transient_accept_error(&error) => {
                retry_log.report(&error, |error| {
                    eprintln!("bridge HTTPS accept: {error}; retrying in 75ms");
                });
                tokio::time::sleep(std::time::Duration::from_millis(75)).await;
            }
            result => return result,
        }
    }
}

fn transient_accept_error(error: &io::Error) -> bool {
    // Linux accept(2) also propagates ENONET; macOS has no such constant.
    #[cfg(target_os = "linux")]
    if error.raw_os_error() == Some(libc::ENONET) {
        return true;
    }
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::TimedOut
    ) || matches!(
        error.raw_os_error(),
        Some(
            libc::EMFILE
                | libc::ENFILE
                | libc::ENOBUFS
                | libc::ENOMEM
                | libc::EPROTO
                | libc::ENOPROTOOPT
                | libc::EHOSTDOWN
                | libc::EOPNOTSUPP
                | libc::ENETDOWN
                | libc::ENETUNREACH
                | libc::EHOSTUNREACH
        )
    )
}

pub async fn serve(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    router: Router,
    shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    serve_connections(
        listener,
        tls,
        router,
        shutdown,
        Arc::new(Semaphore::new(limits::MAX_SOCKETS)),
        limits::TerminalSocketBudget::default(),
    )
    .await
}

async fn serve_connections(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    router: Router,
    mut shutdown: watch::Receiver<bool>,
    sockets: Arc<Semaphore>,
    terminal_sockets: limits::TerminalSocketBudget,
) -> io::Result<()> {
    let (stop, stopped) = watch::channel(false);
    let acceptor = TlsAcceptor::from(tls);
    let mut connections = JoinSet::new();
    // Retain the retry future across completed connections so reaping cannot
    // cancel the backoff and turn resource exhaustion into a busy loop.
    let retry_log = AcceptRetryLog::default();
    let next_connection = || accept_with_retry(|| listener.accept(), &retry_log);
    let mut accepting = Box::pin(next_connection());
    let result = loop {
        if *shutdown.borrow() {
            break Ok(());
        }
        tokio::select! {
            _ = shutdown.changed() => break Ok(()),
            accepted = &mut accepting => {
                accepting.set(next_connection());
                let (socket, peer) = match accepted { Ok(value) => value, Err(error) => break Err(error) };
                let Ok(permit) = sockets.clone().try_acquire_owned() else { continue; };
                let service = router.clone();
                let acceptor = acceptor.clone();
                let stop = stopped.clone();
                let terminal_sockets = terminal_sockets.clone();
                connections.spawn(async move {
                    // The HTTP driver can return while the upgraded I/O is still
                    // live. Keep this socket and its permit in the same join task.
                    let _permit = permit;
                    let _ = connection(socket, peer.ip(), acceptor, service, stop, terminal_sockets).await;
                });
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    };
    // Stop accepting and fence each HTTP socket before bounded teardown.
    drop(accepting);
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
    terminal_sockets: limits::TerminalSocketBudget,
) -> io::Result<()> {
    socket.set_nodelay(true)?;
    let authority = OutputAuthority::default();
    let socket = TcpWriteProgress::new(socket, authority.clone());
    let stream = tokio::select! {
        accepted = timeout(limits::HEADER_TIMEOUT, acceptor.accept(socket)) => {
            accepted.map_err(|_| io::Error::new(io::ErrorKind::TimedOut,"TLS handshake deadline"))??
        }
        _ = stop.changed() => return Ok(()),
    };
    let (upgrade, mut job) = TerminalUpgrade::new(terminal_sockets, stop.clone());
    drive_http(
        stream,
        peer,
        router,
        stop.clone(),
        upgrade,
        authority.clone(),
    )
    .await?;
    if let Ok(job) = job.try_recv() {
        tokio::select! {
            biased;
            _ = wait_for_stop(&mut stop) => {}
            _ = job.run(authority) => {}
        }
    }
    Ok(())
}

async fn drive_http(
    stream: tokio_rustls::server::TlsStream<TcpWriteProgress>,
    peer: IpAddr,
    router: Router,
    mut stop: watch::Receiver<bool>,
    upgrade: TerminalUpgrade,
    authority: OutputAuthority,
) -> io::Result<()> {
    let mut io = BoundedIo::new(stream, authority.clone());
    io.stop = Some(stop.clone());
    let io = TokioIo::new(io);
    let service = TowerToHyperService::new(
        router
            .layer(Extension(PeerAddress(peer)))
            .layer(Extension(authority.clone()))
            .layer(Extension(upgrade)),
    );
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits::HEADER_TIMEOUT)
        .max_buf_size(limits::MAX_HEADER_BYTES)
        .max_headers(64)
        .writev(false);
    let connection = builder.serve_connection(io, service).with_upgrades();
    tokio::pin!(connection);
    tokio::select! {
        biased;
        _ = wait_for_stop(&mut stop) => {
            authority.set(Some(Arc::new(|| false)));
            connection.as_mut().graceful_shutdown();
            let _ = timeout(limits::SOCKET_TEARDOWN_TIMEOUT, connection).await;
            Ok(())
        }
        result = &mut connection => result.map_err(io::Error::other),
    }
}

/// Finite socket stalled-write time, independent of heartbeats and auth refresh.
pub struct BoundedIo<T> {
    inner: T,
    authority: OutputAuthority,
    stop: Option<watch::Receiver<bool>>,
    write_deadline: Option<StalledWriteDeadline>,
    write_progress: u64,
    authority_deadline: Option<Pin<Box<Sleep>>>,
    authority_recheck: Option<Pin<Box<Sleep>>>,
}

impl<T> BoundedIo<T> {
    pub fn new(inner: T, authority: OutputAuthority) -> Self {
        Self {
            inner,
            authority,
            stop: None,
            write_deadline: None,
            write_progress: 0,
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

    fn poll_output<R>(
        &mut self,
        cx: &mut Context<'_>,
        mut poll: impl FnMut(&mut T, &mut Context<'_>) -> Poll<io::Result<R>>,
    ) -> Poll<io::Result<R>> {
        let (inner, stop) = (&mut self.inner, &self.stop);
        let result = self.authority.poll_authorized(&cx.waker().clone(), || {
            // Serialize signaling shutdown with the actual TLS poll. The same
            // guard applies to HTTP headers and to writes after the upgrade.
            let stopped = stop.as_ref().map(watch::Receiver::borrow);
            if stopped.as_deref() == Some(&true) {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "HTTP listener stopped",
                )));
            }
            poll(inner, cx)
        });
        match result {
            Ok((result, deadline)) => {
                self.arm_authority_wakes(cx, deadline);
                self.checked_write(cx, result)
            }
            Err(error) => self.checked_write(cx, Poll::Ready(Err(error))),
        }
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
        let progress = self.authority.0.write_progress.load(Ordering::Relaxed);
        if progress != self.write_progress {
            self.write_progress = progress;
            if self.authority.rechecks_by_clock() {
                self.write_deadline = None;
            }
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
        self.poll_output(cx, |inner, cx| Pin::new(inner).poll_write(cx, buf))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = self.poll_output(cx, |inner, cx| Pin::new(inner).poll_flush(cx));
        if matches!(result, Poll::Ready(Ok(()))) {
            self.authority.finish_flush();
        }
        result
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = self.poll_output(cx, |inner, cx| Pin::new(inner).poll_shutdown(cx));
        if matches!(result, Poll::Ready(Ok(()))) {
            self.authority.finish_flush();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::poll;
    use tokio::io::AsyncWriteExt;
    use tokio::time::{advance, Instant};

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

    #[tokio::test(start_paused = true)]
    async fn pending_terminal_flush_gets_ten_seconds_from_each_transport_write() {
        let authority = OutputAuthority::default();
        authority.track_write_progress();
        let mut socket = BoundedIo::new(NeverDrains, authority.clone());
        let mut flush = Box::pin(socket.flush());
        let before_deadline = STALLED_WRITE_TIMEOUT - std::time::Duration::from_secs(1);
        assert!(poll!(&mut flush).is_pending());

        for _ in 0..2 {
            advance(before_deadline).await;
            // The fake TLS flush is still Pending, but each time the TCP
            // socket beneath it has accepted one more byte.
            authority.0.write_progress.fetch_add(1, Ordering::Relaxed);
            assert!(poll!(&mut flush).is_pending());
        }

        let last_progress = Instant::now();
        advance(before_deadline).await;
        assert!(poll!(&mut flush).is_pending());
        let error = flush.await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(last_progress.elapsed(), STALLED_WRITE_TIMEOUT);
        assert!(!authority.has_pending_write());
    }

    #[tokio::test(start_paused = true)]
    async fn blocked_terminal_write_times_out_in_ten_seconds() {
        let authority = OutputAuthority::default();
        authority.track_write_progress();
        let mut socket = BoundedIo::new(NeverDrains, authority.clone());
        let started = Instant::now();
        let error = socket.write_all(b"frame").await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), STALLED_WRITE_TIMEOUT);
        assert!(!authority.has_pending_write());
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

#[cfg(test)]
mod accept_tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn transient_accept_errors_back_off_then_accept_again() {
        let retry_log = AcceptRetryLog::default();
        for code in [
            libc::EMFILE,
            libc::ENFILE,
            libc::ENOBUFS,
            libc::ENOMEM,
            libc::ECONNABORTED,
            libc::EINTR,
            libc::ENOPROTOOPT,
            libc::EHOSTDOWN,
            #[cfg(target_os = "linux")]
            libc::ENONET,
            libc::EOPNOTSUPP,
        ] {
            let mut calls = 0;
            let started = tokio::time::Instant::now();
            let result = accept_with_retry(
                || {
                    calls += 1;
                    std::future::ready(if calls == 1 {
                        Err(io::Error::from_raw_os_error(code))
                    } else {
                        Ok(42)
                    })
                },
                &retry_log,
            )
            .await
            .unwrap();
            assert_eq!(result, 42);
            assert_eq!(calls, 2);
            assert!(started.elapsed() >= std::time::Duration::from_millis(75));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retry_logs_are_limited_to_one_per_second() {
        let retry_log = AcceptRetryLog::default();
        let error = io::Error::from_raw_os_error(libc::EMFILE);
        let mut lines = Vec::new();
        let started = tokio::time::Instant::now();
        for _ in 0..30 {
            retry_log.report(&error, |_| lines.push(tokio::time::Instant::now()));
            tokio::time::sleep(std::time::Duration::from_millis(75)).await;
        }
        assert_eq!(
            lines.len(),
            3,
            "one immediate log, then about one per second"
        );
        assert_eq!(lines[0], started);
        for pair in lines.windows(2) {
            assert!(pair[1] - pair[0] >= std::time::Duration::from_secs(1));
            assert!(pair[1] - pair[0] < std::time::Duration::from_millis(1075));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fatal_accept_errors_propagate_without_retry() {
        let retry_log = AcceptRetryLog::default();
        let mut calls = 0;
        let result: io::Result<()> = accept_with_retry(
            || {
                calls += 1;
                std::future::ready(Err(io::Error::from_raw_os_error(libc::EBADF)))
            },
            &retry_log,
        )
        .await;
        assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EBADF));
        assert_eq!(calls, 1);
    }
}
