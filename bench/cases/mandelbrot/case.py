import sys
import time

w = 140
h = 140
max_iter = 140
t = time.perf_counter_ns()
count = 0
for py in range(h):
    y0 = (py / h) * 3.0 - 1.5
    for px in range(w):
        x0 = (px / w) * 3.0 - 2.0
        x = 0.0
        y = 0.0
        it = 0
        while x * x + y * y <= 4.0 and it < max_iter:
            xt = x * x - y * y + x0
            y = 2.0 * x * y + y0
            x = xt
            it += 1
        if it == max_iter:
            count += 1
ns = time.perf_counter_ns() - t
print(f"in set: {count}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
