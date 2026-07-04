import json
import sys
import time

path = sys.argv[1]
with open(path) as f:
    text = f.read()
t = time.perf_counter_ns()
items = json.loads(text)
total = 0
ids = 0
for it in items:
    total += it["value"]
    ids += it["id"]
count = len(items)
ns = time.perf_counter_ns() - t
print(f"count={count} sum={total} ids={ids}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
