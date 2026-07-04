import sys
import time

n = int(sys.argv[1]) if len(sys.argv) > 1 else 200000
t = time.perf_counter_ns()
parts = []
for i in range(n):
    parts.append("item")
    parts.append(str(i))
    parts.append(" ")
s = "".join(parts)
hits = len(s.split("item12")) - 1
replaced = s.replace("item9", "ITEM")
ns = time.perf_counter_ns() - t
print(f"len={len(s)} hits={hits} rlen={len(replaced)}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
