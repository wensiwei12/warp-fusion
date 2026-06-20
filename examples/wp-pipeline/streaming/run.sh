#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
LINE_CNT=${LINE_CNT:-3000}

cleanup() {
    if [ -n "${WFUSION_PID:-}" ] && kill -0 "$WFUSION_PID" 2>/dev/null; then kill "$WFUSION_PID" 2>/dev/null; wait "$WFUSION_PID" 2>/dev/null || true; fi
    echo ""; echo "stopped."
}
trap cleanup EXIT

echo "============================================"
echo "  streaming: wpgen → wparse → Arrow TCP → wfusion"
echo "============================================"

# 1. Generate data (wpgen outputs to ../data/gen.dat via wpgen.toml)
echo "1> wpgen: generating $LINE_CNT nginx logs..."
(cd wparse && rm -rf .run && wpgen sample -n "$LINE_CNT" > /dev/null 2>&1)
echo "   → data/gen.dat"

# 2. Start wfusion (listens on TCP :9802, sinks write to ../../data/alerts)
echo "2> wfusion: starting daemon (tcp://0.0.0.0:9802)..."
(cd wfusion && rm -rf ../data/alerts && mkdir -p ../data/alerts && wfusion run --config conf/wfusion.toml &)
WFUSION_PID=$!; sleep 5
echo "   wfusion PID=$WFUSION_PID"

# 3. Run wparse (reads file via file_src ../data/gen*.dat, sends Arrow via TCP)
echo "3> wparse: parsing → Arrow IPC → TCP :9802..."
(cd wparse && wparse batch -p -n "$LINE_CNT" -S 1 > /dev/null 2>&1)
echo "   → Arrow IPC → TCP :9802 → wfusion"

# 4. Flush wfusion
echo "4> flushing wfusion windows..."
kill "$WFUSION_PID" 2>/dev/null || true; wait "$WFUSION_PID" 2>/dev/null || true; sleep 1

# 5. Show alerts
echo ""; echo "wfusion alerts:"
ls -la data/alerts/*.ndjson 2>/dev/null | awk '{printf "  %s  %s bytes\n", $NF, $5}'
echo ""
