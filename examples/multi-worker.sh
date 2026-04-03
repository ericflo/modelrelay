#!/usr/bin/env bash
# Example: one proxy-server with multiple workers serving different models
#
# Each worker connects to its own local backend (llama-server or similar).
# Workers can run on different machines — just point --proxy-url at the
# shared proxy host. This example runs everything on localhost for demonstration.
#
# Layout used here:
#   Worker A: llama3.2:3b   served by a backend on port 8001
#   Worker B: llama3.2:1b   served by a backend on port 8002
#   Worker C: llama3.2:3b   second replica for the same model (load balancing)
#
# The proxy queues requests and distributes them across workers that advertise
# the requested model. No client-side changes are needed when workers come or go.

set -euo pipefail

PROXY_LISTEN="${PROXY_LISTEN:-0.0.0.0:8080}"
WORKER_SECRET="${WORKER_SECRET:-changeme}"
PROXY_PORT="${PROXY_LISTEN##*:}"

# Build if needed
if [[ ! -f target/release/proxy-server || ! -f target/release/worker-daemon ]]; then
    echo "Building release binaries..."
    cargo build --release
fi

# Start the proxy server
echo "Starting proxy-server on $PROXY_LISTEN ..."
./target/release/proxy-server \
    --listen "$PROXY_LISTEN" \
    --worker-secret "$WORKER_SECRET" \
    --max-queue-len 50 \
    --queue-timeout 30 &
PROXY_PID=$!

sleep 1

# Worker A — llama3.2:3b on port 8001
echo "Starting worker-a (llama3.2:3b, backend :8001) ..."
./target/release/worker-daemon \
    --proxy-url "http://127.0.0.1:$PROXY_PORT" \
    --worker-secret "$WORKER_SECRET" \
    --backend-url "http://127.0.0.1:8001" \
    --models "llama3.2:3b" \
    --worker-name "worker-a" \
    --max-concurrency 2 &
WORKER_A_PID=$!

# Worker B — llama3.2:1b on port 8002
echo "Starting worker-b (llama3.2:1b, backend :8002) ..."
./target/release/worker-daemon \
    --proxy-url "http://127.0.0.1:$PROXY_PORT" \
    --worker-secret "$WORKER_SECRET" \
    --backend-url "http://127.0.0.1:8002" \
    --models "llama3.2:1b" \
    --worker-name "worker-b" \
    --max-concurrency 4 &
WORKER_B_PID=$!

# Worker C — second replica for llama3.2:3b (same model, different backend port)
echo "Starting worker-c (llama3.2:3b replica, backend :8003) ..."
./target/release/worker-daemon \
    --proxy-url "http://127.0.0.1:$PROXY_PORT" \
    --worker-secret "$WORKER_SECRET" \
    --backend-url "http://127.0.0.1:8003" \
    --models "llama3.2:3b" \
    --worker-name "worker-c" \
    --max-concurrency 2 &
WORKER_C_PID=$!

echo ""
echo "Proxy: pid $PROXY_PID"
echo "Workers: $WORKER_A_PID (worker-a), $WORKER_B_PID (worker-b), $WORKER_C_PID (worker-c)"
echo ""
echo "Requests for llama3.2:3b are distributed between worker-a and worker-c."
echo "Requests for llama3.2:1b go to worker-b."
echo ""
echo "Test:"
echo "  curl http://127.0.0.1:$PROXY_PORT/v1/chat/completions \\"
echo "    -H 'Content-Type: application/json' \\"
echo "    -d '{\"model\": \"llama3.2:3b\", \"messages\": [{\"role\": \"user\", \"content\": \"Hi\"}]}'"
echo ""
echo "Press Ctrl-C to stop all processes."

trap 'kill $PROXY_PID $WORKER_A_PID $WORKER_B_PID $WORKER_C_PID 2>/dev/null; exit' INT TERM
wait
