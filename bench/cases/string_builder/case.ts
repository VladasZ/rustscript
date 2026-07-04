const n = process.argv[2] ? parseInt(process.argv[2], 10) : 200000;
const t = performance.now();
let s = "";
for (let i = 0; i < n; i++) {
  s += "item";
  s += i;
  s += " ";
}
const hits = s.split("item12").length - 1;
const replaced = s.replaceAll("item9", "ITEM");
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`len=${s.length} hits=${hits} rlen=${replaced.length}`);
console.error(`COMPUTE_NS ${ns}`);
