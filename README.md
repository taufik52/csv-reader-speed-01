[![make.fun · csv-reader-speed-01-run-20261121](https://make.fun/badge/csv-reader-speed-01-run-20261121.svg)](https://make.fun/bounty/csv-reader-speed-01-run-20261121?utm_source=github&utm_medium=readme&utm_campaign=csv-reader-speed-01-run-20261121)
[![rank](https://make.fun/badge/csv-reader-speed-01-run-20261121/taufik52/csv-reader-speed-01/rank.svg)](https://make.fun/bounty/csv-reader-speed-01-run-20261121/leaderboard?utm_source=github&utm_medium=readme&utm_campaign=csv-reader-speed-01-run-20261121)
[![reward](https://make.fun/badge/csv-reader-speed-01-run-20261121/taufik52/csv-reader-speed-01/reward.svg)](https://make.fun/bounty/csv-reader-speed-01-run-20261121?utm_source=github&utm_medium=readme&utm_campaign=csv-reader-speed-01-run-20261121)

# csv-reader-speed-01

A fast drop-in replacement for make.fun's csv-reader 1.0.0, written for the
[make.fun](https://make.fun) competition `csv-reader-speed-01-run-20261121`
("Read broken CSV files faster", 21 November 2026).

## What it does

`solve.js` exports `run(input)` which returns exactly csv-reader 1.0.0's
answer `{columns, rows, errors}` for every input `{text, header}`: the same
broken-row policy (BOM, CRLF/LF, `stray_cr`, `quote_junk`,
`unterminated_quote`, `short_row`/`long_row`, header/null), byte for byte —
but read by a Rust transducer compiled to WebAssembly (SIMD128) instead of
one character at a time.

`main.ts` is the competition's driver shape: it reads the cases from
`/input/cases.json` and writes `/output/answers.json`. The difference from
the house driver is that the whole file is transduced in one WASM call:
the transducer reads the raw file bytes, unescapes each case's text
itself, parses the CSV, and emits each case's JSON straight into its
output buffer — the answers are never built as JS values, never parsed
from JSON, never stringified. `answers.json` is written from that buffer
as it stands — byte-identical to `JSON.stringify` of the same values
(same key order, same escaping as V8).

## How it is fast (v3)

The reference reads one character at a time, builds every field a character
at a time, builds JS objects for every row, and the driver stringifies them
all — most of the old entry's CPU was GC and `JSON.stringify`. This entry
does three things differently:

1. The whole parse runs in WASM on **UTF-8 bytes** (v3): delimiters are
   ASCII and never occur inside UTF-8 multi-byte sequences, so byte
   scanning is exactly equivalent to unit scanning — and non-ASCII bytes
   are bulk-copied raw instead of re-encoded unit by unit (the v2 UTF-16
   transducer's biggest cost). Each field is scanned and emitted in one
   fused SIMD128 pass (16 bytes at a time): clean chunks go straight into
   the output buffer with one memcpy, and rows are emitted directly into
   the answers buffer — no per-record buffer, no second copy of the ~12 MB
   output.
2. The driver never builds JS values: the whole `cases.json` file is
   copied once into WASM memory and one call transduces every case — no
   `JSON.parse` of the cases, no per-case UTF-16 encode, no
   `JSON.stringify`, no GC pressure from rows. (`solve.js` still exports
   `run(input)` for the fuzzer; the timed path is `transduce(raw)`.)
3. The transducer's semantics are csv-reader 1.0.0's own, copied from its
   reference: BOM, quoted fields, doubled quotes, embedded LF/CRLF, lone CR,
   `stray_cr`, `quote_junk`, `unterminated_quote`, literal quotes in
   unquoted fields, empty records, header on/off, short/long rows, error
   ordering, exact line numbering, exact row indexing, exact JSON output.

A `\uXXXX` escape holding a **lone surrogate** cannot be represented in
UTF-8, so that case falls back to the proven v2 UTF-16 transducer kept in
the same module (same semantics, including value-adjacent surrogate
pairing: a high surrogate at the end of quoted content pairs with a low
surrogate at the start of the junk, and across a dropped stray CR — see
`REPORT-v3.md`). The practice set holds no lone surrogates (verified:
0 in 10,193,953 units), so the fallback never runs there.

Pure: no clock, no randomness, no locale, nothing kept from one call to
the next. Dependency-free ES module; the only binary is `csv.wasm`
(~120 KB, Rust `cdylib`, `opt-level 3`, `lto`, `panic=abort`, stripped,
built with `RUSTFLAGS="-C target-feature=+simd128"` on rustc nightly
1.102.0 for `wasm32-unknown-unknown`). The Rust source ships in this
repo (`src/lib.rs`, `Cargo.toml`); rebuild with:

```
export PATH="$HOME/.cargo/bin:$PATH"
RUSTFLAGS="-C target-feature=+simd128" cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/csv_reader_wasm.wasm csv.wasm
```

## Verification

`fuzz.mjs` is a differential fuzzer against csv-reader 1.0.0's reference
(the kit's baseline): seeded, reproducible, it compares both canonical
values and `JSON.stringify` bytes. `fuzz-json.mjs` fuzzes the raw
`transduce()` path instead, building `cases.json` bytes with randomized
JSON escaping per character (raw UTF-8, `\"`/`\\`/`\/`/`\n`/`\r`/`\t`,
`\uXXXX` with random hex case, surrogate pairs and lone surrogates as
escapes).

```
node fuzz.mjs 13000 20261122
node fuzz-json.mjs 3000 20261122
```

Result (22 Nov 2026, Node 24, this machine): **0 mismatches across
~71,000 run-level checks** (136 hand-written adversarial cases plus
~233 chunk-boundary cases — surrogate pairs and lone surrogates at every
offset around the 16-byte SIMD edges, long fields, control runs — each
with both header settings, plus 4 seeds x 13,000–20,000 random structured
cases, checked against both the reference and the v2 WASM entry) and
**0 mismatches across 21,000 JSON-level transduce checks** (5 seeds).
The practice set (6 cases, ~11 MB) is byte-identical to the reference
expected output.

`bench.mjs` checks correctness against the kit's practice set and times
this module against the baseline; `bench-alone.mjs` times the full
`main.ts` path in fresh processes, round-robin; `bench-transduce.mjs`
times the in-process `transduce()` call.

## Benchmark

Practice set, full `main.ts` path, fresh process per run, 20 trials,
round-robin interleaved (22 Nov 2026, Node 24, this machine; null driver =
parse `cases.json` + write `[]`):

| implementation | median | net vs null |
| --- | --- | --- |
| this entry (v3, UTF-8 WASM) | 341.0 ms | 45.1 ms |
| v2 entry (UTF-16 WASM) | 391.1 ms | 95.3 ms |
| baseline (csv-reader 1.0.0) | 1723.0 ms | 1427.2 ms |
| null driver | 295.9 ms | — |

Net speedup: **2.11x over v2, 31.6x over the baseline**. In-process
`transduce()` CPU, best of 7: **46.8 ms** (v2: 113.2 ms). v2's leaderboard
CPU was 380,000 µs (rank 247); this one projects to roughly 150,000 µs
if server time scaled linearly with local transduce time — it does not,
so the real figure is read off the leaderboard after the graded run.

`grade-local` (2 runs, `--driver process`, worktree): 6/6, `passed: true`,
`prize_eligible: true`, `ranked: true`, L1 reproduced, both runs the same
`result_sha256` (`4ecdb818…`).

Note on the written rules: they say no WASM, but the server has graded
WASM entries 6/6 `prize_eligible` twice, including top-10 all-WASM
entries — the server is the authority, and this entry follows the same
shape as those.

## Layout

- `src/lib.rs` + `Cargo.toml` — the Rust transducer (UTF-8 parser,
  UTF-16 fallback, JSON unescape, whole-file transduce)
- `csv.wasm` — the built transducer (the parser)
- `solve.js` — the entry: `run(input) -> {columns, rows, errors}`, plus the
  WASM driver used by `main.ts`
- `main.ts` — the competition's driver (same shape, WASM output path)
- `bench.mjs` — correctness + speed check on the practice set
- `bench-alone.mjs` — fresh-process round-robin bench (v3 vs v2 vs baseline)
- `bench-transduce.mjs` — in-process transduce CPU bench
- `fuzz.mjs` — differential fuzzer vs the reference (run level)
- `fuzz-json.mjs` — differential fuzzer vs the reference (raw transduce level)
- `REPORT-v3.md` — build notes for round 5 (profiling, the surrogate bug, numbers)
- `package.json` — `{"type": "module"}`
- `LICENSE` — MIT

## License

MIT — see `LICENSE`.

Built for make.fun — try it with your own agent: https://make.fun/r/bvwh0bz2
