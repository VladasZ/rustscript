import { readFileSync } from "fs";

const path = process.argv[2];
const text = readFileSync(path, "utf8");
const t = performance.now();
let matches = 0;
let spans = 0;
for (const m of text.matchAll(/w0\d\d/g)) {
  matches++;
  spans += (m.index as number) % 1000;
}
let digits = 0;
for (const m of text.matchAll(/w(\d)(\d)9\d/g)) {
  digits += parseInt(m[1], 10) * 10 + parseInt(m[2], 10);
}
const replaced = text.replace(/w00\d/g, "X");
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`matches=${matches} spans=${spans} digits=${digits} rlen=${replaced.length}`);
console.error(`COMPUTE_NS ${ns}`);
