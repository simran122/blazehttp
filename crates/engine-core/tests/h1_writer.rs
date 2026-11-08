//! `write_request` and `HeaderTemplate` against the request vector corpus (`tests/vectors/h1-request`, suite
//! `engine-core:h1_writer`) and the unit cases of `docs/notes/request-writer.md` §9.1.

mod support {
    pub mod request_vectors;
    pub mod vectors;
}

use engine_core::generated::spec::{EngineDetail, ErrorCode, H1_LIMITS, Method, TooLargeDetail};
use engine_core::h1::{HeaderTemplate, MAX_P0_REQUEST_BYTES, RequestParts, Reuse, write_request};
use support::request_vectors::{RequestCase, RequestExpect};
use support::vectors::load_suite;

const SUITE: &str = "engine-core:h1_writer";
const INTERNAL: ErrorCode = ErrorCode::Engine(EngineDetail::Internal);
const URL_TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Url);
const HEADERS_TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Headers);
const HOST: &[u8] = b"127.0.0.1:3999";
/// `host: 127.0.0.1:3999\r\n`.
const HOST_LINE_LEN: usize = 22;

const ALL_METHODS: [Method; 8] = [
    Method::Get,
    Method::Head,
    Method::Post,
    Method::Put,
    Method::Delete,
    Method::Patch,
    Method::Options,
    Method::Query,
];
const FORBIDDEN: [&str; 17] = [
    "host",
    "transfer-encoding",
    "connection",
    "upgrade",
    "expect",
    "te",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "content-length",
    "content_length",
    "transfer_encoding",
    "proxy_authorization",
    "content_type",
    "user_agent",
    "keep_alive",
    "proxy_connection",
];
const SINGLETONS: [&str; 4] = ["authorization", "cookie", "content-type", "user-agent"];

fn template(fields: &[u8]) -> HeaderTemplate {
    HeaderTemplate::new(HOST, fields, &[]).expect("valid template")
}

/// Every CRLF-terminated line of `headers` counts as one serialised field.
fn line_count(headers: &[u8]) -> u16 {
    let lines = headers.windows(2).filter(|w| *w == b"\r\n").count();
    u16::try_from(lines).expect("few lines")
}

fn parts<'a>(
    method: Method,
    target: &'a [u8],
    headers: &'a [u8],
    body: &'a [u8],
) -> RequestParts<'a> {
    RequestParts {
        method,
        target,
        headers,
        header_lines: line_count(headers),
        body,
    }
}

/// Writes into a fresh buffer; an error must leave the buffer empty.
fn write(
    parts: &RequestParts<'_>,
    template: &HeaderTemplate,
) -> Result<(Vec<u8>, Reuse), ErrorCode> {
    let mut out = b"stale bytes".to_vec();
    match write_request(&mut out, parts, template, template.authority()) {
        Ok(reuse) => Ok((out, reuse)),
        Err(e) => {
            assert_eq!(out, b"", "an error leaves `out` cleared");
            Err(e)
        }
    }
}

fn written(parts: &RequestParts<'_>, template: &HeaderTemplate) -> (Vec<u8>, Reuse) {
    write(parts, template).unwrap_or_else(|e| panic!("{parts:?}: {e}"))
}

fn rejected(parts: &RequestParts<'_>, template: &HeaderTemplate) -> ErrorCode {
    write(parts, template).expect_err("rejected")
}

fn head_of(method: Method, headers: &[u8], body: &[u8]) -> String {
    let (out, _) = written(&parts(method, b"/x", headers, body), &template(b""));
    String::from_utf8(out).expect("ASCII")
}

