#!/usr/bin/env bash
# load-test.sh — Simple load test for ModelRelay
#
# Sends concurrent /v1/chat/completions requests to the proxy and
# reports throughput.  Uses `hey` if available, `wrk` as fallback,
# or plain `curl` loops as a last resort.
#
# Usage:
#   ./extras/load-test.sh                           # defaults: 100 requests, 10 concurrent
#   ./extras/load-test.sh -n 500 -c 20              # 500 requests, 20 concurrent
#   ./extras/load-test.sh -u http://proxy:8080      # custom proxy URL
#   ./extras/load-test.sh -m llama3-8b              # custom model name

set -euo pipefail

# --- Defaults ---
URL="${URL:-http://127.0.0.1:8080}"
MODEL="${MODEL:-llama3-8b}"
REQUESTS=100
CONCURRENCY=10

# --- Parse args ---
while [[ $# -gt 0 ]]; do
    case "$1" in
        -n|--requests)    REQUESTS="$2";     shift 2 ;;
        -c|--concurrency) CONCURRENCY="$2";  shift 2 ;;
        -u|--url)         URL="$2";          shift 2 ;;
        -m|--model)       MODEL="$2";        shift 2 ;;
        -h|--help)
            echo "Usage: $0 [-n requests] [-c concurrency] [-u proxy_url] [-m model]"
            echo ""
            echo "Options:"
            echo "  -n, --requests     Total requests to send (default: 100)"
            echo "  -c, --concurrency  Concurrent requests (default: 10)"
            echo "  -u, --url          Proxy server URL (default: http://127.0.0.1:8080)"
            echo "  -m, --model        Model name (default: llama3-8b)"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

ENDPOINT="${URL}/v1/chat/completions"

BODY=$(cat <<EOF
{
  "model": "${MODEL}",
  "messages": [{"role": "user", "content": "Say hello in one word."}],
  "max_tokens": 16,
  "stream": false
}
EOF
)

echo "=== ModelRelay load test ==="
echo "Target:      ${ENDPOINT}"
echo "Model:       ${MODEL}"
echo "Requests:    ${REQUESTS}"
echo "Concurrency: ${CONCURRENCY}"
echo ""

# --- Try hey first ---
if command -v hey &>/dev/null; then
    echo "Using: hey"
    echo ""
    echo "${BODY}" | hey \
        -n "${REQUESTS}" \
        -c "${CONCURRENCY}" \
        -m POST \
        -T "application/json" \
        -D /dev/stdin \
        "${ENDPOINT}"
    exit 0
fi

# --- Try wrk ---
if command -v wrk &>/dev/null; then
    echo "Using: wrk"
    echo ""
    # wrk needs a lua script for POST with body
    TMPSCRIPT=$(mktemp /tmp/wrk-loadtest-XXXXXX.lua)
    trap 'rm -f "${TMPSCRIPT}"' EXIT
    cat > "${TMPSCRIPT}" <<LUAEOF
wrk.method = "POST"
wrk.headers["Content-Type"] = "application/json"
wrk.body = [[${BODY}]]
LUAEOF

    # wrk uses duration instead of request count; estimate ~1s per request
    DURATION=$(( (REQUESTS / CONCURRENCY) + 5 ))
    wrk -t2 -c"${CONCURRENCY}" -d"${DURATION}s" \
        -s "${TMPSCRIPT}" \
        "${ENDPOINT}"
    exit 0
fi

# --- Fallback: curl loop with background jobs ---
echo "Using: curl (parallel loop — install 'hey' or 'wrk' for better stats)"
echo ""

TMPDIR=$(mktemp -d /tmp/loadtest-XXXXXX)
trap 'rm -rf "${TMPDIR}"' EXIT

STARTED=$(date +%s%N 2>/dev/null || date +%s)
COMPLETED=0
FAILED=0

run_request() {
    local idx=$1
    local outfile="${TMPDIR}/req-${idx}"
    local http_code
    http_code=$(curl -s -o "${outfile}.body" -w "%{http_code}" \
        -X POST \
        -H "Content-Type: application/json" \
        -d "${BODY}" \
        --max-time 120 \
        "${ENDPOINT}" 2>/dev/null || echo "000")
    echo "${http_code}" > "${outfile}.status"
}

# Launch requests in batches of $CONCURRENCY
for (( i=1; i<=REQUESTS; i++ )); do
    run_request "$i" &

    # Throttle concurrency
    if (( i % CONCURRENCY == 0 )); then
        wait
    fi
done
wait

ENDED=$(date +%s%N 2>/dev/null || date +%s)

# Count results
for f in "${TMPDIR}"/req-*.status; do
    [ -f "$f" ] || continue
    code=$(cat "$f")
    if [[ "$code" == "200" ]]; then
        COMPLETED=$((COMPLETED + 1))
    else
        FAILED=$((FAILED + 1))
    fi
done

# Calculate duration
if [[ "${STARTED}" =~ ^[0-9]{10,}$ ]] && [[ "${ENDED}" =~ ^[0-9]{10,}$ ]]; then
    # Nanoseconds available
    ELAPSED_MS=$(( (ENDED - STARTED) / 1000000 ))
    ELAPSED_S=$(awk "BEGIN { printf \"%.2f\", ${ELAPSED_MS}/1000 }")
else
    ELAPSED_S=$(( ENDED - STARTED ))
fi

echo ""
echo "=== Results ==="
echo "Total:     ${REQUESTS}"
echo "Succeeded: ${COMPLETED}"
echo "Failed:    ${FAILED}"
echo "Duration:  ${ELAPSED_S}s"

if command -v awk &>/dev/null && [ "${ELAPSED_S}" != "0" ] && [ "${ELAPSED_S}" != "0.00" ]; then
    RPS=$(awk "BEGIN { printf \"%.1f\", ${COMPLETED}/${ELAPSED_S} }")
    echo "Throughput: ~${RPS} req/s"
fi
