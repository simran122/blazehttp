//! The scripted peer the connection tests drive, with its memory caps and the real-time watchdog.

use std::future::Future;
use std::io::{self, IoSlice};
use std::pin::Pin;
use std::sync::mpsc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::error::Elapsed;
use tokio::time::{Sleep, timeout};

use crate::connection::{Mode, run};

/// Recorded output per script, and recorded write calls. A server that answers forever (a broken boundary scan) or
/// writes forever (a broken write loop, even with zero-byte writes) must fail the test within seconds and at most a
/// few tens of MiB, never grow until the machine runs out of memory.
const OUTPUT_CAP: usize = 64 << 20;
const WRITES_CAP: usize = 100_000;

/// A scripted peer: `input` arrives in reads that never cross a cut (`cuts` sorted). Writes are recorded per call, at
/// most `max_write` bytes each, and at most `cap` bytes and [`WRITES_CAP`] calls in total (then it panics). Reads wait
/// (virtual time) at each `pauses` offset and stop for good at `stall_reads_at`; each write call can wait
/// `write_delay`, and writes or the shutdown can be made to never complete, to test the deadlines.
pub(super) struct Script {
    pub(super) input: Vec<u8>,
    pub(super) cuts: Vec<usize>,
    pub(super) read: usize,
    pub(super) output: Vec<u8>,
    pub(super) writes: Vec<usize>,
    pub(super) shut_down: bool,
    pub(super) cap: usize,
    pub(super) max_write: usize,
    pub(super) stall_reads_at: Option<usize>,
    pub(super) stall_writes: bool,
    pub(super) stall_shutdown: bool,
    /// `(offset, wait)` in offset order: before reading at `offset`, wait `wait`.
    pub(super) pauses: Vec<(usize, Duration)>,
    pub(super) sleeping: Option<Pin<Box<Sleep>>>,
    /// Each write call waits this long (virtual time) before it completes.
    pub(super) write_delay: Option<Duration>,
    pub(super) writing: Option<Pin<Box<Sleep>>>,
}

impl Script {
    pub(super) fn new(input: Vec<u8>, cuts: Vec<usize>) -> Self {
        Self {
            input,
            cuts,
            read: 0,
            output: Vec::new(),
            writes: Vec::new(),
            shut_down: false,
            cap: OUTPUT_CAP,
            max_write: usize::MAX,
            stall_reads_at: None,
            stall_writes: false,
            stall_shutdown: false,
            pauses: Vec::new(),
            sleeping: None,
            write_delay: None,
            writing: None,
        }
    }

    /// Waits out the pause due at the current read offset, if any.
    fn poll_pause(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let Some(&(at, wait)) = self.pauses.first() else {
            return Poll::Ready(());
        };
        if at != self.read {
            return Poll::Ready(());
        }
        let sleeping = self
            .sleeping
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(wait)));
        if sleeping.as_mut().poll(cx).is_pending() {
            return Poll::Pending;
        }
        self.sleeping = None;
        self.pauses.remove(0);
        Poll::Ready(())
    }

    /// Waits out the write delay, if any, before a write call completes.
    fn poll_write_delay(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let Some(delay) = self.write_delay else {
            return Poll::Ready(());
        };
        let writing = self
            .writing
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(delay)));
        if writing.as_mut().poll(cx).is_pending() {
            return Poll::Pending;
        }
        self.writing = None;
        Poll::Ready(())
    }

    fn record(&mut self, bufs: &[IoSlice<'_>]) -> usize {
        assert!(
            self.writes.len() < WRITES_CAP,
            "stub write-call cap of {WRITES_CAP} calls exceeded"
        );
        let mut room = self.max_write;
        let before = self.output.len();
        for buf in bufs {
            let take = buf.len().min(room);
            assert!(
                self.output.len() + take <= self.cap,
                "stub output cap of {} bytes exceeded",
                self.cap
            );
            self.output.extend_from_slice(&buf[..take]);
            room -= take;
        }
        let len = self.output.len() - before;
        self.writes.push(len);
        len
    }
}

impl AsyncRead for Script {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let at = self.read;
        if self.stall_reads_at == Some(at) || self.poll_pause(cx).is_pending() {
            return Poll::Pending;
        }
        let pauses: Vec<usize> = self.pauses.iter().map(|&(offset, _)| offset).collect();
        // `cuts` is sorted, so the next cut is a binary search (a byte-at-a-time script has one cut per byte).
        let next_cut = self
            .cuts
            .get(self.cuts.partition_point(|&cut| cut <= at))
            .copied();
        let stop = next_cut
            .into_iter()
            .chain(self.stall_reads_at)
            .chain(pauses)
            .filter(|&cut| cut > at)
            .min()
            .unwrap_or(self.input.len());
        let n = (stop - at).min(buf.remaining());
        buf.put_slice(&self.input[at..at + n]);
        self.read += n;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Script {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.stall_writes {
            return Poll::Pending;
        }
        Poll::Ready(Ok(self.record(&[IoSlice::new(buf)])))
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        if self.stall_writes || self.poll_write_delay(cx).is_pending() {
            return Poll::Pending;
        }
        Poll::Ready(Ok(self.record(bufs)))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.stall_shutdown {
            return Poll::Pending;
        }
        self.shut_down = true;
        Poll::Ready(Ok(()))
    }
}

/// The virtual-time bound on every scripted connection: a connection that never finishes fails the test at once
/// (the clock is paused, so waiting costs no real time).
const TEST_BOUND: Duration = Duration::from_secs(30);

/// Real time a scripted connection may take. The virtual-time bound cannot stop a synchronous spin (a loop that never
/// awaits), so past this the process is aborted instead of hanging the test run at 100% of a core.
const WATCHDOG: Duration = Duration::from_secs(60);

/// Aborts the process unless dropped within [`WATCHDOG`].
struct Watchdog {
    /// Dropping it disconnects the channel, which disarms the watchdog thread.
    _armed: mpsc::Sender<()>,
}

fn watchdog() -> Watchdog {
    let (armed, disarmed) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        if disarmed.recv_timeout(WATCHDOG) == Err(mpsc::RecvTimeoutError::Timeout) {
            eprintln!(
                "a scripted connection ran past the {WATCHDOG:?} watchdog (a spin?); aborting"
            );
            std::process::abort();
        }
    });
    Watchdog { _armed: armed }
}

/// Runs one connection over `script` on a paused-clock runtime, bounded by [`TEST_BOUND`] of virtual time.
pub(super) fn drive(
    script: &mut Script,
    request_timeout: Duration,
) -> Result<io::Result<()>, Elapsed> {
    drive_within(script, request_timeout, TEST_BOUND)
}

/// [`drive`] with a chosen virtual-time bound.
pub(super) fn drive_within(
    script: &mut Script,
    request_timeout: Duration,
    bound: Duration,
) -> Result<io::Result<()>, Elapsed> {
    let _watchdog = watchdog();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("runtime");
    let mode = Mode::Answer { request_timeout };
    runtime.block_on(async { timeout(bound, run(script, mode)).await })
}
