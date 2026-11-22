// Bench alone (v3): each implementation's full main.ts path in a FRESH
// process, round-robin interleaved so VM load drift hits all of them alike.
// Reports wall ms per run: median / mean / min / max / stdev, and the net
// time after subtracting the null-driver control (read cases.json + write []).
// Usage: node bench-alone.mjs [TRIALS]   (default 30)
// The kit path is fixed to the level-36 kit used for round 5.
import { spawnSync } from "node:child_process";
import { writeFileSync, mkdirSync } from "node:fs";

const KIT = "/home/hatch/workspace/makefun/csv-reader-kit-36/csv-reader-speed-01-run-20261121-kit";
const INPUT = KIT + "/practice/input";
const TRIALS = Number(process.argv[2] || 30);

const IMPLS = [
  { name: "v3", dir: "/home/hatch/workspace/makefun/csv-reader-speed-01-entry" },
  { name: "v2", dir: "/home/hatch/workspace/makefun/csv-reader-v2" },
  { name: "baseline", dir: KIT + "/baselines/csv-reader" },
];

// Null driver: same shape as main.ts minus the parse — the control floor.
mkdirSync("/tmp/bench-null", { recursive: true });
writeFileSync("/tmp/bench-null/main.ts", `
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
const INPUT_DIR = process.env.MAKEFUN_INPUT_DIR ?? "/input";
const OUTPUT_DIR = process.env.MAKEFUN_OUTPUT_DIR ?? "/output";
const cases = JSON.parse(readFileSync(join(INPUT_DIR, "cases.json"), "utf8"));
writeFileSync(join(OUTPUT_DIR, "answers.json"), "[]");
`);
IMPLS.push({ name: "null", dir: "/tmp/bench-null" });

function runOnce(impl, i) {
  const outdir = `/tmp/bench-out-${impl.name}`;
  mkdirSync(outdir, { recursive: true });
  const t0 = process.hrtime.bigint();
  const r = spawnSync("node", ["main.ts"], {
    cwd: impl.dir,
    env: { ...process.env, MAKEFUN_INPUT_DIR: INPUT, MAKEFUN_OUTPUT_DIR: outdir },
    encoding: "utf8",
  });
  const ms = Number(process.hrtime.bigint() - t0) / 1e6;
  if (r.status !== 0) {
    console.log(`${impl.name} trial ${i} FAILED:`, (r.stderr || "").slice(0, 500));
    return null;
  }
  return ms;
}

const stats = {};
for (const impl of IMPLS) stats[impl.name] = [];
for (let t = 0; t < TRIALS; t++) {
  for (const impl of IMPLS) {
    const ms = runOnce(impl, t);
    if (ms !== null) stats[impl.name].push(ms);
  }
  if ((t + 1) % 10 === 0) console.log(`... ${t + 1}/${TRIALS} rounds done`);
}

function summarize(a) {
  const s = [...a].sort((x, y) => x - y);
  const med = s[Math.floor(s.length / 2)];
  const mean = a.reduce((x, y) => x + y, 0) / a.length;
  const sd = Math.sqrt(a.reduce((x, y) => x + (y - mean) ** 2, 0) / a.length);
  return { n: a.length, median: med, mean, min: s[0], max: s[s.length - 1], sd };
}

const out = {};
for (const impl of IMPLS) out[impl.name] = summarize(stats[impl.name]);
console.log(JSON.stringify(out, null, 2));
const ctl = out.null.median;
for (const impl of IMPLS) {
  if (impl.name === "null") continue;
  const net = out[impl.name].median - ctl;
  console.log(`${impl.name}: median ${out[impl.name].median.toFixed(1)} ms, net vs null ${net.toFixed(1)} ms`);
}
console.log(`speedup v3 vs v2 (net): ${((out.v2.median - ctl) / (out.v3.median - ctl)).toFixed(2)}x`);
console.log(`speedup v3 vs baseline (net): ${((out.baseline.median - ctl) / (out.v3.median - ctl)).toFixed(2)}x`);
console.log(`speedup v3 vs v2 (gross median): ${(out.v2.median / out.v3.median).toFixed(2)}x`);
console.log(`speedup v3 vs baseline (gross median): ${(out.baseline.median / out.v3.median).toFixed(2)}x`);
