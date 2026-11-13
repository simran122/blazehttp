//! `write_request` / `HeaderTemplate` fuzz target (`docs/notes/request-writer.md` §9.5). Inputs are structure-aware
//! requests: a method id, a target (valid path bytes, raw bytes or a limit-boundary size), a template host (valid
//! shapes or raw bytes), template and delta field lines built from forbidden, singleton, token and raw names with
//! valid or raw values, and valid names, values and targets with one injected raw byte at any position (≤ 200
//! lines), declared line counts for template and delta that are the number of generated lines (never counted from
//! the bytes, X3) plus an optional skew, a body length (incl. the 64 KiB boundary), filler that puts template or
//! template + delta at the 64 KiB boundary, a subset of sensitive names, and the connection's authority. Properties:
//! 1. **exact outcome:** an independent spec model (plain per-byte loops over RFC 9110/9112 + note §6–§7) gives the
//!    same result, error code included — the writer is neither too lenient nor too strict;
//! 2. on `Ok`: the bytes equal a naive reference serializer; httparse 1.10.1 parses them as one complete request
//!    with the same method, target, fields (values OWS-trimmed on both sides) and body; exactly one `host` (first),
//!    at most one `content-length` equal to the body length, no `transfer-encoding`/`expect`, `connection` only as
//!    the generated `close`, no other design §12 forbidden name in any spelling, singletons at most once, and
//!    `is_sensitive` as the model says; every head byte is VCHAR/SP/HTAB/obs-text or a CR LF pair;
//!    the request line has exactly two SP; the first CRLF CRLF ends the head; `Reuse` follows the §6.5 table;
//! 3. on `Err`: `out` is empty.
//!
//! Run: `cargo +nightly-2026-11-06 fuzz run h1_writer -- -dict=h1_writer.dict -use_value_profile=1 -max_len=200000
//! -rss_limit_mb=2048` (inside the capped container, never on the host).

#![no_main]

use arbitrary::Arbitrary;
use engine_core::generated::spec::{EngineDetail, ErrorCode, H1_LIMITS, Method, TooLargeDetail};
use engine_core::h1::{HeaderTemplate, MAX_P0_REQUEST_BYTES, RequestParts, Reuse, write_request};
use libfuzzer_sys::fuzz_target;

const INTERNAL: ErrorCode = ErrorCode::Engine(EngineDetail::Internal);
const URL: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Url);
const HEADERS: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Headers);
const MAX_LINES: usize = 200;
/// No buffer of this harness may grow past this (owner rule: bounded test buffers, fail fast).
const BUFFER_CAP: usize = 64 << 20;
/// Names the generator spells (case masks, underscores); the `proxy-` ones exercise the prefix rule.
const GENERATED_FORBIDDEN: [&str; 13] = [
    "host",
    "transfer-encoding",
    "connection",
    "upgrade",
    "expect",
    "te",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "proxy-x",
    "content-length",
    "trailer",
    "http2-settings",
];
/// The model's forbidden set (design §12, note §6.4): these names plus every `proxy-` name.
const FORBIDDEN: [&str; 10] = [
    "host",
    "transfer-encoding",
    "connection",
    "upgrade",
    "expect",
    "te",
    "keep-alive",
    "content-length",
    "trailer",
    "http2-settings",
];
const SENSITIVE: [&[u8]; 4] = [b"authorization", b"x-api-key", b"X_Token", b"cookie"];
const SINGLETONS: [&str; 4] = ["authorization", "cookie", "content-type", "user-agent"];
const HOSTS: [&str; 6] = [
    "127.0.0.1:3999",
    "example.com",
    "[::1]:8080",
    "xn--bcher-kva.example:65535",
    "a,b.example",
    "h:1",
];
const METHODS: [Method; 8] = [
    Method::Get,
    Method::Head,
    Method::Post,
    Method::Put,
    Method::Delete,
    Method::Patch,
    Method::Options,
    Method::Query,
];

