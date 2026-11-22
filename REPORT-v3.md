# csv-reader Round 5 — Laporan Rewrite UTF-8 (22 Nov 2026)

Perintah: Cipoz, round 5 — "build a faster entry (target: top 100,
median ≤70,000 µs server)". Repo: taufik52/csv-reader-speed-01.
Kompetisi tutup 23 Nov 2026 09:42 WIB. Intel tambahan (koordinator,
14:35 WIB): entry WASM SIMD lain di 40.000 µs membuktikan target bisa
ditembus; kuota hari ini 5 tersisa.

## Yang dibangun

- `src/lib.rs` + `Cargo.toml` (baru di repo; sebelumnya hanya `csv.wasm`
  tanpa source) — transducer Rust (cdylib, std, tanpa `format!`):
  - **Jalur UTF-8 (v3, timed path)**: parser csv-reader 1.0.0 exact di
    atas byte UTF-8 (delimiter ASCII tak pernah muncul di dalam sekuens
    multi-byte, jadi scan byte = scan unit). JSON unescape langsung ke
    byte UTF-8 (SIMD 16 byte/chunk, bulk-copy bila tanpa `"`/`\`). Scan
    delimiter + emit JSON-escaping fused per field (SIMD128), emit
    langsung ke buffer answers (tanpa row buffer, tanpa copy kedua
    output ~12 MB). `solve.js run()` encode UTF-8 via
    `TextEncoder.encodeInto` (satu copy) dan panggil `csv_parse8`,
    kecuali teks mengandung lone surrogate → jalur UTF-16.
  - **Jalur UTF-16 (v2, fallback)**: parser v2 utuh (`parse_case16`,
    `csv_parse`, `json_string_u16`), dipakai bila satu case mengandung
    lone surrogate di escape `\uXXXX` (transduce) atau di string JS
    (run). Plus perbaikan pairing surrogate (di bawah).
- `solve.js` — ditulis ulang: `run(input)` (kontrak lama, UTF-8 +
  deteksi lone surrogate via regex), `parseCase` (UTF-16, fallback),
  `parseCaseUtf8`, `transduce(raw)` (tak berubah).
- `fuzz.mjs` — path kit diperbaiki ke kit-36, `oldjs` menunjuk ke
  solve.js v2; kasus adversarial baru ADV2 (chunk-boundary: pasangan
  surrogate + lone surrogate di tiap offset sekitar tepi chunk 16 byte,
  field panjang 5.000, run kontrol/backslash/tab).
- `fuzz-json.mjs` (baru) — fuzz jalur `transduce()` mentah: `cases.json`
  dibangun byte-per-byte dengan escaping JSON acak per karakter (raw
  UTF-8, `\"`/`\\`/`\/`, `\uXXXX` hex acak besar/kecil, pasangan
  surrogate sebagai escape, lone surrogate sebagai escape).
- `bench.mjs` (path kit-36), `bench-alone.mjs` (baru, fresh-process
  round-robin v3/v2/baseline/null), `bench-transduce.mjs` (baru,
  transduce CPU in-process + cek byte-identik).
- `main.ts` — komentar diperbarui (v3 UTF-8 + fallback).
- `csv.wasm` — 119.555 byte (dua parser + forced inlining; tetap jauh
  di bawah 1 MiB; total file kerja ~229 KB, di luar .git).

## Profiling (yang diukur, bukan ditebak)

Praktik: 6 case, 10.193.953 unit, 0 lone surrogate, 0 `\u` escape di
file mentah, 418.724 backslash (mayoritas `\"`), 709.592 field,
63.692 record, field rata-rata 14,4 byte. Error: long 603, short 621,
quote_junk 284, stray_cr 260, unterminated 2.

