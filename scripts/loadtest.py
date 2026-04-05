#!/usr/bin/env python3
"""
ModelRelay load testing tool.

Sends concurrent chat/completions requests to a modelrelay-server instance
and reports latency percentiles, throughput, and error rates.

Requires Python 3.8+. Uses only the standard library (no external deps).
For higher concurrency, install aiohttp: pip install aiohttp

Usage:
    python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 100 -c 10
    python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 50 --streaming
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from http.client import HTTPConnection, HTTPSConnection
from urllib.parse import urlparse


def build_payload(model: str, streaming: bool, prompt: str) -> bytes:
    """Build an OpenAI-compatible chat/completions request body."""
    body = {
        "model": model,
        "stream": streaming,
        "messages": [{"role": "user", "content": prompt}],
    }
    return json.dumps(body).encode()


def send_request(
    parsed_url,
    path: str,
    payload: bytes,
    headers: dict,
    streaming: bool,
    timeout: float,
) -> dict:
    """Send a single HTTP request and return timing + status info."""
    ConnClass = HTTPSConnection if parsed_url.scheme == "https" else HTTPConnection
    host = parsed_url.hostname
    port = parsed_url.port or (443 if parsed_url.scheme == "https" else 80)

    result = {
        "status": 0,
        "latency_ms": 0.0,
        "ttfb_ms": 0.0,
        "bytes_received": 0,
        "streaming": streaming,
        "error": None,
        "chunks": 0,
    }

    start = time.monotonic()
    try:
        conn = ConnClass(host, port, timeout=timeout)
        conn.request("POST", path, body=payload, headers=headers)
        resp = conn.getresponse()
        result["status"] = resp.status
        result["ttfb_ms"] = (time.monotonic() - start) * 1000

        if streaming and resp.status == 200:
            chunks = 0
            while True:
                line = resp.readline()
                if not line:
                    break
                result["bytes_received"] += len(line)
                stripped = line.decode("utf-8", errors="replace").strip()
                if stripped.startswith("data: ") and stripped != "data: [DONE]":
                    chunks += 1
            result["chunks"] = chunks
        else:
            body = resp.read()
            result["bytes_received"] = len(body)

        conn.close()
    except Exception as e:
        result["error"] = str(e)

    result["latency_ms"] = (time.monotonic() - start) * 1000
    return result


def percentile(data: list[float], p: float) -> float:
    """Calculate the p-th percentile of a sorted list."""
    if not data:
        return 0.0
    k = (len(data) - 1) * (p / 100)
    f = int(k)
    c = f + 1
    if c >= len(data):
        return data[f]
    return data[f] + (k - f) * (data[c] - data[f])


def run_load_test(args: argparse.Namespace) -> int:
    """Execute the load test and print results."""
    parsed = urlparse(args.url)
    base = parsed.path.rstrip("/") if parsed.path else ""
    path = f"{base}/v1/chat/completions"

    payload = build_payload(args.model, args.streaming, args.prompt)
    headers = {
        "Content-Type": "application/json",
        "Content-Length": str(len(payload)),
    }
    if args.api_key:
        headers["Authorization"] = f"Bearer {args.api_key}"

    print(f"ModelRelay Load Test")
    print(f"{'=' * 50}")
    print(f"Target:      {args.url}{path}")
    print(f"Model:       {args.model}")
    print(f"Requests:    {args.num_requests}")
    print(f"Concurrency: {args.concurrency}")
    print(f"Streaming:   {args.streaming}")
    print(f"Timeout:     {args.timeout}s")
    print(f"{'=' * 50}")
    print()

    results: list[dict] = []
    completed = 0
    lock = threading.Lock()

    def progress_callback(r):
        nonlocal completed
        with lock:
            completed += 1
            if completed % max(1, args.num_requests // 10) == 0 or completed == args.num_requests:
                pct = completed * 100 // args.num_requests
                print(f"  Progress: {completed}/{args.num_requests} ({pct}%)")

    overall_start = time.monotonic()

    with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        futures = []
        for _ in range(args.num_requests):
            f = pool.submit(
                send_request,
                parsed,
                path,
                payload,
                headers,
                args.streaming,
                args.timeout,
            )
            futures.append(f)

        for f in as_completed(futures):
            r = f.result()
            results.append(r)
            progress_callback(r)

    overall_elapsed = time.monotonic() - overall_start

    # Analyze results
    successful = [r for r in results if r["status"] == 200 and r["error"] is None]
    failed = [r for r in results if r["status"] != 200 or r["error"] is not None]

    print()
    print(f"Results")
    print(f"{'=' * 50}")
    print(f"Total requests:  {len(results)}")
    print(f"Successful:      {len(successful)}")
    print(f"Failed:          {len(failed)}")
    print(f"Total time:      {overall_elapsed:.2f}s")

    if results:
        rps = len(results) / overall_elapsed if overall_elapsed > 0 else 0
        print(f"Requests/sec:    {rps:.2f}")

    if successful:
        latencies = sorted(r["latency_ms"] for r in successful)
        ttfbs = sorted(r["ttfb_ms"] for r in successful)
        total_bytes = sum(r["bytes_received"] for r in successful)

        print()
        print(f"Latency (end-to-end)")
        print(f"  Min:    {latencies[0]:.1f} ms")
        print(f"  Mean:   {statistics.mean(latencies):.1f} ms")
        print(f"  Median: {percentile(latencies, 50):.1f} ms")
        print(f"  p90:    {percentile(latencies, 90):.1f} ms")
        print(f"  p95:    {percentile(latencies, 95):.1f} ms")
        print(f"  p99:    {percentile(latencies, 99):.1f} ms")
        print(f"  Max:    {latencies[-1]:.1f} ms")

        print()
        print(f"Time to First Byte (TTFB)")
        print(f"  Min:    {ttfbs[0]:.1f} ms")
        print(f"  Mean:   {statistics.mean(ttfbs):.1f} ms")
        print(f"  Median: {percentile(ttfbs, 50):.1f} ms")
        print(f"  p95:    {percentile(ttfbs, 95):.1f} ms")
        print(f"  Max:    {ttfbs[-1]:.1f} ms")

        print()
        print(f"Data transfer:   {total_bytes:,} bytes")

        if args.streaming:
            chunks = [r["chunks"] for r in successful if r["chunks"] > 0]
            if chunks:
                print(f"Avg chunks/req:  {statistics.mean(chunks):.1f}")

    if failed:
        print()
        print(f"Errors:")
        error_counts: dict[str, int] = {}
        for r in failed:
            key = r["error"] or f"HTTP {r['status']}"
            error_counts[key] = error_counts.get(key, 0) + 1
        for err, count in sorted(error_counts.items(), key=lambda x: -x[1]):
            print(f"  {err}: {count}")

    # JSON output
    if args.json:
        summary = {
            "target": f"{args.url}{path}",
            "model": args.model,
            "num_requests": len(results),
            "concurrency": args.concurrency,
            "streaming": args.streaming,
            "successful": len(successful),
            "failed": len(failed),
            "total_time_s": round(overall_elapsed, 3),
            "requests_per_sec": round(len(results) / overall_elapsed, 2) if overall_elapsed > 0 else 0,
        }
        if successful:
            latencies = sorted(r["latency_ms"] for r in successful)
            summary["latency_ms"] = {
                "min": round(latencies[0], 1),
                "mean": round(statistics.mean(latencies), 1),
                "p50": round(percentile(latencies, 50), 1),
                "p90": round(percentile(latencies, 90), 1),
                "p95": round(percentile(latencies, 95), 1),
                "p99": round(percentile(latencies, 99), 1),
                "max": round(latencies[-1], 1),
            }
        print()
        print(f"JSON summary:")
        print(json.dumps(summary, indent=2))

    return 0 if not failed else 1


def main():
    parser = argparse.ArgumentParser(
        description="ModelRelay load testing tool — sends concurrent chat/completions requests and reports performance metrics.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""Examples:
  # Basic test: 100 requests, 10 concurrent
  python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 100 -c 10

  # Streaming test with API key
  python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 50 -c 5 --streaming --api-key sk-test

  # Quick smoke test with JSON output
  python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 10 -c 2 --json

  # Custom prompt and timeout
  python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 100 --prompt "Explain quantum computing" --timeout 120
""",
    )
    parser.add_argument(
        "--url",
        required=True,
        help="Base URL of the modelrelay-server (e.g. http://localhost:8080)",
    )
    parser.add_argument(
        "--model",
        required=True,
        help="Model name to use in requests",
    )
    parser.add_argument(
        "-n", "--num-requests",
        type=int,
        default=100,
        help="Total number of requests to send (default: 100)",
    )
    parser.add_argument(
        "-c", "--concurrency",
        type=int,
        default=10,
        help="Number of concurrent requests (default: 10)",
    )
    parser.add_argument(
        "--streaming",
        action="store_true",
        help="Use streaming mode (SSE) for requests",
    )
    parser.add_argument(
        "--api-key",
        default=None,
        help="API key for authenticated requests (sent as Bearer token)",
    )
    parser.add_argument(
        "--prompt",
        default="Say hello in exactly three words.",
        help="Prompt text to send (default: 'Say hello in exactly three words.')",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=60.0,
        help="Per-request timeout in seconds (default: 60)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Also print a machine-readable JSON summary at the end",
    )

    args = parser.parse_args()

    if args.num_requests < 1:
        parser.error("--num-requests must be >= 1")
    if args.concurrency < 1:
        parser.error("--concurrency must be >= 1")

    sys.exit(run_load_test(args))


if __name__ == "__main__":
    main()
