# removal: P1a (#24)
"""Executable model of the P-D (parser) and W-B / W-D (writer) validators of docs/notes/h1-speed-research.md
§5.5, §6.3 and §6.5, checked against scalar references.

Run: timeout 300 python3 -I tools/h1-research/scanthru_model.py   (seed 9112; prints the case counts)

Parser part (field section only; the status line is scalar in every candidate):
  * `reference(buf)`: strict RFC 9112 field section with the 100-field limit (checked at a line's first byte, as G)
    -> ('ok', head_end, spans) | ('err', first_bad) | ('more',).
  * `pd_whole(buf)`: P-D over 64-bit blocks with the 6-bit carry state, the h+1 limit, masked flatten, the field
    limit checked before the flatten, and a scalar re-run from the P-D run start on any error.
  * `pd_split(buf, cuts)`: the line-start hand-off between P-D and the scalar parser; once the scalar parser owns a
    partial line it keeps it until the line ends, so P-D never re-classifies a partial line (linear under a trickle).
Writer part (W-A semantics are the reference: grammar + forbidden names + singletons + line count):
  * `delta_reference(block, lines)` vs `wd(block, lines)` (W-D: bit-stream grammar + per-line classification) and
    `wb(block, hints, lines)` (W-B: tiled ranges from untrusted hints; `": "` and CRLF required).
Not modelled: the 64 KiB head limit and the status line (both unchanged scalar code).
"""
import random
from typing import Literal, Optional, Union

Span = tuple[int, int, int, int]
Ok = tuple[Literal['ok'], int, list[Span]]
Err = tuple[Literal['err'], int]
More = tuple[Literal['more']]
Handoff = tuple[Literal['handoff'], int, list[Span]]
Rerun = tuple[Literal['rerun']]
Result = Union[Ok, Err, More]
LineResult = tuple[Literal['ok', 'err', 'more', 'line'], int]
Masks = dict[str, int]

TCHAR = set(b"!#$%&'*+-.^_`|~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz")
FIELD = set([9, 32]) | set(range(0x21, 0x7F)) | set(range(0x80, 0x100))
OWS = (9, 32)
M64 = (1 << 64) - 1
BLOCK = 64
MAX_FIELDS = 100
CLASSIFIED = [0]   # bytes classified by P-D (work counter for the trickle test)


def trim(buf: bytes, start: int, end: int) -> tuple[int, int]:
    while start < end and buf[start] in OWS:
        start += 1
    while end > start and buf[end - 1] in OWS:
        end -= 1
    return start, end


# ---------------------------------------------------------------- scalar parser (reference and tail parser)

def scalar_lines(buf: bytes, i: int, spans: list[Span], one_line: bool = False) -> LineResult:
    """Parse field lines from line start `i`, appending spans. Returns ('ok', end) | ('err', at) |
    ('more', start_of_the_partial_line) | ('line', next_line_start) after one line when `one_line`."""
    n = len(buf)
    while True:
        if i >= n:
            return ('more', i)
        if buf[i] == 13:
            if i + 1 >= n:
                return ('more', i)
            return ('ok', i + 2) if buf[i + 1] == 10 else ('err', i + 1)
        if len(spans) >= MAX_FIELDS:
            return ('err', i)                           # the 101st field, at its first byte (G's rule)
        if buf[i] not in TCHAR:
            return ('err', i)
        j = i
        while j < n and buf[j] in TCHAR:
            j += 1
        if j >= n:
            return ('more', i)
        if buf[j] != 58:
            return ('err', j)
        k = j + 1
        while k < n and buf[k] in FIELD:
            k += 1
        if k >= n:
            return ('more', i)
        if buf[k] != 13:
            return ('err', k)
        if k + 1 >= n:
            return ('more', i)
        if buf[k + 1] != 10:
            return ('err', k + 1)
        vs, ve = trim(buf, j + 1, k)
        spans.append((i, j, vs, ve))
        i = k + 2
        if one_line:
            return ('line', i)


def finish_scalar(t: LineResult, spans: list[Span]) -> Result:
    if t[0] == 'ok':
        return ('ok', t[1], spans)
    return ('err', t[1]) if t[0] == 'err' else ('more',)


def reference(buf: bytes) -> Result:
    spans: list[Span] = []
    return finish_scalar(scalar_lines(buf, 0, spans), spans)


# ---------------------------------------------------------------- P-D

