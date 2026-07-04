import { readFileSync } from "fs";

const path = Bun.argv[2];
const text = readFileSync(path, "utf8");
const t = performance.now();
const counts = new Map<string, number>();
for (const w of text.trim().split(/\s+/)) {
  counts.set(w, (counts.get(w) || 0) + 1);
}
const pairs = [...counts.entries()];
pairs.sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
const ns = Math.round((performance.now() - t) * 1e6);
for (let i = 0; i < 15; i++) {
  console.log(`${pairs[i][0]} ${pairs[i][1]}`);
}
console.error(`COMPUTE_NS ${ns}`);