/// `name` in lower case, UPPER case and miXed case.
fn cases_of(name: &str) -> [String; 3] {
    let mixed = name
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i % 2 == 0 {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect();
    [name.to_owned(), name.to_ascii_uppercase(), mixed]
}

// ---------------------------------------------------------------- brief tests

#[test]
fn get_uses_template_once_and_single_buffer() {
    let (out, reuse) = written(&parts(Method::Get, b"/data", b"", b""), &template(b""));
    assert_eq!(out, b"GET /data HTTP/1.1\r\nhost: 127.0.0.1:3999\r\n\r\n");
    assert_eq!(reuse, Reuse::Allowed);
}

#[test]
fn post_sets_content_length_from_body() {
    let template = template(b"accept: */*\r\n");
    let request = parts(
        Method::Post,
        b"/p?q=1",
        b"content-type: text/plain\r\n",
        b"hello",
    );
    let (out, reuse) = written(&request, &template);
    assert_eq!(
        out,
        b"POST /p?q=1 HTTP/1.1\r\nhost: 127.0.0.1:3999\r\naccept: */*\r\ncontent-type: text/plain\r\n\
          content-length: 5\r\n\r\nhello"
    );
    assert_eq!(reuse, Reuse::Allowed);
}

#[test]
fn rejects_forbidden_user_headers() {
    for name in FORBIDDEN {
        for spelled in cases_of(name) {
            let headers = format!("{spelled}: v\r\n");
            let request = parts(Method::Get, b"/", headers.as_bytes(), b"");
            assert_eq!(rejected(&request, &template(b"")), INTERNAL, "{spelled}");
        }
    }
}

#[test]
fn rejects_target_with_space_ctl_or_non_ascii() {
    for b in (0..=0x20).chain(0x7F..=0xFF).chain(*b"#") {
        for target in [
            vec![b'/', b],
            vec![b'/', b'a', b, b'b'],
            vec![b'/', b'a', b],
        ] {
            let request = parts(Method::Get, &target, b"", b"");
            assert_eq!(rejected(&request, &template(b"")), INTERNAL, "{b:#04x}");
        }
    }
}

#[test]
fn rejects_header_value_ctl_except_htab() {
    for b in (0..0x20).chain([0x7F]).filter(|&b| b != b'\t') {
        for value in [vec![b, b'v'], vec![b'v', b, b'v'], vec![b'v', b]] {
            let headers = [&b"x-a: "[..], &value, b"\r\n"].concat();
            let request = RequestParts {
                header_lines: 1,
                ..parts(Method::Get, b"/", &headers, b"")
            };
            assert_eq!(rejected(&request, &template(b"")), INTERNAL, "{b:#04x}");
        }
    }
}

#[test]
fn rejects_non_token_header_name() {
    for b in (0..=0xFF_u8).filter(|b| !b.is_ascii_alphanumeric() && !b"!#$%&'*+-.^_`|~".contains(b))
    {
        let headers = [&b"x"[..], &[b], b"y: v\r\n"].concat();
        let request = RequestParts {
            header_lines: 1,
            ..parts(Method::Get, b"/", &headers, b"")
        };
        assert_eq!(rejected(&request, &template(b"")), INTERNAL, "{b:#04x}");
    }
}

const INVALID_DEFAULTS: [&[u8]; 8] = [
    b"x-a: b",
    b"x-a:b\r\n",
    b": b\r\n",
    b"x a: b\r\n",
    b"x-a: b\r\n\r\n",
    b"x-a: b\nc\r\n",
    b"x-a: \x00\r\n",
    b"host: other\r\n",
];

#[test]
fn template_rejects_invalid_defaults() {
    for fields in INVALID_DEFAULTS {
        assert_eq!(
            HeaderTemplate::new(HOST, fields, &[]).expect_err("invalid"),
            INTERNAL,
            "{}",
            fields.escape_ascii()
        );
    }
}

#[test]
fn reuses_output_capacity() {
    let template = template(b"accept: */*\r\n");
    let mut out = Vec::new();
    let big = parts(Method::Post, b"/big", b"x-a: 1\r\n", &[b'b'; 4096]);
    let _ = write_request(&mut out, &big, &template, HOST).expect("written");
    let (ptr, capacity) = (out.as_ptr(), out.capacity());
    let small = parts(Method::Get, b"/small", b"", b"");
    let _ = write_request(&mut out, &small, &template, HOST).expect("written");
    assert_eq!((out.as_ptr(), out.capacity()), (ptr, capacity));
    let bad = parts(Method::Get, b"bad", b"", b"");
    assert_eq!(
        write_request(&mut out, &bad, &template, HOST),
        Err(INTERNAL)
    );
    assert_eq!(out, b"");
    assert_eq!((out.as_ptr(), out.capacity()), (ptr, capacity));
}

// ---------------------------------------------------------------- framing and reuse (§6.5)

#[test]
fn post_put_patch_query_reusable_without_connection_header() {
    for method in [Method::Post, Method::Put, Method::Patch, Method::Query] {
        for body in [&b""[..], b"hello"] {
            let (out, reuse) = written(&parts(method, b"/", b"", body), &template(b""));
            let text = String::from_utf8(out).expect("ASCII");
            assert_eq!(reuse, Reuse::Allowed, "{method:?}");
            assert!(!text.contains("connection"), "{method:?}");
            assert!(
                text.contains(&format!("content-length: {}\r\n", body.len())),
                "{method:?}"
            );
        }
    }
}

#[test]
fn query_and_patch_send_content_length_zero() {
    for method in [Method::Query, Method::Patch] {
        assert!(head_of(method, b"", b"").ends_with("\r\ncontent-length: 0\r\n\r\n"));
    }
}

#[test]
fn put_post_empty_body_send_zero() {
    for method in [Method::Put, Method::Post] {
        assert!(head_of(method, b"", b"").ends_with("\r\ncontent-length: 0\r\n\r\n"));
    }
}

#[test]
fn get_head_delete_options_empty_body_send_none() {
    for method in [Method::Get, Method::Head, Method::Delete, Method::Options] {
        let expected = format!(
            "{} /x HTTP/1.1\r\nhost: 127.0.0.1:3999\r\n\r\n",
            method.as_str()
        );
        assert_eq!(head_of(method, b"", b""), expected);
    }
}

#[test]
fn query_with_body_is_framed_and_reusable() {
    let (out, reuse) = written(&parts(Method::Query, b"/q", b"", b"{}"), &template(b""));
    assert_eq!(
        out,
        b"QUERY /q HTTP/1.1\r\nhost: 127.0.0.1:3999\r\ncontent-length: 2\r\n\r\n{}"
    );
    assert_eq!(reuse, Reuse::Allowed);
}

#[test]
fn get_delete_options_with_body_send_connection_close_and_not_reusable() {
    for method in [Method::Get, Method::Delete, Method::Options] {
        let (out, reuse) = written(&parts(method, b"/x", b"x-a: 1\r\n", b"abc"), &template(b""));
        let expected = format!(
            "{} /x HTTP/1.1\r\nhost: 127.0.0.1:3999\r\nx-a: 1\r\ncontent-length: 3\r\nconnection: close\r\n\r\nabc",
            method.as_str()
        );
        assert_eq!(String::from_utf8(out).expect("ASCII"), expected);
        assert_eq!(reuse, Reuse::CloseAfterExchange);
    }
}

#[test]
fn head_with_body_rejected() {
    assert_eq!(
        rejected(&parts(Method::Head, b"/", b"", b"x"), &template(b"")),
        INTERNAL
    );
}

#[test]
fn content_length_digits() {
    for len in [0, 9, 10, 99, 100, 847, 65_536] {
        let body = vec![b'b'; len];
        let (out, _) = written(&parts(Method::Post, b"/", b"", &body), &template(b""));
        let line = format!("\r\ncontent-length: {len}\r\n\r\n");
        assert!(
            out.windows(line.len()).any(|w| w == line.as_bytes()),
            "{len}"
        );
    }
}

#[test]
fn every_method_is_written_as_its_name() {
    for method in ALL_METHODS {
        let body: &[u8] = if method == Method::Head { b"" } else { b"1" };
        let (out, _) = written(&parts(method, b"/", b"", body), &template(b""));
        assert!(out.starts_with(format!("{} / HTTP/1.1\r\n", method.as_str()).as_bytes()));
    }
}

#[test]
fn unknown_method_id_rejected() {
    // The codec maps an id that does not decode to `Engine(Internal)` before the writer runs (note §6.5).
    assert_eq!(Method::from_wire(99), None);
}

// ---------------------------------------------------------------- origin binding and templates

#[test]
fn rejects_template_for_other_authority() {
    let template = HeaderTemplate::new(b"example.com:8080", b"", &[]).expect("valid");
    let request = parts(Method::Get, b"/", b"", b"");
    for other in [
        &b"example.com:8081"[..],
        b"Example.com:8080",
        b"example.com.:8080",
        b"example.com",
    ] {
        let mut out = Vec::new();
        assert_eq!(
            write_request(&mut out, &request, &template, other),
            Err(INTERNAL)
        );
    }
    let mut out = Vec::new();
    assert!(write_request(&mut out, &request, &template, b"example.com:8080").is_ok());
}

const INVALID_HOSTS: [&[u8]; 22] = [
    b"",
    b"a b",
    b"a/b",
    b"user@a",
    b"a?b",
    b"a#b",
    b"a%20b",
    b"a\rb",
    b"a\nb",
    b"a\x00b",
    b"a\x7fb",
    b"a\x80b",
    "é.example".as_bytes(),
    b"[::1",
    b"::1]",
    b"[]",
    b"[::1%en0]",
    b"a:",
    b"a:65536",
    b"a:123456",
    b"a:-1",
    b"a:1:2",
];

#[test]
fn template_rejects_invalid_host() {
    for host in INVALID_HOSTS {
        assert_eq!(
            HeaderTemplate::new(host, b"", &[]).expect_err("invalid"),
            INTERNAL,
            "{}",
            host.escape_ascii()
        );
    }
}

const VALID_HOSTS: [&[u8]; 8] = [
    b"example.com",
    b"127.0.0.1:3999",
    b"[::1]:8080",
    b"[::ffff:1.2.3.4]",
    b"xn--bcher-kva.example",
    b"a,b.example",
    b"a:0",
    b"a:65535",
];

#[test]
fn template_accepts_hosts() {
    for host in VALID_HOSTS {
        let template = HeaderTemplate::new(host, b"", &[]).expect("valid");
        assert_eq!(template.authority(), host);
    }
}

#[test]
fn template_rejects_forbidden_defaults() {
    for name in FORBIDDEN {
        for spelled in cases_of(name) {
            let fields = format!("{spelled}: v\r\n");
            assert_eq!(
                HeaderTemplate::new(HOST, fields.as_bytes(), &[]).expect_err("forbidden"),
                INTERNAL,
                "{spelled}"
            );
        }
    }
}

const SENSITIVE: [&[u8]; 4] = [
    b"authorization",
    b"cookie",
    b"x-api-key",
    b"x-custom-secret",
];

#[test]
fn template_sensitive_flag() {
    for fields in [
        &b"Authorization: t\r\n"[..],
        b"cookie: s=1\r\n",
        b"X-Api-Key: k\r\n",
        b"a: 1\r\nx-custom-secret: z\r\n",
    ] {
        let template = HeaderTemplate::new(HOST, fields, &SENSITIVE).expect("valid");
        assert!(template.is_sensitive(), "{}", fields.escape_ascii());
    }
    for fields in [&b""[..], b"accept: */*\r\n", b"x-api: k\r\n"] {
        assert!(
            !HeaderTemplate::new(HOST, fields, &SENSITIVE)
                .expect("valid")
                .is_sensitive()
        );
    }
}

#[test]
fn template_too_large() {
    let fill = |len: usize| {
        [
            &b"x-fill: "[..],
            &vec![b'a'; len - HOST_LINE_LEN - 10],
            b"\r\n",
        ]
        .concat()
    };
    let max = H1_LIMITS.max_request_fields_bytes;
    assert!(HeaderTemplate::new(HOST, &fill(max), &[]).is_ok());
    assert_eq!(
        HeaderTemplate::new(HOST, &fill(max + 1), &[]).expect_err("large"),
        HEADERS_TOO_LARGE
    );
    let host = vec![b'a'; max];
    assert_eq!(
        HeaderTemplate::new(&host, b"", &[]).expect_err("large"),
        HEADERS_TOO_LARGE
    );
}

#[test]
fn template_debug_hides_bytes() {
    let template = HeaderTemplate::new(
        b"secret-host.example",
        b"authorization: Bearer s3cr3t\r\n",
        &SENSITIVE,
    )
    .expect("valid");
    let debug = format!("{template:?}");
    assert!(
        !debug.contains("secret") && !debug.contains("s3cr3t"),
        "{debug}"
    );
    assert!(debug.contains("sensitive: true"), "{debug}");
}

#[test]
fn parts_debug_hides_bytes() {
    let request = parts(
        Method::Post,
        b"/secret-path",
        b"cookie: s3cr3t\r\n",
        b"secret-body",
    );
    let debug = format!("{request:?}");
    assert!(
        !debug.contains("secret") && !debug.contains("s3cr3t"),
        "{debug}"
    );
    assert!(
        debug.contains("header_lines: 1") && debug.contains("Post"),
        "{debug}"
    );
}

// ---------------------------------------------------------------- header names (§6.4)

#[test]
fn rejects_duplicate_singleton_template_vs_delta() {
    for name in SINGLETONS {
        let template = template(format!("{name}: a\r\n").as_bytes());
        for spelled in cases_of(name) {
            let headers = format!("{spelled}: b\r\n");
            let request = parts(Method::Get, b"/", headers.as_bytes(), b"");
            assert_eq!(rejected(&request, &template), INTERNAL, "{spelled}");
        }
    }
}

#[test]
fn rejects_duplicate_singleton_within_delta() {
    for name in SINGLETONS {
        let once = format!("{name}: a\r\n");
        assert!(
            write(
                &parts(Method::Get, b"/", once.as_bytes(), b""),
                &template(b"")
            )
            .is_ok()
        );
        let twice = format!(
            "{name}: a\r\nx-b: 1\r\n{}: b\r\n",
            name.to_ascii_uppercase()
        );
        let request = parts(Method::Get, b"/", twice.as_bytes(), b"");
        assert_eq!(rejected(&request, &template(b"")), INTERNAL, "{name}");
        let in_template = format!("{name}: a\r\n{name}: b\r\n");
        assert_eq!(
            HeaderTemplate::new(HOST, in_template.as_bytes(), &[]).expect_err("twice"),
            INTERNAL
        );
    }
}

#[test]
fn distinct_singletons_and_repeated_other_names_are_accepted() {
    let all = b"authorization: a\r\ncookie: c\r\ncontent-type: t\r\nuser-agent: u\r\naccept: 1\r\naccept: 2\r\n";
    assert!(write(&parts(Method::Post, b"/", all, b""), &template(b"")).is_ok());
}

#[test]
fn rejects_latin1_truncated_forbidden_names() {
    // A JS name `Ŕe` (U+0154) written as latin1 truncates to `Te`; P2 rejects code points > 0xFF first.
    for name in [&b"Te"[..], b"TE", b"tE"] {
        let headers = [name, b": trailers\r\n"].concat();
        assert_eq!(
            rejected(&parts(Method::Get, b"/", &headers, b""), &template(b"")),
            INTERNAL
        );
    }
}

#[test]
fn rejects_injected_line_by_count() {
    // JS serialised one field each time; the value carried a CRLF that forms a well-formed extra line.
    for headers in [
        &b"x-a: 1\r\nx-forwarded-for: 1.2.3.4\r\n"[..],
        b"accept: a\r\ncookie: injected=1\r\n",
    ] {
        let request = RequestParts {
            header_lines: 1,
            ..parts(Method::Get, b"/", headers, b"")
        };
        assert_eq!(
            rejected(&request, &template(b"")),
            INTERNAL,
            "{}",
            headers.escape_ascii()
        );
    }
}

#[test]
fn header_lines_must_match() {
    let headers = b"a: 1\r\nb: 2\r\n";
    for lines in [0, 1, 3, u16::MAX] {
        let request = RequestParts {
            header_lines: lines,
            ..parts(Method::Get, b"/", headers, b"")
        };
        assert_eq!(rejected(&request, &template(b"")), INTERNAL, "{lines}");
    }
    let empty = RequestParts {
        header_lines: 1,
        ..parts(Method::Get, b"/", b"", b"")
    };
    assert_eq!(rejected(&empty, &template(b"")), INTERNAL);
    assert!(write(&parts(Method::Get, b"/", headers, b""), &template(b"")).is_ok());
}

// ---------------------------------------------------------------- line grammar (§6.4)

const BAD_LINES: [&[u8]; 7] = [
    b"a: b\r\n\r\nc: d\r\n",
    b"a: b",
    b"a: b\r",
    b"a:b\r\n",
    b": b\r\n",
    b"a : b\r\n",
    b"a: b\r\n c\r\n",
];

#[test]
fn rejects_malformed_delta_lines() {
    for headers in BAD_LINES {
        let request = RequestParts {
            header_lines: 1,
            ..parts(Method::Get, b"/", headers, b"")
        };
        assert_eq!(
            rejected(&request, &template(b"")),
            INTERNAL,
            "{}",
            headers.escape_ascii()
        );
        assert_eq!(
            HeaderTemplate::new(HOST, headers, &[]).expect_err("bad"),
            INTERNAL
        );
    }
}

#[test]
fn accepts_obs_text_htab_ows_and_empty_values() {
    for headers in [
        &b"x-a: \x80\xff\r\n"[..],
        b"x-a: a\tb\r\n",
        b"x-a:  \t v \t\r\n",
        b"x-a: \r\n",
    ] {
        let (out, _) = written(&parts(Method::Get, b"/", headers, b""), &template(b""));
        assert!(
            out.windows(headers.len()).any(|w| w == headers),
            "sent as given"
        );
    }
}

// ---------------------------------------------------------------- target (§6.3)

#[test]
fn target_must_be_origin_form() {
    for target in [
        &b"http://evil/"[..],
        b"*",
        b"?x",
        b"",
        b"a/b",
        b"#",
        b"/a#",
        b"/#b",
        b"/a#b",
    ] {
        assert_eq!(
            rejected(&parts(Method::Get, target, b"", b""), &template(b"")),
            INTERNAL
        );
    }
    for target in [
        &b"//evil.example/x"[..],
        b"/",
        b"/a%0d%0a?b=c&d=~!$'()*+,;=:@",
    ] {
        assert!(write(&parts(Method::Get, target, b"", b""), &template(b"")).is_ok());
    }
}

#[test]
fn target_limit() {
    let max = H1_LIMITS.max_request_target_bytes;
    let target = |len: usize| [&b"/"[..], &vec![b'a'; len - 1]].concat();
    assert!(write(&parts(Method::Get, &target(max), b"", b""), &template(b"")).is_ok());
    assert_eq!(
        rejected(
            &parts(Method::Get, &target(max + 1), b"", b""),
            &template(b"")
        ),
        URL_TOO_LARGE
    );
}

// ---------------------------------------------------------------- limits and precedence (§7)

/// A delta that makes template + delta exactly `total` bytes with the host-only template.
fn fill_delta(total: usize) -> Vec<u8> {
    [
        &b"x-fill: "[..],
        &vec![b'a'; total - HOST_LINE_LEN - 10],
        b"\r\n",
    ]
    .concat()
}

#[test]
fn fields_limit() {
    let max = H1_LIMITS.max_request_fields_bytes;
    assert!(
        write(
            &parts(Method::Get, b"/", &fill_delta(max), b""),
            &template(b"")
        )
        .is_ok()
    );
    assert_eq!(
        rejected(
            &parts(Method::Get, b"/", &fill_delta(max + 1), b""),
            &template(b"")
        ),
        HEADERS_TOO_LARGE
    );
}

#[test]
fn body_over_copy_limit_rejected() {
    let max = H1_LIMITS.max_copied_body_bytes;
    assert!(
        write(
            &parts(Method::Post, b"/", b"", &vec![b'b'; max]),
            &template(b"")
        )
        .is_ok()
    );
    assert_eq!(
        rejected(
            &parts(Method::Post, b"/", b"", &vec![b'b'; max + 1]),
            &template(b"")
        ),
        INTERNAL
    );
}

#[test]
fn error_precedence() {
    let long_cr = [&b"/\r"[..], &vec![b'a'; H1_LIMITS.max_request_target_bytes]].concat();
    assert_eq!(
        rejected(&parts(Method::Get, &long_cr, b"", b""), &template(b"")),
        URL_TOO_LARGE
    );
    let large_forbidden = [
        &b"host: x\r\n"[..],
        &fill_delta(H1_LIMITS.max_request_fields_bytes),
    ]
    .concat();
    let request = parts(Method::Get, b"/", &large_forbidden, b"");
    assert_eq!(rejected(&request, &template(b"")), HEADERS_TOO_LARGE);
    let big_body_long_target =
        [&b"/"[..], &vec![b'a'; H1_LIMITS.max_request_target_bytes]].concat();
    let body = vec![b'b'; H1_LIMITS.max_copied_body_bytes + 1];
    let request = parts(Method::Post, &big_body_long_target, b"", &body);
    assert_eq!(
        rejected(&request, &template(b"")),
        INTERNAL,
        "the body check comes first"
    );
    let mut out = Vec::new();
    let bad_delta = parts(Method::Get, b"/", b"host: x\r\n", b"");
    assert_eq!(
        write_request(&mut out, &bad_delta, &template(b""), b"other"),
        Err(INTERNAL)
    );
}

#[test]
fn max_p0_request_bytes_is_the_largest_request() {
    // OPTIONS is the longest method that may carry a body; with a body it adds `connection: close`.
    let target = [
        &b"/"[..],
        &vec![b'a'; H1_LIMITS.max_request_target_bytes - 1],
    ]
    .concat();
    let delta = fill_delta(H1_LIMITS.max_request_fields_bytes);
    let body = vec![b'b'; H1_LIMITS.max_copied_body_bytes];
    let (out, reuse) = written(
        &parts(Method::Options, &target, &delta, &body),
        &template(b""),
    );
    assert_eq!(reuse, Reuse::CloseAfterExchange);
    assert_eq!(out.len(), MAX_P0_REQUEST_BYTES);
    assert!(out.capacity() <= MAX_P0_REQUEST_BYTES, "reserved exactly");
}

// ---------------------------------------------------------------- vectors (§9.2–§9.4)

fn method_of(case: &RequestCase) -> Method {
    ALL_METHODS
        .into_iter()
        .find(|m| m.as_str() == case.method)
        .unwrap_or_else(|| panic!("{}: unknown method", case.name))
}

fn run_case(case: &RequestCase) -> RequestExpect {
    let template = match HeaderTemplate::new(&case.template_host, &case.template_fields, &[]) {
        Ok(template) => template,
        Err(e) => return RequestExpect::Error(e.to_string()),
    };
    let request = RequestParts {
        method: method_of(case),
        target: &case.target,
        headers: &case.headers,
        header_lines: case.header_lines,
        body: &case.body,
    };
    let mut out = Vec::new();
    match write_request(&mut out, &request, &template, &case.conn_authority) {
        Ok(Reuse::Allowed) => RequestExpect::Written {
            bytes: out,
            reuse: "allowed".to_owned(),
        },
        Ok(Reuse::CloseAfterExchange) => RequestExpect::Written {
            bytes: out,
            reuse: "closeAfterExchange".to_owned(),
        },
        Err(e) => {
            assert!(
                out.is_empty(),
                "{}: an error leaves `out` cleared",
                case.name
            );
            RequestExpect::Error(e.to_string())
        }
    }
}

#[test]
fn injection_vectors() {
    let cases = load_suite::<RequestCase>(SUITE);
    assert!(cases.len() > 400, "the whole corpus loads");
    for (file, case) in cases {
        assert_eq!(run_case(&case), case.expect, "{file}/{}", case.name);
    }
}
