//! `write_request` and `HeaderTemplate` against the request vector corpus (`tests/vectors/h1-request`, suite
//! `engine-core:h1_writer`) and the unit cases of `docs/notes/request-writer.md` §9.1.

mod support {
    pub mod request_vectors;
    pub mod vectors;
}

use engine_core::generated::spec::{EngineDetail, ErrorCode, H1_LIMITS, Method, TooLargeDetail};
use engine_core::h1::{HeaderTemplate, MAX_P0_REQUEST_BYTES, RequestParts, Reuse, write_request};
use support::request_vectors::{RequestCase, RequestExpect, RequestReuse};
use support::vectors::load_suite;

const SUITE: &str = "engine-core:h1_writer";
const INTERNAL: ErrorCode = ErrorCode::Engine(EngineDetail::Internal);
const URL_TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Url);
const HEADERS_TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Headers);
const HOST: &[u8] = b"127.0.0.1:3999";
/// `host: 127.0.0.1:3999\r\n`.
const HOST_LINE_LEN: usize = b"host: ".len() + HOST.len() + b"\r\n".len();
/// The filler line's bytes besides its value: `x-fill: ` and CRLF.
const FILL_OVERHEAD: usize = b"x-fill: ".len() + b"\r\n".len();

/// Every generated method, in wire order.
fn all_methods() -> impl Iterator<Item = Method> {
    (0..).map_while(Method::from_wire)
}
/// Every name the writer refuses (note §6.4): framing, routing and hop-by-hop names, the `proxy-` prefix, and the
/// underscore spelling of every forbidden or singleton name.
const FORBIDDEN: [&str; 23] = [
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
    "trailer",
    "http2-settings",
    "http2_settings",
    "proxy-x",
    "proxy_x",
    "proxy-anything-else",
];
const SINGLETONS: [&str; 4] = ["authorization", "cookie", "content-type", "user-agent"];

fn template(fields: &[u8]) -> HeaderTemplate {
    new_template(HOST, fields, &[]).expect("valid template")
}

