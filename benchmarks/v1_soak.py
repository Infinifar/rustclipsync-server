#!/usr/bin/env python3
"""Timed synthetic workload using only a temporary encrypted database.

Usage: python3 benchmarks/v1_soak.py target/release/rustclipsync-server 300
The optional second argument is the duration in seconds (default: 300).
"""
import base64
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
import urllib.request
import uuid


def main():
    if len(sys.argv) not in (2, 3):
        raise SystemExit(__doc__)
    executable = str(Path(sys.argv[1]).resolve())
    duration = int(sys.argv[2]) if len(sys.argv) == 3 else 300
    if not 1 <= duration <= 86400:
        raise SystemExit("duration must be 1..86400 seconds")
    with tempfile.TemporaryDirectory(prefix="rcs-v1-soak-") as directory:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        env = os.environ.copy()
        env.update({"RCS_USER": "benchmark", "RCS_PASSWORD": "benchmark-password-only",
                    "RCS_DB_KEY": "c1" * 32, "RCS_DATA_DIR": directory,
                    "RCS_LISTEN": f"127.0.0.1:{port}"})
        process = subprocess.Popen([executable], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        base = f"http://127.0.0.1:{port}"
        basic = "Basic " + base64.b64encode(b"benchmark:benchmark-password-only").decode()
        device, token = str(uuid.uuid4()), "2d" * 32
        bearer = f"Bearer {device}:{token}"
        stop = threading.Event()
        samples, reads, errors, writes = [], [], [], []

        def request(method, path, auth, body=None, headers=None):
            req = urllib.request.Request(base + path, data=body, method=method,
                                         headers={"Authorization": auth, **(headers or {})})
            with urllib.request.urlopen(req, timeout=5) as response:
                return response.read()

        def reader():
            while not stop.is_set():
                start = time.perf_counter()
                try:
                    request("GET", "/v1/state", bearer)
                    reads.append((time.perf_counter() - start) * 1000)
                except Exception as error:
                    errors.append(type(error).__name__)
                stop.wait(0.5)

        def sample():
            while not stop.is_set():
                try:
                    status = Path(f"/proc/{process.pid}/status").read_text()
                    rss = next(int(line.split()[1]) for line in status.splitlines() if line.startswith("VmRSS:"))
                    stat = Path(f"/proc/{process.pid}/stat").read_text().split()
                    cpu = int(stat[13]) + int(stat[14])
                    samples.append((time.monotonic(), rss, cpu))
                except (OSError, StopIteration, ValueError, IndexError):
                    break
                stop.wait(5)

        try:
            deadline = time.monotonic() + 20
            while True:
                try:
                    if urllib.request.urlopen(base + "/healthz", timeout=1).status == 200:
                        break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.1)
            payload = json.dumps({"deviceId": device, "deviceToken": token}).encode()
            epoch = json.loads(request("POST", "/v1/devices", basic, payload,
                                       {"Content-Type": "application/json"}))["epoch"]
            threads = [threading.Thread(target=reader, daemon=True) for _ in range(8)]
            threads.append(threading.Thread(target=sample, daemon=True))
            for thread in threads:
                thread.start()
            start = time.monotonic()
            revision = 0
            while time.monotonic() - start < duration:
                tick = time.perf_counter()
                try:
                    response = request("PUT", f"/v1/messages/{uuid.uuid4()}", bearer, b"s" * 1024,
                                       {"X-RCS-Epoch": epoch, "X-RCS-Base-Revision": str(revision), "X-RCS-Sensitive": "0"})
                    revision += 1
                    if json.loads(response)["revision"] != revision:
                        raise ValueError("unexpected revision")
                    writes.append((time.perf_counter() - tick) * 1000)
                except Exception as error:
                    errors.append(type(error).__name__)
                    break
                stop.wait(max(0, 0.1 - (time.perf_counter() - tick)))
            elapsed = time.monotonic() - start
            stop.set()
            for thread in threads:
                thread.join(timeout=6)
            if len(samples) >= 2:
                cpu_seconds = (samples[-1][2] - samples[0][2]) / os.sysconf("SC_CLK_TCK")
                cpu_percent = 100 * cpu_seconds / (samples[-1][0] - samples[0][0])
            else:
                cpu_percent = None
            ordered = sorted(writes)
            p95 = ordered[round((len(ordered) - 1) * .95)] if ordered else None
            print(json.dumps({"elapsed_seconds": round(elapsed, 1), "writes": len(writes), "reads": len(reads),
                              "errors": errors[:10], "write_p50_ms": round(statistics.median(writes), 2) if writes else None,
                              "write_p95_ms": round(p95, 2) if p95 is not None else None,
                              "rss_first_mib": round(samples[0][1] / 1024, 2) if samples else None,
                              "rss_last_mib": round(samples[-1][1] / 1024, 2) if samples else None,
                              "rss_peak_mib": round(max(s[1] for s in samples) / 1024, 2) if samples else None,
                              "cpu_one_core_percent": round(cpu_percent, 2) if cpu_percent is not None else None}, indent=2))
            if errors:
                raise SystemExit(1)
        finally:
            stop.set()
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    main()
