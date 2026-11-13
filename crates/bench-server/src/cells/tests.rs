//! Unit tests for the cell catalog and its pre-built responses.

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;

use super::{
    CATALOG, DATE, HEAD_1K, JSON, PAYLOAD, PAYLOAD_GZIP, Refusal, Served, cells, fnv1a64, head,
    prebuild, refusal, response_for,
};
use crate::request::find_head_end;

fn split(response: &[u8]) -> (&[u8], &[u8]) {
    let end = find_head_end(response, 0).expect("blank line") + 4;
    response.split_at(end)
}

fn field<'a>(head: &'a [u8], name: &str) -> Option<&'a str> {
    let head = std::str::from_utf8(head).expect("ASCII head");
    head.split("\r\n")
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
}

fn gunzip(body: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    GzDecoder::new(body)
        .read_to_end(&mut decoded)
        .expect("gzip");
    decoded
}

fn dechunk(mut framed: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let line_end = framed
            .windows(2)
            .position(|w| w == b"\r\n")
            .expect("chunk size line");
        let size =
            usize::from_str_radix(std::str::from_utf8(&framed[..line_end]).expect("hex"), 16)
                .expect("size");
        if size == 0 {
            assert_eq!(&framed[line_end..], b"\r\n\r\n", "last chunk, no trailers");
            return body;
        }
        let data = &framed[line_end + 2..];
        body.extend_from_slice(&data[..size]);
        assert_eq!(&data[size..size + 2], b"\r\n");
        framed = &data[size + 2..];
    }
}

fn decoded_body(head: &[u8], body: &[u8]) -> Vec<u8> {
    if field(head, "transfer-encoding") == Some("chunked") {
        return dechunk(body);
    }
    let len: usize = field(head, "content-length")
        .expect("framing")
        .parse()
        .expect("length");
    assert_eq!(len, body.len(), "content-length matches the body");
    if field(head, "content-encoding") == Some("gzip") {
        gunzip(body)
    } else {
        body.to_vec()
    }
}

#[test]
fn head_builder_reproduces_the_1k_fixture() {
    let len = PAYLOAD.len().to_string();
    let built = head(
        "200 OK",
        &[
            ("content-type", JSON),
            ("content-length", &len),
            ("date", DATE),
        ],
    );
    assert_eq!(built, HEAD_1K);
}

#[test]
fn every_cell_is_framed_correctly_and_matches_its_checksum() {
    for cell in cells() {
        let (head, body) = split(&cell.response);
        let decoded = decoded_body(head, body);
        assert_eq!(fnv1a64(&decoded), cell.body_fnv, "{}", cell.path);
        if cell.path != "/1m" {
            assert_eq!(decoded, PAYLOAD, "{}", cell.path);
        }
    }
}

#[test]
fn committed_gzip_decodes_to_the_payload() {
    assert_eq!(gunzip(PAYLOAD_GZIP), PAYLOAD);
}

#[test]
fn head_only_responses_end_at_the_blank_line() {
    for cell in cells() {
        let head = response_for(cell.path.as_bytes(), true, false).bytes;
        assert_eq!(head, split(&cell.response).0, "{}", cell.path);
        assert_eq!(cell.head_len, head.len(), "{}", cell.path);
    }
    assert_eq!(
        response_for(b"/nope", true, false),
        response_for(b"/nope", false, false)
    );
}

