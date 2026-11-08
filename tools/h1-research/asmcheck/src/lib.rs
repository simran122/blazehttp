//! Compile-only check: does safe Rust (no intrinsics, no unsafe) lower 64-byte block classification to SIMD
//! compare + mask extraction? Inspect with `--emit asm`; nothing here is run.
#![forbid(unsafe_code)]

/// Form 1: iterator fold, one bit per byte.
#[inline(never)]
pub fn eq_mask_fold(block: &[u8; 64], c: u8) -> u64 {
    block
        .iter()
        .enumerate()
        .fold(0_u64, |m, (i, &b)| m | (u64::from(b == c) << i))
}

/// Form 2: per 16-byte chunk to a u16, then combine.
#[inline(never)]
pub fn eq_mask_chunks16(block: &[u8; 64], c: u8) -> u64 {
    let (chunks, _) = block.as_chunks::<16>();
    let mut m = 0_u64;
    for (k, chunk) in chunks.iter().enumerate() {
        let mut part = 0_u16;
        for (i, &b) in chunk.iter().enumerate() {
            part |= u16::from(b == c) << i;
        }
        m |= u64::from(part) << (16 * k);
    }
    m
}

/// Form 3: the four classes a strict head scan needs, in one loop.
#[derive(Clone, Copy, Default)]
pub struct Masks {
    pub colon: u64,
    pub cr: u64,
    pub lf: u64,
    /// Not field content: CTL except HTAB, and DEL (CR and LF included).
    pub bad_value: u64,
}

#[inline(never)]
pub fn class_masks(block: &[u8; 64]) -> Masks {
    let mut m = Masks::default();
    for (i, &b) in block.iter().enumerate() {
        m.colon |= u64::from(b == b':') << i;
        m.cr |= u64::from(b == b'\r') << i;
        m.lf |= u64::from(b == b'\n') << i;
        m.bad_value |= u64::from((b < 0x20 && b != b'\t') || b == 0x7F) << i;
    }
    m
}

/// Form 4: exact SWAR per 8-byte word plus a multiply gather of the high bits (scalar, every target).
const LO7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
const HI: u64 = 0x8080_8080_8080_8080;
const LANES: u64 = 0x0101_0101_0101_0101;
const GATHER: u64 = 0x0102_0408_1020_4080;

#[inline]
fn eq_exact(x: u64, c: u8) -> u64 {
    let t = x ^ LANES.wrapping_mul(u64::from(c));
    !(((t & LO7).wrapping_add(LO7)) | t) & HI
}

#[inline]
fn gather(flags: u64) -> u64 {
    (flags >> 7).wrapping_mul(GATHER) >> 56
}

#[inline(never)]
pub fn eq_mask_swar(block: &[u8; 64], c: u8) -> u64 {
    let (words, _) = block.as_chunks::<8>();
    words.iter().enumerate().fold(0_u64, |m, (k, w)| {
        m | (gather(eq_exact(u64::from_le_bytes(*w), c)) << (8 * k))
    })
}

/// Form 5: table-driven class (gather loads; expected to stay scalar).
pub static TCHAR: [bool; 256] = {
    let mut t = [false; 256];
    let mut c: u8 = 0;
    loop {
        t[c as usize] = c.is_ascii_alphanumeric()
            || matches!(c, b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~');
        if c == u8::MAX {
            break;
        }
        c += 1;
    }
    t
};

#[inline(never)]
pub fn not_tchar_mask_table(block: &[u8; 64]) -> u64 {
    block.iter().enumerate().fold(0_u64, |m, (i, &b)| {
        m | (u64::from(!TCHAR.get(usize::from(b)).copied().unwrap_or(false)) << i)
    })
}

/// Branch-free validation fold over a span (off the critical path): true when every byte is TCHAR.
#[inline(never)]
pub fn all_tchar(s: &[u8]) -> bool {
    s.iter().fold(true, |ok, &b| ok & TCHAR.get(usize::from(b)).copied().unwrap_or(false))
}

/// ScanThru: advance markers `m` through runs of class `c`, with the carry from the previous block.
#[inline(never)]
pub fn scan_thru(m: u64, c: u64, carry: bool) -> (u64, bool) {
    let (s, out) = m.carrying_add(c, carry);
    (s & !c, out)
}

/// Branch-free field-content check over a span, 8 bytes per step (SWAR, exact).
#[inline(never)]
pub fn all_field_content(s: &[u8]) -> bool {
    let (words, tail) = s.as_chunks::<8>();
    let mut bad = 0_u64;
    for w in words {
        let x = u64::from_le_bytes(*w);
        // < 0x20 (exact, 7-bit lanes) and not HTAB, or DEL.
        let lt20 = !(x | (x & LO7).wrapping_add(LANES.wrapping_mul(0x60))) & HI;
        bad |= (lt20 & !eq_exact(x, b'\t')) | eq_exact(x, 0x7F);
    }
    let tail_ok = tail.iter().all(|&b| !((b < 0x20 && b != b'\t') || b == 0x7F));
    bad == 0 && tail_ok
}
