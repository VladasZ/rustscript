import sys
import time


def fib(n):
    return n if n < 2 else fib(n - 1) + fib(n - 2)


n = int(sys.argv[1]) if len(sys.argv) > 1 else 27
t = time.perf_counter_ns()
r = fib(n)
ns = time.perf_counter_ns() - t
print(f"fib({n}) = {r}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