- v2 transduce (best of 7, in-process): **113,2 ms**, byte-identik.
- v3 dua-fase (unescape + parse UTF-8), best of 7: **90,3 ms** — hanya
  1,25x. Breakdown (CPU, warmed): transduce ~53 ms = unescape ~16 ms
  + parse8 ~37 ms. Sintetik (CPU min): all-'a' 10,2M 12,8 ms (lantai
  bulk-copy), `a,` 5,1 jt field 164 ms (~32 ns/field), 16-byte field
  ~600 rb 24,7 ms, realistic 29,1 ms.
- **Unified scanner (chunk + mask di-cache lintas field) DICOBA dan
  DIBATALKAN**: A/B bersih (dua wasm di proses yang sama, bergantian):
  transduce med 97–106 ms vs 79–83 ms (lama); parse8-only 52 vs 36 ms.
  Reload chunk SIMD per field ternyata lebih murah daripada overhead
  state-machine skalar per point. Pelajaran: biaya per-field didominasi
  overhead skalar, bukan op SIMD.
- **Fusi unescape JSON + parse CSV DICOBA dan DIBATALKAN** (22 Nov,
  setelah commit di atas): parser fused yang membaca CSV langsung dari
  byte JSON-escaped (tanpa pass TEXT8), dengan end-finder SIMD untuk
  span string dan fallback UTF-16 saat lone surrogate. A/B bersih
  (dua wasm di proses yang sama, bergantian, CPU): fused best 56,6 ms
  / med 58,5 ms vs dua-fase best 45,8 ms / med 48,3 ms — fused
  **~11 ms LEBIH LAMBAT**. 418 ribu escape butuh decode skalar di
  dalam loop parse, lebih mahal daripada pass unescape SIMD yang
  dihapus, ditambah pass end-finder. Output fused byte-identik dengan
  dua-fase (sha sama), tapi kecepatannya kalah — build dikembalikan ke
  dua-fase (`a69aac8`, tidak berubah). Kode fused tersimpan di luar
  repo sebagai referensi; satu bug splice sempat terjadi saat
  penyambungan (atribut `#[no_mangle]` tertinggal menempel ke enum
  dan `csv_transduce` baru kehilangan ekspornya — diperbaiki, lalu
  tetap dibatalkan karena A/B kalah). Pelajaran: satu pass bukan
  otomatis lebih cepat bila pass itu memindahkan kerja skalar ke
  dalam loop terpanas.
- **Forced `#[inline(always)]` pada reader MENANG**: A/B: parse8-only
  36 → 30 ms, `a,` 149 → 99 ms, transduce med 77–82 → 71–72 ms.
  Inlining `json_string_u8` + `parse_case8` juga: tidak ada beda
  (wasm 100 → 146 KB), dibatalkan. Build final = inline reader saja.
- Cold single-run (proses fresh, 1x transduce, CPU): default tier-up
  85,5 ms vs `--liftoff-only` 101,1 ms vs `--no-wasm-tier-up` 94,7 ms.
  **Jangan paksa Liftoff-only** — TurboFan tetap menang walau cold
  (kompilasi wasm cuma ~4 ms). Klaim "Liftoff-only" kompetitor tidak
  direplikasi; diukur sendiri di mesin ini.
- Hasil akhir (warmed, CPU): **transduce 45,0–46,8 ms** (best of 7:
  46,8 ms), parse8-only ~30 ms, unescape ~15 ms. **2,4x dari v2.**

## Bug yang ditemukan & diperbaiki (fuzz-json)

**Pairing surrogate berdasarkan adjacency NILAI, bukan teks** —
ditemukan fuzz-json (2 mismatch seed 20261122, 5 seed 7), ada juga di
v2 dan lolos dari fuzz v2 (168 rb checks) karena polanya langka:
- high surrogate di akhir konten quoted + low surrogate di awal junk
  (closing quote di antaranya di teks, tapi bersebelahan di nilai) →
  JSON.stringify mengeluarkan karakter astral, emitter lama menulis
  `\ud800\udc00` terpisah.