#[derive(Arbitrary, Debug)]
enum Name {
    /// A forbidden name; bit i of the mask upper-cases letter i, `underscore` writes `-` as `_`.
    Forbidden {
        index: u8,
        case: u32,
        underscore: bool,
    },
    Singleton {
        index: u8,
        case: u32,
        underscore: bool,
    },
    /// `x-` plus lowercase letters chosen from the bytes.
    Token(Vec<u8>),
    Raw(Vec<u8>),
    /// A token name with one raw byte inserted (position classes start / middle / end, note §9.2).
    Inject {
        name: Vec<u8>,
        at: u8,
        byte: u8,
    },
}

#[derive(Arbitrary, Debug)]
enum Value {
    /// Visible ASCII, SP, HTAB and obs-text only.
    Clean(Vec<u8>),
    Raw(Vec<u8>),
    /// A clean value with one raw byte inserted.
    Inject {
        value: Vec<u8>,
        at: u8,
        byte: u8,
    },
}

#[derive(Arbitrary, Debug)]
struct Line {
    name: Name,
    /// `None`: the normal `": "`; otherwise raw separator bytes.
    separator: Option<Vec<u8>>,
    value: Value,
    /// Usually `\r\n`; otherwise raw terminator bytes.
    terminator: Option<Vec<u8>>,
}

#[derive(Arbitrary, Debug)]
enum Target {
    /// `/` plus visible ASCII chosen from the bytes.
    Path(Vec<u8>),
    Raw(Vec<u8>),
    /// A valid path with one raw byte inserted after the leading `/`.
    Inject {
        path: Vec<u8>,
        at: u8,
        byte: u8,
    },
    /// `/` plus `a` bytes, total 16,383 + `extra` (0–3) bytes.
    Boundary {
        extra: u8,
    },
}

#[derive(Arbitrary, Debug)]
enum Host {
    Known(u8),
    Raw(Vec<u8>),
}

#[derive(Arbitrary, Debug)]
enum Body {
    Small(u16),
    /// 65,535 + `extra` (0–3) bytes.
    Boundary {
        extra: u8,
    },
}

#[derive(Arbitrary, Debug)]
struct Input {
    method: u8,
    target: Target,
    host: Host,
    template_lines: Vec<Line>,
    delta_lines: Vec<Line>,
    /// Added to the number of generated delta lines; `None` means JS counted right.
    line_skew: Option<i8>,
    /// Added to the number of generated template lines.
    template_line_skew: Option<i8>,
    /// Bit i selects `SENSITIVE[i]` as a configured sensitive name.
    sensitive: u8,
    body: Body,
    /// `true`: the connection's authority differs from the template host by one byte.
    other_authority: bool,
    /// Pads the delta with a filler line so template + delta is 65,534 + `extra` (0–3) bytes.
    fill_fields: Option<u8>,
    /// Pads the template with a filler line so the host line + fields is 65,534 + `extra` (0–3) bytes.
    fill_template: Option<u8>,
}

/// One request as bytes, built from an `Input`.
struct Request {
    method: Method,
    target: Vec<u8>,
    host: Vec<u8>,
    fields: Vec<u8>,
    field_lines: u16,
    headers: Vec<u8>,
    header_lines: u16,
    body: Vec<u8>,
    authority: Vec<u8>,
    sensitive: Vec<&'static [u8]>,
}

fn spell(canonical: &str, case: u32, underscore: bool) -> Vec<u8> {
    canonical
        .bytes()
        .enumerate()
        .map(|(i, b)| match b {
            b'-' if underscore => b'_',
            _ if case >> (i % 32) & 1 == 1 => b.to_ascii_uppercase(),
            _ => b,
        })
        .collect()
}

/// `x-` plus lowercase letters chosen from the bytes.
fn token(bytes: &[u8]) -> Vec<u8> {
    [
        &b"x-"[..],
        &bytes.iter().map(|b| b'a' + b % 26).collect::<Vec<_>>(),
    ]
    .concat()
}

/// Field content only: control bytes and DEL become SP.
fn clean(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|&b| if b < 0x20 || b == 0x7F { b' ' } else { b })
        .collect()
}

