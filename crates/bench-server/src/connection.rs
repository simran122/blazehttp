//! One client connection: read, answer every complete request with its pre-built bytes, one write per batch, close
//! gracefully (design note §5). Generic over [`Transport`], so plain TCP and TLS share the code with static dispatch.

use std::io::{self, IoSlice};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout, timeout_at};
use tokio_rustls::server::TlsStream;

use crate::cells::{Refusal, refusal, response_for};
use crate::request::{Scan, scan};

/// Read buffer per connection, which also bounds a request head (G6).
const BUFFER_BYTES: usize = 16 * 1024;
/// A batch holding at least one response stops growing before it would pass this many bytes, so a large response is
/// always written alone, straight from its static bytes.
const BATCH_BYTES: usize = 64 * 1024;
/// Responses per batch, well below `IOV_MAX` (1024 on Linux and macOS) for vectored writes.
const BATCH_RESPONSES: usize = 512;
/// After closing the write side, unread input is drained for at most this long and this many bytes, so the kernel
/// does not answer it with a reset that could destroy responses the client has not read yet.
const LINGER_TIME: Duration = Duration::from_secs(1);
const LINGER_BYTES: usize = 64 * 1024;

/// A stream a connection can answer on, and how it writes a batch of pre-built responses.
pub(crate) trait Transport: AsyncRead + AsyncWrite + Unpin {
    /// `true`: one vectored write of the static responses, no copy. `false`: several responses are copied into one
    /// buffer first (TLS, where rustls processes each slice of a vectored write separately; note §7.2).
    const GATHER: bool;
}

impl Transport for TcpStream {
    const GATHER: bool = true;
}

