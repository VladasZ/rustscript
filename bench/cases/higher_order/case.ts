const n = process.argv[2] ? parseInt(process.argv[2], 10) : 100000;
const t = performance.now();
// minstd LCG, exact in f64 so every language generates the same sequence.
let x = 12345;
const v: number[] = [];
for (let i = 0; i < n; i++) {
  x = (x * 48271) % 2147483647;
  v.push(x % 1000);
}
const sum = v.map((a) => a * 3 + 1).filter((a) => a % 2 === 0).reduce((acc, a) => acc + a, 0);
const count = v.filter((a) => a > 500).length;
const anyBig = v.some((a) => a > 995);
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`sum=${sum} count=${count} any=${anyBig}`);
console.error(`COMPUTE_NS ${ns}`);
