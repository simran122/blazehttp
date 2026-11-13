//! Zero allocations per request once the write buffer has grown (`docs/notes/request-writer.md` §8.4). Its own test
//! binary, because `allocation-counter` installs a global allocator that counts the calling thread only.

mod support {
    pub mod write_rings;
}

use engine_core::h1::{HeaderTemplate, MAX_P0_REQUEST_BYTES, write_request};
use support::write_rings::{DEFAULT_LINES, DEFAULTS, HOST, Request, get, post1k, varied};

/// One pass over `ring`; returns the allocations it made.
fn write_all(out: &mut Vec<u8>, ring: &[Request], template: &HeaderTemplate) -> u64 {
    allocation_counter::measure(|| {
        for request in ring {
            let _ = write_request(out, &request.parts(), template, HOST).expect("written");
        }
    })
    .count_total
}

#[test]
fn steady_state_writes_allocate_nothing() {
    let template = HeaderTemplate::new(HOST, DEFAULTS, DEFAULT_LINES, &[]).expect("valid");
    let ring: Vec<Request> = [get(), post1k(), varied()].into_iter().flatten().collect();
    let mut out = Vec::new();
    // Warm-up: one pass grows `out` to the largest request (exact reservations, so it may grow a few times).
    assert!(
        write_all(&mut out, &ring, &template) > 0,
        "the counter sees the warm-up growth"
    );
    let (ptr, capacity) = (out.as_ptr(), out.capacity());
    assert_eq!(write_all(&mut out, &ring, &template), 0);
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
