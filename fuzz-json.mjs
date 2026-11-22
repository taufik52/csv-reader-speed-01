// JSON-level differential fuzz for the transduce() path (v3).
// Builds cases.json RAW bytes with randomized JSON escaping per character
// (raw UTF-8, \" \\ \/ \b \f \n \r \t, \uXXXX with random hex case,
// surrogate pairs as escapes, lone surrogates as escapes), then compares
// transduce(raw) against csv-reader 1.0.0's reference on the decoded inputs.
// This exercises json_string_u8 + the lone-surrogate fallback, which the
// run()-level fuzzer (fuzz.mjs) never touches. Usage:
//   node fuzz-json.mjs [N] [seed]   (default 3000, 20261122)
import { readFileSync } from "node:fs";

const KIT = "/home/hatch/workspace/makefun/csv-reader-kit-36/csv-reader-speed-01-run-20261121-kit";
const { run: ref } = await import(new URL(KIT + "/baselines/csv-reader/solve.js", import.meta.url).href);
const { transduce, wasmExports, wasmMemory } = await import(new URL("./solve.js", import.meta.url).href);

const N = Number(process.argv[2] || 3000);
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

// ---- random CSV-ish text (may hold lone surrogates, astral chars, controls) ----
const FRAGS = ["a", "b", "ab", "x", " ", "0", "é", "ü", "東京", "\t", "a\tb", "\u2028", "\uD800", "\uDC00", "\uD800\uDC00", "\uD83D\uDE00", "\uFEFF", "\\", "a\\b", "\u0000", "\u0001", "\u001F", "\u007F"];
const QUOTES = ["", '"', '""', '"""', '"a', 'a"', '"a"', '"a""b"', '"a"b', '"a\nb"', '"a\r\nb"', '"a\rb"'];
function randField() {
  const r = rnd();
  if (r < 0.5) return pick(FRAGS);
  if (r < 0.8) return pick(QUOTES) + pick(FRAGS) + pick(QUOTES);
  const alpha = ["a", ",", '"', "\n", "\r", " ", "é", "\uD800", "\uDC00", "\\", "\t", "\u0000"];
  let s = "";
  const len = ri(10);
  for (let i = 0; i < len; i++) s += pick(alpha);
  return s;
}
function randText() {
  const rows = ri(4);
  let text = rnd() < 0.15 ? "\uFEFF" : "";
  const cols = 1 + ri(3);
  for (let r = 0; r <= rows; r++) {
    const n = 1 + ri(cols + 2);
    const fields = [];
    for (let i = 0; i < n; i++) fields.push(randField());
    text += fields.join(",");
    if (r < rows) text += pick(["\n", "\r\n", "\r", ""]);
  }
  return text;
}

// ---- encode one JS string as a JSON string literal with random escaping ----
function hex(n, up) {
  const h = n.toString(16).padStart(4, "0");
  return up ? h.toUpperCase() : h;
}
function jsonEscapeChar(ch, code) {
  const r = rnd();
  if (ch === '"') return r < 0.7 ? '\\"' : "\\u" + hex(0x22, r < 0.85);
  if (ch === "\\") return r < 0.7 ? "\\\\" : "\\u" + hex(0x5c, r < 0.85);
  if (ch === "\n") return r < 0.7 ? "\\n" : "\\u" + hex(0x0a, r < 0.85);
  if (ch === "\r") return r < 0.7 ? "\\r" : "\\u" + hex(0x0d, r < 0.85);
  if (ch === "\t") return r < 0.7 ? "\\t" : "\\u" + hex(0x09, r < 0.85);
  if (ch === "\b") return r < 0.5 ? "\\b" : "\\u" + hex(0x08, r < 0.75);
  if (ch === "\f") return r < 0.5 ? "\\f" : "\\u" + hex(0x0c, r < 0.75);
  if (ch === "/") return r < 0.3 ? "\\/" : "/";
  if (code < 0x20) return "\\u" + hex(code, r < 0.5);
  if (code >= 0xd800 && code <= 0xdbff) return "\\u" + hex(code, r < 0.5); // lone high (pair handled by caller path below)
  if (code >= 0xdc00 && code <= 0xdfff) return "\\u" + hex(code, r < 0.5); // lone low
  if (code >= 0x80) return r < 0.25 ? "\\u" + hex(code, r < 0.5) : ch; // raw UTF-8 usually
  return ch;
}
function jsonString(text) {
  let out = '"';
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    // Surrogate pair: emit as two \u escapes or raw astral char, randomly.
    if (code >= 0xd800 && code <= 0xdbff && i + 1 < text.length) {
      const lo = text.charCodeAt(i + 1);
      if (lo >= 0xdc00 && lo <= 0xdfff) {
        if (rnd() < 0.5) {
          out += "\\u" + hex(code, rnd() < 0.5) + "\\u" + hex(lo, rnd() < 0.5);
        } else {
          out += text[i] + text[i + 1];
        }
        i++;
        continue;
      }
    }
    out += jsonEscapeChar(text[i], code);
  }
  return out + '"';
}

let checked = 0, mism = 0;
for (let batch = 0; batch < Math.ceil(N / 50); batch++) {
  const count = Math.min(50, N - batch * 50);
  const inputs = [];
  let rawStr = "[";
  for (let i = 0; i < count; i++) {
    const text = randText();
    const header = rnd() < 0.5;
    inputs.push({ text, header });
    if (i > 0) rawStr += ",";
    rawStr += `{"id":"j${batch}-${i}","input":{"text":${jsonString(text)},"header":${header}}}`;
  }
  rawStr += "]";
  const raw = Buffer.from(rawStr, "utf8");
  // The file must parse to exactly the inputs we generated.
  const parsed = JSON.parse(raw.toString("utf8"));
  for (let i = 0; i < count; i++) {
    if (parsed[i].input.text !== inputs[i].text || parsed[i].input.header !== inputs[i].header) {
      console.log("GENERATOR MISMATCH", JSON.stringify(inputs[i]).slice(0, 200));
      mism++;
    }
  }
  const len = transduce(raw);
  if (len === 0) {
    console.log("TRANSDUCE FAILED batch", batch, rawStr.slice(0, 300));
    mism++;
    continue;
  }
  const got = Buffer.from(new Uint8Array(wasmMemory(), wasmExports.csv_out_ptr(), len)).toString("utf8");
  const want = JSON.stringify(inputs.map((inp) => ref(inp)));
  checked += count;
  if (got !== want) {
    mism++;
    console.log(`MISMATCH batch ${batch}`);
    console.log("  raw :", rawStr.slice(0, 400));
    console.log("  got :", got.slice(0, 400));
    console.log("  want:", want.slice(0, 400));
    if (mism >= 5) { console.log("too many mismatches, stopping"); process.exit(1); }
  }
}
console.log(`TOTAL: ${checked} JSON-level cases checked, ${mism} mismatches (seed ${SEED})`);
process.exit(mism === 0 ? 0 : 1);
