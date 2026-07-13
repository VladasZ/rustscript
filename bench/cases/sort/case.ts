const n = process.argv[2] ? parseInt(process.argv[2], 10) : 50000;
const t = performance.now();
// minstd LCG, exact in f64 so every language generates the same sequence.
let x = 12345;
const v: number[] = [];
for (let i = 0; i < n; i++) {
  x = (x * 48271) % 2147483647;
  v.push(x % 1000000);
}
// Sort through a comparison callback, bucket first, value second.
v.sort((a, b) => (a % 1000) - (b % 1000) || a - b);
const len = v.length;
let probe = 0;
for (let i = 0; i < len; i += Math.floor(len / 10)) {
  probe += v[i];
}
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`first=${v[0]} mid=${v[Math.floor(len / 2)]} last=${v[len - 1]} probe=${probe}`);
console.error(`COMPUTE_NS ${ns}`);
