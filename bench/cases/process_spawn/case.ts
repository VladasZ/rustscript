import { execFileSync } from "node:child_process";

const helper = process.argv[2];
const launches = Number.parseInt(process.argv[3], 10);
const t = performance.now();
let checksum = 0;
for (let i = 0; i < launches; i++) {
  const value = Number.parseInt(execFileSync(helper, [String(i)], { encoding: "utf8" }).trim(), 10);
  checksum = (checksum + value) % 1000000007;
}
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`launches=${launches} checksum=${checksum}`);
console.error(`COMPUTE_NS ${ns}`);
