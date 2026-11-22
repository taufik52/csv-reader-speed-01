// bench-transduce.mjs — in-process transduce() CPU bench for this entry.
// Verifies byte-identity vs the practice expected answers, then reports
// the best-of-N transduce time (wall ms, sorted). Usage:
//   node bench-transduce.mjs [N]   (default 7)
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";

const KIT = "/home/hatch/workspace/makefun/csv-reader-kit-36/csv-reader-speed-01-run-20261121-kit";
const raw = readFileSync(KIT + "/practice/input/cases.json");
const exp = readFileSync(KIT + "/practice/reference/expected.json", "utf8");

const { transduce, wasmExports, wasmMemory } = await import(new URL("./solve.js", import.meta.url).href);
const N = Number(process.argv[2] || 7);

let len = transduce(raw);
const out = Buffer.from(new Uint8Array(wasmMemory(), wasmExports.csv_out_ptr(), len)).toString("utf8");
const want = JSON.stringify(JSON.parse(exp).cases.map((c) => c.output));
console.log("len", len, "byte-identical to expected stringify:", out === want);
console.log("sha256:", createHash("sha256").update(out).digest("hex").slice(0, 16));

const times = [];
for (let i = 0; i < N; i++) {
  const t0 = process.hrtime.bigint();
  transduce(raw);
  times.push(Number(process.hrtime.bigint() - t0) / 1e6);
}
times.sort((a, b) => a - b);
console.log(`transduce ms sorted: ${times.map((t) => t.toFixed(1)).join(", ")}  best: ${times[0].toFixed(1)}`);
