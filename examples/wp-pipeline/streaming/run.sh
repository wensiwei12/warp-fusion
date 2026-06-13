#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
LINE_CNT=${LINE_CNT:-3000}

echo "============================================"
echo "  wp-pipeline streaming: Arrow IPC over TCP"
echo "============================================"

cleanup() { kill $WFUSION_PID 2>/dev/null; wait $WFUSION_PID 2>/dev/null; }
trap cleanup EXIT

echo "1> wpgen: generating $LINE_CNT nginx logs..."
cd wparse && wpgen sample -n "$LINE_CNT" > /dev/null 2>&1 && cd ..

echo "2> wfusion: starting daemon (tcp://127.0.0.1:9801)..."
cd wfusion && rm -rf data/out_dat && mkdir -p data/out_dat/alerts
wfusion run --config conf/wfusion.toml &
WFUSION_PID=$!
sleep 2
cd ..

echo "3> wparse: parsing → Arrow IPC → TCP..."
cd wparse && mkdir -p data/out_dat data/logs
wparse batch -p -n "$LINE_CNT" -S 1 > /dev/null 2>&1
cd ..

sleep 2
echo ""
echo "wfusion alerts:"
ls -la wfusion/data/out_dat/alerts/*.arrow 2>/dev/null | awk '{printf "  %s  %s bytes\n", $NF, $5}'
echo ""
