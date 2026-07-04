import sys
import time

path = sys.argv[1]
with open(path) as f:
    text = f.read()
t = time.perf_counter_ns()
counts = {}
for w in text.split():
    counts[w] = counts.get(w, 0) + 1
pairs = sorted(counts.items(), key=lambda p: (-p[1], p[0]))
ns = time.perf_counter_ns() - t
for i in range(15):
    print(f"{pairs[i][0]} {pairs[i][1]}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