/// `/` plus visible ASCII chosen from the bytes.
fn path(bytes: &[u8]) -> Vec<u8> {
    [
        &b"/"[..],
        &bytes.iter().map(|b| 0x21 + b % 94).collect::<Vec<_>>(),
    ]
    .concat()
}

/// `bytes` with `byte` inserted at `at` modulo its length plus one (start, middle or end).
fn inject(mut bytes: Vec<u8>, at: u8, byte: u8) -> Vec<u8> {
    let at = usize::from(at) % (bytes.len() + 1);
    bytes.insert(at, byte);
    bytes
}

fn name_bytes(name: &Name) -> Vec<u8> {
    match name {
        Name::Forbidden {
            index,
            case,
            underscore,
        } => spell(
            GENERATED_FORBIDDEN[usize::from(*index) % GENERATED_FORBIDDEN.len()],
            *case,
            *underscore,
        ),
        Name::Singleton {
            index,
            case,
            underscore,
        } => spell(
            SINGLETONS[usize::from(*index) % SINGLETONS.len()],
            *case,
            *underscore,
        ),
        Name::Token(bytes) => token(bytes),
        Name::Raw(bytes) => bytes.clone(),
        Name::Inject { name, at, byte } => inject(token(name), *at, *byte),
    }
}

fn line_bytes(line: &Line) -> Vec<u8> {
    let value = match &line.value {
        Value::Clean(bytes) => clean(bytes),
        Value::Raw(bytes) => bytes.clone(),
        Value::Inject { value, at, byte } => inject(clean(value), *at, *byte),
    };
    let separator = line.separator.as_deref().unwrap_or(b": ");
    let terminator = line.terminator.as_deref().unwrap_or(b"\r\n");
    [&name_bytes(&line.name)[..], separator, &value, terminator].concat()
}

/// The serialised lines and how many lines were generated (what JS would count, X3).
fn block(lines: &[Line]) -> (Vec<u8>, usize) {
    let lines = &lines[..lines.len().min(MAX_LINES)];
    (lines.iter().flat_map(line_bytes).collect(), lines.len())
}

/// Appends a filler line so `prefix + block` is 65,534 + `extra % 4` bytes (or as close as the filler allows).
fn fill(block: &mut Vec<u8>, lines: &mut usize, prefix: usize, extra: u8) {
    let want = H1_LIMITS.max_request_fields_bytes - 2 + usize::from(extra % 4);
    let fill = want.saturating_sub(prefix + block.len() + 10);
    block.extend_from_slice(&[&b"x-fill: "[..], &vec![b'f'; fill], b"\r\n"].concat());
    *lines += 1;
}

/// `lines` plus an optional skew, as the `u16` the writer takes (clamped).
fn declared(lines: usize, skew: Option<i8>) -> u16 {
    let lines = i64::try_from(lines).unwrap_or(0) + skew.map_or(0, i64::from);
    u16::try_from(lines.max(0)).unwrap_or(u16::MAX)
}

fn target_bytes(target: &Target) -> Vec<u8> {
    match target {
        Target::Path(bytes) => path(bytes),
        Target::Raw(bytes) => bytes.clone(),
        Target::Inject {
            path: bytes,
            at,
            byte,
        } => {
            let path = path(bytes);
            let at = 1 + usize::from(*at) % path.len();
            [&path[..at], &[*byte], &path[at..]].concat()
        }
        Target::Boundary { extra } => {
            let len = H1_LIMITS.max_request_target_bytes - 1 + usize::from(extra % 4);
            [&b"/"[..], &vec![b'a'; len - 1]].concat()
        }
    }
}

