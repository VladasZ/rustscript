import subprocess
import sys
import time


helper = sys.argv[1]
launches = int(sys.argv[2])
t = time.perf_counter_ns()
checksum = 0
for i in range(launches):
    value = int(subprocess.check_output([helper, str(i)], text=True).strip())
    checksum = (checksum + value) % 1000000007
ns = time.perf_counter_ns() - t
print(f"launches={launches} checksum={checksum}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
