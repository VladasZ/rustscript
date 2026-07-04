const n = process.argv[2] ? parseInt(process.argv[2], 10) : 100000;
const t = performance.now();
// minstd LCG, exact in f64 so every language generates the same sequence.
let x = 12345;
const items: { id: number; value: number; name: string }[] = [];
for (let i = 0; i < n; i++) {
  x = (x * 48271) % 2147483647;
  items.push({ id: i, value: x % 1000, name: `n${x % 10000}` });
}
const out = JSON.stringify(items);
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`len=${out.length}`);
console.error(`COMPUTE_NS ${ns}`);