impl Request {
    fn from(input: &Input) -> Self {
        let host = match &input.host {
            Host::Known(i) => HOSTS[usize::from(*i) % HOSTS.len()].as_bytes().to_vec(),
            Host::Raw(bytes) => bytes.clone(),
        };
        let (mut fields, mut field_count) = block(&input.template_lines);
        if let Some(extra) = input.fill_template {
            fill(&mut fields, &mut field_count, 8 + host.len(), extra);
        }
        let (mut headers, mut header_count) = block(&input.delta_lines);
        if let Some(extra) = input.fill_fields {
            fill(
                &mut headers,
                &mut header_count,
                8 + host.len() + fields.len(),
                extra,
            );
        }
        let body_len = match input.body {
            Body::Small(n) => usize::from(n % 4096),
            Body::Boundary { extra } => {
                H1_LIMITS.max_copied_body_bytes - 1 + usize::from(extra % 4)
            }
        };
        let mut authority = host.clone();
        if input.other_authority {
            authority.push(b'.');
        }
        Self {
            method: METHODS[usize::from(input.method) % METHODS.len()],
            target: target_bytes(&input.target),
            host,
            fields,
            field_lines: declared(field_count, input.template_line_skew),
            headers,
            header_lines: declared(header_count, input.line_skew),
            body: vec![b'b'; body_len],
            authority,
            sensitive: (0..SENSITIVE.len())
                .filter(|i| input.sensitive >> i & 1 == 1)
                .map(|i| SENSITIVE[i])
                .collect(),
        }
    }
}

// ---------------------------------------------------------------- spec model (independent of the writer)

fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn is_field_content(b: u8) -> bool {
    b == b'\t' || (0x20..0x7F).contains(&b) || b >= 0x80
}

/// The names of a block's lines, or `None` when a line breaks `token ": " *field-content CRLF`.
fn spec_names(block: &[u8]) -> Option<Vec<&[u8]>> {
    let mut names = Vec::new();
    let mut i = 0;
    while i < block.len() {
        let start = i;
        while i < block.len() && is_tchar(block[i]) {
            i += 1;
        }
        if i == start || block.get(i..i + 2) != Some(b": ") {
            return None;
        }
        names.push(&block[start..i]);
        i += 2;
        while i < block.len() && is_field_content(block[i]) {
            i += 1;
        }
        if block.get(i..i + 2) != Some(b"\r\n") {
            return None;
        }
        i += 2;
    }
    Some(names)
}

/// Lowercase canonical name and whether the original used `_`.
fn canonical(name: &[u8]) -> (String, bool) {
    let text: String = name
        .iter()
        .map(|&b| char::from(b.to_ascii_lowercase()))
        .collect();
    (text.replace('_', "-"), name.contains(&b'_'))
}

/// Whether a canonical name is forbidden: the §12 set or any `proxy-` name.
fn spec_forbidden(canon: &str) -> bool {
    FORBIDDEN.contains(&canon) || canon.starts_with("proxy-")
}

/// Adds the block's singleton names to `seen`; `false` for a forbidden name or a repeat.
fn spec_admit(names: &[&[u8]], seen: &mut Vec<String>) -> bool {
    for name in names {
        let (canon, underscore) = canonical(name);
        let singleton = SINGLETONS.contains(&canon.as_str());
        if spec_forbidden(&canon) || (singleton && underscore) {
            return false;
        }
        if singleton {
            if seen.contains(&canon) {
                return false;
            }
            seen.push(canon);
        }
    }
    true
}

/// A reg-name as WHATWG serialises it: lowercase `unreserved`/`sub-delims` without `%`, labels non-empty except
/// one trailing dot.
fn spec_reg_name(name: &[u8]) -> bool {
    let bytes_ok = name
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-._~!$&'()*+,;=".contains(&b));
    let text = String::from_utf8_lossy(name);
    let labels: Vec<&str> = text.split('.').collect();
    let labels_ok = labels
        .iter()
        .enumerate()
        .all(|(i, l)| !l.is_empty() || (i == labels.len() - 1 && i > 0));
    !name.is_empty() && bytes_ok && labels_ok
}

/// The WHATWG serialisation of an IPv6 address, modelled on `Ipv6Addr`'s RFC 5952 `Display`, which equals it except
/// for the dotted tail of IPv4-mapped addresses.
fn whatwg_ipv6(address: std::net::Ipv6Addr) -> String {
    match address.to_ipv4_mapped() {
        Some(_) => {
            let [.., high, low] = address.segments();
            format!("::ffff:{high:x}:{low:x}")
        }
        None => address.to_string(),
    }
}