def masks(block: bytes) -> Masks:
    CLASSIFIED[0] += BLOCK
    m: Masks = dict(cr=0, lf=0, colon=0, t=0, v=0, sp=0)
    for k, b in enumerate(block):
        bit = 1 << k
        if b == 13:
            m['cr'] |= bit
        if b == 10:
            m['lf'] |= bit
        if b == 58:
            m['colon'] |= bit
        if b == 32:
            m['sp'] |= bit
        if b in TCHAR:
            m['t'] |= bit
        if b in FIELD:
            m['v'] |= bit
    return m


def bits(x: int) -> list[int]:
    out: list[int] = []
    while x:
        low = x & -x
        out.append(low.bit_length() - 1)
        x ^= low
    return out


def pd_run(buf: bytes, start: int, final: bool, committed: int) -> Union[Ok, Handoff, Rerun]:
    """P-D from line start `start` over complete blocks (and, when `final`, the received tail block), with
    `committed` fields already in the span slots. Returns ('ok', end, spans) | ('handoff', line_start, spans) with
    the spans of complete lines only, or ('rerun',): the caller re-runs the scalar parser from `start`, discarding
    every uncommitted slot (errors and the field limit)."""
    lf_c, cr_c, n_c, p_c, v_c, end_pending = 1, 0, 0, 0, 0, False   # the 6-bit state
    streams: list[list[int]] = [[], [], [], []]   # name starts, colons, value starts, value ends
    base = start
    last_line_start = start
    while base < len(buf):
        block = buf[base:base + BLOCK]
        if len(block) < BLOCK and not final:
            break
        received = (1 << len(block)) - 1
        m = masks(block)
        S = ((m['lf'] << 1) | lf_c) & M64
        A = ((m['cr'] << 1) | cr_c) & M64
        s1 = (S & m['t']) + m['t'] + n_c
        n_c, N = s1 >> 64, s1 & M64 & ~m['t']
        P = ((N << 1) | p_c) & M64
        s2 = P + m['v'] + v_c
        v_c, E = s2 >> 64, s2 & M64 & ~m['v']
        err = (N & ~m['colon']) | (E & ~m['cr']) | (A ^ m['lf']) | (S & ~(m['t'] | m['cr']))
        if end_pending:
            limit = 1                                   # only the LF after the empty-line CR
        else:
            H = S & m['cr'] & received
            limit = M64 if not H else (((H & -H) << 2) - 1) & M64   # bits <= h + 1
        keep = limit & received
        if err & keep:
            return ('rerun',)
        starts = bits(S & m['t'] & keep)
        if committed + len(streams[0]) + len(starts) > MAX_FIELDS:
            return ('rerun',)                           # checked BEFORE the flatten: never past the 100 slots
        streams[0] += [base + k for k in starts]
        streams[1] += [base + k for k in bits(N & keep)]
        streams[2] += [base + k for k in bits(P & keep)]
        streams[3] += [base + k for k in bits(E & keep)]
        assert all(committed + len(s) <= MAX_FIELDS for s in streams)   # the slots are never overrun
        for k in bits(S & keep):
            last_line_start = base + k
        lf_c, cr_c, p_c = m['lf'] >> 63, m['cr'] >> 63, N >> 63
        if end_pending:
            return ('ok', base + 1, zip_spans(buf, base + 1, streams))
        H = S & m['cr'] & received
        if H:
            h = (H & -H).bit_length() - 1
            if h == 63:
                end_pending = True                      # the LF is bit 0 of the next block
                base += BLOCK
                continue
            if h + 1 < len(block):
                return ('ok', base + h + 2, zip_spans(buf, base + h + 2, streams))
            return ('handoff', base + h, zip_spans(buf, base + h, streams))   # CR received, LF not yet
        base += BLOCK
    return ('handoff', last_line_start, zip_spans(buf, last_line_start, streams))


def zip_spans(buf: bytes, limit: int, streams: list[list[int]]) -> list[Span]:
    spans: list[Span] = []
    for k, s in enumerate(streams[0]):
        if s >= limit:
            break
        vs, ve = trim(buf, streams[2][k], streams[3][k])
        spans.append((s, streams[1][k], vs, ve))
    return spans


def pd_whole(buf: bytes) -> Result:
    spans: list[Span] = []
    r = pd_run(buf, 0, True, 0)
    if r[0] == 'rerun':
        return finish_scalar(scalar_lines(buf, 0, spans), spans)
    spans += r[2]
    if r[0] == 'ok':
        return ('ok', r[1], spans)
    return finish_scalar(scalar_lines(buf, r[1], spans), spans)    # the scalar parser finishes the partial line


