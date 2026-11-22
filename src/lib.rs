//! csv-reader 1.0.0, exact reimplementation as a Rust SIMD128 transducer.
//!
//! The parser reads UTF-16 code units (the same units JS strings are indexed
//! by, so every edge case lands exactly as the reference reads it) and emits
//! the answer's JSON text straight into a shared output buffer, so the driver
//! never builds JS values and never runs JSON.stringify: the file the harness
//! scores is written from the WASM memory as it stands.
//!
//! Semantics are csv-reader 1.0.0's own, copied from the reference
//! (baselines/csv-reader/vendor/csv-reader/index.js in the kit):
//!   - a U+FEFF at the very start of the text is dropped;
//!   - a record ends at LF or CRLF, and a line left with no characters is no record;
//!   - outside a quoted field a CR not followed by LF is dropped (stray_cr);
//!   - a field that opens with a quote runs to the next quote that is not doubled,
//!     "" inside it one quote, and each CRLF or lone CR inside it read as LF;
//!     what follows its closing quote, up to the next comma or line break, is
//!     appended as it stands (quote_junk, when the field ends); a quote that
//!     never closes is a literal character, and the text after it is read again
//!     as unquoted (unterminated_quote);
//!   - a quote inside an unquoted field is a literal character; fields stay
//!     strings, never trimmed or converted;
//!   - columns is the first record when header is true, else null; rows holds
//!     the records after it, each padded with "" (short_row, with fields: its
//!     count) or cut (long_row, with extra: the cut fields) to the first
//!     record's width;
//!   - each error is {code, line, row}, in the order met; line the 1-based line
//!     its record starts on, row its record's index in rows (null for the header).
//!
//! Speed comes from the transducer shape: the delimiter scan and the
//! JSON-escaping copy both run 8 UTF-16 units at a time under SIMD128, and a
//! field is only ever copied once, straight into the output buffer. Pure: no
//! clock, no randomness, no locale, nothing kept from one call to the next
//! except the output buffer the driver reads.

#![allow(clippy::all)]
use std::arch::wasm32::*;

// ------------------ driver-visible globals ------------------
// Single-threaded WASM: one call at a time, no reentrancy, so statics are safe.

static mut OUT: Vec<u8> = Vec::new();
static mut OUT_CASES: usize = 0;
static mut INBUF: Vec<u8> = Vec::new();
static mut LAST_START: usize = 0;
static mut LAST_END: usize = 0;
static mut TEXT16: Vec<u16> = Vec::new();

/// Start a new answers array: OUT becomes `[`.
#[no_mangle]
pub extern "C" fn csv_begin() {
    unsafe {
        OUT.clear();
        OUT_CASES = 0;
        OUT.push(b'[');
    }
}

/// Scratch for one case's UTF-16 bytes: ensure room for `cap` bytes plus slack
/// and return the write pointer. The driver copies the case's bytes there.
#[no_mangle]
pub extern "C" fn csv_input_ptr(cap: usize) -> *mut u8 {
    unsafe {
        if INBUF.capacity() < cap + 16 {
            INBUF.reserve(cap + 16);
        }
        INBUF.as_mut_ptr()
    }
}

/// Parse one case and append its JSON object to the answers array.
#[no_mangle]
pub extern "C" fn csv_parse(in_ptr: *const u16, in_len: usize, header: u32) {
    unsafe {
        let t = std::slice::from_raw_parts(in_ptr, in_len);
        // Room for this case's JSON: the text's bytes plus a third again
        // (quotes, escapes, the columns/rows/errors shell), so the emitter
        // never reallocates mid-case.
        let need = OUT.len() + in_len * 2 + (in_len * 2) / 3 + 4096;
        if OUT.capacity() < need {
            OUT.reserve(need - OUT.capacity());
        }
        // The case's JSON starts after the separator comma, if any.
        // The case's JSON starts after the separator comma, if any.
        if OUT_CASES > 0 {
            OUT.push(b',');
        }
        LAST_START = OUT.len();
        OUT_CASES += 1;
        parse_case16(t, header != 0, &mut OUT);
        LAST_END = OUT.len();
    }
}

/// Close the answers array and return its byte length.
#[no_mangle]
pub extern "C" fn csv_finish() -> usize {
    unsafe {
        OUT.push(b']');
        OUT.len()
    }
}

/// Pointer to the answers array's bytes (valid after csv_finish, until the next call).
#[no_mangle]
pub extern "C" fn csv_out_ptr() -> *const u8 {
    unsafe { OUT.as_ptr() }
}

/// Pointer to the last parsed case's JSON object.
#[no_mangle]
pub extern "C" fn csv_last_ptr() -> *const u8 {
    unsafe { OUT.as_ptr().add(LAST_START) }
}

/// Byte length of the last parsed case's JSON object.
#[no_mangle]
pub extern "C" fn csv_last_len() -> usize {
    unsafe { LAST_END - LAST_START }
}

// ------------------ raw cases.json transduce (v2) ------------------
//
// The driver's whole input file is copied to WASM once, and one call parses
// every case from the raw JSON bytes: no JSON.parse in JS, no per-case
// UTF-16 encode, no per-case call. The scanner is a minimal JSON reader
// for the shape the kit's generator emits:
//   [{"id":"...","input":{"text":"...","header":true}}, ...]
// Keys are matched by name at any depth/order; unknown values are skipped
// generically, so key order and extra fields do not matter. A case's text
// is unescaped to UTF-16 units exactly as JSON.parse would produce them:
//   - \" \\ \/ \b \f \n \r \t map to their single unit;
//   - \uXXXX yields that unit verbatim (a lone surrogate stays lone, a pair
//     stays two units — the CSV parser reads units, as JS strings do);
//   - raw UTF-8 multi-byte sequences decode to their unit(s), astral ones
//     to a surrogate pair.
// The input is the harness's own generated JSON, always valid; malformed
// bytes return 0 (the driver treats it as a failed run, as the house
// driver would throw on invalid JSON).

