#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
LINE_CNT=${LINE_CNT:-3000}

# ---- pre-check ----
REQUIRED_WPARSE="0.25"; REQUIRED_WFUSION="0.1"
WF_BUILD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/target/release"
resolve_binary() { local n="$1"; [ -x "$WF_BUILD_DIR/$n" ] && export PATH="$WF_BUILD_DIR:$PATH" && return 0; command -v "$n" 2>/dev/null && return 0; return 1; }
if ! resolve_binary wfusion || ! resolve_binary wparse; then echo "ERROR: wfusion/wparse not found" >&2; exit 1; fi
WFUSION_VER=$(wfusion --version 2>&1 | grep -o '[0-9.]*' | head -1)
WPARSE_VER=$(wparse --version 2>&1 | grep -o '[0-9.]*' | head -1)
if ! printf '%s\n%s' "$REQUIRED_WFUSION" "$WFUSION_VER" | sort -V -C 2>/dev/null; then echo "ERROR: wfusion >= $REQUIRED_WFUSION required, got $WFUSION_VER" >&2; exit 1; fi
if ! printf '%s\n%s' "$REQUIRED_WPARSE" "$WPARSE_VER" | sort -V -C 2>/dev/null; then echo "ERROR: wparse >= $REQUIRED_WPARSE required, got $WPARSE_VER" >&2; exit 1; fi
# -------------------

cleanup() {
    [ -n "${WPARSE_PID:-}" ] && kill "$WPARSE_PID" 2>/dev/null || true
    [ -n "${WFUSION_PID:-}" ] && kill "$WFUSION_PID" 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT

echo "============================================"
echo "  streaming: wpgen → TCP → wparse → Arrow TCP → wfusion"
echo "  wfusion=$WFUSION_VER  wparse=$WPARSE_VER"
echo "============================================"

# 1. Start wfusion (daemon mode)
echo "1> wfusion: daemon, listening on TCP :9802..."
cd wfusion
rm -rf ../data/alerts; mkdir -p ../data/alerts
wfusion run --config conf/wfusion.toml &
WFUSION_PID=$!
cd ..
sleep 5
echo "   wfusion PID=$WFUSION_PID"

# 2. Start wparse (daemon mode)
echo "2> wparse: daemon, tcp_src :9801 → tcp_sink → wfusion :9802..."
cd wparse
rm -rf .run; mkdir -p ../data/logs ../data/rescue
wparse daemon -p &
WPARSE_PID=$!
cd ..
sleep 2
echo "   wparse PID=$WPARSE_PID"

# 3. wpgen sends data over TCP, then closes connection
echo "3> wpgen: sending $LINE_CNT nginx logs over TCP :9801..."
(cd wparse && wpgen sample -n "$LINE_CNT" > /dev/null 2>&1)
echo "   → wpgen done, TCP connection closed"

# 4. Wait for wparse to finish processing
echo "4> waiting for wparse to process..."
sleep 3

# 5. Stop wparse (graceful)
echo "5> stopping wparse..."
kill "$WPARSE_PID" 2>/dev/null || true
wait "$WPARSE_PID" 2>/dev/null || true
echo "   → wparse stopped"

# 6. Stop wfusion (graceful — flush windows → alerts)
echo "6> stopping wfusion..."
kill "$WFUSION_PID" 2>/dev/null || true
wait "$WFUSION_PID" 2>/dev/null || true
echo "   → wfusion stopped"

# 7. Show alerts
echo ""; echo "wfusion alerts:"
ls -la data/alerts/*.ndjson 2>/dev/null | awk '{printf "  %s  %s bytes\n", $NF, $5}'
echo ""
