type Tree = { l: Tree | null; r: Tree | null } | null;

function make(depth: number): Tree {
  return depth === 0 ? null : { l: make(depth - 1), r: make(depth - 1) };
}

function check(t: Tree): number {
  return t === null ? 1 : 1 + check(t.l) + check(t.r);
}

const max = process.argv[2] ? parseInt(process.argv[2], 10) : 11;
const t = performance.now();
let total = 0;
let d = 4;
while (d <= max) {
  const iters = 1 << (max - d + 2);
  for (let i = 0; i < iters; i++) {
    total += check(make(d));
  }
  d += 2;
}
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`total ${total} depth ${max}`);
console.error(`COMPUTE_NS ${ns}`);