def pd_split(buf: bytes, cuts: list[int]) -> Result:
    """Feed `buf` in pieces. P-D runs only at a line start that the scalar parser does not own, with >= 64 bytes
    after it; once the scalar parser has returned More inside a line, it owns that line until it ends."""
    pos = 0
    owned: Optional[int] = None                         # line start owned by the scalar parser
    spans: list[Span] = []
    for end in cuts + [len(buf)]:
        window = buf[:end]
        while True:
            if owned == pos:
                t = scalar_lines(window, pos, spans, one_line=True)
                if t[0] == 'line':
                    pos, owned = t[1], None
                    continue
                if t[0] == 'more':
                    break
                return finish_scalar(t, spans)
            if len(window) - pos >= BLOCK:
                r = pd_run(window, pos, False, len(spans))
                if r[0] == 'ok':
                    return ('ok', r[1], spans + r[2])
                if r[0] == 'handoff':
                    spans += r[2]
                    if r[1] != pos:
                        pos = r[1]
                        continue
                # 'rerun' or no line ends inside the complete blocks: the scalar parser takes over at `pos`
            t = scalar_lines(window, pos, spans)
            if t[0] in ('ok', 'err'):
                return finish_scalar(t, spans)
            pos, owned = t[1], t[1]
            break
    return ('more',)


# ---------------------------------------------------------------- writer: W-A semantics (reference)

FORBIDDEN = {b"host", b"transfer-encoding", b"connection", b"upgrade", b"expect", b"te", b"keep-alive",
             b"proxy-connection", b"proxy-authorization", b"content-length"}
SINGLETON = {b"authorization", b"cookie", b"content-type", b"user-agent"}


def admit(name: bytes, seen: set[bytes]) -> bool:
    """Writer-note W10/W3 rules: forbidden names (and every underscore spelling of a forbidden or singleton
    name) are refused; singleton names may appear once."""
    lower = name.lower()
    canonical = lower.replace(b"_", b"-")
    if canonical in FORBIDDEN or (b"_" in lower and canonical in SINGLETON):
        return False
    if canonical in SINGLETON:
        if canonical in seen:
            return False
        seen.add(canonical)
    return True


def delta_reference(block: bytes, lines: int) -> bool:
    i, n, count = 0, len(block), 0
    seen: set[bytes] = set()
    while i < n:
        j = i
        while j < n and block[j] in TCHAR:
            j += 1
        if j == i or block[j:j + 2] != b": " or not admit(block[i:j], seen):
            return False
        k = j + 2
        while k < n and block[k] in FIELD:
            k += 1
        if block[k:k + 2] != b"\r\n":
            return False
        i, count = k + 2, count + 1
    return count == lines


def layout(block: bytes) -> list[tuple[int, int]]:
    """The true (name_len, value_len) per line of a well-formed delta (what the JS layer would send)."""
    out: list[tuple[int, int]] = []
    i = 0
    while i < len(block):
        c = block.index(b": ", i)
        e = block.index(b"\r\n", c + 2)
        out.append((c - i, e - c - 2))
        i = e + 2
    return out


# ---------------------------------------------------------------- W-D (writer form of the bit-stream validator)

def wd(block: bytes, lines: int) -> bool:
    lf_c, cr_c, n_c, p_c, v_c = 1, 0, 0, 0, 0
    lf_count = 0
    names: list[int] = []
    colons: list[int] = []
    for base in range(0, len(block), BLOCK):
        chunk = block[base:base + BLOCK]
        received = (1 << len(chunk)) - 1
        m = masks(chunk)
        S = ((m['lf'] << 1) | lf_c) & M64
        A = ((m['cr'] << 1) | cr_c) & M64
        s1 = (S & m['t']) + m['t'] + n_c
        n_c, N = s1 >> 64, s1 & M64 & ~m['t']
        N1 = ((N << 1) | (p_c >> 1)) & M64             # the byte after the colon must be SP
        P = ((N << 2) | p_c) & M64                      # the value starts after ": "
        s2 = P + m['v'] + v_c
        v_c, E = s2 >> 64, s2 & M64 & ~m['v']
        err = (N & ~m['colon']) | (N1 & ~m['sp']) | (E & ~m['cr']) | (A ^ m['lf']) | (S & ~m['t'])
        if err & received:
            return False
        names += [base + k for k in bits(S & m['t'] & received)]     # name bounds for the classification
        colons += [base + k for k in bits(N & received)]
        lf_count += bin(m['lf']).count('1')
        lf_c, cr_c, p_c = m['lf'] >> 63, m['cr'] >> 63, N >> 62   # 2 bits: colons at 62 and 63
    ends_on_line = len(block) == 0 or block[-1] == 10
    if not (ends_on_line and n_c == 0 and v_c == 0 and lf_count == lines):
        return False
    seen: set[bytes] = set()
    return all(admit(block[s:c], seen) for s, c in zip(names, colons))   # M2-1: per-line classification


