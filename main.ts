// csv-reader 1.0.0, the house entry's driver shape: the harness runs
// `node /bot/main.ts` (node22). It reads every case from /input/cases.json (a
// JSON array of {id, input}), answers each one alone, and writes
// /output/answers.json: a JSON array of the answers, one per case and in the
// cases' order. Outside a container MAKEFUN_INPUT_DIR and MAKEFUN_OUTPUT_DIR
// stand in for /input and /output.
//
// The difference from the house driver: the answers are never built as JS
// values and never stringified. The Rust transducer emits each case's JSON
// straight into its output buffer, and answers.json is written from that
// buffer as it stands — byte-for-byte the JSON the parser emitted, which is
// byte-identical to JSON.stringify of the same values (same key order, same
// escaping as V8: lowercase \u hex, lone surrogates escaped, non-ASCII raw).

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { setFlagsFromString } from "node:v8";

// V8's incremental marking off, set before solve.js is imported so all of its
// work runs under it (the rules: the house baseline runs with
// --no-incremental-marking, and an entry may tune V8's flags too).
setFlagsFromString("--no-incremental-marking");

const { parseCase, wasmExports, wasmMemory } = await import(
  new URL("./solve.js", import.meta.url).href
);

const INPUT_DIR = process.env.MAKEFUN_INPUT_DIR ?? "/input";
const OUTPUT_DIR = process.env.MAKEFUN_OUTPUT_DIR ?? "/output";

const cases = JSON.parse(readFileSync(join(INPUT_DIR, "cases.json"), "utf8"));
wasmExports.csv_begin();
for (const c of cases) parseCase(c.input);
const len = wasmExports.csv_finish();
const out = wasmExports.csv_out_ptr();
writeFileSync(join(OUTPUT_DIR, "answers.json"), new Uint8Array(wasmMemory(), out, len));
