//! One client connection: read, answer every complete request with its pre-built bytes, one write per batch, close
//! gracefully (design note §5). Generic over the stream, so plain TCP and TLS share the code with static dispatch.

use std::io::{self, IoSlice};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Instant, timeout, timeout_at};

use crate::cells::{Refusal, refusal, response_for};
use crate::request::{Scan, scan};

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
    /// Answer requests. A request (head and declared body) must arrive, and each batch must be written, within
    /// `request_timeout`; an idle connection with nothing buffered is not timed.
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
    /// The last batched response ends the connection (refusal, or the client's close once its body is read).
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
    /// The client asked to close: close once the body still owed (`skip`) has been read.
    closing: bool,
    batch: Vec<IoSlice<'static>>,
    batch_bytes: usize,
    request_timeout: Duration,
    /// When the started request (head, then declared body) must be complete.
    request_deadline: Option<Instant>,
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
        self.end += n;
        Ok(if n == 0 { Input::Eof } else { Input::Bytes })
    }

    /// A body is still owed, or the buffer holds part of a request head (empty lines alone do not count).
    fn request_started(&self) -> bool {
        self.skip > 0
            || self.buf[self.start..self.end]
                .iter()
                .any(|&b| b != b'\r' && b != b'\n')
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
            Scan::Complete(r) => match response_for(r.target, r.head_only, r.http10) {
                Ok(response) => Step::Respond(Answer {
                    response,
                    head_len: r.head_len,
                    body_len: r.body_len,
                    close: r.close,
                }),
                Err(kind) => Step::Refuse(kind),
            },
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
        (self.skip, self.resume, self.closing) = (answer.body_len, 0, answer.close);
        if answer.body_len == 0 {
            self.request_deadline = None;
        }
    }

    /// Drops buffered body bytes still owed; the request is complete (its deadline cleared) once the last one is gone.
    fn skip_body(&mut self) {
        let buffered = self.end - self.start;
        let dropped = usize::try_from(self.skip).map_or(buffered, |skip| skip.min(buffered));
        self.start += dropped;
        self.skip -= dropped as u64;
        if dropped > 0 && self.skip == 0 {
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

    /// Writes and flushes the batch within the request timeout (a peer that never reads cannot hold the task).
    async fn write_batch(&mut self) -> io::Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let limit = self.request_timeout;
        timeout(limit, self.write_gathered())
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        self.batch.clear();
        self.batch_bytes = 0;
        Ok(())
    }

    /// One vectored write of the static responses (repeated only for a partial write), then a flush (TLS streams may
    /// still hold ciphertext after a write; a no-op on TCP).
    async fn write_gathered(&mut self) -> io::Result<()> {
        let mut slices = &mut self.batch[..];
        while !slices.is_empty() {
            let n = self.stream.write_vectored(slices).await?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            IoSlice::advance_slices(&mut slices, n);
        }
        self.stream.flush().await
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
mod tests {
    use std::io::{self, IoSlice};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use std::future::Future;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    use tokio::time::error::Elapsed;
    use tokio::time::{Sleep, timeout};

    use super::{BATCH_BYTES, BUFFER_BYTES, Mode, run};
    use crate::cells::{Refusal, refusal, response_for};

    /// The response a cell sends (never a refusal in these tests).
    fn wire(target: &[u8], head_only: bool) -> &'static [u8] {
        response_for(target, head_only, false).expect("an HTTP/1.1 request is answered")
    }

    /// Body larger than the read buffer, so skipping it spans several reads.
    const BIG_BODY: usize = BUFFER_BYTES + 3_000;

    /// Recorded output per script. A server that answers forever (a broken boundary scan) must fail the test within
    /// seconds and a few MiB, never grow until the machine runs out of memory.
    const OUTPUT_CAP: usize = 64 << 20;

    /// A scripted peer: `input` arrives in reads that never cross a cut; writes are recorded per call, at most
    /// `max_write` bytes each and `cap` bytes in total (then it panics). Reads wait (virtual time) at each `pauses`
    /// offset and stop for good at `stall_reads_at`; writes or the shutdown can be made to never complete, to test the
    /// deadlines.
    struct Script {
        input: Vec<u8>,
        cuts: Vec<usize>,
        read: usize,
        output: Vec<u8>,
        writes: Vec<usize>,
        shut_down: bool,
        cap: usize,
        max_write: usize,
        stall_reads_at: Option<usize>,
        stall_writes: bool,
        stall_shutdown: bool,
        /// `(offset, wait)` in offset order: before reading at `offset`, wait `wait`.
        pauses: Vec<(usize, Duration)>,
        sleeping: Option<Pin<Box<Sleep>>>,
    }

    impl Script {
        fn new(input: Vec<u8>, cuts: Vec<usize>) -> Self {
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

        fn record(&mut self, bufs: &[IoSlice<'_>]) -> usize {
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
            let stop = self
                .cuts
                .iter()
                .copied()
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
            _: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            if self.stall_writes {
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

    /// Runs one connection over `script` on a paused-clock runtime, bounded by [`TEST_BOUND`].
    fn drive(script: &mut Script, request_timeout: Duration) -> Result<io::Result<()>, Elapsed> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .expect("runtime");
        let mode = Mode::Answer { request_timeout };
        runtime.block_on(async { timeout(TEST_BOUND, run(script, mode)).await })
    }

    fn serve(script: &mut Script) {
        drive(script, Duration::from_secs(60))
            .expect("connection finished within the test bound")
            .expect("connection");
    }

    fn get(path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nhost: bench\r\n\r\n")
    }

    /// GETs, a HEAD, an unknown path, a small body and one larger than the buffer, ending with `Connection: close`.
    fn pipelined() -> (Vec<u8>, Vec<u8>) {
        let body = "x".repeat(BIG_BODY);
        let input = [
            get("/1k"),
            format!("POST /1k-h20 HTTP/1.1\r\ncontent-length: {BIG_BODY}\r\n\r\n{body}"),
            "\r\nHEAD /1k-gzip HTTP/1.1\r\n\r\n".to_owned(),
            "POST /1k HTTP/1.1\r\ncontent-length: 5\r\n\r\nhello".to_owned(),
            get("/nope"),
            "GET /1k-chunked HTTP/1.1\r\nconnection: close\r\n\r\n".to_owned(),
        ]
        .concat();
        let responses: [&[u8]; 6] = [
            wire(b"/1k", false),
            wire(b"/1k-h20", false),
            wire(b"/1k-gzip", true),
            wire(b"/1k", false),
            wire(b"/nope", false),
            wire(b"/1k-chunked", false),
        ];
        (input.into_bytes(), responses.concat())
    }

    #[test]
    fn every_split_point_gives_the_unsplit_output() {
        let (input, expected) = pipelined();
        for cut in 0..=input.len() {
            let mut script = Script::new(input.clone(), vec![cut]);
            serve(&mut script);
            assert!(script.output == expected, "split at {cut}");
            assert!(script.shut_down, "split at {cut}");
        }
    }

    #[test]
    fn batches_stop_before_passing_the_byte_limit() {
        let one = wire(b"/1k", false);
        let count = BATCH_BYTES / one.len() + 10;
        let mut script = Script::new(get("/1k").repeat(count).into_bytes(), Vec::new());
        serve(&mut script);
        assert!(script.output == one.repeat(count), "batched output differs");
        // 64 KiB spelled out, so the test also pins the constant: every full batch holds as many 1 KiB responses as fit.
        let full_batch = 65_536 / one.len() * one.len();
        assert_eq!(script.writes[0], full_batch, "{:?}", script.writes);
        assert!(script.writes.len() >= 2, "{:?}", script.writes);
        assert!(
            script.writes.iter().all(|&n| n <= BATCH_BYTES),
            "{:?}",
            script.writes
        );
    }

    #[test]
    #[should_panic(expected = "stub output cap")]
    fn stub_fails_fast_past_its_output_cap() {
        let mut script = Script::new(get("/1m").into_bytes(), Vec::new());
        script.cap = 1024;
        serve(&mut script);
    }

    #[test]
    fn head_just_under_the_buffer_is_answered() {
        // 16 KiB spelled out, so the test also pins the buffer size.
        let filler = "a".repeat(16 * 1024 - 100);
        let input = format!("GET /1k HTTP/1.1\r\nx-big: {filler}\r\n\r\n");
        let mut script = Script::new(input.into_bytes(), Vec::new());
        serve(&mut script);
        assert_eq!(script.output, wire(b"/1k", false));
    }

    #[test]
    fn a_large_response_is_written_alone() {
        let (small, large) = (wire(b"/1k", false), wire(b"/1m", false));
        let input = [get("/1k"), get("/1m"), get("/1k")].concat().into_bytes();
        let mut script = Script::new(input, Vec::new());
        serve(&mut script);
        assert_eq!(script.writes, [small.len(), large.len(), small.len()]);
    }

    #[test]
    fn invalid_request_after_pipelined_ones_gets_400_after_their_responses() {
        let input = [
            get("/1k"),
            get("/1k-h20"),
            "GET /1k HTTP/9\r\n\r\n".to_owned(),
            get("/1k"),
        ]
        .concat();
        let mut script = Script::new(input.into_bytes(), Vec::new());
        serve(&mut script);
        let parts: [&[u8]; 3] = [
            wire(b"/1k", false),
            wire(b"/1k-h20", false),
            refusal(Refusal::BadRequest),
        ];
        assert!(
            script.output == parts.concat(),
            "responses before the 400 differ"
        );
        assert!(script.shut_down);
    }

    #[test]
    fn batches_stop_at_512_responses() {
        let not_found = wire(b"/x", false);
        let input = "GET /x HTTP/1.1\r\n\r\n".repeat(600);
        let mut script = Script::new(input.into_bytes(), Vec::new());
        serve(&mut script);
        // 512 spelled out to pin the limit; 512 small 404s stay far below the byte limit.
        assert_eq!(script.writes, [512 * not_found.len(), 88 * not_found.len()]);
    }

    #[test]
    fn closing_drains_at_most_64_kib_of_unread_input() {
        let mut input = b"BROKEN\r\n\r\n".to_vec();
        input.resize(BUFFER_BYTES * 20, b'x');
        let mut script = Script::new(input, Vec::new());
        serve(&mut script);
        assert_eq!(script.output, refusal(Refusal::BadRequest));
        // The first read fills the buffer; lingering then reads 64 KiB (spelled out to pin the limit) and stops.
        assert_eq!(script.read, BUFFER_BYTES + 65_536);
    }

    #[test]
    fn head_larger_than_the_buffer_gets_431() {
        let input = format!(
            "GET /1k HTTP/1.1\r\nx-big: {}\r\n\r\n",
            "a".repeat(BUFFER_BYTES)
        );
        let mut script = Script::new(input.into_bytes(), Vec::new());
        serve(&mut script);
        assert_eq!(script.output, refusal(Refusal::HeadTooLarge));
        assert!(script.shut_down);
    }

    #[test]
    fn end_of_stream_closes_gracefully() {
        let mut script = Script::new(get("/1k").into_bytes(), Vec::new());
        serve(&mut script);
        assert_eq!(script.output, wire(b"/1k", false));
        assert!(
            script.shut_down,
            "TLS needs close_notify on end of stream too"
        );
    }

    #[test]
    fn connection_close_waits_for_the_declared_body() {
        let body_len = BUFFER_BYTES * 20;
        let head =
            format!("POST /1k HTTP/1.1\r\nconnection: close\r\ncontent-length: {body_len}\r\n\r\n");
        let mut input = head.into_bytes();
        input.resize(input.len() + body_len, b'x');
        let mut script = Script::new(input.clone(), Vec::new());
        serve(&mut script);
        assert_eq!(script.output, wire(b"/1k", false));
        assert_eq!(
            script.read,
            input.len(),
            "the whole body is read before closing"
        );
        assert!(script.shut_down);
    }

    #[test]
    fn short_writes_give_the_same_output() {
        let (input, expected) = pipelined();
        for max_write in [1, 7, 1000] {
            let mut script = Script::new(input.clone(), Vec::new());
            script.max_write = max_write;
            serve(&mut script);
            assert!(script.output == expected, "max_write {max_write}");
        }
    }

    #[test]
    fn a_zero_byte_write_is_an_error() {
        let mut script = Script::new(get("/1k").into_bytes(), Vec::new());
        script.max_write = 0;
        let result = drive(&mut script, Duration::from_secs(60)).expect("finished");
        assert_eq!(
            result.expect_err("write zero").kind(),
            io::ErrorKind::WriteZero
        );
    }

    #[test]
    fn a_peer_that_never_reads_is_dropped_after_the_request_timeout() {
        let mut script = Script::new(get("/1k").into_bytes(), Vec::new());
        script.stall_writes = true;
        let result =
            drive(&mut script, Duration::from_secs(1)).expect("finished before the test bound");
        assert_eq!(
            result.expect_err("timed out").kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn a_shutdown_that_never_completes_is_bounded_by_the_linger_time() {
        let mut script = Script::new(b"BROKEN\r\n\r\n".to_vec(), Vec::new());
        script.stall_shutdown = true;
        let result =
            drive(&mut script, Duration::from_secs(60)).expect("finished before the test bound");
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(script.output, refusal(Refusal::BadRequest));
    }

    #[test]
    fn a_slow_body_is_closed_after_the_request_timeout() {
        let head = "POST /1k HTTP/1.1\r\ncontent-length: 1000\r\n\r\n";
        let input = format!("{head}0123456789").into_bytes();
        let mut script = Script::new(input.clone(), Vec::new());
        script.stall_reads_at = Some(input.len());
        let result =
            drive(&mut script, Duration::from_secs(1)).expect("finished before the test bound");
        assert!(result.is_ok(), "{result:?}");
        assert!(script.shut_down);
    }

    #[test]
    fn empty_lines_after_a_request_do_not_start_the_request_timeout() {
        let input = format!("{}\r\n", get("/1k")).into_bytes();
        let mut script = Script::new(input.clone(), Vec::new());
        script.stall_reads_at = Some(input.len());
        let still_open = drive(&mut script, Duration::from_secs(1));
        assert!(
            still_open.is_err(),
            "an idle client was closed: {still_open:?}"
        );
        assert!(!script.shut_down);
    }

    /// Request 1 arms a deadline; after a long idle gap, request 2 must get a fresh one, not the expired one.
    #[test]
    fn each_request_gets_its_own_deadline() {
        let (first, second) = (get("/1k"), get("/1k-h20"));
        let input = [first.clone(), second.clone()].concat().into_bytes();
        // The cut makes request 1 arrive in two reads, so it arms a deadline.
        let mut script = Script::new(input, vec![5]);
        let gap = Duration::from_secs(5);
        script.pauses = vec![
            (first.len(), gap),
            (first.len() + 10, Duration::from_millis(500)),
        ];
        drive(&mut script, Duration::from_secs(1))
            .expect("finished")
            .expect("connection");
        assert_eq!(
            script.output,
            [wire(b"/1k", false), wire(b"/1k-h20", false)].concat()
        );
    }

    #[test]
    fn a_refusal_respects_the_batch_response_limit() {
        let not_found = wire(b"/x", false);
        let input = format!("{}BROKEN\r\n\r\n", "GET /x HTTP/1.1\r\n\r\n".repeat(512));
        let mut script = Script::new(input.into_bytes(), Vec::new());
        serve(&mut script);
        let bad_request = refusal(Refusal::BadRequest);
        assert_eq!(script.writes, [512 * not_found.len(), bad_request.len()]);
    }

    #[test]
    fn lingering_drains_exactly_64_kib() {
        let mut input = b"BROKEN\r\n\r\n".to_vec();
        input.resize(BUFFER_BYTES * 20, b'x');
        let cuts = (1..20).map(|i| BUFFER_BYTES + i * 10_000).collect();
        let mut script = Script::new(input, cuts);
        serve(&mut script);
        assert_eq!(script.read, BUFFER_BYTES + 65_536);
    }
}