impl Transport for TlsStream<TcpStream> {
    const GATHER: bool = false;
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Mode {
    /// Answer requests; a request head must arrive within the head timeout.
    Answer { head_timeout: Duration },
    /// Read everything, never write (the G2 stalled-server cell).
    Stall,
}

pub(crate) async fn run<S: Transport>(stream: S, mode: Mode) -> io::Result<()> {
    match mode {
        Mode::Answer { head_timeout } => Connection::new(stream, head_timeout).answer().await,
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
    /// A partial request head did not complete within the head timeout.
    HeadTimeout,
}

/// Why taking requests from the buffer stopped.
enum Progress {
    /// No complete request left: read more.
    NeedBytes,
    /// The batch is full: write it, then keep taking from the buffer.
    BatchFull,
    /// The last batched response ends the connection (refusal or the client's close).
    Close,
}

/// What to do after the buffered requests are answered.
enum Next {
    Read,
    Close,
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
    batch: Vec<IoSlice<'static>>,
    batch_bytes: usize,
    /// Copy buffer for transports that do not gather.
    out: Vec<u8>,
    head_timeout: Duration,
    /// When the partial request head in the buffer must be complete.
    head_deadline: Option<Instant>,
}

impl<S: Transport> Connection<S> {
    fn new(stream: S, head_timeout: Duration) -> Self {
        Self {
            stream,
            buf: vec![0; BUFFER_BYTES].into_boxed_slice(),
            start: 0,
            end: 0,
            resume: 0,
            skip: 0,
            batch: Vec::new(),
            batch_bytes: 0,
            out: Vec::new(),
            head_timeout,
            head_deadline: None,
        }
    }

    async fn answer(mut self) -> io::Result<()> {
        loop {
            let next = match self.receive().await? {
                Input::Bytes => self.answer_buffered().await?,
                Input::Eof => return Ok(()),
                Input::HeadTooLarge => {
                    self.push(refusal(Refusal::HeadTooLarge));
                    self.write_batch().await?;
                    Next::Close
                }
                Input::HeadTimeout => Next::Close,
            };
            if let Next::Close = next {
                return self.close().await;
            }
            self.compact();
        }
    }

    /// Reads more bytes; a partial head in the buffer must complete before its deadline.
    async fn receive(&mut self) -> io::Result<Input> {
        if self.end == self.buf.len() {
            return Ok(Input::HeadTooLarge);
        }
        let read = self.stream.read(&mut self.buf[self.end..]);
        let n = if self.end > self.start {
            let deadline = *self
                .head_deadline
                .get_or_insert_with(|| Instant::now() + self.head_timeout);
            match timeout_at(deadline, read).await {
                Ok(n) => n?,
                Err(_) => return Ok(Input::HeadTimeout),
            }
        } else {
            read.await?
        };
        self.end += n;
        Ok(if n == 0 { Input::Eof } else { Input::Bytes })
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
            let (response, head_len, body_len, close) =
                match scan(&self.buf[self.start..self.end], self.resume) {
                    Scan::Complete(r) => (
                        response_for(r.target, r.head_only),
                        r.head_len,
                        r.body_len,
                        r.close,
                    ),
                    Scan::Partial { resume } => {
                        self.resume = resume;
                        return Progress::NeedBytes;
                    }
                    Scan::Invalid => {
                        self.push(refusal(Refusal::BadRequest));
                        return Progress::Close;
                    }
                };
            if !self.has_room_for(response) {
                return Progress::BatchFull;
            }
            self.push(response);
            self.start += head_len;
            (self.skip, self.resume, self.head_deadline) = (body_len, 0, None);
            if close {
                return Progress::Close;
            }
        }
    }

    fn skip_body(&mut self) {
        let buffered = self.end - self.start;
        let dropped = usize::try_from(self.skip).map_or(buffered, |skip| skip.min(buffered));
        self.start += dropped;
        self.skip -= dropped as u64;
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

    async fn write_batch(&mut self) -> io::Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        if S::GATHER {
            self.write_gathered().await?;
        } else {
            self.write_copied().await?;
        }
        self.batch.clear();
        self.batch_bytes = 0;
        // TLS streams may still hold ciphertext after a write (tokio-rustls); a no-op on TCP.
        self.stream.flush().await
    }

    /// One vectored write of the static responses (repeated only for a partial write).
    async fn write_gathered(&mut self) -> io::Result<()> {
        let mut slices = &mut self.batch[..];
        while !slices.is_empty() {
            let n = self.stream.write_vectored(slices).await?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            IoSlice::advance_slices(&mut slices, n);
        }
        Ok(())
    }

    /// One response straight from its static bytes; several copied into one buffer.
    async fn write_copied(&mut self) -> io::Result<()> {
        if let [response] = self.batch.as_slice() {
            return self.stream.write_all(response).await;
        }
        self.out.clear();
        for response in &self.batch {
            self.out.extend_from_slice(response);
        }
        self.stream.write_all(&self.out).await
    }

    /// Moves a partial request to the front of the buffer.
    fn compact(&mut self) {
        self.buf.copy_within(self.start..self.end, 0);
        self.end -= self.start;
        self.start = 0;
    }

    /// Closes the write side (TLS: `close_notify`), then drains unread input for a bounded time and size.
    async fn close(mut self) -> io::Result<()> {
        self.stream.shutdown().await?;
        let mut drained = 0;
        let linger = async {
            while drained < LINGER_BYTES {
                match self.stream.read(&mut self.buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => drained += n,
                }
            }
        };
        // Lingering ends on EOF, on the byte limit, or here on the time limit; all three just close.
        let _ = timeout(LINGER_TIME, linger).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, IoSlice};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    use super::{BATCH_BYTES, BUFFER_BYTES, Mode, Transport, run};
    use crate::cells::{Refusal, refusal, response_for};

    /// Body larger than the read buffer, so skipping it spans several reads.
    const BIG_BODY: usize = BUFFER_BYTES + 3_000;

    /// A scripted peer: `input` arrives in reads that never cross a cut, writes are recorded per call.
    #[derive(Default)]
    struct Script<const GATHER: bool> {
        input: Vec<u8>,
        cuts: Vec<usize>,
        read: usize,
        output: Vec<u8>,
        writes: Vec<usize>,
        shut_down: bool,
    }

    impl<const GATHER: bool> Script<GATHER> {
        fn new(input: Vec<u8>, cuts: Vec<usize>) -> Self {
            Self {
                input,
                cuts,
                ..Self::default()
            }
        }

        fn record(&mut self, bufs: &[IoSlice<'_>]) -> usize {
            let before = self.output.len();
            for buf in bufs {
                self.output.extend_from_slice(buf);
            }
            self.writes.push(self.output.len() - before);
            self.output.len() - before
        }
    }

    impl<const GATHER: bool> AsyncRead for Script<GATHER> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let at = self.read;
            let stop = self
                .cuts
                .iter()
                .copied()
                .find(|&cut| cut > at)
                .unwrap_or(self.input.len());
            let n = (stop - at).min(buf.remaining());
            buf.put_slice(&self.input[at..at + n]);
            self.read += n;
            Poll::Ready(Ok(()))
        }
    }

