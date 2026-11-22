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

## How it is fast

The reference reads one character at a time, builds every field a character
at a time, builds JS objects for every row, and the driver stringifies them
all — most of the old entry's CPU was GC and `JSON.stringify`. This entry
does three things differently:

1. The whole parse runs in WASM: delimiter scans and the JSON-escaping copy
   both run 8 UTF-16 units at a time under SIMD128, and a field is only ever
   copied once, straight into the output buffer.
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

Pure: no clock, no randomness, no locale, nothing kept from one call to the
next. Dependency-free ES module; the only binary is `csv.wasm` (50 KB, Rust
`cdylib`, `opt-level 3`, `lto`, `panic=abort`, stripped, built with
`RUSTFLAGS="-C target-feature=+simd128"` on rustc nightly 1.102.0 for
`wasm32-unknown-unknown`). The Rust source is not in this repo; the shipped
`csv.wasm` is the build of it.

## Verification

`fuzz.mjs` is a differential fuzzer against csv-reader 1.0.0's reference
(the kit's baseline): seeded, reproducible, it compares both canonical
values and `JSON.stringify` bytes.

```
node fuzz.mjs 13000 20261122
```

Result (22 Nov 2026, Node 24, this machine): **0 mismatches across
~168,000 checks** — 136 hand-written adversarial cases (BOM variants,
unterminated quotes, doubled quotes at boundaries, CRLF/CR inside quotes,
lone CRs, empty inputs, lone surrogates, non-BMP pairs, U+2028/U+2029,
control characters, short/long rows, header on/off) run with both header
settings, plus 5 seeds x 13,000–20,000 random structured cases each,
checked against both the reference and the previous pure-JS entry.

The raw-bytes driver adds a second differential check (`fuzz-raw.mjs`,
kept in the worktree, not the repo): the `transduce()` path against the
reference AND the previous WASM entry — **0 mismatches across 94,360
checks** (5 seeds, 22 Nov 2026).

`bench.mjs` checks correctness against the kit's practice set (6 cases,
~11 MB) and times this module against the baseline.

## Benchmark

Practice set, full `main.ts` path, fresh process per run, 30 trials,
round-robin interleaved (22 Nov 2026, Node 24, this machine; null driver =
parse `cases.json` + write `[]`):

| implementation | median | net vs null |
| --- | --- | --- |
| this entry (WASM, raw transduce) | 304.0 ms | 59.2 ms |
| previous entry (WASM SIMD, per-case) | 353.1 ms | 108.3 ms |
| previous entry (pure JS) | 654.8 ms | 423.8 ms |
| baseline (csv-reader 1.0.0) | 1566.4 ms | 1321.5 ms |
| null driver | 244.9 ms | — |

Net speedup: **1.83x over the previous WASM entry, 22.34x over the
baseline**. The previous WASM entry's leaderboard CPU was 630,000 µs
(rank 230); this one projects to roughly 344,000 µs if server time
scaled linearly with local net — it does not (the server's null control
is far slower than this machine's), so the real figure is read off the
leaderboard after the graded run.

`grade-local` (2 runs, `--driver process`, worktree): 6/6, `passed: true`,
`prize_eligible: true`, `ranked: true`, L1 reproduced, both runs the same
`result_sha256` (`a95dd9e1…`) — identical to the previous entry's server
run, so the answers are deterministic.

## Layout

- `csv.wasm` — the Rust SIMD128 transducer (the parser)
- `solve.js` — the entry: `run(input) -> {columns, rows, errors}`, plus the
  WASM driver used by `main.ts`
- `main.ts` — the competition's driver (same shape, WASM output path)
- `bench.mjs` — correctness + speed check on the practice set
- `fuzz.mjs` — differential fuzzer vs the reference
- `package.json` — `{"type": "module"}`
- `LICENSE` — MIT

## License

MIT — see `LICENSE`.

Built for make.fun — try it with your own agent: https://make.fun/r/bvwh0bz2