fn skip_ws(b: &[u8], mut pos: usize) -> usize {
    while pos < b.len() && matches!(b[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

/// Skip one JSON value starting at pos; returns the position past it, or
/// usize::MAX when the bytes are not a value.
fn json_skip(b: &[u8], mut pos: usize) -> usize {
    pos = skip_ws(b, pos);
    if pos >= b.len() {
        return usize::MAX;
    }
    match b[pos] {
        b'"' => {
            pos += 1;
            while pos < b.len() {
                let c = b[pos];
                if c == b'\\' {
                    pos += 2;
                    continue;
                }
                if c == b'"' {
                    return pos + 1;
                }
                pos += 1;
            }
            usize::MAX
        }
        b'{' => {
            pos += 1;
            pos = skip_ws(b, pos);
            if pos < b.len() && b[pos] == b'}' {
                return pos + 1;
            }
            loop {
                pos = skip_ws(b, pos);
                if pos >= b.len() || b[pos] != b'"' {
                    return usize::MAX;
                }
                pos = json_skip(b, pos);
                if pos == usize::MAX {
                    return usize::MAX;
                }
                pos = skip_ws(b, pos);
                if pos >= b.len() || b[pos] != b':' {
                    return usize::MAX;
                }
                pos = json_skip(b, pos + 1);
                if pos == usize::MAX {
                    return usize::MAX;
                }
                pos = skip_ws(b, pos);
                if pos >= b.len() {
                    return usize::MAX;
                }
                if b[pos] == b',' {
                    pos += 1;
                    continue;
                }
                if b[pos] == b'}' {
                    return pos + 1;
                }
                return usize::MAX;
            }
        }
        b'[' => {
            pos += 1;
            pos = skip_ws(b, pos);
            if pos < b.len() && b[pos] == b']' {
                return pos + 1;
            }
            loop {
                pos = json_skip(b, pos);
                if pos == usize::MAX {
                    return usize::MAX;
                }
                pos = skip_ws(b, pos);
                if pos >= b.len() {
                    return usize::MAX;
                }
                if b[pos] == b',' {
                    pos += 1;
                    continue;
                }
                if b[pos] == b']' {
                    return pos + 1;
                }
                return usize::MAX;
            }
        }
        b't' => {
            if b[pos..].starts_with(b"true") {
                pos + 4
            } else {
                usize::MAX
            }
        }
        b'f' => {
            if b[pos..].starts_with(b"false") {
                pos + 5
            } else {
                usize::MAX
            }
        }
        b'n' => {
            if b[pos..].starts_with(b"null") {
                pos + 4
            } else {
                usize::MAX
            }
        }
        _ => {
            let s = pos;
            while pos < b.len()
                && matches!(b[pos], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
            {
                pos += 1;
            }
            if pos == s {
                usize::MAX
            } else {
                pos
            }
        }
    }
}

/// Decode the JSON string starting at b[pos] == b'"' into UTF-16 units in
/// dst. Returns the position past the closing quote, or usize::MAX.
fn json_string_u16(b: &[u8], mut pos: usize, dst: &mut Vec<u16>) -> usize {
    let n = b.len();
    pos += 1; // past the opening quote
    let mut seg = pos; // start of the current plain-ASCII run
    while pos < n {
        let c = b[pos];
        if c == b'"' {
            dst.extend(b[seg..pos].iter().map(|&x| x as u16));
            return pos + 1;
        }
        if c == b'\\' {
            dst.extend(b[seg..pos].iter().map(|&x| x as u16));
            if pos + 1 >= n {
                return usize::MAX;
            }
            match b[pos + 1] {
                b'"' => {
                    dst.push(0x22);
                    pos += 2;
                }
                b'\\' => {
                    dst.push(0x5C);
                    pos += 2;
                }
                b'/' => {
                    dst.push(0x2F);
                    pos += 2;
                }
                b'b' => {
                    dst.push(0x08);
                    pos += 2;
                }
                b'f' => {
                    dst.push(0x0C);
                    pos += 2;
                }
                b'n' => {
                    dst.push(0x0A);
                    pos += 2;
                }
                b'r' => {
                    dst.push(0x0D);
                    pos += 2;
                }
                b't' => {
                    dst.push(0x09);
                    pos += 2;
                }
                b'u' => {
                    if pos + 5 >= n {
                        return usize::MAX;
                    }
                    let hex = |i: usize| -> u16 {
                        match b[pos + 2 + i] {
                            b'0'..=b'9' => (b[pos + 2 + i] - b'0') as u16,
                            b'a'..=b'f' => (b[pos + 2 + i] - b'a' + 10) as u16,
                            b'A'..=b'F' => (b[pos + 2 + i] - b'A' + 10) as u16,
                            _ => 0xFFFF,
                        }
                    };
                    dst.push((hex(0) << 12) | (hex(1) << 8) | (hex(2) << 4) | hex(3));
                    pos += 6;
                }
                _ => return usize::MAX,
            }
            seg = pos;
            continue;
        }
        if c < 0x80 {
            pos += 1;
            continue;
        }
        // Multi-byte UTF-8: flush the ASCII run, decode one code point.
        dst.extend(b[seg..pos].iter().map(|&x| x as u16));
        if c >= 0xC2 && c < 0xE0 && pos + 1 < n {
            dst.push((((c & 0x1F) as u32) << 6 | ((b[pos + 1] & 0x3F) as u32)) as u16);
            pos += 2;
        } else if c >= 0xE0 && c < 0xF0 && pos + 2 < n {
            let cp = (((c & 0x0F) as u32) << 12)
                | (((b[pos + 1] & 0x3F) as u32) << 6)
                | ((b[pos + 2] & 0x3F) as u32);
            dst.push(cp as u16);
            pos += 3;
        } else if c >= 0xF0 && c < 0xF5 && pos + 3 < n {
            let cp = (((c & 0x07) as u32) << 18)
                | (((b[pos + 1] & 0x3F) as u32) << 12)
                | (((b[pos + 2] & 0x3F) as u32) << 6)
                | ((b[pos + 3] & 0x3F) as u32);
            let v = cp - 0x10000;
            dst.push(0xD800 + ((v >> 10) as u16));
            dst.push(0xDC00 + ((v & 0x3FF) as u16));
            pos += 4;
        } else {
            return usize::MAX;
        }
        seg = pos;
    }
    usize::MAX
}

/// Read one object's `"key": value` pairs, calling the key handler for
/// each. `on` returns Ok(next pos) to continue the object, Err(()) to
/// abort the whole transduce. Returns the position past the closing `}`,
/// or usize::MAX.
fn json_object<F>(b: &[u8], mut pos: usize, mut on: F) -> usize
where
    F: FnMut(&[u8], usize) -> Result<usize, ()>,
{
    // b[pos] == b'{'
    pos += 1;
    pos = skip_ws(b, pos);
    if pos < b.len() && b[pos] == b'}' {
        return pos + 1;
    }
    loop {
        pos = skip_ws(b, pos);
        if pos >= b.len() || b[pos] != b'"' {
            return usize::MAX;
        }
        let ks = pos + 1;
        let mut ke = ks;
        while ke < b.len() && b[ke] != b'"' {
            if b[ke] == b'\\' {
                ke += 1;
            }
            ke += 1;
        }
        if ke >= b.len() {
            return usize::MAX;
        }
        let key = &b[ks..ke];
        pos = ke + 1;
        pos = skip_ws(b, pos);
        if pos >= b.len() || b[pos] != b':' {
            return usize::MAX;
        }
        pos = skip_ws(b, pos + 1);
        match on(key, pos) {
            Ok(next) => pos = next,
            Err(()) => return usize::MAX,
        }
        pos = skip_ws(b, pos);
        if pos >= b.len() {
            return usize::MAX;
        }
        if b[pos] == b',' {
            pos += 1;
            continue;
        }
        if b[pos] == b'}' {
            return pos + 1;
        }
        return usize::MAX;
    }
}

// ------------------ UTF-8 path (v3) ------------------
//
// The timed path works on bytes throughout. Delimiters (`,` `\n` `\r` `"`) are
// ASCII and never occur inside UTF-8 multi-byte sequences, so byte scanning is
// exactly equivalent to unit scanning; non-ASCII bytes are bulk-copied raw (as
// V8 JSON.stringify emits them). The JSON unescape and the CSV parse are two
// fused SIMD passes: each 16-byte chunk is scanned once for its delimiters
// and escape-needing bytes, and clean chunks go straight into the output
// buffer with one memcpy. A lone surrogate (\uXXXX escape) cannot be
// represented in UTF-8: if one is seen, that case falls back to the proven
// v2 UTF-16 transducer (parse_case16).

static mut TEXT8: Vec<u8> = Vec::new();

fn hex4(b: &[u8], at: usize) -> u16 {
    let h = |i: usize| -> u16 {
        match b[at + i] {
            b'0'..=b'9' => (b[at + i] - b'0') as u16,
            b'a'..=b'f' => (b[at + i] - b'a' + 10) as u16,
            b'A'..=b'F' => (b[at + i] - b'A' + 10) as u16,
            _ => 0xFFFF,
        }
    };
    (h(0) << 12) | (h(1) << 8) | (h(2) << 4) | h(3)
}

/// Handle the JSON escape starting at the backslash b[d]. Appends the decoded
/// UTF-8 bytes to dst. Returns the position past the escape, or None when the
/// bytes are not a valid escape. Sets *lone when a lone surrogate is seen
/// (dst then holds U+FFFD as a placeholder; the caller re-parses via UTF-16).
#[inline(always)]
fn handle_escape_u8(b: &[u8], d: usize, dst: &mut Vec<u8>, lone: &mut bool) -> Option<usize> {
    let n = b.len();
    if d + 1 >= n {
        return None;
    }
    match b[d + 1] {
        b'"' => {
            dst.push(b'"');
            Some(d + 2)
        }
        b'\\' => {
            dst.push(b'\\');
            Some(d + 2)
        }
        b'/' => {
            dst.push(b'/');
            Some(d + 2)
        }
        b'b' => {
            dst.push(0x08);
            Some(d + 2)
        }
        b'f' => {
            dst.push(0x0C);
            Some(d + 2)
        }
        b'n' => {
            dst.push(b'\n');
            Some(d + 2)
        }
        b'r' => {
            dst.push(b'\r');
            Some(d + 2)
        }
        b't' => {
            dst.push(b'\t');
            Some(d + 2)
        }
        b'u' => {
            if d + 5 >= n {
                return None;
            }
            let u = hex4(b, d + 2);
            if (0xD800..=0xDBFF).contains(&u) {
                if d + 11 < n && b[d + 6] == b'\\' && b[d + 7] == b'u' {
                    let v = hex4(b, d + 8);
                    if (0xDC00..=0xDFFF).contains(&v) {
                        let cp = 0x10000u32 + (((u as u32 - 0xD800) << 10) | (v as u32 - 0xDC00));
                        dst.push((0xF0 | (cp >> 18)) as u8);
                        dst.push((0x80 | ((cp >> 12) & 0x3F)) as u8);
                        dst.push((0x80 | ((cp >> 6) & 0x3F)) as u8);
                        dst.push((0x80 | (cp & 0x3F)) as u8);
                        return Some(d + 12);
                    }
                }
                *lone = true;
                dst.extend_from_slice(b"\xEF\xBF\xBD");
                Some(d + 6)
            } else if (0xDC00..=0xDFFF).contains(&u) {
                *lone = true;
                dst.extend_from_slice(b"\xEF\xBF\xBD");
                Some(d + 6)
            } else if u < 0x80 {
                dst.push(u as u8);
                Some(d + 6)
            } else if u < 0x800 {
                dst.push((0xC0 | (u >> 6)) as u8);
                dst.push((0x80 | (u & 0x3F)) as u8);
                Some(d + 6)
            } else {
                dst.push((0xE0 | (u >> 12)) as u8);
                dst.push((0x80 | ((u >> 6) & 0x3F)) as u8);
                dst.push((0x80 | (u & 0x3F)) as u8);
                Some(d + 6)
            }
        }
        _ => None,
    }
}

/// Decode the JSON string starting at b[pos] == b'"' into UTF-8 bytes in dst.
/// Returns (position past the closing quote, had_lone_surrogate), or
/// (usize::MAX, lone) when the bytes are not a valid string.
fn json_string_u8(b: &[u8], mut pos: usize, dst: &mut Vec<u8>) -> (usize, bool) {
    let n = b.len();
    pos += 1; // past the opening quote
    let mut lone = false;
    while pos + 16 <= n {
        let v = unsafe { v128_load(b.as_ptr().add(pos) as *const v128) };
        let m = v128_or(
            i8x16_eq(v, i8x16_splat(b'"' as i8)),
            i8x16_eq(v, i8x16_splat(b'\\' as i8)),
        );
        let bits = i8x16_bitmask(m) as u32;
        if bits == 0 {
            unsafe { bulk_copy(dst, b.as_ptr().add(pos), 16) };
            pos += 16;
            continue;
        }
        let d = pos + bits.trailing_zeros() as usize;
        unsafe { bulk_copy(dst, b.as_ptr().add(pos), d - pos) };
        if b[d] == b'"' {
            return (d + 1, lone);
        }
        match handle_escape_u8(b, d, dst, &mut lone) {
            Some(next) => pos = next,
            None => return (usize::MAX, lone),
        }
    }
    while pos < n {
        let c = b[pos];
        if c == b'"' {
            return (pos + 1, lone);
        }
        if c == b'\\' {
            match handle_escape_u8(b, pos, dst, &mut lone) {
                Some(next) => pos = next,
                None => return (usize::MAX, lone),
            }
            continue;
        }
        dst.push(c);
        pos += 1;
    }
    (usize::MAX, lone)
}

/// Bulk-copy n bytes from src into out (capacity checked). No-op for n == 0.
#[inline(always)]
unsafe fn bulk_copy(out: &mut Vec<u8>, src: *const u8, n: usize) {
    if n == 0 {
        return;
    }
    if out.len() + n > out.capacity() {
        out.reserve(n + 4096);
    }
    let dst = out.as_mut_ptr().add(out.len());
    std::ptr::copy_nonoverlapping(src, dst, n);
    out.set_len(out.len() + n);
}

/// Emit one byte as JSON-escaped UTF-8 (V8 JSON.stringify rules).
#[inline(always)]
fn emit_byte8(out: &mut Vec<u8>, c: u8) {
    match c {
        b'"' => out.extend_from_slice(b"\\\""),
        b'\\' => out.extend_from_slice(b"\\\\"),
        b'\n' => out.extend_from_slice(b"\\n"),
        b'\r' => out.extend_from_slice(b"\\r"),
        b'\t' => out.extend_from_slice(b"\\t"),
        0x08 => out.extend_from_slice(b"\\b"),
        0x0C => out.extend_from_slice(b"\\f"),
        0x00..=0x1F => {
            out.extend_from_slice(b"\\u00");
            let hex = b"0123456789abcdef";
            out.push(hex[(c >> 4) as usize]);
            out.push(hex[(c & 0xF) as usize]);
        }
        _ => out.push(c),
    }
}

/// Scalar JSON-escape emit of t[a..b]; returns the bytes kept (= b - a).
#[inline(always)]
fn emit_scalar8(out: &mut Vec<u8>, t: &[u8], a: usize, b: usize) -> usize {
    out.reserve((b - a) * 2 + 16);
    let mut i = a;
    while i < b {
        emit_byte8(out, t[i]);
        i += 1;
    }
    b - a
}

/// SIMD masks for the 16 bytes at ptr: (delim mask, escape mask).
/// Delimiters are d0/d1/d2; escape-needing bytes are `"` `\` and < 0x20
/// (via (v & 0xE0) == 0, which is false for every byte >= 0x80).
#[inline(always)]
unsafe fn masks16(ptr: *const u8, d0: u8, d1: u8, d2: u8) -> (u32, u32) {
    let v = v128_load(ptr as *const v128);
    let dm = v128_or(
        v128_or(i8x16_eq(v, i8x16_splat(d0 as i8)), i8x16_eq(v, i8x16_splat(d1 as i8))),
        i8x16_eq(v, i8x16_splat(d2 as i8)),
    );
    let em = v128_or(
        v128_or(i8x16_eq(v, i8x16_splat(0x22)), i8x16_eq(v, i8x16_splat(0x5C))),
        i8x16_eq(v128_and(v, i8x16_splat(0xE0u8 as i8)), i8x16_splat(0)),
    );
    (i8x16_bitmask(dm) as u32, i8x16_bitmask(em) as u32)
}

// ------------------ the UTF-8 parser (same semantics as parse_case16) ------------------

struct P8<'a> {
    t: &'a [u8],
    pos: usize,
    line: u32,
    rec_line: u32,
    rec_row: i64, // -1 means null (the header record)
    width: i64,   // -1 until the first record sets it
    err: Vec<u8>,
    err_n: usize,
}

impl<'a> P8<'a> {
    fn error(&mut self, code: &str) {
        if self.err_n > 0 {
            self.err.push(b',');
        }
        self.err_n += 1;
        self.err.extend_from_slice(b"{\"code\":\"");
        self.err.extend_from_slice(code.as_bytes());
        self.err.extend_from_slice(b"\",\"line\":");
        push_u32(&mut self.err, self.rec_line);
        self.err.extend_from_slice(b",\"row\":");
        if self.rec_row < 0 {
            self.err.extend_from_slice(b"null");
        } else {
            push_i64(&mut self.err, self.rec_row);
        }
        self.err.push(b'}');
    }

    fn error_short(&mut self, fields: usize) {
        if self.err_n > 0 {
            self.err.push(b',');
        }
        self.err_n += 1;
        self.err.extend_from_slice(b"{\"code\":\"short_row\",\"line\":");
        push_u32(&mut self.err, self.rec_line);
        self.err.extend_from_slice(b",\"row\":");
        if self.rec_row < 0 {
            self.err.extend_from_slice(b"null");
        } else {
            push_i64(&mut self.err, self.rec_row);
        }
        self.err.extend_from_slice(b",\"fields\":");
        push_u32(&mut self.err, fields as u32);
        self.err.push(b'}');
    }

    fn error_long(&mut self, extra: &[u8]) {
        if self.err_n > 0 {
            self.err.push(b',');
        }
        self.err_n += 1;
        self.err.extend_from_slice(b"{\"code\":\"long_row\",\"line\":");
        push_u32(&mut self.err, self.rec_line);
        self.err.extend_from_slice(b",\"row\":");
        if self.rec_row < 0 {
            self.err.extend_from_slice(b"null");
        } else {
            push_i64(&mut self.err, self.rec_row);
        }
        self.err.extend_from_slice(b",\"extra\":[");
        self.err.extend_from_slice(extra);
        self.err.extend_from_slice(b"]}");
    }
}

/// Read an unquoted field, appending its JSON-escaped content to `out`.
/// Returns (how it ended, bytes kept).
#[inline(always)]
fn read_unquoted8(p: &mut P8, out: &mut Vec<u8>) -> (u8, usize) {
    let t = p.t;
    let n = t.len();
    let mut added = 0usize;
    while p.pos + 16 <= n {
        let (dm, em) = unsafe { masks16(t.as_ptr().add(p.pos), b',', b'\n', b'\r') };
        if dm == 0 && em == 0 {
            unsafe { bulk_copy(out, t.as_ptr().add(p.pos), 16) };
            p.pos += 16;
            added += 16;
            continue;
        }
        if dm == 0 {
            added += emit_scalar8(out, t, p.pos, p.pos + 16);
            p.pos += 16;
            continue;
        }
        let di = dm.trailing_zeros() as usize;
        let em_before = em & ((1u32 << di) - 1);
        if em_before == 0 {
            unsafe { bulk_copy(out, t.as_ptr().add(p.pos), di) };
        } else {
            emit_scalar8(out, t, p.pos, p.pos + di);
        }
        added += di;
        let dpos = p.pos + di;
        let c = t[dpos];
        if c == b',' {
            p.pos = dpos + 1;
            return (COMMA, added);
        }
        if c == b'\n' {
            p.pos = dpos + 1;
            p.line += 1;
            return (LINE_BREAK, added);
        }
        // CR: a line break when followed by LF, else a stray CR, dropped.
        if dpos + 1 < n && t[dpos + 1] == b'\n' {
            p.pos = dpos + 2;
            p.line += 1;
            return (LINE_BREAK, added);
        }
        p.error("stray_cr");
        p.pos = dpos + 1;
    }
    while p.pos < n {
        let c = t[p.pos];
        if c == b',' {
            p.pos += 1;
            return (COMMA, added);
        }
        if c == b'\n' {
            p.pos += 1;
            p.line += 1;
            return (LINE_BREAK, added);
        }
        if c == b'\r' {
            if p.pos + 1 < n && t[p.pos + 1] == b'\n' {
                p.pos += 2;
                p.line += 1;
                return (LINE_BREAK, added);
            }
            p.error("stray_cr");
            p.pos += 1;
            continue;
        }
        emit_byte8(out, c);
        added += 1;
        p.pos += 1;
    }
    (END, added)
}

/// What follows a quoted field's closing quote (p.pos is just past it).
#[inline(always)]
fn post_quoted8(p: &mut P8, out: &mut Vec<u8>) -> (u8, usize) {
    if p.pos >= p.t.len() {
        return (END, 0);
    }
    let c = p.t[p.pos];
    if c == b',' {
        p.pos += 1;
        return (COMMA, 0);
    }
    if c == b'\n' {
        p.pos += 1;
        p.line += 1;
        return (LINE_BREAK, 0);
    }
    if c == b'\r' && p.pos + 1 < p.t.len() && p.t[p.pos + 1] == b'\n' {
        p.pos += 2;
        p.line += 1;
        return (LINE_BREAK, 0);
    }
    // What follows the closing quote is appended as it stands.
    let (ended, added) = read_unquoted8(p, out);
    if added > 0 {
        p.error("quote_junk");
    }
    (ended, added)
}

/// Read a quoted field whose opening quote is at p.pos, appending its
/// JSON-escaped content to `out`. Returns (how it ended, bytes kept in this
/// field, quoted content plus any junk).
#[inline(always)]
fn read_quoted8(p: &mut P8, out: &mut Vec<u8>) -> (u8, usize) {
    let open = p.pos;
    let open_line = p.line;
    let out_mark = out.len();
    p.pos += 1; // past the opening quote
    let mut added = 0usize;
    loop {
        while p.pos + 16 <= p.t.len() {
            let (dm, em) = unsafe { masks16(p.t.as_ptr().add(p.pos), b'"', b'\n', b'\r') };
            if dm == 0 && em == 0 {
                unsafe { bulk_copy(out, p.t.as_ptr().add(p.pos), 16) };
                p.pos += 16;
                added += 16;
                continue;
            }
            if dm == 0 {
                added += emit_scalar8(out, p.t, p.pos, p.pos + 16);
                p.pos += 16;
                continue;
            }
            let di = dm.trailing_zeros() as usize;
            let em_before = em & ((1u32 << di) - 1);
            if em_before == 0 {
                unsafe { bulk_copy(out, p.t.as_ptr().add(p.pos), di) };
            } else {
                emit_scalar8(out, p.t, p.pos, p.pos + di);
            }
            added += di;
            let dpos = p.pos + di;
            let c = p.t[dpos];
            if c == b'"' {
                if dpos + 1 < p.t.len() && p.t[dpos + 1] == b'"' {
                    // Doubled quote: one quote in the value.
                    out.extend_from_slice(b"\\\"");
                    added += 1;
                    p.pos = dpos + 2;
                    continue;
                }
                // Closing quote.
                p.pos = dpos + 1;
                let (ended, junk) = post_quoted8(p, out);
                return (ended, added + junk);
            }
            // CR or LF inside quotes: each one LF, each counts a line.
            out.extend_from_slice(b"\\n");
            added += 1;
            p.line += 1;
            if c == b'\r' && dpos + 1 < p.t.len() && p.t[dpos + 1] == b'\n' {
                p.pos = dpos + 2;
            } else {
                p.pos = dpos + 1;
            }
        }
        // Tail (< 16 bytes left): scalar scan for the next " / LF / CR.
        let mut i = p.pos;
        let mut found: Option<(usize, u8)> = None;
        while i < p.t.len() {
            let c = p.t[i];
            if c == b'"' || c == b'\n' || c == b'\r' {
                found = Some((i, c));
                break;
            }
            i += 1;
        }
        match found {
            None => {
                // The quote never closes: it is a literal character, and the
                // text after it is read again as unquoted.
                out.truncate(out_mark);
                p.pos = open + 1;
                p.line = open_line;
                p.error("unterminated_quote");
                out.extend_from_slice(b"\\\"");
                let (ended, uadded) = read_unquoted8(p, out);
                return (ended, uadded + 1);
            }
            Some((dpos, c)) => {
                added += emit_scalar8(out, p.t, p.pos, dpos);
                if c == b'"' {
                    if dpos + 1 < p.t.len() && p.t[dpos + 1] == b'"' {
                        out.extend_from_slice(b"\\\"");
                        added += 1;
                        p.pos = dpos + 2;
                        continue;
                    }
                    p.pos = dpos + 1;
                    let (ended, junk) = post_quoted8(p, out);
                    return (ended, added + junk);
                }
                out.extend_from_slice(b"\\n");
                added += 1;
                p.line += 1;
                if c == b'\r' && dpos + 1 < p.t.len() && p.t[dpos + 1] == b'\n' {
                    p.pos = dpos + 2;
                } else {
                    p.pos = dpos + 1;
                }
            }
        }
    }
}

/// Read one field (quoted or not) into `tgt` as a JSON string, `"..."`.
/// Returns (how it ended, whether it opened with a quote, bytes kept).
#[inline(always)]
fn read_field8(p: &mut P8, tgt: &mut Vec<u8>, cnt: &mut usize) -> (u8, bool, usize) {
    let quoted = p.pos < p.t.len() && p.t[p.pos] == b'"';
    if *cnt > 0 {
        tgt.push(b',');
    }
    tgt.push(b'"');
    let (ended, added) = if quoted {
        read_quoted8(p, tgt)
    } else {
        read_unquoted8(p, tgt)
    };
    tgt.push(b'"');
    *cnt += 1;
    (ended, quoted, added)
}

/// Parse one case's UTF-8 CSV text, emitting its JSON object straight into
/// `out` (the answers buffer). No per-record buffer: rows are emitted
/// directly, with a truncate back to the record's start mark when a record
/// turns out to hold only stray CRs (its stray_cr errors stay in p.err).
fn parse_case8(t: &[u8], header: bool, out: &mut Vec<u8>) {
    let mut p = P8 {
        t,
        pos: 0,
        line: 1,
        rec_line: 1,
        rec_row: -1,
        width: -1,
        err: Vec::new(),
        err_n: 0,
    };
    if t.len() >= 3 && t[0] == 0xEF && t[1] == 0xBB && t[2] == 0xBF {
        p.pos = 3;
    }
    let mut seen_first = false;
    let mut rows_n = 0usize;
    let mut extra: Vec<u8> = Vec::new();
    out.extend_from_slice(b"{\"columns\":");
    if !header {
        out.extend_from_slice(b"null,\"rows\":[");
    }
    loop {
        // Lines with no characters are no records.
        while p.pos < t.len() {
            let c = t[p.pos];
            if c == b'\n' {
                p.pos += 1;
                p.line += 1;
            } else if c == b'\r' && p.pos + 1 < t.len() && t[p.pos + 1] == b'\n' {
                p.pos += 2;
                p.line += 1;
            } else {
                break;
            }
        }
        if p.pos >= t.len() {
            break;
        }
        p.rec_line = p.line;
        p.rec_row = if header && !seen_first { -1 } else { rows_n as i64 };
        let is_columns = header && !seen_first;
        let mark = out.len();
        if !is_columns && rows_n > 0 {
            out.push(b',');
        }
        out.push(b'[');
        extra.clear();
        let mut row_n = 0usize;
        let mut extra_n = 0usize;
        let mut field_idx = 0usize;
        let mut quoted_last = false;
        let mut first_added = 0usize;
        loop {
            let to_extra = p.width >= 0 && (field_idx as i64) >= p.width;
            let (ended, quoted, added) = if to_extra {
                read_field8(&mut p, &mut extra, &mut extra_n)
            } else {
                read_field8(&mut p, out, &mut row_n)
            };
            quoted_last = quoted;
            if field_idx == 0 {
                first_added = added;
            }
            field_idx += 1;
            if ended != COMMA {
                break;
            }
        }
        // A line that held only stray CRs is left with no characters: no record.
        if field_idx == 1 && !quoted_last && first_added == 0 {
            out.truncate(mark);
            continue;
        }
        if p.width < 0 {
            p.width = field_idx as i64;
        }
        if (field_idx as i64) < p.width {
            for _ in field_idx..(p.width as usize) {
                out.extend_from_slice(b",\"\"");
            }
            p.error_short(field_idx);
        } else if (field_idx as i64) > p.width {
            p.error_long(&extra);
        }
        out.push(b']');
        if is_columns {
            seen_first = true;
            out.extend_from_slice(b",\"rows\":[");
        } else {
            if !seen_first {
                seen_first = true;
            }
            rows_n += 1;
        }
    }
    if !seen_first && header {
        out.extend_from_slice(b"[],\"rows\":[");
    }
    out.extend_from_slice(b"],\"errors\":[");
    out.extend_from_slice(&p.err);
    out.extend_from_slice(b"]}");
}

/// Parse one UTF-8 case and append its JSON object to the answers array.
#[no_mangle]
pub extern "C" fn csv_parse8(in_ptr: *const u8, in_len: usize, header: u32) {
    unsafe {
        let t = std::slice::from_raw_parts(in_ptr, in_len);
        let need = OUT.len() + in_len + in_len / 3 + 4096;
        if OUT.capacity() < need {
            OUT.reserve(need - OUT.capacity());
        }
        if OUT_CASES > 0 {
            OUT.push(b',');
        }
        LAST_START = OUT.len();
        OUT_CASES += 1;
        parse_case8(t, header != 0, &mut OUT);
        LAST_END = OUT.len();
    }
}

/// Transduce a whole cases.json file (v3): parse every case from the raw
/// bytes and append each answer's JSON to the answers array. The text of
/// each case is unescaped straight to UTF-8 bytes; a lone surrogate in a
/// \uXXXX escape falls back to the UTF-16 transducer for that case.
/// Returns the answers byte length (0 on malformed input); the bytes sit
/// at csv_out_ptr().
#[no_mangle]
pub extern "C" fn csv_transduce(in_ptr: *const u8, in_len: usize) -> usize {
    unsafe {
        let b = std::slice::from_raw_parts(in_ptr, in_len);
        OUT.clear();
        OUT_CASES = 0;
        OUT.push(b'[');
        OUT.reserve(in_len + in_len / 3 + 65536);
        TEXT8.clear();
        TEXT8.reserve(in_len);
        TEXT16.clear();
        let mut pos = skip_ws(b, 0);
        if pos >= b.len() || b[pos] != b'[' {
            return 0;
        }
        pos += 1;
        pos = skip_ws(b, pos);
        if pos < b.len() && b[pos] == b']' {
            OUT.push(b']');
            return OUT.len();
        }
        loop {
            pos = skip_ws(b, pos);
            if pos >= b.len() || b[pos] != b'{' {
                return 0;
            }
            let mut header = false;
            let mut have_text = false;
            let mut lone = false;
            let mut text_start = 0usize;
            // One case object; "text"/"header" may sit here or inside "input".
            let after = json_object(b, pos, |key, vpos| {
                if key == b"text" {
                    if vpos >= b.len() || b[vpos] != b'"' {
                        return Err(());
                    }
                    text_start = vpos;
                    TEXT8.clear();
                    let (next, l) = json_string_u8(b, vpos, &mut TEXT8);
                    if next == usize::MAX {
                        return Err(());
                    }
                    lone = l;
                    have_text = true;
                    Ok(next)
                } else if key == b"header" {
                    if b[vpos..].starts_with(b"true") {
                        header = true;
                        Ok(vpos + 4)
                    } else if b[vpos..].starts_with(b"false") {
                        header = false;
                        Ok(vpos + 5)
                    } else {
                        Err(())
                    }
                } else if key == b"input" {
                    if vpos >= b.len() || b[vpos] != b'{' {
                        return Err(());
                    }
                    let next = json_object(b, vpos, |key2, vpos2| {
                        if key2 == b"text" {
                            if vpos2 >= b.len() || b[vpos2] != b'"' {
                                return Err(());
                            }
                            text_start = vpos2;
                            TEXT8.clear();
                            let (next2, l) = json_string_u8(b, vpos2, &mut TEXT8);
                            if next2 == usize::MAX {
                                return Err(());
                            }
                            lone = l;
                            have_text = true;
                            Ok(next2)
                        } else if key2 == b"header" {
                            if b[vpos2..].starts_with(b"true") {
                                header = true;
                                Ok(vpos2 + 4)
                            } else if b[vpos2..].starts_with(b"false") {
                                header = false;
                                Ok(vpos2 + 5)
                            } else {
                                Err(())
                            }
                        } else {
                            let next2 = json_skip(b, vpos2);
                            if next2 == usize::MAX {
                                Err(())
                            } else {
                                Ok(next2)
                            }
                        }
                    });
                    if next == usize::MAX {
                        Err(())
                    } else {
                        Ok(next)
                    }
                } else {
                    let next = json_skip(b, vpos);
                    if next == usize::MAX {
                        Err(())
                    } else {
                        Ok(next)
                    }
                }
            });
            if after == usize::MAX || !have_text {
                return 0;
            }
            pos = after;
            if OUT_CASES > 0 {
                OUT.push(b',');
            }
            LAST_START = OUT.len();
            OUT_CASES += 1;
            if lone {
                // Lone surrogate: re-decode the same span to UTF-16 and run
                // the proven v2 transducer for this case.
                TEXT16.clear();
                if json_string_u16(b, text_start, &mut TEXT16) == usize::MAX {
                    return 0;
                }
                let text_len = TEXT16.len();
                let need = OUT.len() + text_len * 2 + (text_len * 2) / 3 + 4096;
                if OUT.capacity() < need {
                    OUT.reserve(need - OUT.capacity());
                }
                let t = std::slice::from_raw_parts(TEXT16.as_ptr(), text_len);
                parse_case16(t, header, &mut OUT);
            } else {
                let text_len = TEXT8.len();
                let need = OUT.len() + text_len + text_len / 3 + 4096;
                if OUT.capacity() < need {
                    OUT.reserve(need - OUT.capacity());
                }
                let t = std::slice::from_raw_parts(TEXT8.as_ptr(), text_len);
                parse_case8(t, header, &mut OUT);
            }
            LAST_END = OUT.len();
            pos = skip_ws(b, pos);
            if pos >= b.len() {
                return 0;
            }
            if b[pos] == b',' {
                pos += 1;
                continue;
            }
            if b[pos] == b']' {
                OUT.push(b']');
                return OUT.len();
            }
            return 0;
        }
    }
}

// ------------------ small number formatting (no format!) ------------------

fn push_u32(out: &mut Vec<u8>, mut v: u32) {
    if v == 0 {
        out.push(b'0');
        return;
    }
    let mut buf = [0u8; 10];
    let mut i = 10;
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    out.extend_from_slice(&buf[i..]);
}

fn push_i64(out: &mut Vec<u8>, v: i64) {
    if v < 0 {
        out.push(b'-');
        push_u32(out, (-v) as u32);
    } else {
        push_u32(out, v as u32);
    }
}

fn push_unicode_escape(out: &mut Vec<u8>, u: u16) {
    out.extend_from_slice(b"\\u");
    let hex = b"0123456789abcdef";
    out.push(hex[((u >> 12) & 0xF) as usize]);
    out.push(hex[((u >> 8) & 0xF) as usize]);
    out.push(hex[((u >> 4) & 0xF) as usize]);
    out.push(hex[(u & 0xF) as usize]);
}

// ------------------ JSON string emission ------------------

/// Emit one UTF-16 unit (possibly a surrogate pair with t[i+1], when i+1 < end)
/// as JSON-escaped UTF-8. Returns the units consumed (1 or 2).
#[inline(always)]
fn emit_unit_at(out: &mut Vec<u8>, t: &[u16], i: usize, end: usize) -> usize {
    let u = t[i];
    match u {
        0x22 => {
            out.extend_from_slice(b"\\\"");
            1
        }
        0x5C => {
            out.extend_from_slice(b"\\\\");
            1
        }
        0x0A => {
            out.extend_from_slice(b"\\n");
            1
        }
        0x0D => {
            out.extend_from_slice(b"\\r");
            1
        }
        0x09 => {
            out.extend_from_slice(b"\\t");
            1
        }
        0x08 => {
            out.extend_from_slice(b"\\b");
            1
        }
        0x0C => {
            out.extend_from_slice(b"\\f");
            1
        }
        0x00..=0x1F => {
            out.extend_from_slice(b"\\u00");
            let hex = b"0123456789abcdef";
            out.push(hex[(u >> 4) as usize]);
            out.push(hex[(u & 0xF) as usize]);
            1
        }
        0x00..=0x7F => {
            out.push(u as u8);
            1
        }
        0xD800..=0xDBFF => {
            if i + 1 < end {
                let v = t[i + 1];
                if (0xDC00..=0xDFFF).contains(&v) {
                    let cp = 0x10000u32 + (((u as u32 - 0xD800) << 10) | (v as u32 - 0xDC00));
                    out.push((0xF0 | (cp >> 18)) as u8);
                    out.push((0x80 | ((cp >> 12) & 0x3F)) as u8);
                    out.push((0x80 | ((cp >> 6) & 0x3F)) as u8);
                    out.push((0x80 | (cp & 0x3F)) as u8);
                    return 2;
                }
            }
            push_unicode_escape(out, u);
            1
        }
        0xDC00..=0xDFFF => {
            push_unicode_escape(out, u);
            1
        }
        _ => {
            if u < 0x800 {
                out.push((0xC0 | (u >> 6)) as u8);
                out.push((0x80 | (u & 0x3F)) as u8);
            } else {
                out.push((0xE0 | (u >> 12)) as u8);
                out.push((0x80 | ((u >> 6) & 0x3F)) as u8);
                out.push((0x80 | (u & 0x3F)) as u8);
            }
            1
        }
    }
}

/// Append the JSON-escaped UTF-8 of t[a..b] to out. Returns the units emitted.
#[inline(always)]
fn emit_segment(out: &mut Vec<u8>, t: &[u16], a: usize, b: usize) -> usize {
    if a >= b {
        return 0;
    }
    let n_units = b - a;
    // The fast path below writes 8 bytes at a time through a raw pointer, so
    // capacity must be there BEFORE it runs. This covers every buffer a
    // segment can land in (the answers buffer, and the per-record row/extra
    // buffers, which start at 256 bytes): the per-case reserve alone only
    // covers the answers buffer, and without this a long field corrupts the
    // row buffer's heap block (dlmalloc trap on drop — found 22 Nov 22:47).
    out.reserve(n_units * 3 + 64);
    let mut p = a;
    while p + 8 <= b {
        unsafe {
            let v = v128_load(t.as_ptr().add(p) as *const v128);
            // All ASCII? (v & 0xFF80) == 0 for every lane.
            if !v128_any_true(v128_and(v, i16x8_splat(0xFF80u16 as i16))) {
                let bytes = i8x16_narrow_i16x8(v, v);
                // Escape-needing bytes: '"' (0x22), '\\' (0x5C), or < 0x20.
                let q = i8x16_eq(bytes, i8x16_splat(0x22));
                let bs = i8x16_eq(bytes, i8x16_splat(0x5C));
                let ctl = i8x16_eq(
                    v128_and(bytes, i8x16_splat(0xE0u8 as i8)),
                    i8x16_splat(0),
                );
                let esc = v128_or(v128_or(q, bs), ctl);
                if !v128_any_true(esc) {
                    let dst = out.as_mut_ptr().add(out.len());
                    std::ptr::copy_nonoverlapping(&bytes as *const v128 as *const u8, dst, 8);
                    out.set_len(out.len() + 8);
                    p += 8;
                    continue;
                }
            }
        }
        // Slow path: 8 units, one at a time (non-ASCII or escape-needing).
        // Advance by what was actually consumed: a surrogate pair starting
        // at the chunk's last unit takes 2, so p must move past it, not by 8,
        // or its low half would be emitted again as a lone surrogate.
        let mut i = 0;
        while i < 8 {
            i += emit_unit_at(out, t, p + i, b);
        }
        p += i;
    }
    while p < b {
        p += emit_unit_at(out, t, p, b);
    }
    n_units
}

/// Emit a surrogate pair as one astral character in raw UTF-8.
#[inline(always)]
fn emit_pair_utf8(out: &mut Vec<u8>, hi: u16, lo: u16) {
    let cp = 0x10000u32 + (((hi as u32 - 0xD800) << 10) | (lo as u32 - 0xDC00));
    out.push((0xF0 | (cp >> 18)) as u8);
    out.push((0x80 | ((cp >> 12) & 0x3F)) as u8);
    out.push((0x80 | ((cp >> 6) & 0x3F)) as u8);
    out.push((0x80 | (cp & 0x3F)) as u8);
}

/// Emit t[a..b] like emit_segment, but with value-adjacent surrogate
/// pairing: a high surrogate that ends the span is held, not written, and
/// pairs with a low surrogate that starts the next span (the two can be
/// adjacent in the value while a dropped stray CR or a closing quote sits
/// between them in the text). Returns the units kept (= b - a).
#[inline(always)]
fn emit_span_pend(p: &mut P, out: &mut Vec<u8>, t: &[u16], a: usize, b: usize) -> usize {
    if a >= b {
        return 0;
    }
    let mut start = a;
    if p.pend_hi != 0 {
        if (0xDC00..=0xDFFF).contains(&t[a]) {
            let hi = p.pend_hi;
            p.pend_hi = 0;
            emit_pair_utf8(out, hi, t[a]);
            start = a + 1;
            if start >= b {
                return b - a;
            }
        } else {
            p.flush_pend(out);
        }
    }
    if (0xD800..=0xDBFF).contains(&t[b - 1]) {
        emit_segment(out, t, start, b - 1);
        p.pend_hi = t[b - 1];
    } else {
        emit_segment(out, t, start, b);
    }
    b - a
}

// ------------------ delimiter scan (SIMD128) ------------------

/// The next position at or after `pos` holding one of d0, d1, d2 (or None).
#[target_feature(enable = "simd128")]
unsafe fn find_delim(t: &[u16], mut pos: usize, d0: u16, d1: u16, d2: u16) -> Option<(usize, u16)> {
    let n = t.len();
    if pos >= n {
        return None;
    }
    let ptr = t.as_ptr();
    let s0 = i16x8_splat(d0 as i16);
    let s1 = i16x8_splat(d1 as i16);
    let s2 = i16x8_splat(d2 as i16);
    while pos + 8 <= n {
        let v = v128_load(ptr.add(pos) as *const v128);
        let m = v128_or(v128_or(i16x8_eq(v, s0), i16x8_eq(v, s1)), i16x8_eq(v, s2));
        let bits = i16x8_bitmask(m) as u32;
        if bits != 0 {
            let d = pos + bits.trailing_zeros() as usize;
            return Some((d, *t.get_unchecked(d)));
        }
        pos += 8;
    }
    if pos < n {
        if n >= 8 {
            // Overlapped chunk ending at n; mask off lanes before pos.
            let start = n - 8;
            let v = v128_load(ptr.add(start) as *const v128);
            let m = v128_or(v128_or(i16x8_eq(v, s0), i16x8_eq(v, s1)), i16x8_eq(v, s2));
            let mut bits = i16x8_bitmask(m) as u32;
            let skip = pos - start;
            if skip > 0 {
                bits &= !((1u32 << skip) - 1);
            }
            if bits != 0 {
                let d = start + bits.trailing_zeros() as usize;
                return Some((d, *t.get_unchecked(d)));
            }
        } else {
            while pos < n {
                let c = t[pos];
                if c == d0 || c == d1 || c == d2 {
                    return Some((pos, c));
                }
                pos += 1;
            }
        }
    }
    None
}

// ------------------ the parser ------------------

/** How a field ended: 0 at a comma, 1 at a line break, 2 at the end of the text. */
const COMMA: u8 = 0;
const LINE_BREAK: u8 = 1;
const END: u8 = 2;

struct P<'a> {
    t: &'a [u16],
    pos: usize,
    line: u32,
    rec_line: u32,
    rec_row: i64, // -1 means null (the header record)
    width: i64,   // -1 until the first record sets it
    err: Vec<u8>,
    err_n: usize,
    // A high surrogate that ended the previous emitted span and has not
    // been written yet: JSON.stringify pairs surrogates by adjacency in
    // the VALUE, and the value can join units the text keeps apart (a
    // stray CR dropped between them, or a closing quote between quoted
    // content and junk), so the pairing must be decided at the next
    // value unit, not at the next text unit. 0 = none held.
    pend_hi: u16,
}

impl<'a> P<'a> {
    fn error(&mut self, code: &str) {
        if self.err_n > 0 {
            self.err.push(b',');
        }
        self.err_n += 1;
        self.err.extend_from_slice(b"{\"code\":\"");
        self.err.extend_from_slice(code.as_bytes());
        self.err.extend_from_slice(b"\",\"line\":");
        push_u32(&mut self.err, self.rec_line);
        self.err.extend_from_slice(b",\"row\":");
        if self.rec_row < 0 {
            self.err.extend_from_slice(b"null");
        } else {
            push_i64(&mut self.err, self.rec_row);
        }
        self.err.push(b'}');
    }

    fn error_short(&mut self, fields: usize) {
        if self.err_n > 0 {
            self.err.push(b',');
        }
        self.err_n += 1;
        self.err.extend_from_slice(b"{\"code\":\"short_row\",\"line\":");
        push_u32(&mut self.err, self.rec_line);
        self.err.extend_from_slice(b",\"row\":");
        if self.rec_row < 0 {
            self.err.extend_from_slice(b"null");
        } else {
            push_i64(&mut self.err, self.rec_row);
        }
        self.err.extend_from_slice(b",\"fields\":");
        push_u32(&mut self.err, fields as u32);
        self.err.push(b'}');
    }

    fn error_long(&mut self, extra: &[u8]) {
        if self.err_n > 0 {
            self.err.push(b',');
        }
        self.err_n += 1;
        self.err.extend_from_slice(b"{\"code\":\"long_row\",\"line\":");
        push_u32(&mut self.err, self.rec_line);
        self.err.extend_from_slice(b",\"row\":");
        if self.rec_row < 0 {
            self.err.extend_from_slice(b"null");
        } else {
            push_i64(&mut self.err, self.rec_row);
        }
        self.err.extend_from_slice(b",\"extra\":[");
        self.err.extend_from_slice(extra);
        self.err.extend_from_slice(b"]}");
    }

    /// Write a held high surrogate out as a lone `\uXXXX` escape (it
    /// turned out not to be followed by a low surrogate in the value).
    fn flush_pend(&mut self, out: &mut Vec<u8>) {
        if self.pend_hi != 0 {
            let hi = self.pend_hi;
            self.pend_hi = 0;
            push_unicode_escape(out, hi);
        }
    }
}

/// Read an unquoted field, appending its JSON-escaped content to `out`.
/// Returns (how it ended, units kept in this call).
fn read_unquoted(p: &mut P, out: &mut Vec<u8>) -> (u8, usize) {
    let t = p.t;
    let mut added = 0usize;
    loop {
        let found = unsafe { find_delim(t, p.pos, 44, 10, 13) };
        match found {
            None => {
                added += emit_span_pend(p, out, t, p.pos, t.len());
                p.pos = t.len();
                return (END, added);
            }
            Some((d, c)) => {
                added += emit_span_pend(p, out, t, p.pos, d);
                if c == 44 {
                    p.pos = d + 1;
                    return (COMMA, added);
                } else if c == 10 {
                    p.pos = d + 1;
                    p.line += 1;
                    return (LINE_BREAK, added);
                } else {
                    // CR: a line break when followed by LF, else a stray CR, dropped.
                    if d + 1 < t.len() && t[d + 1] == 10 {
                        p.pos = d + 2;
                        p.line += 1;
                        return (LINE_BREAK, added);
                    }
                    p.error("stray_cr");
                    p.pos = d + 1;
                    continue;
                }
            }
        }
    }
}

/// What follows a quoted field's closing quote (p.pos is just past it).
fn post_quoted(p: &mut P, out: &mut Vec<u8>) -> (u8, usize) {
    if p.pos >= p.t.len() {
        return (END, 0);
    }
    let c = p.t[p.pos];
    if c == 44 {
        p.pos += 1;
        return (COMMA, 0);
    }
    if c == 10 {
        p.pos += 1;
        p.line += 1;
        return (LINE_BREAK, 0);
    }
    if c == 13 && p.pos + 1 < p.t.len() && p.t[p.pos + 1] == 10 {
        p.pos += 2;
        p.line += 1;
        return (LINE_BREAK, 0);
    }
    // What follows the closing quote is appended as it stands.
    let (ended, added) = read_unquoted(p, out);
    if added > 0 {
        p.error("quote_junk");
    }
    (ended, added)
}

/// Read a quoted field whose opening quote is at p.pos, appending its
/// JSON-escaped content to `out`. Returns (how it ended, units kept after the
/// quoted part, for the quote_junk check — always 0 here; the junk path counts).
fn read_quoted(p: &mut P, out: &mut Vec<u8>) -> (u8, usize) {
    let t = p.t;
    let open = p.pos;
    let open_line = p.line;
    let out_mark = out.len();
    p.pos += 1; // past the opening quote
    let mut seg_start = p.pos;
    loop {
        let found = unsafe { find_delim(t, p.pos, 34, 10, 13) };
        match found {
            None => {
                // The quote never closes: it is a literal character, and the
                // text after it is read again as unquoted.
                out.truncate(out_mark);
                p.pos = open + 1;
                p.line = open_line;
                p.pend_hi = 0; // the held high belonged to discarded content
                p.error("unterminated_quote");
                out.extend_from_slice(b"\\\"");
                return read_unquoted(p, out);
            }
            Some((d, c)) => {
                if c == 34 {
                    if d + 1 < t.len() && t[d + 1] == 34 {
                        // Doubled quote: one quote in the value.
                        emit_span_pend(p, out, t, seg_start, d);
                        p.flush_pend(out);
                        out.extend_from_slice(b"\\\"");
                        p.pos = d + 2;
                        seg_start = d + 2;
                        continue;
                    }
                    // Closing quote.
                    emit_span_pend(p, out, t, seg_start, d);
                    p.pos = d + 1;
                    return post_quoted(p, out);
                }
                // CR or LF inside quotes: each one LF, each counts a line.
                emit_span_pend(p, out, t, seg_start, d);
                p.flush_pend(out);
                out.extend_from_slice(b"\\n");
                p.line += 1;
                if c == 13 && d + 1 < t.len() && t[d + 1] == 10 {
                    p.pos = d + 2;
                } else {
                    p.pos = d + 1;
                }
                seg_start = p.pos;
                continue;
            }
        }
    }
}

/// Read one field (quoted or not) into `tgt` as a JSON string, `"..."`.
/// Returns (how it ended, whether it opened with a quote).
fn read_field(p: &mut P, tgt: &mut Vec<u8>, cnt: &mut usize) -> (u8, bool) {
    let quoted = p.pos < p.t.len() && p.t[p.pos] == 34;
    if *cnt > 0 {
        tgt.push(b',');
    }
    tgt.push(b'"');
    let (ended, _) = if quoted {
        read_quoted(p, tgt)
    } else {
        read_unquoted(p, tgt)
    };
    p.flush_pend(tgt);
    tgt.push(b'"');
    *cnt += 1;
    (ended, quoted)
}

/// Parse one record (p.pos is at its first field) into `row` (and `extra`
/// for fields past the width), reusing the caller's buffers across records.
/// Returns Some(()) when the record counts, or None when it is skipped (a
/// line left with no characters). Updates p.width and p.err. The caller sets
/// p.rec_line / p.rec_row first.
fn parse_record(p: &mut P, row: &mut Vec<u8>, extra: &mut Vec<u8>) -> Option<()> {
    row.clear();
    extra.clear();
    let mut row_n = 0usize;
    let mut extra_n = 0usize;
    let mut field_idx = 0usize;
    let mut quoted_last = false;
    row.push(b'[');
    loop {
        let to_extra = p.width >= 0 && (field_idx as i64) >= p.width;
        let (ended, quoted) = if to_extra {
            read_field(p, extra, &mut extra_n)
        } else {
            read_field(p, row, &mut row_n)
        };
        quoted_last = quoted;
        field_idx += 1;
        if ended != COMMA {
            break;
        }
    }
    row.push(b']');
    // A line that held only stray CRs is left with no characters: no record.
    if field_idx == 1 && !quoted_last && row.as_slice() == b"[\"\"]" {
        return None;
    }
    if p.width < 0 {
        p.width = field_idx as i64;
    }
    if (field_idx as i64) < p.width {
        row.pop(); // the ']'
        for _ in field_idx..(p.width as usize) {
            row.extend_from_slice(b",\"\"");
        }
        row.push(b']');
        p.error_short(field_idx);
    } else if (field_idx as i64) > p.width {
        p.error_long(extra);
    }
    Some(())
}

fn parse_case16(t: &[u16], header: bool, out: &mut Vec<u8>) {
    let mut p = P {
        t,
        pos: 0,
        line: 1,
        rec_line: 1,
        rec_row: -1,
        width: -1,
        err: Vec::new(),
        err_n: 0,
        pend_hi: 0,
    };
    if !t.is_empty() && t[0] == 0xFEFF {
        p.pos = 1;
    }
    let mut seen_first = false;
    let mut rows_n = 0usize;
    // Reused across records: parse_record clears them each time, so no
    // per-record allocation on the hot path (63k+ records a set).
    let mut row: Vec<u8> = Vec::with_capacity(1024);
    let mut extra: Vec<u8> = Vec::new();
    loop {
        // Lines with no characters are no records.
        while p.pos < t.len() {
            let c = t[p.pos];
            if c == 10 {
                p.pos += 1;
                p.line += 1;
            } else if c == 13 && p.pos + 1 < t.len() && t[p.pos + 1] == 10 {
                p.pos += 2;
                p.line += 1;
            } else {
                break;
            }
        }
        if p.pos >= t.len() {
            break;
        }
        p.rec_line = p.line;
        p.rec_row = if header && !seen_first { -1 } else { rows_n as i64 };
        match parse_record(&mut p, &mut row, &mut extra) {
            None => continue,
            Some(()) => {
                if !seen_first {
                    seen_first = true;
                    out.extend_from_slice(b"{\"columns\":");
                    if header {
                        out.extend_from_slice(&row);
                    } else {
                        out.extend_from_slice(b"null");
                    }
                    out.extend_from_slice(b",\"rows\":[");
                    if !header {
                        out.extend_from_slice(&row);
                        rows_n += 1;
                    }
                } else {
                    if rows_n > 0 {
                        out.push(b',');
                    }
                    out.extend_from_slice(&row);
                    rows_n += 1;
                }
            }
        }
    }
    if !seen_first {
        out.extend_from_slice(b"{\"columns\":");
        if header {
            out.extend_from_slice(b"[]");
        } else {
            out.extend_from_slice(b"null");
        }
        out.extend_from_slice(b",\"rows\":[");
    }
    out.extend_from_slice(b"],\"errors\":[");
    out.extend_from_slice(&p.err);
    out.extend_from_slice(b"]}");
}
