//! One client connection: read, answer every complete request with its pre-built bytes, one write per batch, close
//! gracefully (design note §5). Generic over the stream, so plain TCP and TLS share the code with static dispatch.

use std::future::{Future, poll_fn};
use std::io::{self, IoSlice};
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Instant, timeout, timeout_at};

use crate::cells::{Refusal, Served, refusal, response_for};
use crate::request::{Scan, Wants, scan};

/// Read buffer per connection, which also bounds a request head (G6).
const BUFFER_BYTES: usize = 16 * 1024;
/// A batch holding at least one response stops growing before it would pass this many bytes, so a large response is
/// always written alone, straight from its static bytes.
const BATCH_BYTES: usize = 64 * 1024;
/// Responses per batch, well below `IOV_MAX` (1024 on Linux and macOS) for vectored writes.
const BATCH_RESPONSES: usize = 512;
/// Closing (shutdown, `close_notify` on TLS, then draining unread input) takes at most this long, and drains at most
/// this many bytes, so the kernel does not answer unread input with a reset that could destroy responses the client
/// has not read yet.
const LINGER_TIME: Duration = Duration::from_secs(1);
const LINGER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Mode {
    /// Answer requests. A request (head and declared body) must arrive within `request_timeout`, and each write or
    /// flush call that does not complete at once must complete within it; an idle connection with nothing buffered is
    /// not timed.
    Answer { request_timeout: Duration },
    /// Read everything, never write (the G2 stalled-server cell).
    Stall,
}

pub(crate) async fn run<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    mode: Mode,
) -> io::Result<()> {
    match mode {
        Mode::Answer { request_timeout } => Connection::new(stream, request_timeout).answer().await,
        Mode::Stall => drain(stream).await,
    }
}

async fn drain<S: AsyncRead + Unpin>(mut stream: S) -> io::Result<()> {
    let mut buf = vec![0; BUFFER_BYTES];
    while stream.read(&mut buf).await? != 0 {}
    Ok(())
}

