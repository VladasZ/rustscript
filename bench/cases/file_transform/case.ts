import { readFileSync, writeFileSync } from "node:fs";

const input = process.argv[2];
const output = process.argv[3];
const t = performance.now();
const text = readFileSync(input, "utf8");
const selected: string[] = [];
for (const line of text.split(/\r?\n/)) {
  if (line.includes("w000")) selected.push(line.replaceAll("w00", "W"));
}
const transformed = selected.length === 0 ? "" : `${selected.join("\n")}\n`;
writeFileSync(output, transformed);
const saved = readFileSync(output, "utf8");
let checksum = 0;
for (const byte of Buffer.from(saved)) checksum = (checksum + byte) % 1000000007;
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`lines=${selected.length} bytes=${Buffer.byteLength(saved)} checksum=${checksum}`);
console.error(`COMPUTE_NS ${ns}`);
