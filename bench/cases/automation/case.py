import json
import re
import sys
import time


config_path = sys.argv[1]
input_path = sys.argv[2]
output_path = sys.argv[3]
t = time.perf_counter_ns()
with open(config_path) as config_file:
    config = json.load(config_file)
with open(input_path) as input_file:
    text = input_file.read()
counts = {}
total = 0
for found in re.finditer(config["pattern"], text):
    token = found.group(0)
    counts[token] = counts.get(token, 0) + 1
    total += 1
pairs = sorted(counts.items(), key=lambda pair: (-pair[1], pair[0]))
top = [{"token": token, "count": count} for token, count in pairs[: config["top"]]]
saved = json.dumps({"total": total, "unique": len(counts), "top": top}, separators=(",", ":"))
with open(output_path, "w") as output_file:
    output_file.write(saved)
with open(output_path) as output_file:
    reread = output_file.read()
checksum = sum(reread.encode()) % 1000000007
ns = time.perf_counter_ns() - t
print(f"total={total} unique={len(counts)} bytes={len(reread.encode())} checksum={checksum}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
