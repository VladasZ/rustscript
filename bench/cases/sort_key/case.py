import sys
import time

n = int(sys.argv[1]) if len(sys.argv) > 1 else 50000
t = time.perf_counter_ns()
x = 12345
values = []
for _ in range(n):
    x = x * 48271 % 2147483647
    values.append(x % 1000000)
values.sort(key=lambda value: (value % 1000, value))
length = len(values)
probe = 0
i = 0
while i < length:
    probe += values[i]
    i += length // 10
ns = time.perf_counter_ns() - t
print(
    f"first={values[0]} mid={values[length // 2]} "
    f"last={values[length - 1]} probe={probe}"
)
print(f"COMPUTE_NS {ns}", file=sys.stderr)
