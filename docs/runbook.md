# Operational Runbook

This guide covers day-to-day operations for running ModelRelay in
production.  It assumes you have one `modelrelay-server` instance and one or
more `modelrelay-worker` processes.

---

## Health Checks

### Proxy Server

The proxy server exposes standard HTTP endpoints.  A simple health
probe:

```bash
# Quick liveness check — any 2xx/4xx means the process is up.
curl -sf http://proxy:8080/v1/models && echo "OK"
```

`/v1/models` returns the list of models currently routable through
connected workers.  An empty `data` array means the proxy is running but
no workers are registered.

### Worker Daemon

The worker daemon does not expose its own HTTP port — it connects
*outward* to the proxy.  Health is observable from the proxy side:

```bash
# Check if workers are registered by listing models.
curl -s http://proxy:8080/v1/models | jq '.data[].id'
```

If expected models are missing, the worker is either down or failed to
register.  Check worker logs for connection errors or authentication
failures.

---

## Checking Worker Registration

After starting a new worker, confirm it registered:

```bash
# Should include the worker's advertised models.
curl -s http://proxy:8080/v1/models | jq .
```

If a worker's models don't appear within ~10 seconds:

1. **Check the worker secret** — does `WORKER_SECRET` on the worker
   match the proxy?
2. **Check connectivity** — can the worker reach `PROXY_URL`?
   ```bash
   curl -v http://proxy:8080/v1/worker/connect
   # Should get 400 or upgrade-required, not a connection error
   ```
3. **Check worker logs** — look for `register` / `register_ack` messages
   or error lines.

---

## Draining a Worker Gracefully

To remove a worker from rotation without dropping in-flight requests:

1. **Send SIGTERM** to the modelrelay-worker process.  The daemon initiates a
   graceful disconnect — the proxy sends a `GracefulShutdown` message and
   stops routing new requests to that worker.

2. **In-flight requests finish normally.**  The proxy waits up to
   `drain_timeout_secs` (from the shutdown message) for active requests
   to complete.

3. **Once idle, the WebSocket closes.**  The worker process exits.

```bash
# Graceful stop via systemd
systemctl stop modelrelay-worker@gpu-box-1

# Or with Docker
docker stop --time 60 worker-gpu-box-1
```

**Monitoring drain progress:** Watch the proxy logs for
`"worker drained"` or similar messages.  If the worker still has
in-flight requests, you'll see ongoing `ResponseChunk` / `ResponseComplete`
messages until they finish.

---

## Scaling Workers

### Adding a worker

Start a new `modelrelay-worker` instance pointing at the same proxy:

```bash
PROXY_URL=http://proxy:8080 \
WORKER_SECRET=your-secret \
WORKER_NAME=gpu-box-4 \
BACKEND_URL=http://localhost:8000 \
  modelrelay-worker --models llama3-8b
```

The proxy discovers it within seconds via the WebSocket registration
handshake.  No proxy restart or config change needed.

### Removing a worker

Use the graceful drain procedure above.  The proxy automatically routes
around disconnected workers.

### Scaling the proxy

The proxy is a single-process server.  To scale:

- **Vertical:** increase `MAX_QUEUE_LEN` and system file descriptor limits.
- **Horizontal:** run multiple proxy instances behind a load balancer,
  but note that each worker connects to one proxy.  Workers must be
  distributed across proxy instances manually or via DNS round-robin.

---

## Log Interpretation

### Proxy Server

| Log pattern | Meaning |
|-------------|---------|
| `worker registered` / `register_ack` | Worker connected and authenticated |
| `request dispatched` | Request sent to a worker |
| `response complete` | Worker returned a result |
| `worker heartbeat timed out` | Worker missed pings — WebSocket closed |
| `request requeued` | Worker died mid-request, retrying on another worker |
| `requeue exhausted` | Request failed after `MAX_REQUEUE_COUNT` (3) retries |
| `queue full` | Rejected request — queue at `MAX_QUEUE_LEN` capacity |
| `queue timeout` | Request sat in queue longer than `QUEUE_TIMEOUT_SECS` |
| `graceful shutdown` | Worker drain initiated |

### Worker Daemon

| Log pattern | Meaning |
|-------------|---------|
| `connected to proxy` | WebSocket connection established |
| `registered` | Registration acknowledged by proxy |
| `forwarding request` | Proxying a request to the local backend |
| `backend error` | Local backend returned an error or is unreachable |
| `cancelled` | Proxy sent a cancel for an in-flight request |
| `graceful shutdown` | Drain in progress, finishing active requests |

### Adjusting log verbosity

Set `LOG_LEVEL` environment variable on either component:

```bash
LOG_LEVEL=debug modelrelay-server   # trace, debug, info (default), warn, error
LOG_LEVEL=debug modelrelay-worker
```

---

## Common Failure Modes

### Worker can't connect to proxy

**Symptoms:** Worker logs show connection refused or timeouts.

**Checklist:**
1. Is the proxy running?  `curl http://proxy:8080/v1/models`
2. Is `PROXY_URL` correct?  The worker connects to
   `{PROXY_URL}/v1/worker/connect` via WebSocket.
3. Firewall / network: the worker makes an *outbound* connection to the
   proxy — no inbound ports needed on the worker machine.
4. If using TLS (nginx/reverse proxy in front), ensure WebSocket upgrade
   headers are forwarded.  See the [TLS Setup guide](tls.md).

### Worker registers but requests fail

**Symptoms:** `/v1/models` shows the model, but requests return 502 or
timeout.