    impl<const GATHER: bool> AsyncWrite for Script<GATHER> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(self.record(&[IoSlice::new(buf)])))
        }

        fn poll_write_vectored(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(self.record(bufs)))
        }

        fn is_write_vectored(&self) -> bool {
            true
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.shut_down = true;
            Poll::Ready(Ok(()))
        }
    }

    impl<const GATHER: bool> Transport for &mut Script<GATHER> {
        const GATHER: bool = GATHER;
    }

    fn serve<const GATHER: bool>(script: &mut Script<GATHER>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        let mode = Mode::Answer {
            head_timeout: Duration::from_secs(60),
        };
        runtime.block_on(run(script, mode)).expect("connection");
    }

    fn get(path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nhost: bench\r\n\r\n")
    }

    /// GETs, a HEAD, an unknown path and a body larger than the buffer, ending with `Connection: close`.
    fn pipelined() -> (Vec<u8>, Vec<u8>) {
        let body = "x".repeat(BIG_BODY);
        let input = [
            get("/1k"),
            format!("POST /1k-h20 HTTP/1.1\r\ncontent-length: {BIG_BODY}\r\n\r\n{body}"),
            "\r\nHEAD /1k-gzip HTTP/1.1\r\n\r\n".to_owned(),
            get("/nope"),
            "GET /1k-chunked HTTP/1.1\r\nconnection: close\r\n\r\n".to_owned(),
        ]
        .concat();
        let responses: [&[u8]; 5] = [
            response_for(b"/1k", false),
            response_for(b"/1k-h20", false),
            response_for(b"/1k-gzip", true),
            response_for(b"/nope", false),
            response_for(b"/1k-chunked", false),
        ];
        (input.into_bytes(), responses.concat())
    }

    #[test]
    fn every_split_point_gives_the_unsplit_output() {
        let (input, expected) = pipelined();
        for cut in 0..=input.len() {
            let mut script = Script::<true>::new(input.clone(), vec![cut]);
            serve(&mut script);
            assert!(script.output == expected, "split at {cut}");
            assert!(script.shut_down, "split at {cut}");
        }
    }

    #[test]
    fn copying_transport_writes_the_same_bytes() {
        let (input, expected) = pipelined();
        let mut script = Script::<false>::new(input, Vec::new());
        serve(&mut script);
        assert!(script.output == expected, "copied output differs");
    }

    #[test]
    fn batches_stop_before_passing_the_byte_limit() {
        let one = response_for(b"/1k", false);
        let count = BATCH_BYTES / one.len() + 10;
        let mut script = Script::<true>::new(get("/1k").repeat(count).into_bytes(), Vec::new());
        serve(&mut script);
        assert!(script.output == one.repeat(count), "batched output differs");
        assert!(script.writes.len() >= 2, "{:?}", script.writes);
        assert!(
            script.writes.iter().all(|&n| n <= BATCH_BYTES),
            "{:?}",
            script.writes
        );
    }

    #[test]
    fn a_large_response_is_written_alone() {
        let (small, large) = (response_for(b"/1k", false), response_for(b"/1m", false));
        let input = [get("/1k"), get("/1m"), get("/1k")].concat().into_bytes();
        let mut script = Script::<true>::new(input, Vec::new());
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
        let mut script = Script::<true>::new(input.into_bytes(), Vec::new());
        serve(&mut script);
        let parts: [&[u8]; 3] = [
            response_for(b"/1k", false),
            response_for(b"/1k-h20", false),
            refusal(Refusal::BadRequest),
        ];
        assert!(
            script.output == parts.concat(),
            "responses before the 400 differ"
        );
        assert!(script.shut_down);
    }

    #[test]
    fn head_larger_than_the_buffer_gets_431() {
        let input = format!(
            "GET /1k HTTP/1.1\r\nx-big: {}\r\n\r\n",
            "a".repeat(BUFFER_BYTES)
        );
        let mut script = Script::<true>::new(input.into_bytes(), Vec::new());
        serve(&mut script);
        assert_eq!(script.output, refusal(Refusal::HeadTooLarge));
        assert!(script.shut_down);
    }

    #[test]
    fn end_of_stream_without_a_request_writes_nothing() {
        let mut script = Script::<true>::new(Vec::new(), Vec::new());
        serve(&mut script);
        assert_eq!(script.output, b"");
        assert!(!script.shut_down);
    }
}
