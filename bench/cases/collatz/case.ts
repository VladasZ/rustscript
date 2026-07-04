function steps(start: number): number {
  let n = start;
  let c = 0;
  while (n !== 1) {
    if (n % 2 === 0) {
      n = n / 2;
    } else {
      n = 3 * n + 1;
    }
    c++;
  }
  return c;
}

const limit = process.argv[2] ? parseInt(process.argv[2], 10) : 10000;
const t = performance.now();
let total = 0;
for (let i = 1; i <= limit; i++) {
  total += steps(i);
}
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`total steps for 1..${limit}: ${total}`);
console.error(`COMPUTE_NS ${ns}`);
