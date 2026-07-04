const n = process.argv[2] ? parseInt(process.argv[2], 10) : 20000;
const t = performance.now();
let sum = 0;
for (let i = 0; i < n; i++) {
  sum += i;
  console.log(`${i} ${sum}`);
}
const ns = Math.round((performance.now() - t) * 1e6);
console.error(`COMPUTE_NS ${ns}`);
