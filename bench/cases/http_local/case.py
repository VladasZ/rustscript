import http.client
import json
import sys
import time
from urllib.parse import urlsplit


base = urlsplit(sys.argv[1])
requests = int(sys.argv[2])
t = time.perf_counter_ns()
connection = http.client.HTTPConnection(base.hostname, base.port)
ids = 0
values = 0
for i in range(requests):
    connection.request("GET", f"/item/{i}")
    response = connection.getresponse()
    item = json.loads(response.read())
    ids += item["id"]
    values += item["value"]
connection.close()
ns = time.perf_counter_ns() - t
print(f"requests={requests} ids={ids} values={values}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
