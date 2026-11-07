//! One client connection: read, answer every complete request with its pre-built bytes, one write per batch
//! (design note §5). Generic over the stream, so plain TCP and TLS share the code with static dispatch.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::cells::response_for;
use crate::request::{Scan, scan};

/// Read buffer per connection, which also bounds a request head (G6).
const BUFFER_BYTES: usize = 16 * 1024;
/// A batch stops growing past this many response bytes, which bounds the copy buffer for pipelined responses.
const BATCH_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Mode {
    Answer,
    /// Read everything, never write (the G2 stalled-server cell).
    Stall,
}

pub(crate) async fn run<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    mode: Mode,
) -> io::Result<()> {
    match mode {
        Mode::Answer => Connection::new(stream).answer().await,
        Mode::Stall => drain(stream).await,
    }
}

async fn drain<S: AsyncRead + Unpin>(mut stream: S) -> io::Result<()> {
    let mut buf = vec![0; BUFFER_BYTES];
    while stream.read(&mut buf).await? != 0 {}
    Ok(())
}

/// Why taking requests from the buffer stopped.
enum Progress {
    /// No complete request left: read more.
    NeedBytes,
    /// The batch is full: write it, then keep taking from the buffer.
    BatchFull,
    /// A request the fixture does not answer: write what came before it, then close.
    Refused,
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
    batch: Vec<&'static [u8]>,
    batch_bytes: usize,
    /// Copy buffer, used only when a batch holds more than one response.
    out: Vec<u8>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    fn new(stream: S) -> Self {
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
        }
    }

    async fn answer(mut self) -> io::Result<()> {
        while self.fill().await? {
            loop {
                let progress = self.take_requests();
                self.write_batch().await?;
                match progress {
                    Progress::NeedBytes => break,
                    Progress::BatchFull => {}
                    Progress::Refused => return Ok(()),
                }
            }
            self.compact();
        }
        Ok(())
    }

    /// Reads more bytes; `false` at end of stream or when a request head does not fit the buffer.
    async fn fill(&mut self) -> io::Result<bool> {
        if self.end == self.buf.len() {
            return Ok(false);
        }
        let n = self.stream.read(&mut self.buf[self.end..]).await?;
        self.end += n;
        Ok(n != 0)
    }

    fn take_requests(&mut self) -> Progress {
        loop {
            self.skip_body();
            if self.skip > 0 {
                return Progress::NeedBytes;
            }
            if self.batch_bytes >= BATCH_BYTES {
                return Progress::BatchFull;
            }
            match scan(&self.buf[self.start..self.end], self.resume) {
                Scan::Complete(request) => {
                    let response = response_for(request.target);
                    self.batch.push(response);
                    self.batch_bytes += response.len();
                    self.start += request.head_len;
                    self.skip = request.body_len;
                    self.resume = 0;
                }
                Scan::Partial { resume } => {
                    self.resume = resume;
                    return Progress::NeedBytes;
                }
                Scan::Refused => return Progress::Refused,
            }
        }
    }

    fn skip_body(&mut self) {
        let buffered = self.end - self.start;
        let dropped = usize::try_from(self.skip).map_or(buffered, |skip| skip.min(buffered));
        self.start += dropped;
        self.skip -= dropped as u64;
    }

    /// Writes the batch in one `write_all`: straight from the static bytes for one response, copied for several.
    async fn write_batch(&mut self) -> io::Result<()> {
        match self.batch.as_slice() {
            [] => return Ok(()),
            [response] => self.stream.write_all(response).await?,
            responses => {
                self.out.clear();
                for response in responses {
                    self.out.extend_from_slice(response);
                }
                self.stream.write_all(&self.out).await?;
            }
        }
        self.batch.clear();
        self.batch_bytes = 0;
        // TLS streams may still hold ciphertext after `write_all` (tokio-rustls); a no-op on TCP.
        self.stream.flush().await
    }

    /// Moves a partial request to the front of the buffer.
    fn compact(&mut self) {
        self.buf.copy_within(self.start..self.end, 0);
        self.end -= self.start;
        self.start = 0;
    }
}
