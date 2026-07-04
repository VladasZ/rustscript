import sys
import time

n = int(sys.argv[1]) if len(sys.argv) > 1 else 100000
t = time.perf_counter_ns()
# minstd LCG, exact in f64 so every language generates the same sequence.
x = 12345
v = []
for _ in range(n):
    x = x * 48271 % 2147483647
    v.append(x % 1000)
total = sum(b for b in (a * 3 + 1 for a in v) if b % 2 == 0)
count = len([a for a in v if a > 500])
any_big = any(a > 995 for a in v)
ns = time.perf_counter_ns() - t
print(f"sum={total} count={count} any={str(any_big).lower()}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
