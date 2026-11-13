//! Unit tests for one connection, driven by a scripted peer ([`script`]) on a paused clock.

mod script;

use std::io;
use std::time::Duration;

use self::script::{Script, drive, drive_within};
use super::{BATCH_BYTES, BUFFER_BYTES, LINGER_TIME};
use crate::cells::{Refusal, refusal, response_for};

/// The response a cell sends to an HTTP/1.1 request.
fn wire(target: &[u8], head_only: bool) -> &'static [u8] {
    response_for(target, head_only, false).bytes
}

/// Body larger than the read buffer, so skipping it spans several reads.
const BIG_BODY: usize = BUFFER_BYTES + 3_000;

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
#[should_panic(expected = "write-call cap")]
fn stub_fails_fast_past_its_write_call_cap() {
    let mut script = Script::new(get("/1m").into_bytes(), Vec::new());
    script.max_write = 1;
    serve(&mut script);
}

/// After exactly 64 KiB of unread input the linger must stop at once: one more read on a quiet peer would hold the
/// slot until the linger time runs out.
#[test]
fn lingering_stops_at_64_kib_without_another_read() {
    let mut input = b"BROKEN\r\n\r\n".to_vec();
    input.resize(BUFFER_BYTES * 20, b'x');
    let mut script = Script::new(input, Vec::new());
    script.stall_reads_at = Some(BUFFER_BYTES + 65_536);
    let finished = drive_within(&mut script, Duration::from_secs(60), LINGER_TIME / 2);
    finished
        .expect("closed without waiting for more input")
        .expect("connection");
    assert_eq!(script.output, refusal(Refusal::BadRequest));
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
fn a_refused_version_closes_after_its_body() {
    let body_len = BUFFER_BYTES * 20;
    let head = format!("POST /1k HTTP/2.0\r\ncontent-length: {body_len}\r\n\r\n");
    let mut input = head.into_bytes();
    input.resize(input.len() + body_len, b'x');
    let mut script = Script::new(input.clone(), Vec::new());
    serve(&mut script);
    assert_eq!(script.output, refusal(Refusal::VersionNotSupported));
    assert_eq!(
        script.read,
        input.len(),
        "the whole body is read before closing"
    );
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

/// A slow reader that keeps taking bytes is not cut off: the deadline bounds each write call, not the whole batch.
#[test]
fn a_slow_but_progressing_reader_gets_the_whole_response() {
    let mut script = Script::new(get("/1k").into_bytes(), Vec::new());
    script.max_write = 100;
    script.write_delay = Some(Duration::from_millis(300));
    drive(&mut script, Duration::from_secs(1))
        .expect("finished before the test bound")
        .expect("no write timed out");
    assert_eq!(script.output, wire(b"/1k", false));
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

/// The same after a request with a body: its deadline ends when the last body byte is read.
#[test]
fn a_request_after_a_body_gets_its_own_deadline() {
    let first = "POST /1k HTTP/1.1\r\ncontent-length: 10\r\n\r\n0123456789".to_owned();
    let second = get("/1k-h20");
    let input = [first.clone(), second].concat().into_bytes();
    let mut script = Script::new(input, vec![5, first.len() - 4]);
    script.pauses = vec![
        (first.len(), Duration::from_secs(5)),
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

/// A head that trickles in must still complete within one deadline: reads do not restart it.
#[test]
fn a_trickled_head_is_closed_at_its_deadline() {
    let input = get("/1k").into_bytes();
    let mut script = Script::new(input, Vec::new());
    script.pauses = (1..=6)
        .map(|i| (i * 5, Duration::from_millis(300)))
        .collect();
    drive(&mut script, Duration::from_secs(1))
        .expect("finished")
        .expect("connection");
    assert_eq!(script.output, b"", "no response after the deadline");
    assert!(script.shut_down);
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
