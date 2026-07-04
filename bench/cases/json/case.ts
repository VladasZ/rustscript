import { readFileSync } from "fs";

const path = process.argv[2];
const text = readFileSync(path, "utf8");
const t = performance.now();
const items = JSON.parse(text);
let sum = 0;
let ids = 0;
for (const it of items) {
  sum += it.value;
  ids += it.id;
}
const count = items.length;
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`count=${count} sum=${sum} ids=${ids}`);
console.error(`COMPUTE_NS ${ns}`);