fn spec_host(host: &[u8]) -> bool {
    let (name_ok, rest) = if host.first() == Some(&b'[') {
        match host.iter().position(|&b| b == b']') {
            Some(close) => {
                let inner = std::str::from_utf8(&host[1..close]).unwrap_or("");
                (
                    inner
                        .parse::<std::net::Ipv6Addr>()
                        .is_ok_and(|address| whatwg_ipv6(address) == inner),
                    &host[close + 1..],
                )
            }
            None => (false, &host[..0]),
        }
    } else {
        let n = host.iter().position(|&b| b == b':').unwrap_or(host.len());
        (spec_reg_name(&host[..n]), &host[n..])
    };
    let port = rest.strip_prefix(b":");
    let port_ok = rest.is_empty()
        || port.is_some_and(|p| {
            (1..=5).contains(&p.len())
                && p[0] != b'0'
                && p.iter().all(u8::is_ascii_digit)
                && std::str::from_utf8(p)
                    .ok()
                    .and_then(|p| p.parse::<u32>().ok())
                    .is_some_and(|p| p <= 65_535)
        });
    name_ok && port_ok
}

/// Whether two names are equal with ASCII case folded and `_` read as `-` on both sides.
fn spec_same_name(a: &[u8], b: &[u8]) -> bool {
    let fold = |s: &[u8]| -> Vec<u8> {
        s.iter()
            .map(|&c| {
                if c == b'_' {
                    b'-'
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect()
    };
    fold(a) == fold(b)
}

/// The template's outcome per note §7 T1–T2: its singleton names and whether it is sensitive.
fn spec_template(r: &Request) -> Result<(Vec<String>, bool), ErrorCode> {
    if 6 + r.host.len() + 2 + r.fields.len() > H1_LIMITS.max_request_fields_bytes {
        return Err(HEADERS);
    }
    let mut seen = Vec::new();
    let names = spec_names(&r.fields).ok_or(INTERNAL)?;
    let count_ok = names.len() == usize::from(r.field_lines);
    if !spec_host(&r.host) || !spec_admit(&names, &mut seen) || !count_ok {
        return Err(INTERNAL);
    }
    let sensitive = names
        .iter()
        .any(|n| r.sensitive.iter().any(|s| spec_same_name(n, s)));
    Ok((seen, sensitive))
}

/// Origin-form per note §6.3: `/` first; visible ASCII except `#`, `"`, `<`, `>`; no `\` before the first `?`.
fn spec_target(target: &[u8]) -> bool {
    let path_end = target
        .iter()
        .position(|&b| b == b'?')
        .unwrap_or(target.len());
    let (path, query) = target.split_at(path_end);
    target.first() == Some(&b'/')
        && target
            .iter()
            .all(|&b| (0x21..=0x7E).contains(&b) && !b"#\"<>".contains(&b))
        && !path.iter().any(|b| b"\\^`{}".contains(b))
        && !query.contains(&b'\'')
}

/// `write_request`'s outcome per note §7 checks 1–7, given the template's singleton names.
fn spec_write(r: &Request, mut seen: Vec<String>) -> Result<(), ErrorCode> {
    let template_len = 6 + r.host.len() + 2 + r.fields.len();
    if r.body.len() > H1_LIMITS.max_copied_body_bytes {
        return Err(INTERNAL);
    }
    if r.target.len() > H1_LIMITS.max_request_target_bytes {
        return Err(URL);
    }
    if template_len + r.headers.len() > H1_LIMITS.max_request_fields_bytes {
        return Err(HEADERS);
    }
    let head_with_body = r.method == Method::Head && !r.body.is_empty();
    if r.authority != r.host || head_with_body || !spec_target(&r.target) {
        return Err(INTERNAL);
    }
    let names = spec_names(&r.headers).ok_or(INTERNAL)?;
    if !spec_admit(&names, &mut seen) || names.len() != usize::from(r.header_lines) {
        return Err(INTERNAL);
    }
    // RFC 9110 §9.3.7: OPTIONS content needs a content-type (template or delta).
    let content_type = seen.iter().any(|s| s == "content-type");
    if r.method == Method::Options && !r.body.is_empty() && !content_type {
        return Err(INTERNAL);
    }
    Ok(())
}

/// (send `content-length`, send `connection: close`) per note §6.5.
fn spec_framing(method: Method, body_len: usize) -> (bool, bool) {
    let defines_content = matches!(
        method,
        Method::Post | Method::Put | Method::Patch | Method::Query
    );
    let close = body_len > 0 && matches!(method, Method::Get | Method::Delete | Method::Options);
    (defines_content || close, close)
}

/// A deliberately naive serializer of an accepted request.
fn reference_bytes(r: &Request) -> Vec<u8> {
    let (length, close) = spec_framing(r.method, r.body.len());
    let mut out = format!("{} ", r.method.as_str()).into_bytes();
    out.extend(&r.target);
    out.extend(b" HTTP/1.1\r\nhost: ");
    out.extend(&r.host);
    out.extend(b"\r\n");
    out.extend(&r.fields);
    out.extend(&r.headers);
    if length {
        out.extend(format!("content-length: {}\r\n", r.body.len()).into_bytes());
    }
    if close {
        out.extend(b"connection: close\r\n");
    }
    out.extend(b"\r\n");
    out.extend(&r.body);
    out
}

// ---------------------------------------------------------------- properties on accepted output

/// Every head byte is VCHAR, SP, HTAB or obs-text, or a CR immediately followed by LF; two SP in the request line.
fn check_head_bytes(head: &[u8]) {
    for (i, &b) in head.iter().enumerate() {
        match b {
            b'\r' => assert_eq!(head.get(i + 1), Some(&b'\n'), "CR only before LF"),
            b'\n' => assert_eq!(
                head.get(i.wrapping_sub(1)),
                Some(&b'\r'),
                "LF only after CR"
            ),
            _ => assert!(is_field_content(b), "byte {b:#04x} at {i}"),
        }
    }
    let line_end = head
        .windows(2)
        .position(|w| w == b"\r\n")
        .expect("request line");
    assert_eq!(
        head[..line_end].split(|&b| b == b' ').count(),
        3,
        "two SP in the request line"
    );
}

fn trim_ows(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|&b| b != b' ' && b != b'\t')
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|&b| b != b' ' && b != b'\t')
        .map_or(start, |e| e + 1);
    &value[start..end.max(start)]
}

/// The expected field lines `(name, OWS-trimmed value)` of a block.
fn expected_fields(block: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    block
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let line = line.strip_suffix(b"\r").expect("CRLF");
            let colon = line.iter().position(|&b| b == b':').expect("colon");
            (
                line[..colon].to_vec(),
                trim_ows(&line[colon + 1..]).to_vec(),
            )
        })
        .collect()
}

