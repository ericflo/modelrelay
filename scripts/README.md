# Scripts

## loadtest.py — Load Testing Tool

A Python load testing tool for benchmarking modelrelay-server performance. Sends concurrent OpenAI-compatible chat/completions requests and reports latency percentiles, throughput, and error rates.

### Requirements

- Python 3.8+
- No external dependencies (stdlib only)

### Quick Start

```bash
# Start modelrelay-server and at least one worker, then:

# Basic test: 100 requests, 10 concurrent
python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 100 -c 10

# Streaming mode
python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 50 -c 5 --streaming

# With API key authentication
python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 100 --api-key sk-test-key

# JSON output for CI/scripting
python3 scripts/loadtest.py --url http://localhost:8080 --model my-model -n 100 --json
```

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--url` | (required) | Base URL of modelrelay-server |
| `--model` | (required) | Model name for requests |
| `-n, --num-requests` | 100 | Total requests to send |
| `-c, --concurrency` | 10 | Concurrent request count |
| `--streaming` | off | Use SSE streaming mode |
| `--api-key` | none | Bearer token for auth |
| `--prompt` | "Say hello in exactly three words." | Prompt text |
| `--timeout` | 60s | Per-request timeout |
| `--json` | off | Print JSON summary |

### Output

The tool reports:

- **Total / Successful / Failed** request counts
- **Requests/sec** throughput
- **Latency percentiles**: min, mean, median, p90, p95, p99, max
- **TTFB** (time to first byte) percentiles
- **Data transfer** total bytes
- **Streaming chunks** per request (when `--streaming`)
- **Error breakdown** by type

### Example Output

```
ModelRelay Load Test
==================================================
Target:      http://localhost:8080/v1/chat/completions
Model:       llama3
Requests:    100
Concurrency: 10
Streaming:   False
Timeout:     60.0s
==================================================

  Progress: 100/100 (100%)

Results
==================================================
Total requests:  100
Successful:      100
Failed:          0
Total time:      4.23s
Requests/sec:    23.64

Latency (end-to-end)
  Min:    28.3 ms
  Mean:   412.5 ms
  Median: 389.2 ms
  p90:    623.1 ms
  p95:    712.4 ms
  p99:    891.0 ms
  Max:    943.2 ms

Time to First Byte (TTFB)
  Min:    12.1 ms
  Mean:   45.3 ms
  Median: 38.7 ms
  p95:    89.2 ms
  Max:    124.5 ms

Data transfer:   45,200 bytes
```
