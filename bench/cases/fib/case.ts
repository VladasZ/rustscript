function fib(n: number): number {
  return n < 2 ? n : fib(n - 1) + fib(n - 2);
}

const n = process.argv[2] ? parseInt(process.argv[2], 10) : 27;
const t = performance.now();
const r = fib(n);
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`fib(${n}) = ${r}`);
console.error(`COMPUTE_NS ${ns}`);
