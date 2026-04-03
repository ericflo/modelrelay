#!/usr/bin/env bash
# Example: proxy-server + worker-daemon pointing at a local llama-server
#
# Assumes llama-server (from llama.cpp) is already running on port 8000:
#   llama-server -m /path/to/model.gguf --host 127.0.0.1 --port 8000
#
# Adjust MODELS, PROXY_LISTEN, and WORKER_SECRET to match your setup.

set -euo pipefail

PROXY_LISTEN="${PROXY_LISTEN:-0.0.0.0:8080}"
WORKER_SECRET="${WORKER_SECRET:-changeme}"
BACKEND_URL="${BACKEND_URL:-http://127.0.0.1:8000}"
MODELS="${MODELS:-llama3.2:3b}"
WORKER_NAME="${WORKER_NAME:-local-gpu}"

# Build if needed
if [[ ! -f target/release/proxy-server || ! -f target/release/worker-daemon ]]; then
    echo "Building release binaries..."
    cargo build --release
fi

# Start the proxy server in the background
echo "Starting proxy-server on $PROXY_LISTEN ..."
./target/release/proxy-server \
    --listen "$PROXY_LISTEN" \
    --worker-secret "$WORKER_SECRET" &
PROXY_PID=$!

# Give the proxy a moment to bind
sleep 1

# Start a single worker pointing at the local llama-server
echo "Starting worker-daemon for models: $MODELS ..."
./target/release/worker-daemon \
    --proxy-url "http://127.0.0.1:${PROXY_LISTEN##*:}" \
    --worker-secret "$WORKER_SECRET" \
    --backend-url "$BACKEND_URL" \
    --models "$MODELS" \
    --worker-name "$WORKER_NAME" &
WORKER_PID=$!

echo ""
echo "Proxy running (pid $PROXY_PID), worker running (pid $WORKER_PID)"
echo ""
echo "Test with:"
echo "  curl http://127.0.0.1:${PROXY_LISTEN##*:}/v1/chat/completions \\"
echo "    -H 'Content-Type: application/json' \\"
echo "    -d '{\"model\": \"$MODELS\", \"messages\": [{\"role\": \"user\", \"content\": \"Hello!\"}], \"stream\": false}'"
echo ""
echo "Press Ctrl-C to stop."

# Clean up both processes on exit
trap 'kill $PROXY_PID $WORKER_PID 2>/dev/null; exit' INT TERM
wait
