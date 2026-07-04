import sys
import time


def make(depth):
    return None if depth == 0 else (make(depth - 1), make(depth - 1))


def check(t):
    return 1 if t is None else 1 + check(t[0]) + check(t[1])


max_depth = int(sys.argv[1]) if len(sys.argv) > 1 else 11
t = time.perf_counter_ns()
total = 0
d = 4
while d <= max_depth:
    iters = 1 << (max_depth - d + 2)
    for _ in range(iters):
        total += check(make(d))
    d += 2
ns = time.perf_counter_ns() - t
print(f"total {total} depth {max_depth}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
