//! The accept loop: a bounded number of open connections, one task each, and retries on transient accept errors
//! (design note §5). The listener is behind the [`Listen`] trait so the retry path is testable with a fake.

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// Pause after a transient accept error, so a full file table is not polled in a busy loop.
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

/// Accepts forever, running `handle(stream)` as one task per connection. A permit is taken *before* accepting, so at
/// the limit new clients wait in the kernel's listen backlog instead of in memory (G6).
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
/// per kind and retried after [`RETRY_PAUSE`]; connections that died in the accept queue are skipped.
async fn next_connection<L: Listen>(listener: &L, logged: &mut ErrorLog) -> io::Result<TcpStream> {
    loop {
        match listener.accept().await {
            Ok(stream) if stream.set_nodelay(true).is_ok() => return Ok(stream),
            Ok(_) => {}
            Err(e) if is_transient(&e) => {
                logged.once(&e);
                tokio::time::sleep(RETRY_PAUSE).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Accept errors that say nothing about the listener itself: resource limits and connections lost in the queue, and
/// on Linux the network errors `accept(2)` passes on from the new socket ("Error handling" in its man page).
fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::OutOfMemory
    ) || e
        .raw_os_error()
        .is_some_and(|code| TRANSIENT_ERRNO.contains(&code))
}

#[cfg(target_os = "linux")]
const TRANSIENT_ERRNO: &[i32] = &[
    libc::EMFILE,
    libc::ENFILE,
    libc::ENOBUFS,
    libc::ENOMEM,
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
const TRANSIENT_ERRNO: &[i32] = &[
    libc::EMFILE,
    libc::ENFILE,
    libc::ENOBUFS,
    libc::ENOMEM,
    libc::ECONNABORTED,
];

#[cfg(not(unix))]
const TRANSIENT_ERRNO: &[i32] = &[];

/// Remembers which error kinds were already reported, so a persistent condition prints one line, not one per retry.
#[derive(Default)]
struct ErrorLog {
    seen: Vec<(io::ErrorKind, Option<i32>)>,
}

impl ErrorLog {
    fn once(&mut self, e: &io::Error) {
        let key = (e.kind(), e.raw_os_error());
        if !self.seen.contains(&key) {
            self.seen.push(key);
            eprintln!("bench-server: accept failed ({e}); retrying");
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

    use super::{ErrorLog, Listen, is_transient, next_connection};

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
    async fn other_errors_stop_the_loop() {
        let listener = flaky(vec![io::Error::from(io::ErrorKind::InvalidInput)]).await;
        let error = within_seconds(next_connection(&listener, &mut ErrorLog::default()))
            .await
            .expect_err("fatal");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn classifies_transient_errors() {
        for code in [
            libc::EMFILE,
            libc::ENFILE,
            libc::ENOBUFS,
            libc::ENOMEM,
            libc::ECONNABORTED,
            libc::ECONNRESET,
        ] {
            assert!(is_transient(&io::Error::from_raw_os_error(code)), "{code}");
        }
        assert!(!is_transient(&io::Error::from_raw_os_error(libc::EBADF)));
        assert!(!is_transient(&io::Error::from(io::ErrorKind::InvalidInput)));
    }
}
