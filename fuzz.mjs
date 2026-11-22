// Differential fuzz: WASM entry (v3 UTF-8 transducer) vs csv-reader 1.0.0
// reference (kit baseline) vs the v2 UTF-16 WASM entry.
// Compares both canonical values AND JSON.stringify bytes (catches key order).
// Seeded PRNG, so a run is reproducible. Usage: node fuzz.mjs [N] [seed]
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const KIT = "/home/hatch/workspace/makefun/csv-reader-kit-36/csv-reader-speed-01-run-20261121-kit";

const { run: mine } = await import(new URL("./solve.js", import.meta.url).href);
const { run: ref } = await import(new URL(join(KIT, "baselines", "csv-reader", "solve.js"), import.meta.url).href);
const { run: oldjs } = await import(new URL("/home/hatch/workspace/makefun/csv-reader-v2/solve.js", import.meta.url).href);

const N = Number(process.argv[2] || 13000);
const SEED = Number(process.argv[3] || 20261122);

function mulberry32(a) {
  return function () {
    a |= 0; a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rnd = mulberry32(SEED);
const pick = (arr) => arr[Math.floor(rnd() * arr.length)];
const ri = (n) => Math.floor(rnd() * n);

// ---- generators ----
const FRAGS = ["a", "b", "ab", "x", " ", "  ", "0", "1.5", "é", "ü", "Zürich", "東京", "naïve", "O'Brien", "#42", "a b", "\t", "a\tb", "\u2028", "\u2029", "\u007F", "\uD800", "\uDC00", "\uD800\uDC00", "\uD83D\uDE00", "\uFEFF", "a\uFEFFb"];
const BREAKS = ["", ",", "\n", "\r", "\r\n"];
const QUOTES = ["", '"', '""', '"""', '""""', '"a', 'a"', '"a"', '"a""b"', '"a"b', '"a" ', ' "a"', '"a\nb"', '"a\r\nb"', '"a\rb"', '"a\r"', '"a\n"', '"\n"', '"\r\n"', '"a', '"a""', '"a"""'];

function randField() {
  const r = rnd();
  if (r < 0.45) return pick(FRAGS);
  if (r < 0.8) return pick(QUOTES) + pick(FRAGS) + pick(QUOTES);
  // random char soup
  const alpha = ["a", "b", ",", '"', "\n", "\r", " ", "é", "\uD800", "\uDC00", "\uFEFF", "0"];
  let s = "";
  const len = ri(12);
  for (let i = 0; i < len; i++) s += pick(alpha);
  return s;
}

function randCase() {
  const header = rnd() < 0.5;
  const rows = ri(5);
  let text = rnd() < 0.15 ? "\uFEFF" : "";
  const cols = 1 + ri(4);
  for (let r = 0; r <= rows; r++) {
    const n = 1 + ri(cols + 2);
    const fields = [];
    for (let i = 0; i < n; i++) fields.push(randField());
    text += fields.join(",");
    if (r < rows) text += pick(["\n", "\r\n", "\r", ""]);
  }
  if (rnd() < 0.1) text += pick(["\n", "\r\n", "\r", ",", '"', "\uFEFF"]);
  return { text, header };
}

// ---- adversarial hand cases ----
const ADV = [
  "", "\uFEFF", "\uFEFF\uFEFF", "\uFEFFa", "\uFEFF\na", "a", ",", ",,", ",\n,", "\n", "\r", "\r\n", "\n\n", "\r\r", "\r\n\r\n",
  '"', '""', '"""', '""""', '"a', '"a"', '"a""', '"a"""', '"a""""', '"a"b', '"a" b', '"a",b', '"a"\n', '"a"\r', '"a"\r\n',
  '"a\nb"', '"a\r\nb"', '"a\rb"', '"a\r"', '"a\n"', '"\n"', '"\r"', '"\r\n"', '"a\nb\nc"', '"a""b""c"', '"a""', '"""a"""',
  'a"b', 'a"b"c', 'a""b', '"a"b"c"d', 'a,"b"c', 'a,"b" c,d', '"a"x"y"z', 'x"y"z,w',
  "a\rb", "a\r\rb", "a\rb\nc", "a\r", "a\r\r", "\ra", "\r\na", "a\n\rb", "a\r\nb\rc",
  "a,b\r", "a,b\r\n", "a,b\n", "a,b", "a,b,", "a,b,,", ",a", ",a,", "a,,b",
  "a,b\nc", "a,b\nc,d,e", "a,b\nc,d", "a,b\nc,d,e,f", "a\nb\nc", "a,b\n\nc,d", "a,b\n\r\nc,d",
  '"a",b\nc,"d\ne",f', 'h1,h2\na,b\n"q\nq",r', "h1,h2\n\"a\rb\",c",
  "a,b\n\"unterm", "a,b\n\"unterm\nc,d", "\"unterm,a,b", "\"unterm",
  "a,\"b\nc", "a,\"b\r\nc\nd,e", "\"a\nb\",c\nd,e",
  "é,ü\n\uD800,\uDC00", "\uD800\uDC00,\uD83D\uDE00", "a\uD800b,\uDC00c", "\uD800", "\uDC00", "\uD800\uD800", "\uDC00\uDC00",
  "\u2028,\u2029", "a\u2028b", "\"a\u2028b\",c", "a,b\u0000c", "\u0000", "\"\u0000\"", "a\u0001b", "\u001Fb",
  "a,b\n\"\"\"x\"\"\",y", "\"\"\"\",a", "\"\",", "\"\",\"\"", "\"a\",\"b\",\"c\"",
  "h\n\"a\"\"b\"", "h\n\"a\"\"\"", "h\n\"\"\"a", "h\n\"a\"\"\"\"b\"",
  "a,b,c\nd,e", "a,b,c\nd", "a,b,c\nd,e,f,g", "a\nb,c,d",
  "\n\na,b\n\nc,d\n\n", "a,b\nc,d\n", "a,b\nc,d\r\n", "a,b\r\nc,d\r\n",
  " ", "  ,  ", " a , b ", "\ta\t,\tb\t",
  "1,2.5,2024-01-01", "\"1,000\",2", "\"a,b\",\"c,d\"",
  "a,\"b\"\"c\"d,e", "\"a\"\"b\"c,d", "\"a\"b\"c\"d,e",
  "\"a\rb\nc\rd\",e", "\"a\r\nb\r\nc\",d",
  "x\ry\nz\rw", "x\r\ry", "x\n\ny", "x\r\n\r\ny",
  "\uFEFF\"a\",b", "\uFEFFa,b\nc,d", "a\uFEFFb,c",
];

// ---- v3 chunk-boundary cases: surrogate pairs / lone surrogates at every
// offset around the 16-byte SIMD chunk edges, quoted and unquoted, plus long
// fields, backslash/tab/control runs, and BOM + long text ----
const ADV2 = [];
for (const k of [0, 1, 13, 14, 15, 16, 17, 18, 29, 30, 31, 32, 33, 47, 48, 49]) {
  const pad = "a".repeat(k);
  ADV2.push(pad + "\uD800\uDC00" + ",b", pad + "\uD800" + ",b", pad + "\uDC00" + ",b");
  ADV2.push(pad + "\uD83D\uDE00" + ",b", pad + "\uD800\uD800\uDC00" + ",b", pad + "\uD800\uDC00\uDC00" + ",b");
  ADV2.push("\"" + pad + "\uD800\uDC00" + "\",b", "\"" + pad + "\uD800" + "\",b", "\"" + pad + "\uDC00" + "\",b");
  ADV2.push(pad + ",\"\uD800\uDC00\"", pad + ",\"\uD800\"", pad + "\"\"");
  ADV2.push(pad + "\uD800\uDC00" + "\n" + pad + ",c");
}
ADV2.push(
  "a".repeat(5000) + ",b", "\"" + "a".repeat(5000) + "\",b", "a".repeat(4096) + "\uD800\uDC00,b",
  "a".repeat(1000) + "\"\"\"\"\"\"\"\",b", "\"" + "x".repeat(100) + "\"\"\"\"\"\"y\",z",
  "\\\\,\\\\\\\\,a", "a\\b\\c,d", "\t\t,\t", "a\t\t\tb,c", "\u0000\u0001\u0002\u001F,",
  "\"\u0000\u0001\u001F\",a", "\u0000".repeat(40) + ",b", "é".repeat(100) + ",ü",
  "\uFEFF" + "a".repeat(100) + ",b", "a\uFEFF".repeat(30) + ",b",
  "\"a\"\"b\"\"c\"\"d\",e", "\"".repeat(33) + ",a", "\"".repeat(34) + ",a",
  "a\r".repeat(20) + "b", "\r".repeat(17) + "a", "\n".repeat(17) + "a,b",
  "x".repeat(15) + "\r\n" + "y".repeat(15), "x".repeat(16) + "\r" + "y",
  "\"multi\nline\nwith\nnewlines\",b\nc,d", "\"a\rb\rc\rd\",e",
);

let checked = 0, mism = 0;
function check(input, label) {
  checked++;
  let m, r, o;
  try { m = mine(input); } catch (e) { console.log("MINE THREW", label, e.message, JSON.stringify(input).slice(0, 200)); mism++; return; }
  try { r = ref(input); } catch (e) { console.log("REF THREW", label, e.message); mism++; return; }
  try { o = oldjs(input); } catch (e) { console.log("OLD THREW", label, e.message); mism++; return; }
  const mb = JSON.stringify(m), rb = JSON.stringify(r), ob = JSON.stringify(o);
  if (mb !== rb || mb !== ob) {
    mism++;
    console.log(`MISMATCH ${label} input=${JSON.stringify(input).slice(0, 300)}`);
    console.log("  mine:", mb.slice(0, 400));
    console.log("  ref :", rb.slice(0, 400));
    if (mb !== ob) console.log("  old :", ob.slice(0, 400));
    if (mism >= 5) { console.log("too many mismatches, stopping"); process.exit(1); }
  }
}

for (const a of ADV) check({ text: a, header: false }, "adv-nohdr");
for (const a of ADV) check({ text: a, header: true }, "adv-hdr");
console.log(`adversarial: ${ADV.length * 2} cases, mismatches so far: ${mism}`);
for (const a of ADV2) check({ text: a, header: false }, "adv2-nohdr");
for (const a of ADV2) check({ text: a, header: true }, "adv2-hdr");
console.log(`adversarial2 (chunk boundaries): ${ADV2.length * 2} cases, mismatches so far: ${mism}`);
for (let i = 0; i < N; i++) check(randCase(), `rand-${i}`);
console.log(`TOTAL: ${checked} cases checked, ${mism} mismatches (seed ${SEED})`);
process.exit(mism === 0 ? 0 : 1);