/// Runs `op` with a deadline of `limit`, armed only when `op` does not complete on its first poll: a write that the
/// socket takes at once (the common case) costs no timer.
async fn within<T>(limit: Duration, op: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    let mut op = pin!(op);
    if let Poll::Ready(done) = poll_fn(|cx| Poll::Ready(op.as_mut().poll(cx))).await {
        return done;
    }
    timeout(limit, op)
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

/// Bytes other than CR and LF: anything beyond empty lines starts a request head.
fn has_content(bytes: &[u8]) -> bool {
    bytes.iter().any(|&b| b != b'\r' && b != b'\n')
}

/// What the last read produced.
enum Input {
    Bytes,
    Eof,
    /// A partial request head fills the whole buffer.
    HeadTooLarge,
    /// A started request (head or declared body) did not complete within the request timeout.
    RequestTimeout,
}

/// Why taking requests from the buffer stopped.
enum Progress {
    /// No complete request left (or its body is still arriving): read more.
    NeedBytes,
    /// The batch is full: write it, then keep taking from the buffer.
    BatchFull,
    /// The last batched response ends the connection (a 400 refusal at once; or, once the request's declared body is
    /// read: the client's close, a 505, or a close-delimited response).
    Close,
}

/// What to do after the buffered requests are answered.
enum Next {
    Read,
    Close,
}

/// The next thing the buffered bytes ask for.
enum Step {
    Respond(Answer),
    Refuse(Refusal),
    NeedBytes,
}

/// A complete request's response and what it consumes.
struct Answer {
    response: &'static [u8],
    head_len: usize,
    body_len: u64,
    close: bool,
}

struct Connection<S> {
    stream: S,
    buf: Box<[u8]>,
    /// Unconsumed bytes are `buf[start..end]`.
    start: usize,
    end: usize,
    /// Where the blank-line search resumes, relative to `start`.
    resume: usize,
    /// Request body bytes still to drop.
    skip: u64,
    /// The last response ends the connection (the client's close, a 505, or a close-delimited response): close once
    /// the body still owed (`skip`) has been read.
    closing: bool,
    batch: Vec<IoSlice<'static>>,
    batch_bytes: usize,
    request_timeout: Duration,
    /// When the started request (head, then declared body) must be complete.
    request_deadline: Option<Instant>,
    /// The unconsumed bytes hold more than empty lines (CR/LF): a request head has started.
    head_seen: bool,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    fn new(stream: S, request_timeout: Duration) -> Self {
        Self {
            stream,
            buf: vec![0; BUFFER_BYTES].into_boxed_slice(),
            start: 0,
            end: 0,
            resume: 0,
            skip: 0,
            closing: false,
            batch: Vec::new(),
            batch_bytes: 0,
            request_timeout,
            request_deadline: None,
            head_seen: false,
        }
    }

    async fn answer(mut self) -> io::Result<()> {
        loop {
            let next = match self.receive().await? {
                Input::Bytes => self.answer_buffered().await?,
                Input::Eof | Input::RequestTimeout => Next::Close,
                Input::HeadTooLarge => {
                    self.push(refusal(Refusal::HeadTooLarge));
                    self.write_batch().await?;
                    Next::Close
                }
            };
            if let Next::Close = next {
                return self.close().await;
            }
            self.compact();
        }
    }

    /// Reads more bytes; a started request must complete before its deadline.
    async fn receive(&mut self) -> io::Result<Input> {
        if self.end == self.buf.len() {
            return Ok(Input::HeadTooLarge);
        }
        let deadline = self.request_started().then(|| {
            *self
                .request_deadline
                .get_or_insert_with(|| Instant::now() + self.request_timeout)
        });
        let read = self.stream.read(&mut self.buf[self.end..]);
        let n = match deadline {
            Some(deadline) => match timeout_at(deadline, read).await {
                Ok(n) => n?,
                Err(_) => return Ok(Input::RequestTimeout),
            },
            None => read.await?,
        };
        self.head_seen |= has_content(&self.buf[self.end..self.end + n]);
        self.end += n;
        Ok(if n == 0 { Input::Eof } else { Input::Bytes })
    }

    /// A body is still owed, or the buffer holds part of a request head (empty lines alone do not count).
    fn request_started(&self) -> bool {
        self.skip > 0 || self.head_seen
    }

    /// Recomputes [`Self::head_seen`] after bytes were consumed (it stops at the first content byte, normally the next
    /// request's first byte).
    fn rescan_head(&mut self) {
        self.head_seen = has_content(&self.buf[self.start..self.end]);
    }

    async fn answer_buffered(&mut self) -> io::Result<Next> {
        loop {
            let progress = self.take_requests();
            self.write_batch().await?;
            match progress {
                Progress::NeedBytes => return Ok(Next::Read),
                Progress::BatchFull => {}
                Progress::Close => return Ok(Next::Close),
            }
        }
    }

    fn take_requests(&mut self) -> Progress {
        loop {
            self.skip_body();
            if self.skip > 0 {
                return Progress::NeedBytes;
            }
            if self.closing {
                return Progress::Close;
            }
            match self.next_step() {
                Step::NeedBytes => return Progress::NeedBytes,
                Step::Refuse(kind) => {
                    let response = refusal(kind);
                    if !self.has_room_for(response) {
                        return Progress::BatchFull;
                    }
                    self.push(response);
                    return Progress::Close;
                }
                Step::Respond(answer) => {
                    if !self.has_room_for(answer.response) {
                        return Progress::BatchFull;
                    }
                    self.consume(&answer);
                }
            }
        }
    }

    fn next_step(&mut self) -> Step {
        match scan(&self.buf[self.start..self.end], self.resume) {
            Scan::Complete(r) => {
                let served = match r.wants {
                    Wants::Target(line) => response_for(line.target, line.head_only, line.http10),
                    // Answered at once; the connection closes once the declared body has been skipped.
                    Wants::UnsupportedVersion => Served {
                        bytes: refusal(Refusal::VersionNotSupported),
                        close: true,
                    },
                };
                Step::Respond(Answer {
                    response: served.bytes,
                    head_len: r.head_len,
                    body_len: r.body_len,
                    close: r.close || served.close,
                })
            }
            Scan::Partial { resume } => {
                self.resume = resume;
                Step::NeedBytes
            }
            Scan::Refused(kind) => Step::Refuse(kind),
        }
    }

    /// Batches the response and moves past the request head; its body (if any) is skipped next.
    fn consume(&mut self, answer: &Answer) {
        self.push(answer.response);
        self.start += answer.head_len;
        self.rescan_head();
        (self.skip, self.resume, self.closing) = (answer.body_len, 0, answer.close);
        if answer.body_len == 0 {
            self.request_deadline = None;
        }
    }

    /// Drops buffered body bytes still owed; the request is complete (its deadline cleared) once the last one is gone.
    /// Returns early when nothing is dropped, so the head is rescanned only when the buffer changed.
    fn skip_body(&mut self) {
        let buffered = self.end - self.start;
        let dropped = usize::try_from(self.skip).map_or(buffered, |skip| skip.min(buffered));
        if dropped == 0 {
            return;
        }
        self.start += dropped;
        self.skip -= dropped as u64;
        self.rescan_head();
        if self.skip == 0 {
            self.request_deadline = None;
        }
    }

    fn has_room_for(&self, response: &[u8]) -> bool {
        self.batch.is_empty()
            || (self.batch_bytes + response.len() <= BATCH_BYTES
                && self.batch.len() < BATCH_RESPONSES)
    }

    fn push(&mut self, response: &'static [u8]) {
        self.batch.push(IoSlice::new(response));
        self.batch_bytes += response.len();
    }

    /// Writes and flushes the batch, then empties it ([`Self::write_gathered`] bounds each call).
    async fn write_batch(&mut self) -> io::Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        self.write_gathered().await?;
        self.batch.clear();
        self.batch_bytes = 0;
        Ok(())
    }

    /// One vectored write of the static responses (repeated only for a partial write), then a flush (TLS streams may
    /// still hold ciphertext after a write; a no-op on TCP). Each call must make progress within the request timeout,
    /// so a peer that stops reading cannot hold the connection while a slow but steady one is not cut off.
    async fn write_gathered(&mut self) -> io::Result<()> {
        let limit = self.request_timeout;
        let mut slices = &mut self.batch[..];
        while !slices.is_empty() {
            let n = within(limit, self.stream.write_vectored(slices)).await?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            IoSlice::advance_slices(&mut slices, n);
        }
        within(limit, self.stream.flush()).await
    }

    /// Moves a partial request to the front of the buffer.
    fn compact(&mut self) {
        if self.start == 0 {
            return;
        }
        self.buf.copy_within(self.start..self.end, 0);
        self.end -= self.start;
        self.start = 0;
    }

    /// Closes the write side (TLS: `close_notify`), then drains unread input; all of it within [`LINGER_TIME`] and
    /// at most [`LINGER_BYTES`].
    async fn close(mut self) -> io::Result<()> {
        let closing = async {
            self.stream.shutdown().await?;
            let mut drained = 0;
            while drained < LINGER_BYTES {
                let room = (LINGER_BYTES - drained).min(self.buf.len());
                match self.stream.read(&mut self.buf[..room]).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => drained += n,
                }
            }
            Ok(())
        };
        // Past the time limit the connection is simply dropped.
        timeout(LINGER_TIME, closing).await.unwrap_or(Ok(()))
    }
}

#[cfg(test)]
mod tests;
