"""Executable model of the P-D (parser) and W-D (writer) bit-stream validators of
docs/notes/h1-speed-research.md §5.5 and §6.5, checked against scalar references.

Run: python3 -I spike/h1-research/scanthru_model.py   (seed 9112; prints the case counts)

Parser part (field section only; the status line is scalar in every candidate):
  * `reference(buf)`: strict RFC 9112 field section -> ('ok', head_end, spans) | ('err', first_bad) | ('more',).
  * `pd_whole(buf)`: P-D over 64-bit blocks with the 6-bit carry state, the H+1 limit and masked flatten.
  * `pd_split(chunks)`: the line-start hand-off between P-D (complete 64-byte blocks) and the scalar parser
    (tails and trickles): every line is committed exactly once.
Writer part:
  * `delta_reference(block, lines)` vs `wd(block, lines)`: the writer form (no empty line, exact ": ",
    the block must end right after a CRLF, LF count == declared lines).
"""
import random
from typing import Literal, Union

Span = tuple[int, int, int, int]
Ok = tuple[Literal['ok'], int, list[Span]]
Err = tuple[Literal['err'], int]
More = tuple[Literal['more']]
Handoff = tuple[Literal['handoff'], int, list[Span]]
Result = Union[Ok, Err, More]
LineResult = tuple[Literal['ok', 'err', 'more'], int]
Masks = dict[str, int]

TCHAR = set(b"!#$%&'*+-.^_`|~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz")
FIELD = set([9, 32]) | set(range(0x21, 0x7F)) | set(range(0x80, 0x100))
OWS = (9, 32)
M64 = (1 << 64) - 1
BLOCK = 64


def trim(buf: bytes, start: int, end: int) -> tuple[int, int]:
    while start < end and buf[start] in OWS:
        start += 1
    while end > start and buf[end - 1] in OWS:
        end -= 1
    return start, end


# ---------------------------------------------------------------- scalar parser (reference and tail parser)

def scalar_lines(buf: bytes, i: int, spans: list[Span]) -> LineResult:
    """Parse complete field lines from line start `i`, appending spans. Returns
    ('ok', end) | ('err', at) | ('more', line_start_of_the_partial_line)."""
    n = len(buf)
    while True:
        if i >= n:
            return ('more', i)
        if buf[i] == 13:
            if i + 1 >= n:
                return ('more', i)
            return ('ok', i + 2) if buf[i + 1] == 10 else ('err', i + 1)
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


def reference(buf: bytes) -> Result:
    spans: list[Span] = []
    r = scalar_lines(buf, 0, spans)
    if r[0] == 'ok':
        return ('ok', r[1], spans)
    return ('err', r[1]) if r[0] == 'err' else ('more',)


# ---------------------------------------------------------------- P-D

def masks(block: bytes) -> Masks:
    m: Masks = dict(cr=0, lf=0, colon=0, t=0, v=0)
    for k, b in enumerate(block):
        bit = 1 << k
        if b == 13:
            m['cr'] |= bit
        if b == 10:
            m['lf'] |= bit
        if b == 58:
            m['colon'] |= bit
        if b in TCHAR:
            m['t'] |= bit
        if b in FIELD:
            m['v'] |= bit
    return m


def bits(x: int) -> list[int]:
    out = []
    while x:
        low = x & -x
        out.append(low.bit_length() - 1)
        x ^= low
    return out


def pd_run(buf: bytes, start: int, final: bool) -> Union[Ok, Err, Handoff]:
    """P-D from line start `start` over complete blocks (and, when `final`, the received tail block).
    Returns ('ok', end, spans) | ('err', at) | ('handoff', line_start, spans): spans of committed lines only."""
    lf_c, cr_c, n_c, p_c, v_c, end_pending = 1, 0, 0, 0, 0, False   # the 6-bit state
    starts: list[int] = []
    colons: list[int] = []
    vfrom: list[int] = []
    vend: list[int] = []
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
        err &= limit & received
        if err:
            return ('err', base + bits(err)[0])           # real impl: re-run the block on the scalar parser
        keep = limit & received
        starts += [base + k for k in bits(S & m['t'] & keep)]
        colons += [base + k for k in bits(N & keep)]
        vfrom += [base + k for k in bits(P & keep)]
        vend += [base + k for k in bits(E & keep)]
        for k in bits(S & keep):
            last_line_start = base + k
        lf_c, cr_c, p_c = m['lf'] >> 63, m['cr'] >> 63, N >> 63
        if end_pending:
            return finish(buf, base + 1, starts, colons, vfrom, vend)
        H = S & m['cr'] & received
        if H:
            h = (H & -H).bit_length() - 1
            if h == 63:
                end_pending = True                      # the LF is bit 0 of the next block
                base += BLOCK
                continue
            if h + 1 < len(block):
                return finish(buf, base + h + 2, starts, colons, vfrom, vend)
            return handoff(buf, base + h, starts, colons, vfrom, vend)   # CR received, LF not yet
        base += BLOCK
    return handoff(buf, last_line_start, starts, colons, vfrom, vend)


def zip_spans(buf: bytes, limit: int, starts: list[int], colons: list[int], vfrom: list[int],
              vend: list[int]) -> list[Span]:
    spans: list[Span] = []
    for k, s in enumerate(starts):
        if s >= limit:
            break
        vs, ve = trim(buf, vfrom[k], vend[k])
        spans.append((s, colons[k], vs, ve))
    return spans


