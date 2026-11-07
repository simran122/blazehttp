# Test vectors

Ported and own test cases are data files under `tests/vectors/` (design §6.1 step 3). `cargo test` runs them
against the engine now; Vitest and the conformance suite (native and undici) run the same files later.

## File format

```json
{
  "source": {
    "project": "llhttp",
    "url": "https://github.com/nodejs/llhttp/blob/<sha>/test/response/content-length.md",
    "tag": "v9.4.3",
    "commit": "<sha>",
    "licence": "MIT",
    "copyright": "<verbatim from the upstream licence file>"
  },
  "cases": [
    {
      "name": "content-length / Tabs in `Content-Length` (surrounding)",
      "input": "HTTP/1.1 200 OK\r\nContent-Length:\t42\t\r\n\r\n",
      "inputEncoding": "utf8",
      "request": { "method": "GET" },
      "expect": { "native": { "error": "PROTOCOL/incomplete-body" } },
      "reference": { "llhttp": "<llhttp's test log for this input, verbatim>" }
    }
  ]
}
```

Validated by `tools/vectors/schema.ts` (`loadVectorFile`, strict: unknown keys are errors) and read in Rust by
`crates/engine-core/tests/support/vectors.rs` (`load_suite`).

| Field | Meaning |
|---|---|
| `source` | Where the cases come from; `THIRD_PARTY_NOTICES` is generated from it. `tag` (optional) is the upstream release tag, `commit` the exact commit it points to, and `url` uses that commit. Own vectors use project `blazehttp`, licence `MIT OR Apache-2.0`, no `tag` (no notice). |
| `name` | Unique within the file. Imported cases are `<upstream file stem> / <upstream heading>`. |
| `input` | **Every byte the server sends; then the server closes the connection (EOF).** An empty input means the server closed before sending anything. |
| `inputEncoding` | How the case's byte strings — `input`, expected header names and values, and the body — become bytes. `utf8`: the UTF-8 encoding of the string. `latin1`: each code point U+0000–U+00FF is one byte (so a single obs-text byte such as `\x80` or `\xa0` can be written); used only when a byte ≥ 0x80 occurs, otherwise `utf8` (the TS loader rejects a `latin1` case without such a byte and any code point above U+00FF). |
| `request.method` | Method of the request this response answers; `HEAD` means no body (§5.2 Bodies). One of the generated `METHODS` (checked by both loaders). |
| `expect.native` | `{ "ok": … }` or `{ "error": "CODE/detail" }`. |
| `expect.native.error` | An error the engine can produce (a wire code), in its display form, exactly as `ErrorCode`'s `Display` (Rust) and `formatErrorCode` (TS) print it. TS checks against the generated `ENGINE_ERRORS` and Rust against `ErrorCode::ALL_SAMPLES`; specgen emits both from the same wire codes, so both loaders accept the same set (non-wire codes such as `HTTP_STATUS` or `PARSE/json` are rejected). |
| `expect.native.ok.status` | Status of the final (non-1xx) response, 100–599 (checked by the TS loader); interim 1xx heads are skipped. |
| `expect.native.ok.headers` | The final response's fields in wire order: name exactly as sent, value without surrounding OWS (RFC 9110 §5.5). |
| `expect.native.ok.body` | The body bytes, decoded with the case's `inputEncoding`: empty for HEAD/204/304; `Content-Length` bytes; or everything up to EOF when close-delimited. |
| `expect.native.ok.keepAlive` | Whether the response's own heads and framing allow reusing the connection: false when close-delimited, with `Connection: close` in the final head **or in any interim head of the exchange** (sticky, RFC 9112 §9.6), on HTTP/1.0 without `Connection: keep-alive` in the final head, or for a 204/304 carrying `Transfer-Encoding` or `Content-Length` > 0 unless the request was `HEAD` (accepted without a body, not reused; design §5.2 Bodies). Bytes left after the response end (pipelined data) are **not** represented here: §5.1 closes such a connection at the connection layer, which its own tests cover. |
| `expect.undici` | Optional in P0 and required from P4 (`removal: P4 (#5)`), when the undici transport runs the same vectors. |
| `reference` | Informational: what reference implementations did with the input, keyed by project. |
| `derivedFrom` | Informational provenance of a case re-derived from an upstream test: `"<file>:<test> @ <tag>"`, e.g. `"src/lib.rs:test_no_space_after_colon @ v1.10.1"`. |

