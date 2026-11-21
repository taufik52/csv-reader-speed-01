# csv-reader-speed-01

A fast drop-in replacement for make.fun's csv-reader 1.0.0, written for the
[make.fun](https://make.fun) competition `csv-reader-speed-01-run-20261121`
("Read broken CSV files faster", 21 November 2026).

## What it does

`solve.js` exports `run(input)` which returns exactly csv-reader 1.0.0's
answer `{columns, rows, errors}` for every input `{text, header}`: the same
broken-row policy (BOM, CRLF/LF, `stray_cr`, `quote_junk`,
`unterminated_quote`, `short_row`/`long_row`, header/null), byte for byte —
but read with bulk scans instead of one character at a time.

`main.ts` is the competition's own driver (unchanged): it reads the cases
from `/input/cases.json`, answers each one alone with `run()`, and writes
`/output/answers.json`.

## How it is fast

The reference reads one character at a time and builds every field a
character at a time. This module does three things differently:

1. An unquoted field is found with one regex scan for the next delimiter
   (`,`, LF or CR) instead of a per-character loop, and sliced whole.
2. A quoted field is found with `indexOf` for its closing quote and sliced
   whole instead of built a character at a time; a field is only copied when
   it has to be (a stray CR, a doubled quote, or a line break inside quotes).
3. Line breaks inside a quoted field are normalised and counted in a single
   `replace` pass.

Pure: no clock, no randomness, no locale, nothing kept from one call to the
next. Dependency-free ES module.

## Benchmark

`bench.mjs` checks correctness against the kit's practice set (6 cases,
~11 MB) and times this module against the baseline:

```
node bench.mjs --trials 5
```

Result on the practice set (21 Nov 2026, Node 24, this machine):
correctness 6/6, median speedup ~2.5x over the baseline
(mine ~200 ms vs baseline ~500 ms over the 6 cases).

## Layout

- `solve.js` — the entry: `run(input) -> {columns, rows, errors}`
- `main.ts` — the competition's driver (unchanged)
- `bench.mjs` — correctness + speed check on the practice set
- `package.json` — `{"type": "module"}`
- `LICENSE` — MIT

## License

MIT — see `LICENSE`.
