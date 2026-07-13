import { readFileSync, writeFileSync } from "node:fs";

const configPath = process.argv[2];
const inputPath = process.argv[3];
const outputPath = process.argv[4];
const t = performance.now();
const config = JSON.parse(readFileSync(configPath, "utf8"));
const text = readFileSync(inputPath, "utf8");
const regex = new RegExp(config.pattern, "g");
const counts = new Map<string, number>();
let total = 0;
for (const found of text.matchAll(regex)) {
  const token = found[0];
  counts.set(token, (counts.get(token) || 0) + 1);
  total++;
}
const pairs = [...counts.entries()];
pairs.sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
const top = pairs.slice(0, config.top).map(([token, count]) => ({ token, count }));
const saved = JSON.stringify({ total, unique: counts.size, top });
writeFileSync(outputPath, saved);
const reread = readFileSync(outputPath, "utf8");
let checksum = 0;
for (const byte of Buffer.from(reread)) checksum = (checksum + byte) % 1000000007;
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`total=${total} unique=${counts.size} bytes=${Buffer.byteLength(reread)} checksum=${checksum}`);
console.error(`COMPUTE_NS ${ns}`);