# ---------------------------------------------------------------- W-B (tiled ranges from untrusted hints)

def wb(block: bytes, hints: list[tuple[int, int]], lines: int) -> bool:
    pos = 0
    seen: set[bytes] = set()
    for name_len, value_len in hints:
        if name_len < 1:
            return False
        c = pos + name_len
        e = c + 2 + value_len
        if e + 2 > len(block):                          # every range via a checked get(); never out of bounds
            return False
        name, sep, value, crlf = block[pos:c], block[c:c + 2], block[c + 2:e], block[e:e + 2]
        if sep != b": " or crlf != b"\r\n":             # R2-1: both are required
            return False
        if not all(b in TCHAR for b in name) or not all(b in FIELD for b in value):
            return False
        if not admit(name, seen):
            return False
        pos = e + 2
    return pos == len(block) and len(hints) == lines


# ---------------------------------------------------------------- generators and checks

def gen_head(rng: random.Random) -> bytes:
    names = [b"Content-Type", b"x", b"Set-Cookie", b"ETag", b"a!#$%&'*+-.^_`|~9", b"Cache-Control"]
    vals = [b"", b"text/html; charset=utf-8", b" spaced  ", b"\t", b"a:b:c", bytes([0x80, 0xFF]), b"x" * 70]
    out = bytearray()
    for _ in range(rng.randrange(0, 8)):
        out += rng.choice(names) + b":" + rng.choice(vals) + b"\r\n"
    out += b"\r\n" + rng.choice([b"", b"Content-Length: 0\r\n\r\n", b"c:d\r\n"]) + bytes(
        rng.randrange(0, 256) for _ in range(rng.randrange(0, 70)))
    mutate(rng, out)
    if rng.random() < 0.3:
        out = out[:rng.randrange(0, len(out) + 1)]
    return bytes(out)


def mutate(rng: random.Random, out: bytearray) -> None:
    hostile = [0, 9, 10, 13, 32, 58, 0x7F, 0x80, 0x0B, ord('a'), ord('(')]
    for _ in range(rng.choice([0, 0, 1, 2])):
        k = rng.randrange(0, len(out) + 1)
        op = rng.randrange(3)
        if op == 0:
            out.insert(k, rng.choice(hostile))
        elif op == 1 and k < len(out):
            del out[k]
        elif k < len(out):
            out[k] = rng.choice(hostile)


def gen_delta(rng: random.Random) -> tuple[bytes, int, bool]:
    names = [b"content-type", b"x-request-id", b"authorization", b"a", b"x-" + b"n" * 70, b"Cookie",
             b"content-length", b"Transfer-Encoding", b"user_agent", b"accept"]
    vals = [b"", b"application/json", b"https://x/y?z=1", b"Bearer " + b"t" * 60, b"\t v \t"]
    out = bytearray()
    lines = rng.randrange(0, 6)
    for _ in range(lines):
        out += rng.choice(names) + b": " + rng.choice(vals) + b"\r\n"
    mutated = rng.random() < 0.5
    if mutated:
        mutate(rng, out)
    return bytes(out), lines + rng.choice([0, 0, 0, 1, -1]), mutated


def heads_with_fields(count: int) -> list[bytes]:
    """Field-limit cases: `count` short fields, then the head end and a body line (99/100/101 fields)."""
    line = b"x-f: v\r\n"
    return [line * count + b"\r\n", line * count + b"\r\nc:d\r\n", line * count]


def check_parser(rng: random.Random, cases: int) -> dict[str, int]:
    kinds: dict[str, int] = {}
    for t in range(cases):
        b = gen_head(rng)
        r = reference(b)
        assert pd_whole(b) == r, (t, b, r, pd_whole(b))
        cuts = sorted(rng.sample(range(1, len(b)), min(len(b) - 1, rng.randrange(0, 4)))) if len(b) > 1 else []
        assert pd_split(b, cuts) == r, (t, b, cuts, r, pd_split(b, cuts))
        kinds[r[0]] = kinds.get(r[0], 0) + 1
    return kinds


