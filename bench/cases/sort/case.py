import sys
import time
from functools import cmp_to_key

n = int(sys.argv[1]) if len(sys.argv) > 1 else 50000
t = time.perf_counter_ns()
# minstd LCG, exact in f64 so every language generates the same sequence.
x = 12345
v = []
for _ in range(n):
    x = x * 48271 % 2147483647
    v.append(x % 1000000)
# Sort through a comparison callback, bucket first, value second.
def compare(a, b):
    bucket = (a % 1000) - (b % 1000)
    return bucket if bucket else a - b


v.sort(key=cmp_to_key(compare))
length = len(v)
probe = 0
i = 0
while i < length:
    probe += v[i]
    i += length // 10
ns = time.perf_counter_ns() - t
print(f"first={v[0]} mid={v[length // 2]} last={v[length - 1]} probe={probe}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
