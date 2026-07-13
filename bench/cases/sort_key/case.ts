const n = process.argv[2] ? parseInt(process.argv[2], 10) : 50000;
const t = performance.now();
let x = 12345;
const values: number[] = [];
for (let i = 0; i < n; i++) {
  x = (x * 48271) % 2147483647;
  values.push(x % 1000000);
}
const decorated = values.map((value) => ({
  bucket: value % 1000,
  value,
}));
decorated.sort((a, b) => a.bucket - b.bucket || a.value - b.value);
const sorted = decorated.map((item) => item.value);
const len = sorted.length;
let probe = 0;
for (let i = 0; i < len; i += Math.floor(len / 10)) {
  probe += sorted[i];
}
const ns = Math.round((performance.now() - t) * 1e6);
console.log(
  `first=${sorted[0]} mid=${sorted[Math.floor(len / 2)]} ` +
    `last=${sorted[len - 1]} probe=${probe}`,
);
console.error(`COMPUTE_NS ${ns}`);
