import sys
import time

n = int(sys.argv[1]) if len(sys.argv) > 1 else 20000
t = time.perf_counter_ns()
total = 0
for i in range(n):
    total += i
    print(f"{i} {total}")
ns = time.perf_counter_ns() - t
print(f"COMPUTE_NS {ns}", file=sys.stderr)