**Checklist:**
1. Is the local backend running?  `curl http://localhost:8000/v1/models`
   (or whatever `BACKEND_URL` is set to)
2. Does the backend support the requested endpoint?
   (`/v1/chat/completions`, `/v1/messages`, `/v1/responses`)
3. Check worker logs for `backend error` messages.
4. Try a direct request to the backend to isolate the issue.

### Requests queue but never complete

**Symptoms:** Clients hang, then get a timeout error after
`QUEUE_TIMEOUT_SECS`.

**Causes:**
- No workers are connected (check `/v1/models`)
- Workers are at capacity (`max_concurrent` reached on all workers)
- Workers are connected but not advertising the requested model

**Fix:** Add more workers, increase `max_concurrent` if the hardware
allows, or reduce `QUEUE_TIMEOUT_SECS` to fail faster.

### Streaming responses arrive corrupted

**Symptoms:** SSE chunks arrive garbled or out of order.

**Checklist:**
1. Ensure no intermediate proxy is buffering.  Disable response
   buffering in nginx:
   ```
   proxy_buffering off;
   ```
2. If using a CDN or reverse proxy, ensure it supports chunked transfer
   encoding and doesn't aggregate small writes.

### High memory usage on the proxy

**Symptoms:** Proxy RSS grows over time.

**Causes:**
- Large queue of pending requests (each holds the full request body)
- Many concurrent streaming responses with large chunk buffers

**Fix:** Lower `MAX_QUEUE_LEN`, set `QUEUE_TIMEOUT_SECS` to a shorter
value, or add workers to drain the queue faster.

### Worker keeps reconnecting

**Symptoms:** Worker logs show repeated connect/disconnect cycles.

**Causes:**
- Heartbeat timeout — the worker or network is too slow to respond to
  pings within `HEARTBEAT_INTERVAL`
- `WORKER_SECRET` mismatch — worker connects, fails auth, gets
  disconnected, retries

**Fix:** Check secrets match, check network latency between worker and
proxy.

---

## Configuration Quick Reference

### Proxy Server

| Env Var | Default | Description |
|---------|---------|-------------|
| `LISTEN_ADDR` | `127.0.0.1:8080` | HTTP listen address |
| `PROVIDER_NAME` | `local` | Provider name for routing |
| `WORKER_SECRET` | *(required)* | Shared secret for worker auth |
| `MAX_QUEUE_LEN` | `100` | Max queued requests before rejecting |
| `QUEUE_TIMEOUT_SECS` | `30` | How long a request can wait in queue |
| `REQUEST_TIMEOUT_SECS` | `300` | Total request timeout (5 min) |
| `LOG_LEVEL` | `info` | Log verbosity |

### Worker Daemon

| Env Var | Default | Description |
|---------|---------|-------------|
| `PROXY_URL` | `http://127.0.0.1:8080` | Proxy server URL |
| `WORKER_SECRET` | *(required)* | Must match proxy's secret |
| `WORKER_NAME` | `worker` | Human-readable worker name |
| `BACKEND_URL` | `http://127.0.0.1:8000` | Local model server URL |
| `LOG_LEVEL` | `info` | Log verbosity |

---

## Windows Service

### Checking Service Status

```powershell
Get-Service ModelRelayServer
Get-Service ModelRelayWorker
```

### Starting and Stopping

```powershell
Start-Service ModelRelayServer
Stop-Service ModelRelayServer

Start-Service ModelRelayWorker
Stop-Service ModelRelayWorker
```

`Stop-Service` sends a stop control signal and waits for the process to
exit.  ModelRelay handles this as a graceful shutdown — in-flight
requests finish before the process terminates.  To set an explicit
timeout:

```powershell
# Stop with a 60-second timeout (kills the process if it doesn't exit in time)
Stop-Service ModelRelayServer -NoWait
Start-Sleep -Seconds 60
(Get-Service ModelRelayServer).WaitForStatus("Stopped", "00:00:05")
```

### Logs

Windows Services don't write to stdout by default.  Two options:

1. **Windows Event Log** — ModelRelay writes to the Application log.
   View with:
   ```powershell
   Get-EventLog -LogName Application -Source ModelRelayServer -Newest 50
   ```

2. **File logging via `RUST_LOG`** — set `RUST_LOG` as a system
   environment variable and redirect output to a file by wrapping the
   binary in a small script, or use the `RUST_LOG_FILE` convention if
   supported.  The simplest approach:
   ```powershell
   [Environment]::SetEnvironmentVariable("RUST_LOG", "info", "Machine")
   ```

### Draining a Worker

To drain a worker gracefully before maintenance:

```powershell
# Stop the service — this triggers graceful shutdown.
Stop-Service ModelRelayWorker

# Verify it has stopped.
Get-Service ModelRelayWorker
```

The worker completes in-flight requests before exiting, identical to the
`systemctl stop` behavior on Linux.

---

## Monitoring Checklist

For production deployments, monitor these signals:

- [ ] **Proxy process is up** — HTTP health check on `/v1/models`
- [ ] **At least one worker registered** — `/v1/models` returns non-empty `data`
- [ ] **Queue depth** — watch for sustained queue growth (log `queue full` errors)
- [ ] **Request latency** — track time from client request to first byte
- [ ] **Worker reconnect rate** — frequent reconnects indicate network or auth issues
- [ ] **Error rates** — 4xx (client errors) vs 5xx (backend/proxy errors)
- [ ] **Backend health** — each worker's local model server should be independently monitored
