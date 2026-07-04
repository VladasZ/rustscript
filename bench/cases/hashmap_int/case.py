import sys
import time

n = int(sys.argv[1]) if len(sys.argv) > 1 else 150000
t = time.perf_counter_ns()
# minstd LCG, exact in f64 so every language generates the same sequence.
x = 12345
counts = {}
for _ in range(n):
    x = x * 48271 % 2147483647
    k = x % 65536
    counts[k] = counts.get(k, 0) + 1
total = 0
hits = 0
for k in range(65536):
    c = counts.get(k)
    if c is not None:
        total += c
        hits += 1
ns = time.perf_counter_ns() - t
print(f"keys={len(counts)} hits={hits} total={total}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
