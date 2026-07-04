import sys
import time


def steps(start):
    n = start
    c = 0
    while n != 1:
        if n % 2 == 0:
            n = n // 2
        else:
            n = 3 * n + 1
        c += 1
    return c


limit = 10000
t = time.perf_counter_ns()
total = 0
for i in range(1, limit + 1):
    total += steps(i)
ns = time.perf_counter_ns() - t
print(f"total steps for 1..{limit}: {total}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