#[test]
fn http10_gets_chunked_cells_close_delimited() {
    for cell in cells() {
        let answer = response_for(cell.path.as_bytes(), false, true);
        let Some(variant) = &cell.close_delimited else {
            assert_eq!(
                answer,
                Served {
                    bytes: &cell.response,
                    close: false
                },
                "{}",
                cell.path
            );
            continue;
        };
        assert_eq!(
            answer,
            Served {
                bytes: &variant.response,
                close: true
            },
            "{}",
            cell.path
        );
        let head = &variant.response[..variant.head_len];
        assert!(
            !String::from_utf8_lossy(head).contains("transfer-encoding"),
            "{}",
            cell.path
        );
        assert_eq!(
            &variant.response[variant.head_len..],
            PAYLOAD,
            "{}",
            cell.path
        );
        let head_only = response_for(cell.path.as_bytes(), true, true);
        assert_eq!(
            head_only,
            Served {
                bytes: head,
                close: true
            },
            "{}",
            cell.path
        );
    }
    assert!(
        cells().iter().any(|cell| cell.close_delimited.is_some()),
        "the chunked cell has a variant"
    );
}

#[test]
fn refusals_close_the_connection() {
    for (kind, status) in [
        (Refusal::BadRequest, "400 "),
        (Refusal::VersionNotSupported, "505 "),
        (Refusal::HeadTooLarge, "431 "),
    ] {
        let (head, body) = split(refusal(kind));
        assert!(
            head.starts_with(format!("HTTP/1.1 {status}").as_bytes()),
            "{kind:?}"
        );
        assert_eq!(field(head, "connection"), Some("close"), "{kind:?}");
        assert!(body.is_empty(), "{kind:?}");
    }
}

/// Set by [`prebuild_builds_the_catalog_in_a_fresh_process`] for the child it starts.
const FRESH_PROCESS: &str = "BENCH_SERVER_FRESH_PROCESS";

/// Must run alone in a fresh process (other tests build the catalog as a side effect), so it is ignored here and
/// run by [`prebuild_builds_the_catalog_in_a_fresh_process`]. Run any other way (`--include-ignored` in the
/// shared process), it checks nothing instead of failing at random.
#[test]
#[ignore = "run in a fresh process by prebuild_builds_the_catalog_in_a_fresh_process"]
fn prebuild_builds_the_catalog() {
    if std::env::var_os(FRESH_PROCESS).is_none() {
        return;
    }
    assert!(
        LazyLock::get(&CATALOG).is_none(),
        "nothing built the catalog yet"
    );
    prebuild();
    assert!(
        LazyLock::get(&CATALOG).is_some(),
        "prebuild built the catalog"
    );
}

/// How long the child test process may take.
const CHILD_WITHIN: Duration = Duration::from_secs(30);
/// Bytes of each child output stream kept for the assertions; the rest is read and dropped.
const OUTPUT_KEPT: u64 = 1 << 20;

/// Reads a child's output stream on its own thread while the child runs, so a chatty child cannot fill the pipe
/// and block; keeps the first [`OUTPUT_KEPT`] bytes.
fn read_on_a_thread(mut pipe: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        (&mut pipe)
            .take(OUTPUT_KEPT)
            .read_to_end(&mut kept)
            .expect("read the child's output");
        io::copy(&mut pipe, &mut io::sink()).expect("drain the child's output");
        kept
    })
}

#[test]
fn prebuild_builds_the_catalog_in_a_fresh_process() {
    let exe = std::env::current_exe().expect("test binary");
    let name = "cells::tests::prebuild_builds_the_catalog";
    let mut child = Command::new(exe)
        .args(["--exact", name, "--ignored", "--test-threads", "1"])
        .env(FRESH_PROCESS, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the test binary");
    let stdout = read_on_a_thread(child.stdout.take().expect("piped stdout"));
    let stderr = read_on_a_thread(child.stderr.take().expect("piped stderr"));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("child status") {
            break status;
        }
        if started.elapsed() > CHILD_WITHIN {
            child.kill().expect("kill the child");
            child.wait().expect("reap the child");
            panic!("the fresh-process test ran past {CHILD_WITHIN:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout.join().expect("stdout reader");
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = stderr.join().expect("stderr reader");
    assert!(
        status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the ignored test ran: {stdout}"
    );
}

#[test]
fn fnv1a64_matches_the_reference_vectors() {
    // Test vectors from the FNV reference (draft-eastlake-fnv, "" and "a").
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
}
