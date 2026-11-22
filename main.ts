// csv-reader 1.0.0, the house entry's driver shape: the harness runs
// `node /bot/main.ts` (node22). It reads every case from /input/cases.json
// and writes /output/answers.json: a JSON array of the answers, one per case
// and in the cases' order. Outside a container MAKEFUN_INPUT_DIR and
// MAKEFUN_OUTPUT_DIR stand in for /input and /output.
//
// The difference from the house driver: the whole file is transduced in
// one WASM call — the answers are never built as JS values, never parsed
// from JSON, never stringified. The Rust transducer (v3: UTF-8 bytes
// throughout, fused SIMD scan+emit per field) reads the raw file bytes,
// unescapes each case's text itself, parses the CSV, and emits each case's
// JSON straight into its output buffer; answers.json is written from that
// buffer as it stands — byte-for-byte the JSON the parser emitted, which
// is byte-identical to JSON.stringify of the same values (same key order,
// same escaping as V8: lowercase \u hex, lone surrogates escaped,
// non-ASCII raw). A lone surrogate in a \uXXXX escape falls back to the
// v2 UTF-16 transducer for that case, inside the same module.

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { setFlagsFromString } from "node:v8";

// V8's incremental marking off, set before solve.js is imported so all of its
// work runs under it (the rules: the house baseline runs with
// --no-incremental-marking, and an entry may tune V8's flags too).
setFlagsFromString("--no-incremental-marking");

const { transduce, wasmExports, wasmMemory } = await import(
  new URL("./solve.js", import.meta.url).href
);

const INPUT_DIR = process.env.MAKEFUN_INPUT_DIR ?? "/input";
const OUTPUT_DIR = process.env.MAKEFUN_OUTPUT_DIR ?? "/output";

const raw = readFileSync(join(INPUT_DIR, "cases.json"));
const len = transduce(raw);
if (len === 0) throw new Error("csv_transduce: malformed cases.json");
const out = wasmExports.csv_out_ptr();
writeFileSync(join(OUTPUT_DIR, "answers.json"), new Uint8Array(wasmMemory(), out, len));
