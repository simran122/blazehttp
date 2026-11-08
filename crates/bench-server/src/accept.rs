//! The accept loop: a bounded number of open connections, one task each, and retries on transient accept errors
//! (design note §5). The listener is behind the [`Listen`] trait so the retry path is testable with a fake.

use std::future::Future;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// Pause after running out of a resource (descriptors, buffers, memory), so a full table is not polled in a busy
/// loop. Errors about one queued connection are retried at once.
const RETRY_PAUSE: Duration = Duration::from_millis(10);

/// A source of accepted TCP connections.
pub(crate) trait Listen {
    fn accept(&self) -> impl Future<Output = io::Result<TcpStream>> + Send;
}

impl Listen for TcpListener {
    async fn accept(&self) -> io::Result<TcpStream> {
        TcpListener::accept(self).await.map(|(stream, _)| stream)
    }
}

/// Accepts until an accept error classified [`AcceptFailure::Fatal`], running `handle(stream)` as one task per
/// connection. A permit is taken *before* accepting, so at the limit new clients wait in the kernel's listen backlog
/// instead of in memory (G6).
pub(crate) async fn accept_loop<L, H, F>(
    listener: &L,
    limit: &Arc<Semaphore>,
    tasks: &mut JoinSet<()>,
    handle: H,
) -> io::Result<()>
where
    L: Listen,
    H: Fn(TcpStream) -> F,
    F: Future<Output = io::Result<()>> + Send + 'static,
{
    let mut logged = ErrorLog::default();
    loop {
        let permit = Arc::clone(limit)
            .acquire_owned()
            .await
            .map_err(io::Error::other)?;
        let stream = next_connection(listener, &mut logged).await?;
        while tasks.try_join_next().is_some() {}
        let connection = handle(stream);
        tasks.spawn(async move {
            // A failed connection (reset, timeout, bad handshake) ends only itself.
            let _ = connection.await;
            drop(permit);
        });
    }
}

/// The next connection with `TCP_NODELAY` set (responses must not wait for Nagle). Transient errors are logged once
/// per kind and retried: after [`RETRY_PAUSE`] when a resource ran out, at once for a connection lost in the queue.
/// Connections that died before `TCP_NODELAY` could be set are skipped.
async fn next_connection<L: Listen>(listener: &L, logged: &mut ErrorLog) -> io::Result<TcpStream> {
    loop {
        match listener.accept().await {
            Ok(stream) if stream.set_nodelay(true).is_ok() => return Ok(stream),
            Ok(_) => {}
            Err(e) => match classify(&e) {
                AcceptFailure::ResourcesExhausted => {
                    logged.once(&e, &mut io::stderr());
                    tokio::time::sleep(RETRY_PAUSE).await;
                }
                AcceptFailure::ConnectionLost => logged.once(&e, &mut io::stderr()),
                AcceptFailure::Fatal => return Err(e),
            },
        }
    }
}

/// What an accept error says, following "Error handling" in `accept(2)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcceptFailure {
    /// The process or system ran out of descriptors, buffers or memory: retry after a pause.
    ResourcesExhausted,
    /// One queued connection failed (aborted, reset, or on Linux a network error passed on from the new socket):
    /// retry at once.
    ConnectionLost,
    /// Anything else says the listener itself is broken.
    Fatal,
}

fn classify(e: &io::Error) -> AcceptFailure {
    let code = e.raw_os_error();
    if e.kind() == io::ErrorKind::OutOfMemory || code.is_some_and(|c| RESOURCE_ERRNO.contains(&c)) {
        return AcceptFailure::ResourcesExhausted;
    }
    let lost = matches!(
        e.kind(),
        io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
    ) || code.is_some_and(|c| CONNECTION_ERRNO.contains(&c));
    if lost {
        AcceptFailure::ConnectionLost
    } else {
        AcceptFailure::Fatal
    }
}

#[cfg(unix)]
const RESOURCE_ERRNO: &[i32] = &[libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM];

#[cfg(target_os = "linux")]
const CONNECTION_ERRNO: &[i32] = &[
    libc::ECONNABORTED,
    libc::EPROTO,
    libc::ENETDOWN,
    libc::ENOPROTOOPT,
    libc::EHOSTDOWN,
    libc::ENONET,
    libc::EHOSTUNREACH,
    libc::EOPNOTSUPP,
    libc::ENETUNREACH,
];

#[cfg(all(unix, not(target_os = "linux")))]
const CONNECTION_ERRNO: &[i32] = &[libc::ECONNABORTED];

#[cfg(not(unix))]
const RESOURCE_ERRNO: &[i32] = &[];

#[cfg(not(unix))]
const CONNECTION_ERRNO: &[i32] = &[];

/// Remembers which error kinds were already reported, so a persistent condition prints one line, not one per retry.
#[derive(Default)]
struct ErrorLog {
    seen: Vec<(io::ErrorKind, Option<i32>)>,
}

