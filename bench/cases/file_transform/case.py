import sys
import time


input_path = sys.argv[1]
output_path = sys.argv[2]
t = time.perf_counter_ns()
with open(input_path) as source:
    text = source.read()
selected = [line.replace("w00", "W") for line in text.splitlines() if "w000" in line]
transformed = "" if not selected else "\n".join(selected) + "\n"
with open(output_path, "w") as destination:
    destination.write(transformed)
with open(output_path) as saved_file:
    saved = saved_file.read()
checksum = sum(saved.encode()) % 1000000007
ns = time.perf_counter_ns() - t
print(f"lines={len(selected)} bytes={len(saved.encode())} checksum={checksum}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
