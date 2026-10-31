#!/usr/bin/env python3
"""Synthetic loopback benchmark against a disposable encrypted database.

Usage: python3 benchmarks/v1_benchmark.py target/release/rustclipsync-server
It never reads the system clipboard or the production database.
"""
import base64
import concurrent.futures
import json
import os
from pathlib import Path
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid


def pct(values, percentile):
    ordered = sorted(values)
    return ordered[round((len(ordered) - 1) * percentile)]


def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    executable = str(Path(sys.argv[1]).resolve())
    with tempfile.TemporaryDirectory(prefix="rcs-v1-bench-") as directory:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        env = os.environ.copy()
        env.update({"RCS_USER": "benchmark", "RCS_PASSWORD": "benchmark-password-only",
                    "RCS_DB_KEY": "a4" * 32, "RCS_DATA_DIR": directory,
                    "RCS_LISTEN": f"127.0.0.1:{port}"})
        process = subprocess.Popen([executable], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        peak = [0]
        sampling = threading.Event()

        def memory():
            while not sampling.is_set():
                try:
                    for line in Path(f"/proc/{process.pid}/status").read_text().splitlines():
                        if line.startswith("VmRSS:"):
                            peak[0] = max(peak[0], int(line.split()[1]))
                            break
                except (FileNotFoundError, ProcessLookupError):
                    break
                sampling.wait(0.005)

        sampler = threading.Thread(target=memory, daemon=True)
        sampler.start()
        base = f"http://127.0.0.1:{port}"
        basic = "Basic " + base64.b64encode(b"benchmark:benchmark-password-only").decode()
        device, token = str(uuid.uuid4()), "c5" * 32
        bearer = f"Bearer {device}:{token}"

        def request(method, path, auth, body=None, headers=None):
            req = urllib.request.Request(base + path, data=body, method=method,
                                         headers={"Authorization": auth, **(headers or {})})
            with urllib.request.urlopen(req, timeout=30) as response:
                return response.status, response.read()

        try:
            deadline = time.monotonic() + 20
            while True:
                try:
                    if urllib.request.urlopen(base + "/healthz", timeout=1).status == 200:
                        break
                except (OSError, urllib.error.URLError):
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.1)
            time.sleep(0.1)
            baseline_kib = peak[0]
            payload = json.dumps({"deviceId": device, "deviceToken": token}).encode()
            epoch = json.loads(request("POST", "/v1/devices", basic, payload,
                                       {"Content-Type": "application/json"})[1])["epoch"]

            def read_state(_):
                started = time.perf_counter()
                status, _ = request("GET", "/v1/state", bearer)
                assert status == 200
                return (time.perf_counter() - started) * 1000

            started = time.perf_counter()
            with concurrent.futures.ThreadPoolExecutor(max_workers=16) as executor:
                reads = list(executor.map(read_state, range(1600)))
            read_seconds = time.perf_counter() - started

            writes = []
            started = time.perf_counter()
            for revision in range(200):
                message = str(uuid.uuid4())
                tick = time.perf_counter()
                result = request("PUT", f"/v1/messages/{message}", bearer, b"x" * 1024,
                                 {"X-RCS-Epoch": epoch, "X-RCS-Base-Revision": str(revision), "X-RCS-Sensitive": "0"})
                assert json.loads(result[1])["revision"] == revision + 1
                writes.append((time.perf_counter() - tick) * 1000)
            write_seconds = time.perf_counter() - started
            before_large = peak[0]
            tick = time.perf_counter()
            result = request("PUT", f"/v1/messages/{uuid.uuid4()}", bearer, b"z" * (10 * 1024 * 1024),
                             {"X-RCS-Epoch": epoch, "X-RCS-Base-Revision": "200", "X-RCS-Sensitive": "0"})
            assert json.loads(result[1])["revision"] == 201
            large_ms = (time.perf_counter() - tick) * 1000
            time.sleep(0.1)
            print(json.dumps({
                "read_requests": len(reads), "read_concurrency": 16,
                "read_rps": round(len(reads) / read_seconds, 1),
                "read_latency_ms": {"p50": round(statistics.median(reads), 2), "p95": round(pct(reads, .95), 2), "p99": round(pct(reads, .99), 2)},
                "write_requests": len(writes), "write_size_bytes": 1024,
                "write_rps": round(len(writes) / write_seconds, 1),
                "write_latency_ms": {"p50": round(statistics.median(writes), 2), "p95": round(pct(writes, .95), 2), "p99": round(pct(writes, .99), 2)},
                "large_bytes": 10 * 1024 * 1024, "large_ms": round(large_ms, 2),
                "idle_rss_mib": round(baseline_kib / 1024, 2),
                "peak_before_large_mib": round(before_large / 1024, 2),
                "peak_rss_mib": round(peak[0] / 1024, 2),
            }, ensure_ascii=False, indent=2))
        finally:
            sampling.set()
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            sampler.join(timeout=1)


if __name__ == "__main__":
    main()
