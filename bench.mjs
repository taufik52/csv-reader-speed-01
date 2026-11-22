// bench.mjs — correctness + speed check of this entry's solve.js against
// csv-reader 1.0.0 (the baseline) on the kit's practice set.
//
// Usage (from this repo root):
//   node bench.mjs [--trials N] [--kit <kit dir>]
// Defaults: trials 5, kit = ../csv-reader-kit-36/csv-reader-speed-01-run-20261121-kit
// Reads practice/input/cases.json and practice/reference/expected.json from the kit.
// Prints per-case correctness (mine vs expected, baseline vs expected) and the
// median wall time of each implementation over the trials, with the speedup.

import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const trialsIdx = args.indexOf('--trials');
const trials = (trialsIdx !== -1 && Number(args[trialsIdx + 1])) || 5;
const kitIdx = args.indexOf('--kit');
const kitArg = kitIdx !== -1 ? args[kitIdx + 1] : undefined;
const kit = kitArg ?? join(here, '..', 'csv-reader-kit-36', 'csv-reader-speed-01-run-20261121-kit');

const { run: mine } = await import(new URL('./solve.js', import.meta.url).href);
const { run: baseline } = await import(new URL(join(kit, 'baselines', 'csv-reader', 'solve.js'), import.meta.url).href);

const cases = JSON.parse(readFileSync(join(kit, 'practice', 'input', 'cases.json'), 'utf8'));
const expected = JSON.parse(readFileSync(join(kit, 'practice', 'reference', 'expected.json'), 'utf8'));

function canonical(v) {
  if (Array.isArray(v)) return `[${v.map(canonical).join(',')}]`;
  if (v !== null && typeof v === 'object') {
    const keys = Object.keys(v).sort();
    return `{${keys.map((k) => `${JSON.stringify(k)}:${canonical(v[k])}`).join(',')}}`;
  }
  return JSON.stringify(v) ?? 'null';
}

function firstDiff(a, b, path = '') {
  if (canonical(a) === canonical(b)) return null;
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return `${path}: length ${a.length} vs ${b.length}`;
    for (let i = 0; i < a.length; i++) {
      const d = firstDiff(a[i], b[i], `${path}[${i}]`);
      if (d) return d;
    }
    return `${path}: arrays differ`;
  }
  if (a !== null && b !== null && typeof a === 'object' && typeof b === 'object') {
    const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
    for (const k of keys) {
      const d = firstDiff(a[k], b[k], `${path}.${k}`);
      if (d) return d;
    }
    return `${path}: objects differ`;
  }
  return `${path}: ${JSON.stringify(a)?.slice(0, 80)} vs ${JSON.stringify(b)?.slice(0, 80)}`;
}

const expById = new Map(expected.cases.map((c) => [c.id, c.output]));
let minePass = 0;
let basePass = 0;
for (const c of cases) {
  const m = mine(c.input);
  const b = baseline(c.input);
  const e = expById.get(c.id);
  const mOk = canonical(m) === canonical(e);
  const bOk = canonical(b) === canonical(e);
  minePass += mOk ? 1 : 0;
  basePass += bOk ? 1 : 0;
  console.log(`${c.id}: mine ${mOk ? 'PASS' : 'FAIL'}  baseline ${bOk ? 'PASS' : 'FAIL'}` + (mOk ? '' : `  mine-diff: ${firstDiff(m, e)}`));
}
console.log(`correctness: mine ${minePass}/${cases.length}  baseline ${basePass}/${cases.length}`);

function timeAll(fn) {
  const t0 = process.hrtime.bigint();
  for (const c of cases) fn(c.input);
  return Number(process.hrtime.bigint() - t0) / 1e6;
}
const mineTimes = [];
const baseTimes = [];
for (let t = 0; t < trials; t++) {
  baseTimes.push(timeAll(baseline));
  mineTimes.push(timeAll(mine));
}
const median = (a) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];
const mMed = median(mineTimes);
const bMed = median(baseTimes);
console.log(`time over ${cases.length} cases (${trials} trials, ms): mine median ${mMed.toFixed(1)} [${mineTimes.map((t) => t.toFixed(0)).join(', ')}]  baseline median ${bMed.toFixed(1)} [${baseTimes.map((t) => t.toFixed(0)).join(', ')}]`);
console.log(`speedup: ${(bMed / mMed).toFixed(2)}x (baseline/mine)`);
if (minePass !== cases.length) process.exitCode = 1;
