const n = process.argv[2] ? parseInt(process.argv[2], 10) : 150000;
const t = performance.now();
// minstd LCG, exact in f64 so every language generates the same sequence.
let x = 12345;
const counts = new Map<number, number>();
for (let i = 0; i < n; i++) {
  x = (x * 48271) % 2147483647;
  const k = x % 65536;
  counts.set(k, (counts.get(k) || 0) + 1);
}
let total = 0;
let hits = 0;
for (let k = 0; k < 65536; k++) {
  const c = counts.get(k);
  if (c !== undefined) {
    total += c;
    hits++;
  }
}
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`keys=${counts.size} hits=${hits} total=${total}`);
console.error(`COMPUTE_NS ${ns}`);
