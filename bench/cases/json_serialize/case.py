import json
import sys
import time

n = int(sys.argv[1]) if len(sys.argv) > 1 else 100000
t = time.perf_counter_ns()
# minstd LCG, exact in f64 so every language generates the same sequence.
x = 12345
items = []
for i in range(n):
    x = x * 48271 % 2147483647
    items.append({"id": i, "value": x % 1000, "name": f"n{x % 10000}"})
out = json.dumps(items, separators=(",", ":"))
ns = time.perf_counter_ns() - t
print(f"len={len(out)}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
