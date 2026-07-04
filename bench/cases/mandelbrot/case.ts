const w = 140;
const h = 140;
const maxIter = 140;
const t = performance.now();
let count = 0;
for (let py = 0; py < h; py++) {
  const y0 = (py / h) * 3.0 - 1.5;
  for (let px = 0; px < w; px++) {
    const x0 = (px / w) * 3.0 - 2.0;
    let x = 0.0;
    let y = 0.0;
    let it = 0;
    while (x * x + y * y <= 4.0 && it < maxIter) {
      const xt = x * x - y * y + x0;
      y = 2.0 * x * y + y0;
      x = xt;
      it++;
    }
    if (it === maxIter) count++;
  }
}
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`in set: ${count}`);
console.error(`COMPUTE_NS ${ns}`);