- high surrogate + stray CR (terbuang) + low surrogate → sama.
Perbaikan: state `pend_hi` di parser UTF-16 (`P.pend_hi`): high
surrogate di ujung span ditahan, tidak ditulis; span berikut yang
diawali low surrogate mengeluarkan pasangan astral UTF-8; bila bukan
low, high ditulis sebagai `\ud800` (flush di akhir field, sebelum
`"` penutup, dan sebelum emisi `\"`/`\n` langsung; di-clear — bukan
di-flush — saat truncate unterminated_quote). Jalur UTF-8 kebal (tak
ada surrogate di teks UTF-8). Pasca-perbaikan: fuzz-json 21.000 kasus
(5 seed) 0 mismatch; fuzz run-level 55.214 checks (3 seed pasca-fix)
0 mismatch.

## Verifikasi (build final)

- Practice 6/6, answers.json **byte-identik** dengan
  `JSON.stringify(expected)`.
- Fuzz diferensial vs reference + vs v2: **~71.000 checks run-level
  (4 seed), 0 mismatch**; **21.000 checks JSON-level transduce
  (5 seed), 0 mismatch**; uji lone-surrogate transduce khusus
  (pasangan/lone/astral/`\u00e9`/`\uFFFF`, malformed → 0,
  `[]` → 2) lolos.
- grade-local 2x `--driver process --worktree` **pada build final
  `a69aac8`** (22 Nov, setelah perbaikan surrogate dan setelah fusi
  dibatalkan): 6/6, passed, prize_eligible, L1 reproduced byte for
  byte, result_sha256 identik di kedua run
  `b91ff2369a0d3fcdab6a4b8eb4e810a8f9890e0e4c1d97d08a5e69e48393f37e`
  (berbeda dari `4ecdb818…` di run pra-perbaikan karena wasm berubah
  oleh perbaikan surrogate). Catatan CLI: node lokal v24.22.1 vs
  server v26.13.1; "authored 0.000000000 of 2144 counted lines"
  (transcript gate jalan saat submission, domain koordinator).
- Bench-alone, fresh process, **30 trial round-robin** (median wall,
  run 22 Nov siang): v3 254,4 ms (net 13,4), v2 319,8 ms (net 78,8),
  baseline 1.646,4 ms (net 1.405,4). **Gross: 1,26x vs v2; 6,47x vs
  baseline.** Catatan: angka "net" bergantung pada kontrol null yang
  melakukan JSON.parse penuh atas input 11 MB — metrik utama adalah
  transduce CPU A/B (2,4x) dan median gross di atas. Run sebelumnya
  (20 trial): v3 341,0 ms, v2 391,1 ms, baseline 1.723,0 ms — beban VM
  berubah-ubah antar run; perbandingan dalam satu run yang sama yang
  sahih.
- Repo **~229 KB file kerja (≤1 MiB)**. `node --check` solve/main/fuzz/
  bench semua OK. Fuzz segar pada build final: **8.738 checks
  run-level (seed 20261122, termasuk 272 adversarial + 466
  chunk-boundary), 0 mismatch**; **3.000 checks JSON-level transduce,
  0 mismatch**.
- Risiko tercatat: aturan tertulis bilang "no WASM" — server sudah
  grading entry WASM 6/6 prize_eligible dua kali termasuk top-10
  all-WASM, jadi server yang jadi otoritas; bentuk entry sama dengan
  yang sudah lolos itu.

## Status

Build final **`a69aac8`** **memenuhi submit gate** (transduce lokal
46,8 ms ≤ 60 ms, demonstrably better dari v2, 2,4x) tapi **belum
mencapai target ≤30 ms**. Fusi unescape+parse — peluang terbesar yang
tersisa — sudah dicoba dan **kalah di A/B** (56,6 ms vs 45,8 ms),
jadi tidak ada lagi tuas besar yang terbukti di meja. Proyeksi server
kasar (rasio server:lokal v2 ~3,3x): ~150.000 µs (di luar top 100
yang butuh ≤70.000 µs, tapi jauh lebih baik dari v2 380.000 µs).
Keputusan submit di tangan koordinator.