/// `HeaderTemplate::new` with the line count of honestly serialised `fields`.
fn new_template(
    host: &[u8],
    fields: &[u8],
    sensitive: &[&[u8]],
) -> Result<HeaderTemplate, ErrorCode> {
    HeaderTemplate::new(host, fields, line_count(fields), sensitive)
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
            new_template(HOST, fields, &[]).expect_err("invalid"),
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
fn post_put_patch_query_send_content_length_even_when_empty_and_stay_reusable() {
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
        // `content-type` because OPTIONS content requires it (RFC 9110 §9.3.7).
        let request = parts(method, b"/x", b"content-type: text/plain\r\n", b"abc");
        let (out, reuse) = written(&request, &template(b""));
        let expected = format!(
            "{} /x HTTP/1.1\r\nhost: 127.0.0.1:3999\r\ncontent-type: text/plain\r\ncontent-length: 3\r\n\
             connection: close\r\n\r\nabc",
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
    for method in all_methods() {
        let body: &[u8] = if method == Method::Head { b"" } else { b"1" };
        let request = parts(method, b"/", b"content-type: text/plain\r\n", body);
        let (out, _) = written(&request, &template(b""));
        assert!(out.starts_with(format!("{} / HTTP/1.1\r\n", method.as_str()).as_bytes()));
    }
}

#[test]
fn method_from_wire_rejects_unknown_id() {
    // An unknown id is unrepresentable, so it never reaches the writer; the codec's `Engine(Internal)` for it is
    // tested with the codec (Task 6, note §9.1).
    assert_eq!(Method::from_wire(99), None);
}

// ---------------------------------------------------------------- origin binding and templates

/// Connection authorities that differ from `example.com:8080` by port, case, a trailing dot or a missing port.
const OTHER_AUTHORITIES: [&[u8]; 4] = [
    b"example.com:8081",
    b"Example.com:8080",
    b"example.com.:8080",
    b"example.com",
];

#[test]
fn rejects_template_for_other_authority() {
    let template = new_template(b"example.com:8080", b"", &[]).expect("valid");
    let request = parts(Method::Get, b"/", b"", b"");
    for other in OTHER_AUTHORITIES {
        let mut out = Vec::new();
        assert_eq!(
            write_request(&mut out, &request, &template, other),
            Err(INTERNAL)
        );
    }
    let mut out = Vec::new();
    assert!(write_request(&mut out, &request, &template, b"example.com:8080").is_ok());
}

const INVALID_HOSTS: [&[u8]; 35] = [
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
    b"[.]",
    b"[:]",
    b"[ffff]",
    b"[1.2.3.4]",
    b"[1:2:3:4:5:6:7:8:9:a:b]",
    b".",
    b".a",
    b"a..b",
    b"A.EXAMPLE",
    b"example.Com",
    b"a:00080",
    b"a:0",
    b"a:080",
];

#[test]
fn template_rejects_invalid_host() {
    for host in INVALID_HOSTS {
        assert_eq!(
            new_template(host, b"", &[]).expect_err("invalid"),
            INTERNAL,
            "{}",
            host.escape_ascii()
        );
    }
}

const VALID_HOSTS: [&[u8]; 9] = [
    b"example.com",
    b"127.0.0.1:3999",
    b"[::1]:8080",
    b"[::ffff:1.2.3.4]",
    b"xn--bcher-kva.example",
    b"a,b.example",
    b"a:1",
    b"a:65535",
    b"example.com.",
];

#[test]
fn template_accepts_hosts() {
    for host in VALID_HOSTS {
        let template = new_template(host, b"", &[]).expect("valid");
        assert_eq!(template.authority(), host);
    }
}

#[test]
fn template_rejects_forbidden_defaults() {
    for name in FORBIDDEN {
        for spelled in cases_of(name) {
            let fields = format!("{spelled}: v\r\n");
            assert_eq!(
                new_template(HOST, fields.as_bytes(), &[]).expect_err("forbidden"),
                INTERNAL,
                "{spelled}"
            );
        }
    }
}

const SENSITIVE: [&[u8]; 5] = [
    b"authorization",
    b"cookie",
    b"x-api-key",
    b"x-custom-secret",
    b"X_Token",
];

/// Default fields that hold a `SENSITIVE` name in another case or underscore spelling.
const SENSITIVE_FIELDS: [&[u8]; 7] = [
    b"Authorization: t\r\n",
    b"cookie: s=1\r\n",
    b"X-Api-Key: k\r\n",
    b"a: 1\r\nx-custom-secret: z\r\n",
    b"x_api_key: k\r\n",
    b"X_Custom_Secret: z\r\n",
    b"x-token: t\r\n",
];

#[test]
fn template_sensitive_flag() {
    for fields in SENSITIVE_FIELDS {
        let template = new_template(HOST, fields, &SENSITIVE).expect("valid");
        assert!(template.is_sensitive(), "{}", fields.escape_ascii());
    }
    for fields in [&b""[..], b"accept: */*\r\n", b"x-api: k\r\n"] {
        assert!(
            !new_template(HOST, fields, &SENSITIVE)
                .expect("valid")
                .is_sensitive()
        );
    }
}

#[test]
fn template_too_large() {
    let max = H1_LIMITS.max_request_fields_bytes;
    assert!(new_template(HOST, &fill_template(max), &[]).is_ok());
    assert_eq!(
        new_template(HOST, &fill_template(max + 1), &[]).expect_err("large"),
        HEADERS_TOO_LARGE
    );
    let host = vec![b'a'; max];
    assert_eq!(
        new_template(&host, b"", &[]).expect_err("large"),
        HEADERS_TOO_LARGE
    );
}

#[test]
fn template_is_send_and_sync() {
    // Task 6's ShardConfig moves the template from the control plane to its shard thread (note §6.2, X1).
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<HeaderTemplate>();
}

#[test]
fn template_debug_hides_bytes() {
    let template = new_template(
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
        for spelled in cases_of(name) {
            let twice = format!("{name}: a\r\nx-b: 1\r\n{spelled}: b\r\n");
            let request = parts(Method::Get, b"/", twice.as_bytes(), b"");
            assert_eq!(rejected(&request, &template(b"")), INTERNAL, "{spelled}");
            let in_template = format!("{name}: a\r\n{spelled}: b\r\n");
            assert_eq!(
                new_template(HOST, in_template.as_bytes(), &[]).expect_err("twice"),
                INTERNAL,
                "{spelled}"
            );
        }
    }
}

#[test]
fn distinct_singletons_and_repeated_other_names_are_accepted() {
    // The writer enforces only the singleton subset; one line per non-list name is the P2 contract (note §10.4).
    let all = b"authorization: a\r\ncookie: c\r\ncontent-type: t\r\nuser-agent: u\r\naccept: 1\r\naccept: 2\r\n";
    assert!(write(&parts(Method::Post, b"/", all, b""), &template(b"")).is_ok());
    let range_twice = b"range: bytes=0-1\r\nrange: bytes=5-9\r\n";
    assert!(write(&parts(Method::Get, b"/", range_twice, b""), &template(b"")).is_ok());
    let repeat_of_default = parts(Method::Get, b"/", b"x-api-key: b\r\n", b"");
    assert!(write(&repeat_of_default, &template(b"x-api-key: a\r\n")).is_ok());
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
        assert_eq!(new_template(HOST, headers, &[]).expect_err("bad"), INTERNAL);
    }
}

/// Field lines with obs-text, HTAB, OWS kept as given, and an empty value.
const VALID_VALUE_LINES: [&[u8]; 4] = [
    b"x-a: \x80\xff\r\n",
    b"x-a: a\tb\r\n",
    b"x-a:  \t v \t\r\n",
    b"x-a: \r\n",
];

#[test]
fn accepts_obs_text_htab_ows_and_empty_values() {
    for headers in VALID_VALUE_LINES {
        let (out, _) = written(&parts(Method::Get, b"/", headers, b""), &template(b""));
        assert!(
            out.windows(headers.len()).any(|w| w == headers),
            "sent as given"
        );
    }
}

// ---------------------------------------------------------------- target (§6.3)

/// Targets that are not origin-form as note §6.3 defines it.
const INVALID_TARGETS: [&[u8]; 15] = [
    b"http://evil/",
    b"*",
    b"?x",
    b"",
    b"a/b",
    b"#",
    b"/a#",
    b"/#b",
    b"/a#b",
    b"/a\"b",
    b"/a<b",
    b"/a>b",
    b"/a\\b",
    b"/\\evil.example",
    b"/a?q=\"",
];
/// Origin-form targets, including `//` (note §6.3 decision), percent-encoded data and raw query bytes WHATWG keeps.
const VALID_TARGETS: [&[u8]; 5] = [
    b"//evil.example/x",
    b"/",
    b"/a%0d%0a?b=c&d=~!$'()*+,;=:@",
    b"/a?q=\\^`{|}",
    // A second `?` stays inside the query, so the backslash after it is query data.
    b"/a?x?\\y",
];

#[test]
fn target_must_be_origin_form() {
    for target in INVALID_TARGETS {
        assert_eq!(
            rejected(&parts(Method::Get, target, b"", b""), &template(b"")),
            INTERNAL
        );
    }
    for target in VALID_TARGETS {
        assert!(write(&parts(Method::Get, target, b"", b""), &template(b"")).is_ok());
    }
}

#[test]
fn target_limit() {
    let max = H1_LIMITS.max_request_target_bytes;
    assert!(
        write(
            &parts(Method::Get, &target_of(max), b"", b""),
            &template(b"")
        )
        .is_ok()
    );
    assert_eq!(
        rejected(
            &parts(Method::Get, &target_of(max + 1), b"", b""),
            &template(b"")
        ),
        URL_TOO_LARGE
    );
}

// ---------------------------------------------------------------- limits and precedence (§7)

/// A target of exactly `len` bytes: `/` and `a` bytes.
fn target_of(len: usize) -> Vec<u8> {
    [&b"/"[..], &vec![b'a'; len - 1]].concat()
}

/// Template fields that make the host line + fields exactly `total` bytes.
fn fill_template(total: usize) -> Vec<u8> {
    fill_delta(total)
}

/// A delta that makes template + delta exactly `total` bytes with the host-only template.
fn fill_delta(total: usize) -> Vec<u8> {
    [
        &b"x-fill: "[..],
        &vec![b'a'; total - HOST_LINE_LEN - FILL_OVERHEAD],
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

/// One way a request breaks a §7 check; `error_precedence` combines two and expects the earlier row's code.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Violation {
    BodyOverLimit,
    TargetOverLimit,
    FieldsOverLimit,
    OtherAuthority,
    HeadWithBody,
    BadTarget,
    BadDelta,
}

/// Pairs of §7 rows whose codes differ, each with the code of the earlier row.
const PRECEDENCE: [(Violation, Violation, ErrorCode); 8] = [
    (
        Violation::BodyOverLimit,
        Violation::TargetOverLimit,
        INTERNAL,
    ),
    (
        Violation::BodyOverLimit,
        Violation::FieldsOverLimit,
        INTERNAL,
    ),
    (
        Violation::TargetOverLimit,
        Violation::FieldsOverLimit,
        URL_TOO_LARGE,
    ),
    (
        Violation::TargetOverLimit,
        Violation::OtherAuthority,
        URL_TOO_LARGE,
    ),
    (
        Violation::TargetOverLimit,
        Violation::BadTarget,
        URL_TOO_LARGE,
    ),
    (
        Violation::FieldsOverLimit,
        Violation::OtherAuthority,
        HEADERS_TOO_LARGE,
    ),
    (
        Violation::FieldsOverLimit,
        Violation::HeadWithBody,
        HEADERS_TOO_LARGE,
    ),
    (
        Violation::FieldsOverLimit,
        Violation::BadDelta,
        HEADERS_TOO_LARGE,
    ),
];

/// The outcome of a valid `POST /a` with one byte of body after both violations are applied.
fn violating(first: Violation, second: Violation) -> Result<Reuse, ErrorCode> {
    let has = |v| first == v || second == v;
    let method = if has(Violation::HeadWithBody) {
        Method::Head
    } else {
        Method::Post
    };
    let mut target = if has(Violation::TargetOverLimit) {
        vec![b'a'; H1_LIMITS.max_request_target_bytes]
    } else {
        b"a".to_vec()
    };
    target.insert(
        0,
        if has(Violation::BadTarget) {
            b'*'
        } else {
            b'/'
        },
    );
    let mut headers = if has(Violation::FieldsOverLimit) {
        fill_delta(H1_LIMITS.max_request_fields_bytes + 1)
    } else {
        Vec::new()
    };
    if has(Violation::BadDelta) {
        headers.extend_from_slice(b"host: x\r\n");
    }
    let body_len = if has(Violation::BodyOverLimit) {
        H1_LIMITS.max_copied_body_bytes + 1
    } else {
        1
    };
    let authority: &[u8] = if has(Violation::OtherAuthority) {
        b"other"
    } else {
        HOST
    };
    let body = vec![b'b'; body_len];
    let request = parts(method, &target, &headers, &body);
    write_request(&mut Vec::new(), &request, &template(b""), authority)
}

#[test]
fn error_precedence() {
    for (first, second, expected) in PRECEDENCE {
        assert_eq!(
            violating(first, second),
            Err(expected),
            "{first:?} before {second:?}"
        );
        for single in [first, second] {
            assert!(
                violating(single, single).is_err(),
                "{single:?} alone is an error"
            );
        }
    }
    let long_cr = [&b"/\r"[..], &vec![b'a'; H1_LIMITS.max_request_target_bytes]].concat();
    assert_eq!(
        rejected(&parts(Method::Get, &long_cr, b"", b""), &template(b"")),
        URL_TOO_LARGE
    );
}

#[test]
fn template_rejects_injected_line_by_count() {
    // A JS default value `1\r\nauthorization: Basic evil` serialised as one field.
    let fields = b"x-a: 1\r\nauthorization: Basic evil\r\n";
    for lines in [0, 1, 3, u16::MAX] {
        assert_eq!(
            HeaderTemplate::new(HOST, fields, lines, &[]).expect_err("count"),
            INTERNAL,
            "{lines}"
        );
    }
    assert!(HeaderTemplate::new(HOST, fields, 2, &[]).is_ok());
    assert!(HeaderTemplate::new(HOST, b"", 0, &[]).is_ok());
    assert_eq!(
        HeaderTemplate::new(HOST, b"", 1, &[]).expect_err("count"),
        INTERNAL
    );
}

#[test]
fn options_with_body_requires_content_type() {
    // RFC 9110 §9.3.7: a client that sends content in OPTIONS MUST send a valid Content-Type.
    let body_only = parts(Method::Options, b"/", b"", b"xyz");
    assert_eq!(rejected(&body_only, &template(b"")), INTERNAL);
    let in_delta = parts(
        Method::Options,
        b"/",
        b"Content-Type: text/plain\r\n",
        b"xyz",
    );
    assert!(write(&in_delta, &template(b"")).is_ok());
    assert!(write(&body_only, &template(b"content-type: text/plain\r\n")).is_ok());
    assert!(write(&parts(Method::Options, b"/", b"", b""), &template(b"")).is_ok());
}

#[test]
fn max_p0_request_bytes_is_the_largest_request() {
    // OPTIONS is the longest method that may carry a body; with a body it adds `connection: close`.
    let target = [
        &b"/"[..],
        &vec![b'a'; H1_LIMITS.max_request_target_bytes - 1],
    ]
    .concat();
    // OPTIONS content needs a content-type (RFC 9110 §9.3.7); the filler line tops the fields up to the limit.
    let content_type = b"content-type: a\r\n";
    let fill = fill_delta(H1_LIMITS.max_request_fields_bytes - content_type.len());
    let delta = [&content_type[..], &fill].concat();
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
    all_methods()
        .find(|m| m.as_str() == case.method)
        .unwrap_or_else(|| panic!("{}: unknown method", case.name))
}

fn run_case(case: &RequestCase) -> RequestExpect {
    let template = match HeaderTemplate::new(
        &case.template_host,
        &case.template_fields,
        case.template_field_lines,
        &[],
    ) {
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
        Ok(reuse) => RequestExpect::Written {
            bytes: out,
            reuse: RequestReuse::from(reuse),
        },
        Err(e) => {
            assert_eq!(out, b"", "{}: an error leaves `out` cleared", case.name);
            RequestExpect::Error(e.to_string())
        }
    }
}

/// Every request vector file and its case count (docs/notes/vectors.md).
const VECTOR_FILES: [(&str, usize); 5] = [
    ("h1-request/blazehttp-rules.json", 270),
    ("h1-request/undici-selected.json", 38),
    ("h1-request/go-selected.json", 17),
    ("h1-request/httpguts-selected.json", 145),
    ("h1-request/hyper-selected.json", 5),
];

#[test]
fn injection_vectors() {
    let cases = load_suite::<RequestCase>(SUITE);
    for (file, count) in VECTOR_FILES {
        let loaded = cases.iter().filter(|(f, _)| f == file).count();
        assert_eq!(loaded, count, "{file}");
    }
    assert_eq!(
        cases.len(),
        VECTOR_FILES.iter().map(|(_, n)| n).sum::<usize>()
    );
    for (file, case) in cases {
        assert_eq!(run_case(&case), case.expect, "{file}/{}", case.name);
    }
}