The Rust loader reads `name`, `input`, `inputEncoding`, `request.method` and `expect.native` and decodes every byte
string with the case's encoding; the informational fields are validated by the TS loader only.

`tests/vectors/index.json` maps every vector file to the suites that consume it, e.g.
`{ "h1-response/llhttp-sample.json": ["engine-core:h1_response"] }`. `pnpm vectors:index`
(`tools/vectors/check-index.ts`, run in CI) fails on files missing from the index, index entries without a file and
entries without a suite. `load_suite` also panics on an unregistered `.json` file next to a suite's files and on a
file without cases. The Rust loader is a module of the integration tests (`tests/support/vectors.rs`, included as a
submodule as in [The Book §11.3, "Submodules in Integration Tests"](https://doc.rust-lang.org/book/ch11-03-test-organization.html#submodules-in-integration-tests)),
so it is not compiled into the crate; `serde`/`serde_json` are dev-dependencies only.

`THIRD_PARTY_NOTICES` is generated by `pnpm vectors:notices` (one block per distinct project, licence and copyright,
with the licence text) and checked in CI with `pnpm vectors:notices --check`. A licence whose text names an organisation
(BSD-3-Clause's "Neither the name of …" clause) has its text per project (`PROJECT_LICENCE_TEXTS`, verbatim from the
upstream licence file); generic SPDX texts (`LICENCE_TEXTS`) are only for licences that name no one, so a new BSD-3
source fails until its own text is added.

## llhttp import

**Upstream vectors are imported from the latest published (non-prerelease) release tag; the exact commit is
recorded; refresh at the start of every phase with `--refresh`.**

- The latest release comes from `GET https://api.github.com/repos/nodejs/llhttp/releases/latest` (GitHub returns
  the most recent published release, never a draft or prerelease). llhttp publishes every release twice: the release
  tag `release/vX` holds the generated C sources only, and the source tag `vX` holds the tree with `test/`. The
  importer uses the source tag (it strips `release/`) and resolves its commit with
  `git ls-remote https://github.com/nodejs/llhttp.git refs/tags/vX refs/tags/vX^{}` (the peeled `^{}` commit of the
  annotated tag). Every file is downloaded at that commit.
- Current import: tag [`v9.4.3`](https://github.com/nodejs/llhttp/tree/v9.4.3) (published 2026-07-30), commit
  `45c8699d0ca8431ab366c8706e613b2e2ac62c04`. Licence: MIT; at this commit the licence file is
  [`LICENSE`](https://github.com/nodejs/llhttp/blob/45c8699d0ca8431ab366c8706e613b2e2ac62c04/LICENSE) (llhttp has no
  `LICENSE-MIT`). Copyright line, verbatim: `Copyright © 2018 Fedor Indutny`.
- `pnpm vectors:import-llhttp` re-imports at the tag recorded in the existing files (use it after changing an
  override) and fails if that tag now points to a different commit than the recorded one;
  `pnpm vectors:import-llhttp --refresh` resolves the latest release again, rewrites the files and prints the added,
  removed and changed cases per file, which then need the Step 5 review below. Both need network (every request and
  `git ls-remote` time out after 30 s); the downloaded files are parsed, never executed; the licence file must be the
  exact MIT text. All four files are built and validated before any is written, so a failed run leaves the previous
  import untouched. Commit the rewritten `tests/vectors/h1-response/llhttp-*.json` and run `pnpm vectors:notices`.
  CI never touches the network for vectors. The tools resolve paths from their own location, so they run from any
  working directory.

P0 imports `test/response/{sample,content-length,connection,invalid}.md`; `transfer-encoding.md` comes with chunked
decoding in P1a (`removal: P1a (#7)` in the importer). The importer refuses to run when an override is unused or when
a case needs a decision it cannot make (see below).

TODO(#7): map `HPE_INVALID_CONTENT_LENGTH` by reason string before importing `transfer-encoding.md` (llhttp reports a
`Content-Length` after `Transfer-Encoding` with it; natively that is `PROTOCOL/smuggling-risk`, design §9.2 and
`docs/notes/response-parser.md` §11 D10).

### How llhttp's harness reads the markdown (reproduced exactly)

Studied at the pinned commit:
[`test/md-test.ts`](https://github.com/nodejs/llhttp/blob/45c8699d0ca8431ab366c8706e613b2e2ac62c04/test/md-test.ts),
[`test/fixtures/index.ts`](https://github.com/nodejs/llhttp/blob/45c8699d0ca8431ab366c8706e613b2e2ac62c04/test/fixtures/index.ts),
and its dependencies mdgator 1.1.2
([`src/mdgator.ts`](https://github.com/indutny/mdgator/blob/c73bf65c09ad4109d3f4b259968996e7bef2cede/src/mdgator.ts))
and llparse-test-fixture 5.1.0
([`src/result.ts`](https://github.com/nodejs/llparse-test-fixture/blob/d2a2142d4b31499e208fe444c5279725f1ae2433/src/result.ts),
[`src/native/fixture.c`](https://github.com/nodejs/llparse-test-fixture/blob/d2a2142d4b31499e208fe444c5279725f1ae2433/src/native/fixture.c)).

- **Line endings:** markdown-it (under mdgator) turns CRLF and CR in the markdown into LF before parsing; the importer
  does the same, so a CRLF checkout imports identically.
- **Tests:** mdgator turns every heading below the top level that owns fenced blocks into a test; the
  `<!-- meta={…} -->` comment before the `http` block gives its `type` (`response`, or a lenient mode) and flags
  (`skipBody`, `noScan`, `pause`). A fence's content is its lines joined with `\n`, including a final `\n`.
- **Input normalisation**, in this order (`normaliseInput` in `tools/vectors/import-llhttp.ts`):
  1. remove one trailing `\n`;
  2. remove escaped newlines (`\` followed by CRLF, CR or LF): a line ending in `\` continues on the next line;
  3. convert every remaining CRLF, CR or LF to CRLF;
  4. replace the escapes `\r`, `\n`, `\t`, `\f`, then `\xHH…` (hex, to the char code) and `\NNN` (1–3 octal digits);
  5. evaluate `${…}` as JavaScript — **not reproduced**: the importer never runs upstream code and skips such a case
     (none in the P0 files);
  6. the Windows-only shell escaping (`^|`, `^&`) is not part of the input on POSIX and is not reproduced.

  Other backslashes stay as they are (for example `\"` in `content-length.md` stays two bytes; llhttp's own log
  confirms it).
- **Encoding:** the fixture passes the input as a process argument, so the bytes are the UTF-8 encoding of the string
  (`Buffer.byteLength` sizes the scan ranges). Vectors therefore use `inputEncoding: "utf8"`.
- **Expected log:** md-test replaces `\t` and `\f` in the log block too, and drops the final empty line. Each line is
  `off=<byte offset> <event>`; spans print as `len=N span[name]="<raw bytes>"`, with CR and LF printed as separate
  `span[name]=cr|lf` lines (`llparse__print_span`). Error lines carry the numeric code, whose `HPE_*` name the importer
  reads from [`src/llhttp/constants.ts`](https://github.com/nodejs/llhttp/blob/45c8699d0ca8431ab366c8706e613b2e2ac62c04/src/llhttp/constants.ts).
- **Proof of fidelity:** the importer checks every span in every log against the normalised input at the logged
  offset and length; any difference stops the import. All spans of the four files match.

### Deriving the native expectation

Cases whose `type` is not `response` (llhttp lenient modes) are skipped: blazehttp has one strict mode (§5.2).
`skipBody: true` is llhttp's hook for "the request was HEAD", so it becomes `request.method: "HEAD"`; every other
case is `GET`. Then, walking llhttp's log:

- **llhttp reports an error** before the final response completes → `HPE_UNEXPECTED_CONTENT_LENGTH` (which llhttp
  also uses for a duplicate `Content-Length` and for `Content-Length` with `Transfer-Encoding`) and
  `HPE_INVALID_TRANSFER_ENCODING` map to `PROTOCOL/smuggling-risk`; every other `HPE_*` maps to
  `PROTOCOL/malformed-response` (§5.2 last bullet, §8). The map is a lookup table (`ERROR_BY_LLHTTP`).
- **1xx heads** are skipped as interim responses; `101` is not interim for us, and llhttp then stops with
  `HPE_PAUSED_UPGRADE`, which maps to `PROTOCOL/malformed-response` (§5.2: `101` rejected). An interim head
  carrying `Content-Length`, `Transfer-Encoding` or `Connection` stops the import until an override decides it: natively
  a 1xx with `Transfer-Encoding` or a `Content-Length` other than `0` is `PROTOCOL/smuggling-risk`, and a
  `Connection: close` in it makes the final response `keepAlive: false` (§5.2 Interim heads); no P0 case has one.
- **Final response** (status ≥ 200): `ok` with status, headers, body and keep-alive as defined in the format table:
  - HEAD, 204 and 304 have no body regardless of headers; a 204/304 answering a non-`HEAD` request with
    `Transfer-Encoding` or `Content-Length` > 0 is `keepAlive: false` (§5.2 Bodies);
  - `Transfer-Encoding: chunked` with a body → skipped until P1a (no chunked decoding yet; #7);
  - one `Content-Length` of `1*DIGIT` within `u64` after OWS trimming (§5.2, RFC 9110 §5.5) → that many bytes; if
    EOF comes first →
    `PROTOCOL/incomplete-body`;
  - neither → close-delimited: the rest of the input, `keepAlive: false`.
- **EOF** before a final head: `NETWORK/closed-before-response` only when the input is empty; once any byte of the
  exchange's response arrived — interim heads included — `PROTOCOL/malformed-response` (§5.2 EOF before the final
  head). The test helper applies the same rule.
- Events after the final response completes (llhttp parsing pipelined data, or its "Data after `Connection: close`"
  error) do not change the delivered response (§5.1, see `keepAlive`).
- Anything else — a protocol other than `HTTP`, a version other than 1.0/1.1, a status above 599, bytes before a
  status line, a line break inside a header value, another `Transfer-Encoding`, `Transfer-Encoding` with
  `Content-Length` or on HTTP/1.0, a repeated or non-numeric `Content-Length`, input ending before a final head —
  stops the import until `tools/vectors/overrides/llhttp.json` decides the case.

**Overrides** (`tools/vectors/overrides/llhttp.json`) map a case name to `{ "native": …, "rule": "§5.2 …" }`. The
rule must cite a design section; an override without one, or one that no case uses, stops the import. An override
decides its case outright: llhttp's log for that case is neither modelled nor span-checked, so it also settles cases
whose log the importer cannot read.

### Step 5 review: every case llhttp accepts

Each accepted case was checked against §5.2. Overridden (llhttp accepts or its log cannot be modelled, we reject).
Before the overrides bypassed the log, every span of every case in the four files — overridden ones included — was
checked against the normalised input and matched.

| Case | Expectation | Rule |
|---|---|---|
| `sample / RTSP response` | `PROTOCOL/malformed-response` | §5.2 Status line: exactly `HTTP/1.0` or `HTTP/1.1` |
| `sample / ICE response` | `PROTOCOL/malformed-response` | §5.2 Status line: exactly `HTTP/1.0` or `HTTP/1.1` |
| `sample / HTTP version 0.9` | `PROTOCOL/malformed-response` | §5.2 Status line: HTTP/0.9 rejected |
| `sample / Response starting with CRLF` | `PROTOCOL/malformed-response` | §5.2 Status line: the head starts with `HTTP/1.x`; no empty lines are skipped before it |
| `connection / HTTP 100 first, then 400` | `PROTOCOL/malformed-response` | §5.2 Status line: the same after an interim response (an extra CRLF follows the `100` head) |
| `invalid / CR before response and tab after HTTP version` | `PROTOCOL/malformed-response` | §5.2 Line endings: bare CR (llhttp rejects it too, after skipping the leading CR) |

Accepted with a derived expectation, and the decisions behind them:

- `sample / Simple response`, `Underscore in header key`, `bonjourmadame.fr` (HTTP/1.0 + `Connection: keep-alive`
  → `keepAlive: true`; empty `Pragma` value), `Non ASCII in status line` (obs-text in the reason phrase has no control
  characters, RFC 9112 §4), `Empty reason phrase after space`, `Empty body…`, `No headers and no body`,
  `No Content-Length, no Transfer-Encoding` (close-delimited): accept.
- `sample / No reason phrase` (`HTTP/1.1 301\r\n\r\n`): accept, status 301 with an empty reason, close-delimited
  empty body (§5.2 Status line: CRLF straight after the code is an empty reason; owner decision, §21).
- `content-length / Response without Content-Length, but with body`: close-delimited (`Connection: close`, no
  framing): the body is everything up to EOF, `keepAlive: false`. The `\"` in the upstream input stays two bytes.
- `invalid / Extra space between status code and reason`: accept; the reason phrase may start with SP
  (RFC 9112 §4 `reason-phrase = 1*( HTAB / SP / VCHAR / obs-text )`), and §5.2's "one SP" is the separator.
- `sample / Google 301` (219 announced, 213 sent), `content-length / Tabs in Content-Length (surrounding)` (OWS
  around the value is not part of it, RFC 9110 §5.5; 42 announced, none sent) and `connection / HTTP 103 first, then
  200` (17 announced, 16 sent): upstream inputs end before their `Content-Length`, so under EOF semantics they
  expect `PROTOCOL/incomplete-body`.
- `content-length / Content-Length reset when no body is received`: HEAD → no body.
- `connection / Proxy-Connection`: `keepAlive: true`. `Proxy-Connection` is not `Connection`; llhttp honours it, we
  do not (it is not an HTTP/1.1 field, RFC 9110 §7.6.1).
- `connection / HTTP/1.0 … EOF-terminated`, `HTTP/1.1 with EOF-terminated 200 status`, `HTTP 200 response with
  Upgrade header`: close-delimited, `keepAlive: false`; `Connection: upgrade` on a 200 is ignored.
- `connection / HTTP 200 response with Upgrade header and Content-Length`: 4-byte body, `keepAlive: true`;
  `Connection: upgrade` and `Upgrade` on a 200 are ignored (no upgrade was requested).
- `connection / … 204 status` (three strict cases; the fourth, lenient one is skipped), `HTTP 304 with Content-Length`, `HTTP 304 with Transfer-Encoding`,
  `HTTP/1.1 with keep-alive disabled, content-length`: the first response is delivered without body bytes beyond its
  framing; the trailing data is a connection-level matter (§5.1). The two 304 cases announce a body (`Content-Length:
  10`, `Transfer-Encoding: chunked`) to a `GET`, so they are `keepAlive: false` (§5.2 Bodies, derived by the
  importer).

None of the accepted inputs contains a bare LF, obs-fold or whitespace before a colon: llhttp's strict mode rejects
those itself, and they map to `PROTOCOL/malformed-response`.

### Skipped cases

| Case | Reason |
|---|---|
| `sample / amazon.com` | chunked body; chunked decoding arrives in P1a ([#7](https://github.com/simran122/blazehttp/issues/7)) |
| `sample / Spaces in header value` | chunked body; P1a (#7) |
| `content-length / Content-Length-X` | chunked body; P1a (#7) |
| `connection / HTTP 200 response with Upgrade header and Transfer-Encoding` | chunked body; P1a (#7) |
| `sample / No carriage ret (lenient)` | llhttp lenient mode `response-lenient-optional-cr-before-lf` |
| `connection / HTTP/1.1 with keep-alive disabled, content-length (lenient)` | llhttp lenient mode `response-lenient-data-after-close` |
| `connection / HTTP/1.1 with keep-alive disabled and 204 status (lenient)` | llhttp lenient mode `response-lenient-keep-alive` |
| `invalid / Bare CR after response line (lenient)` | llhttp lenient mode `response-lenient-optional-lf-after-cr` |
| `invalid / Only LFs present and no body (lenient)` | llhttp lenient mode `response-lenient-all` |
| `invalid / Only LFs present (lenient)` | llhttp lenient mode `response-lenient-all` |

The strict twin of every lenient case is imported, so the inputs are still covered.

### Counts (llhttp v9.4.3)

| File | Cases | Overridden | Skipped |
|---|---|---|---|
| `h1-response/llhttp-sample.json` | 17 | 4 | 3 |
| `h1-response/llhttp-content-length.json` | 3 | 0 | 1 |
| `h1-response/llhttp-connection.json` | 15 | 1 | 3 |
| `h1-response/llhttp-invalid.json` | 16 | 1 | 3 |
| **Total** | **51** | **6** | **10** |

## Own and per-source vector files (Task 4)

Written by hand from design §5.2 (`docs/notes/response-parser.md` §9), not imported:

| File | Source | Cases | Content |
|---|---|---|---|
| `h1-response/blazehttp-rules.json` | blazehttp (`MIT OR Apache-2.0`, no notice) | 144 | one rejecting case and its accepting neighbour for every §5.2 head rule, the security-research classes (note §9.2) and the limit boundaries |
| `h1-response/go-selected.json` | Go `go1.27.3` (BSD-3-Clause) | 13 | re-derived from `response_test.go` / `transfer_test.go` |
| `h1-response/hyper-selected.json` | hyper `v1.12.0` (MIT) | 1 | re-derived from `src/proto/h1/role.rs` |
| `h1-response/httparse-selected.json` | httparse `v1.10.1` (MIT of MIT OR Apache-2.0) | 8 | re-derived from `src/lib.rs` `mod tests` |
| `h1-response/curl-selected.json` | curl `curl-8_23_0` (curl) | 4 | re-derived from `tests/data` (CRLF where curl's file uses LF) |

Each per-source case names its upstream test in `derivedFrom`; the source block records the latest published release
tag, its commit and the verbatim copyright line of the licence file at that commit (`THIRD_PARTY_NOTICES`).

**Limit vectors** are built by one deterministic rule so they can be regenerated: every byte counts, CRLF included;
filler fields are `x-fill-NNNN: a…a` (4-digit index), each line 1,000 bytes except one last field sized to hit the
total exactly (or, for the field-count cases, `x-fill-NNNN: a`); status-line cases use `HTTP/1.1 200 ` and a reason of
`r` bytes. Boundaries: status line 8,192 / 8,193 bytes; 100 / 101 fields; head 65,536 / 65,537 bytes; a 32,768-byte
`103` head plus a final head totalling 65,536 / 65,537; a final status line that fits 8 KiB but not the budget left
after a 65,436-byte interim head; 10 / 11 interim heads.
