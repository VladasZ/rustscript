import { scheduler } from "node:timers/promises";

const tasks = Number.parseInt(process.argv[2], 10);
const yieldsPerTask = 100;

async function scheduled(value: number): Promise<number> {
  for (let iteration = 0; iteration < yieldsPerTask; iteration++) {
    await scheduler.yield();
  }
  return value;
}

async function main(): Promise<void> {
  const t = performance.now();
  const values = await Promise.all(
    Array.from({ length: tasks }, (_, value) => scheduled(value)),
  );
  const checksum = values.reduce((sum, value) => sum + value, 0);
  const ns = Math.round((performance.now() - t) * 1e6);
  console.log(`tasks=${tasks} yields=${tasks * yieldsPerTask} checksum=${checksum}`);
  console.error(`COMPUTE_NS ${ns}`);
}

await main();