/// httparse parses `out` as one complete request matching `r`.
fn check_oracle(out: &[u8], r: &Request) -> usize {
    let crlfs = |b: &[u8]| b.windows(2).filter(|w| w == b"\r\n").count();
    // host + template + delta + content-length + connection, plus one spare slot.
    let mut headers = vec![httparse::EMPTY_HEADER; 4 + crlfs(&r.fields) + crlfs(&r.headers)];
    let mut request = httparse::Request::new(&mut headers);
    let status = request.parse(out).expect("httparse accepts our output");
    let httparse::Status::Complete(n) = status else {
        panic!("httparse: partial head")
    };
    assert_eq!(request.method, Some(r.method.as_str()));
    assert_eq!(request.path.map(str::as_bytes), Some(&r.target[..]));
    let got: Vec<(Vec<u8>, Vec<u8>)> = request
        .headers
        .iter()
        .map(|h| (h.name.as_bytes().to_vec(), trim_ows(h.value).to_vec()))
        .collect();
    let mut want =
        expected_fields(&[&b"host: "[..], &r.host, b"\r\n", &r.fields, &r.headers].concat());
    let (length, close) = spec_framing(r.method, r.body.len());
    if length {
        want.push((
            b"content-length".to_vec(),
            r.body.len().to_string().into_bytes(),
        ));
    }
    if close {
        want.push((b"connection".to_vec(), b"close".to_vec()));
    }
    assert_eq!(got, want, "fields");
    assert_eq!(&out[n..], &r.body[..], "body after the head");
    n
}

