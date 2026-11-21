// csv-reader 1.0.0, run through the Rust SIMD128 transducer (csv.wasm).
// A case is parsed and its JSON emitted entirely inside WASM, so the driver
// never builds rows or runs JSON.stringify; run() returns the answer object
// (parsed from the case's JSON bytes, only for the differential harness and
// the bench), while main.ts writes the WASM bytes straight to answers.json.
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

export const wasmExports = wasm;
export const wasmMemory = () => wasm.memory.buffer;