impl ErrorLog {
    /// Reports `e` on `out` the first time its kind is seen; a failed report is ignored (a closed stderr must not
    /// stop the server, and `eprintln!` would panic on it).
    fn once(&mut self, e: &io::Error, out: &mut impl Write) {
        let key = (e.kind(), e.raw_os_error());
        if !self.seen.contains(&key) {
            self.seen.push(key);
            let _ = writeln!(out, "bench-server: accept failed ({e}); retrying");
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::future::Future;
    use std::io;
    use std::sync::Mutex;
    use std::time::Duration;

    use tokio::net::{TcpListener, TcpStream};

    use super::{AcceptFailure, ErrorLog, Listen, classify, next_connection};

    /// Fails with the queued errors first, then accepts for real.
    struct Flaky {
        errors: Mutex<Vec<io::Error>>,
        inner: TcpListener,
    }

    impl Listen for Flaky {
        async fn accept(&self) -> io::Result<TcpStream> {
            let queued = self.errors.lock().expect("errors").pop();
            match queued {
                Some(e) => Err(e),
                None => self.inner.accept().await.map(|(stream, _)| stream),
            }
        }
    }

    /// A broken retry loop fails the test in seconds instead of hanging it.
    async fn within_seconds<T>(work: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), work)
            .await
            .expect("accept loop finished within 5 s")
    }

    async fn flaky(errors: Vec<io::Error>) -> Flaky {
        let inner = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        Flaky {
            errors: Mutex::new(errors),
            inner,
        }
    }

    #[tokio::test]
    async fn transient_errors_are_retried_and_logged_once_per_kind() {
        let errors =
            [libc::EMFILE, libc::EMFILE, libc::ECONNABORTED].map(io::Error::from_raw_os_error);
        let listener = flaky(errors.into()).await;
        let addr = listener.inner.local_addr().expect("addr");
        let client = tokio::spawn(TcpStream::connect(addr));
        let mut logged = ErrorLog::default();
        let accepted = within_seconds(next_connection(&listener, &mut logged))
            .await
            .expect("accepted after retries");
        assert!(accepted.nodelay().expect("nodelay"));
        assert_eq!(logged.seen.len(), 2);
        client.await.expect("join").expect("client connected");
    }

    #[tokio::test]
    async fn connection_errors_are_retried_without_a_pause() {
        let errors = (0..300)
            .map(|_| io::Error::from_raw_os_error(libc::ECONNABORTED))
            .collect();
        let listener = flaky(errors).await;
        let addr = listener.inner.local_addr().expect("addr");
        let client = tokio::spawn(TcpStream::connect(addr));
        let started = std::time::Instant::now();
        within_seconds(next_connection(&listener, &mut ErrorLog::default()))
            .await
            .expect("accepted after retries");
        // A 10 ms pause per error would take 3 s.
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        client.await.expect("join").expect("client connected");
    }

    /// A stderr that has been closed.
    struct Closed;

    impl io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn logging_to_a_closed_stderr_does_not_panic() {
        let mut logged = ErrorLog::default();
        logged.once(&io::Error::from_raw_os_error(libc::EMFILE), &mut Closed);
        assert_eq!(logged.seen.len(), 1);
    }

    #[tokio::test]
    async fn other_errors_stop_the_loop() {
        let listener = flaky(vec![io::Error::from(io::ErrorKind::InvalidInput)]).await;
        let error = within_seconds(next_connection(&listener, &mut ErrorLog::default()))
            .await
            .expect_err("fatal");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn classifies_accept_errors() {
        let exhausted = [libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM];
        for code in exhausted {
            let failure = classify(&io::Error::from_raw_os_error(code));
            assert_eq!(failure, AcceptFailure::ResourcesExhausted, "{code}");
        }
        for code in [libc::ECONNABORTED, libc::ECONNRESET] {
            let failure = classify(&io::Error::from_raw_os_error(code));
            assert_eq!(failure, AcceptFailure::ConnectionLost, "{code}");
        }
        for e in [
            io::Error::from_raw_os_error(libc::EBADF),
            io::Error::from(io::ErrorKind::InvalidInput),
        ] {
            assert_eq!(classify(&e), AcceptFailure::Fatal, "{e}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn classifies_the_linux_network_errors_as_lost_connections() {
        let linux = [
            libc::EPROTO,
            libc::ENETDOWN,
            libc::ENOPROTOOPT,
            libc::EHOSTDOWN,
            libc::ENONET,
            libc::EHOSTUNREACH,
            libc::EOPNOTSUPP,
            libc::ENETUNREACH,
        ];
        for code in linux {
            let failure = classify(&io::Error::from_raw_os_error(code));
            assert_eq!(failure, AcceptFailure::ConnectionLost, "{code}");
        }
    }
}
