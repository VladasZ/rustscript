const base = process.argv[2];
const requests = Number.parseInt(process.argv[3], 10);

async function main(): Promise<void> {
  const t = performance.now();
  let ids = 0;
  let values = 0;
  for (let i = 0; i < requests; i++) {
    const item = await (await fetch(`${base}/item/${i}`)).json();
    ids += item.id;
    values += item.value;
  }
  const ns = Math.round((performance.now() - t) * 1e6);
  console.log(`requests=${requests} ids=${ids} values=${values}`);
  console.error(`COMPUTE_NS ${ns}`);
}

await main();