/// Exactly one host (first), one generated content-length at most, `connection` only as the generated `close`, no
/// other design §12 forbidden name in any spelling (read from the output itself), singletons at most once.
fn check_field_counts(out: &[u8], head_len: usize, r: &Request) {
    let fields = expected_fields(
        &out[..head_len - 2][out.windows(2).position(|w| w == b"\r\n").unwrap() + 2..],
    );
    let count = |n: &str| {
        fields
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(n.as_bytes()))
            .count()
    };
    assert!(fields[0].0 == b"host" && count("host") == 1);
    assert!(
        count("content-length") <= 1 && count("transfer-encoding") == 0 && count("expect") == 0
    );
    for (name, value) in &fields {
        if name.eq_ignore_ascii_case(b"content-length") {
            assert_eq!(value, r.body.len().to_string().as_bytes());
        }
        if name.eq_ignore_ascii_case(b"connection") {
            assert_eq!(value, b"close");
        }
    }
    for (name, _) in &fields[1..] {
        let (canon, underscore) = canonical(name);
        let generated = name == b"content-length" || name == b"connection";
        assert!(
            generated || !spec_forbidden(&canon),
            "forbidden field {}",
            name.escape_ascii()
        );
        assert!(
            !(underscore && SINGLETONS.contains(&canon.as_str())),
            "underscore spelling of a singleton {}",
            name.escape_ascii()
        );
    }
    // Every spelling counts towards its singleton: `Content-Type` and `content_type` are one field.
    let spellings = |s: &str| {
        fields
            .iter()
            .filter(|(name, _)| canonical(name).0 == s)
            .count()
    };
    assert!(
        SINGLETONS.iter().all(|s| spellings(s) <= 1),
        "a singleton repeated"
    );
}

fn check_accepted(out: &[u8], reuse: Reuse, r: &Request) {
    assert_eq!(out, reference_bytes(r), "reference serializer");
    let head_len = check_oracle(out, r);
    assert_eq!(
        out.windows(4).position(|w| w == b"\r\n\r\n"),
        Some(head_len - 4),
        "the first CRLF CRLF ends the head"
    );
    check_head_bytes(&out[..head_len]);
    check_field_counts(out, head_len, r);
    let (_, close) = spec_framing(r.method, r.body.len());
    assert_eq!(
        reuse,
        if close {
            Reuse::CloseAfterExchange
        } else {
            Reuse::Allowed
        }
    );
    assert!(out.len() <= MAX_P0_REQUEST_BYTES);
}

fuzz_target!(|input: Input| {
    let r = Request::from(&input);
    let template = HeaderTemplate::new(&r.host, &r.fields, r.field_lines, &r.sensitive);
    let model = spec_template(&r);
    assert_eq!(
        template.as_ref().err(),
        model.as_ref().err(),
        "template outcome"
    );
    let (Ok(template), Ok((seen, sensitive))) = (template, model) else {
        return;
    };
    assert_eq!(template.is_sensitive(), sensitive, "sensitive flag");
    let parts = RequestParts {
        method: r.method,
        target: &r.target,
        headers: &r.headers,
        header_lines: r.header_lines,
        body: &r.body,
    };
    let mut out = Vec::new();
    let result = write_request(&mut out, &parts, &template, &r.authority);
    let expected = spec_write(&r, seen);
    assert_eq!(
        result.as_ref().err(),
        expected.as_ref().err(),
        "write outcome for {parts:?}"
    );
    assert!(out.capacity() <= BUFFER_CAP, "bounded harness buffer");
    match result {
        Ok(reuse) => check_accepted(&out, reuse, &r),
        Err(_) => assert!(out.is_empty(), "an error leaves `out` empty"),
    }
});