def finish(buf: bytes, end: int, *streams: list[int]) -> Ok:
    return ('ok', end, zip_spans(buf, end, *streams))


def handoff(buf: bytes, line_start: int, *streams: list[int]) -> Handoff:
    return ('handoff', line_start, zip_spans(buf, line_start, *streams))


def pd_whole(buf: bytes) -> Result:
    r = pd_run(buf, 0, True)
    if r[0] == 'handoff':                                # the scalar parser finishes the partial line
        spans = r[2]
        t = scalar_lines(buf, r[1], spans)
        return ('ok', t[1], spans) if t[0] == 'ok' else (('err', t[1]) if t[0] == 'err' else ('more',))
    return r


def pd_split(buf: bytes, cuts: list[int]) -> Result:
    """Feed `buf` in pieces; P-D runs whenever >= 64 bytes lie beyond the committed line start, the
    scalar parser otherwise. `pos` is always a line start and `spans` holds every committed line once."""
    pos = 0
    spans: list[Span] = []
    for end in cuts + [len(buf)]:
        window = buf[:end]
        while True:
            if len(window) - pos >= BLOCK:
                r = pd_run(window, pos, False)
                if r[0] != 'handoff':
                    return (r[0], r[1], spans + r[2]) if r[0] == 'ok' else r
                spans += r[2]
                if r[1] != pos:
                    pos = r[1]
                    continue
                # no line ends inside the complete blocks: the scalar parser takes this line
            t = scalar_lines(window, pos, spans)
            if t[0] == 'ok':
                return ('ok', t[1], spans)
            if t[0] == 'err':
                return t
            pos = t[1]
            break
        # the partial line is re-validated by the scalar parser on the next call (the real G resumes mid-line)
        partial: list[Span] = []
        t = scalar_lines(window, pos, partial)
        if t[0] == 'err':
            return t
    return ('more',)


# ---------------------------------------------------------------- W-D (writer form)

def delta_reference(block: bytes, lines: int) -> bool:
    i, n, count = 0, len(block), 0
    while i < n:
        j = i
        while j < n and block[j] in TCHAR:
            j += 1
        if j == i or block[j:j + 2] != b": ":
            return False
        k = j + 2
        while k < n and block[k] in FIELD:
            k += 1
        if block[k:k + 2] != b"\r\n":
            return False
        i, count = k + 2, count + 1
    return count == lines


def wd(block: bytes, lines: int) -> bool:
    lf_c, cr_c, n_c, p_c, v_c = 1, 0, 0, 0, 0
    lf_count = 0
    for base in range(0, len(block), BLOCK):
        chunk = block[base:base + BLOCK]
        received = (1 << len(chunk)) - 1
        m = masks(chunk)
        sp = sum(1 << k for k, b in enumerate(chunk) if b == 32)
        S = ((m['lf'] << 1) | lf_c) & M64
        A = ((m['cr'] << 1) | cr_c) & M64
        s1 = (S & m['t']) + m['t'] + n_c
        n_c, N = s1 >> 64, s1 & M64 & ~m['t']
        N1 = ((N << 1) | (p_c >> 1)) & M64             # the byte after the colon must be SP
        P = ((N << 2) | p_c) & M64                      # the value starts after ": "
        s2 = P + m['v'] + v_c
        v_c, E = s2 >> 64, s2 & M64 & ~m['v']
        err = (N & ~m['colon']) | (N1 & ~sp) | (E & ~m['cr']) | (A ^ m['lf']) | (S & ~m['t'])
        if err & received:
            return False
        lf_count += bin(m['lf']).count('1')
        lf_c, cr_c, p_c = m['lf'] >> 63, m['cr'] >> 63, N >> 62   # 2 bits: colons at 62 and 63
    ends_on_line = len(block) == 0 or block[-1] == 10
    return ends_on_line and n_c == 0 and v_c == 0 and lf_count == lines


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


def gen_delta(rng: random.Random) -> tuple[bytes, int]:
    names = [b"content-type", b"x-request-id", b"authorization", b"a", b"x-" + b"n" * 70]
    vals = [b"", b"application/json", b"https://x/y?z=1", b"Bearer " + b"t" * 60, b"\t v \t"]
    out = bytearray()
    lines = rng.randrange(0, 6)
    for _ in range(lines):
        out += rng.choice(names) + b": " + rng.choice(vals) + b"\r\n"
    mutate(rng, out)
    return bytes(out), lines + rng.choice([0, 0, 0, 1, -1])


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
    return len(regressions)


def check_writer(rng: random.Random, cases: int) -> int:
    smuggle = b"a: b\r\n\r\nGET /admin HTTP/1.1\r\nhost: internal\r\n"
    assert not wd(smuggle, 1) and not delta_reference(smuggle, 1)
    unterminated = b"a: " + b"v" * 61
    assert not wd(unterminated, 0) and not wd(unterminated, 1)
    for k in (0, 1):
        assert not wd(b": x\r\n", k) and not delta_reference(b": x\r\n", k)   # empty name
    accepted = 0
    for t in range(cases):
        block, lines = gen_delta(rng)
        r = delta_reference(block, lines)
        assert wd(block, lines) == r, (t, block, lines, r)
        accepted += r
    return accepted


if __name__ == "__main__":
    rng = random.Random(9112)
    print("parser", check_parser(rng, 100_000))
    print("parser regressions + boundary sweep", check_parser_boundaries())
    print("writer accepted", check_writer(rng, 100_000), "of 100000")
