import sys
import time

n = 250000
t = time.perf_counter_ns()
is_prime = bytearray([1]) * (n + 1)
is_prime[0] = 0
is_prime[1] = 0
i = 2
while i * i <= n:
    if is_prime[i]:
        j = i * i
        while j <= n:
            is_prime[j] = 0
            j += i
    i += 1
count = sum(is_prime[2:])
ns = time.perf_counter_ns() - t
print(f"primes up to {n}: {count}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
