import re
import sys
import time

path = sys.argv[1]
with open(path) as f:
    text = f.read()
t = time.perf_counter_ns()
matches = 0
spans = 0
for m in re.finditer(r"w0\d\d", text):
    matches += 1
    spans += m.start() % 1000
digits = 0
for m in re.finditer(r"w(\d)(\d)9\d", text):
    digits += int(m.group(1)) * 10 + int(m.group(2))
replaced = re.sub(r"w00\d", "X", text)
ns = time.perf_counter_ns() - t
print(f"matches={matches} spans={spans} digits={digits} rlen={len(replaced)}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
