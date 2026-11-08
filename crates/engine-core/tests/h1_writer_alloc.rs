//! Zero allocations per request once the write buffer has grown (`docs/notes/request-writer.md` §8.4). Its own test
//! binary, because `allocation-counter` installs a global allocator that counts the calling thread only.

use engine_core::generated::spec::Method;
use engine_core::h1::{HeaderTemplate, MAX_P0_REQUEST_BYTES, RequestParts, write_request};

const HOST: &[u8] = b"127.0.0.1:3999";
const RING: usize = 256;
const METHODS: [Method; 7] = [
    Method::Get,
    Method::Post,
    Method::Put,
    Method::Delete,
    Method::Patch,
    Method::Options,
    Method::Query,
];

/// One owned request of the varied ring.
struct Owned {
    method: Method,
    target: Vec<u8>,
    headers: Vec<u8>,
    header_lines: u16,
    body: Vec<u8>,
}

impl Owned {
    fn parts(&self) -> RequestParts<'_> {
        RequestParts {
            method: self.method,
            target: &self.target,
            headers: &self.headers,
            header_lines: self.header_lines,
            body: &self.body,
        }
    }
}

/// A deterministic ring of mixed methods, targets of 8–200 bytes, 0–6 extra fields and bodies of 0–2 KiB.
fn ring() -> Vec<Owned> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % u64::try_from(bound).expect("small")).expect("small")
    };
    (0..RING)
        .map(|i| {
            let fields = next(7);
            let headers: Vec<u8> = (0..fields)
                .flat_map(|f| format!("x-field-{f}: {}\r\n", "v".repeat(4 + next(76))).into_bytes())
                .collect();
            Owned {
                method: METHODS[i % METHODS.len()],
                target: [&b"/"[..], &vec![b'p'; 7 + next(193)]].concat(),
                headers,
                header_lines: u16::try_from(fields).expect("small"),
                body: vec![b'b'; next(2049)],
            }
        })
        .collect()
}

#[test]
fn steady_state_writes_allocate_nothing() {
    let template =
        HeaderTemplate::new(HOST, b"accept: */*\r\nuser-agent: blazehttp\r\n", &[]).expect("valid");
    let ring = ring();
    // Warm-up: one pass grows `out` to the ring's largest request (exact reservations, so it may grow a few times).
    let mut out = Vec::new();
    for request in &ring {
        let _ = write_request(&mut out, &request.parts(), &template, HOST).expect("warm-up");
    }
    let (ptr, capacity) = (out.as_ptr(), out.capacity());
    let info = allocation_counter::measure(|| {
        for request in &ring {
            let _ = write_request(&mut out, &request.parts(), &template, HOST).expect("written");
        }
    });
    assert_eq!(info.count_total, 0, "{info:?}");
    assert_eq!(
        (out.as_ptr(), out.capacity()),
        (ptr, capacity),
        "the buffer never moved or grew"
    );
    assert!(
        capacity <= MAX_P0_REQUEST_BYTES,
        "bounded by the largest P0 request"
    );
}