def check_parser_boundaries() -> int:
    regressions = [
        b"N:" + b"v" * 59 + b"\r\n\r\n" + b"Content-Length: 0\r\n\r\n",   # empty-line CR at bit 63
        b"a:b\r\n\rX",                                                  # bare CR after the last CRLF
        b"a:b\r\n\r\nc:d\r\n",                                          # body line after the head end
    ]
    for b in regressions:
        assert pd_whole(b) == reference(b), b
    for pad in range(0, 140):
        for tail in (b"\r\n", b"\r", b"\rX", b"\n", b"\r\nc:d\r\n\r\n"):
            for b in (b"N:" + b"v" * pad + b"\r\n" + tail, b"N" * pad + b":v\r\n" + tail):
                r = reference(b)
                assert pd_whole(b) == r, (pad, tail)
                for cut in range(1, len(b)):                   # every single split point
                    assert pd_split(b, [cut]) == r, (pad, tail, cut)
    limits = 0
    for count in (97, 98, 99, 100, 101, 102):                  # R2-2: the 100-field limit, whole and split
        for b in heads_with_fields(count):
            r = reference(b)
            assert pd_whole(b) == r, (count, b[-20:], r)
            for cut in range(1, len(b), 7):
                assert pd_split(b, [cut]) == r, (count, cut)
            limits += 1
    assert reference(heads_with_fields(101)[0])[0] == 'err' and reference(heads_with_fields(100)[0])[0] == 'ok'
    return len(regressions) + limits


def check_trickle() -> int:
    """P-D work stays linear when a long line arrives one byte per call (the scalar parser owns partial lines)."""
    b = b"x-long: " + b"v" * 4000 + b"\r\n" + b"x-f: v\r\n" * 40 + b"\r\n"
    CLASSIFIED[0] = 0
    assert pd_split(b, list(range(1, len(b)))) == reference(b)
    assert CLASSIFIED[0] <= 2 * len(b) + 2 * BLOCK, CLASSIFIED[0]
    return CLASSIFIED[0]


def check_writer(rng: random.Random, cases: int) -> tuple[int, int]:
    smuggle = b"a: b\r\n\r\nGET /admin HTTP/1.1\r\nhost: internal\r\n"
    assert not wd(smuggle, 1) and not delta_reference(smuggle, 1)
    unterminated = b"a: " + b"v" * 61
    assert not wd(unterminated, 0) and not wd(unterminated, 1)
    for k in (0, 1):
        assert not wd(b": x\r\n", k) and not delta_reference(b": x\r\n", k)   # empty name
        assert not wb(b": x\r\n", [(0, 1)], 1) and not wb(b": x\r\n", [(1, 0)], 1)
    # R2-1: hints that tile a smuggled request with no ": " / CRLF at the hinted places
    smuggle_b = b"a\r\n\r\nGET /admin HTTP/1.1\r\nhost: x\r\n"
    assert not wb(smuggle_b, [(1, 0), (3, 14), (4, 1)], 3)
    assert not wb(b"a: b\r\n", [(1, 60_000)], 1)                       # out-of-range hint
    # M2-1: forbidden and duplicate singleton names are refused by every form
    for bad in (b"content-length: 5\r\n", b"Transfer-Encoding: chunked\r\n", b"content_length: 1\r\n",
                b"cookie: a\r\nCookie: b\r\n", b"user_agent: x\r\n"):
        n = bad.count(b"\r\n")
        assert not delta_reference(bad, n) and not wd(bad, n) and not wb(bad, layout(bad), n), bad
    accepted = hinted = 0
    for t in range(cases):
        block, lines, mutated = gen_delta(rng)
        r = delta_reference(block, lines)
        assert wd(block, lines) == r, (t, block, lines, r)
        accepted += r
        if r:                                                          # W-B with the true layout and corrupted hints
            hints = layout(block)
            assert wb(block, hints, lines)
            if hints:
                k = rng.randrange(len(hints))
                bad = list(hints)
                bad[k] = (bad[k][0] + rng.choice([-1, 1]), bad[k][1] + rng.choice([-1, 0, 1]))
                assert not wb(block, bad, lines), (t, block, bad)
            hinted += 1
        elif not mutated:
            assert not wb(block, layout(block), lines)                  # forbidden names or a wrong line count
    return accepted, hinted


if __name__ == "__main__":
    rng = random.Random(9112)
    print("parser", check_parser(rng, 100_000))
    print("parser regressions + boundary sweep + field limits", check_parser_boundaries())
    print("trickle bytes classified by P-D", check_trickle())
    accepted, hinted = check_writer(rng, 100_000)
    print("writer accepted", accepted, "of 100000; W-B hint checks", hinted)
