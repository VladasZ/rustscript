const n = process.argv[2] ? parseInt(process.argv[2], 10) : 250000;
const t = performance.now();
const isPrime = new Uint8Array(n + 1).fill(1);
isPrime[0] = 0;
isPrime[1] = 0;
for (let i = 2; i * i <= n; i++) {
  if (isPrime[i]) {
    for (let j = i * i; j <= n; j += i) isPrime[j] = 0;
  }
}
let count = 0;
for (let k = 2; k <= n; k++) if (isPrime[k]) count++;
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`primes up to ${n}: ${count}`);
console.error(`COMPUTE_NS ${ns}`);
