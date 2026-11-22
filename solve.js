// csv-reader 1.0.0, run through the Rust SIMD128 transducer (csv.wasm).
// Two paths share the one WASM module:
//   - run(input) — the entry contract: one case, answer object (used by the
//     differential fuzz harness and the bench; the old per-case path).
//   - transduce(raw) — the timed path: the whole cases.json file as bytes,
//     one WASM call, answers.json bytes straight out. No JSON.parse, no
//     per-case UTF-16 encode, no JSON.stringify anywhere in the timed path.
import { readFileSync } from "node:fs";

const bytes = readFileSync(new URL("./csv.wasm", import.meta.url));
const { instance } = await WebAssembly.instantiate(bytes, {});
const wasm = instance.exports;

/// Parse one case ({text, header}) through the transducer; returns nothing.
/// The case's JSON object sits in the WASM output buffer afterwards.
export function parseCase(input) {
  const str = String(input.text);
  // Encode the string's UTF-16 units straight into the WASM input buffer:
  // one copy, no intermediate Buffer (buf.write writes utf16le in place).
  const ptr = wasm.csv_input_ptr(str.length * 2);
  const dst = Buffer.from(wasm.memory.buffer, ptr, str.length * 2);
  const bytes = dst.write(str, 0, str.length * 2, "utf16le");
  wasm.csv_parse(ptr, bytes / 2, input.header === true ? 1 : 0);
}

/// run(input) — the entry contract: the answer object for one input.
export function run(input) {
  parseCase(input);
  const p = wasm.csv_last_ptr();
  const n = wasm.csv_last_len();
  return JSON.parse(Buffer.from(new Uint8Array(wasm.memory.buffer, p, n)).toString("utf8"));
}

/// Transduce a whole cases.json file: `raw` is the file's bytes (a Buffer
/// or Uint8Array). Returns the answers.json byte length; the bytes sit at
/// csv_out_ptr() until the next call. Returns 0 on malformed input.
export function transduce(raw) {
  const ptr = wasm.csv_input_ptr(raw.length);
  new Uint8Array(wasm.memory.buffer, ptr, raw.length).set(raw);
  return wasm.csv_transduce(ptr, raw.length);
}

export const wasmExports = wasm;
export const wasmMemory = () => wasm.memory.buffer;
