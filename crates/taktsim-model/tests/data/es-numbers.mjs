// Generates es-numbers.txt: IEEE 754 doubles (as 16 hex digits) and the
// text ECMAScript's Number.prototype.toString gives them, the number form
// of RFC 8785 (JCS). The test `jcs_numbers_match_ecmascript` checks
// taktsim_model::jcs::number_to_string against every line.
//
// Run with Node.js (any version; the output was made with 26.12.0):
//   node crates/taktsim-model/tests/data/es-numbers.mjs > crates/taktsim-model/tests/data/es-numbers.txt
//
// Deterministic: a fixed xorshift64* generator, no Math.random.

let state = 0x9e3779b97f4a7c15n;
const mask = (1n << 64n) - 1n;
function next() {
  state ^= state >> 12n;
  state ^= (state << 25n) & mask;
  state ^= state >> 27n;
  return (state * 0x2545f4914f6cdd1dn) & mask;
}

const view = new DataView(new ArrayBuffer(8));
const lines = [];
function emit(x) {
  if (!Number.isFinite(x)) return;
  view.setFloat64(0, x);
  lines.push(`${view.getBigUint64(0).toString(16).padStart(16, "0")} ${String(x)}`);
}

// 1500 random bit patterns: every exponent equally likely.
for (let i = 0; i < 1500; i++) {
  view.setBigUint64(0, next());
  emit(view.getFloat64(0));
}
// 1000 values with 17 significant digits whose exact decimal value ends in
// 25 or 75: halfway between two 17-digit candidates (the tie rule).
for (let i = 0; i < 1000; i++) {
  const r = next();
  const scale = 2 ** (50 + Number(r % 3n)); // 2^50 .. 2^52: ulp 1/4 .. 1
  const quarter = Number((r >> 8n) % 4n) / 4;
  emit(scale + Number((r >> 16n) % BigInt(2 ** 40)) + quarter);
}
// 500 values around the plain/exponent boundaries (1e-7, 1e-6, 1e21) and
// powers of ten.
for (let i = 0; i < 500; i++) {
  const r = next();
  const base = [1e-7, 1e-6, 1e21, 1e20, 10 ** (Number(r % 40n) - 20)][Number((r >> 8n) % 5n)];
  const nudge = Number((r >> 16n) % 2001n) - 1000;
  view.setFloat64(0, base);
  view.setBigUint64(0, view.getBigUint64(0) + BigInt(nudge));
  emit(view.getFloat64(0));
  emit(-view.getFloat64(0));
}
console.log(lines.join("\n"));
