// Differential fuzz: WASM entry vs csv-reader 1.0.0 reference (kit baseline).
// Compares both canonical values AND JSON.stringify bytes (catches key order).
// Seeded PRNG, so a run is reproducible. Usage: node fuzz.mjs [N] [seed]
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const KIT = "/home/hatch/workspace/makefun/work/csv-reader-speed-01-run-20261121-kit/csv-reader-speed-01-run-20261121-kit";

const { run: mine } = await import(new URL("./solve.js", import.meta.url).href);
const { run: ref } = await import(new URL(join(KIT, "baselines", "csv-reader", "solve.js"), import.meta.url).href);
const { run: oldjs } = await import(new URL("../csv-reader-speed-01-entry/solve.js", import.meta.url).href);

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
for (let i = 0; i < N; i++) check(randCase(), `rand-${i}`);
console.log(`TOTAL: ${checked} cases checked, ${mism} mismatches (seed ${SEED})`);
process.exit(mism === 0 ? 0 : 1);
