import sys
import time

n = int(sys.argv[1]) if len(sys.argv) > 1 else 50000
t = time.perf_counter_ns()
# minstd LCG, exact in f64 so every language generates the same sequence.
x = 12345
v = []
for _ in range(n):
    x = x * 48271 % 2147483647
    v.append(x % 1000000)
# Sort through a per element callback, bucket first, value second.
v.sort(key=lambda a: (a % 1000, a))
length = len(v)
probe = 0
i = 0
while i < length:
    probe += v[i]
    i += length // 10
ns = time.perf_counter_ns() - t
print(f"first={v[0]} mid={v[length // 2]} last={v[length - 1]} probe={probe}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
