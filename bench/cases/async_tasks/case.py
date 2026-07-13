import asyncio
import sys
import time


tasks = int(sys.argv[1])
yields_per_task = 100


async def scheduled(value):
    for _ in range(yields_per_task):
        await asyncio.sleep(0)
    return value


async def main():
    t = time.perf_counter_ns()
    values = await asyncio.gather(*(scheduled(value) for value in range(tasks)))
    checksum = sum(values)
    ns = time.perf_counter_ns() - t
    print(f"tasks={tasks} yields={tasks * yields_per_task} checksum={checksum}")
    print(f"COMPUTE_NS {ns}", file=sys.stderr)


asyncio.run(main())
