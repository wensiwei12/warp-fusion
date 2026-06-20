#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
LINE_CNT=${LINE_CNT:-5000}
PORT=${PORT:-9800}

cleanup() {
    if [ -n "${WPARSE_PID:-}" ] && kill -0 "$WPARSE_PID" 2>/dev/null; then kill "$WPARSE_PID" 2>/dev/null; wait "$WPARSE_PID" 2>/dev/null || true; fi
    if [ -n "${WFUSION_PID:-}" ] && kill -0 "$WFUSION_PID" 2>/dev/null; then kill "$WFUSION_PID" 2>/dev/null; wait "$WFUSION_PID" 2>/dev/null || true; fi
    echo ""
    echo "stopped."
}
trap cleanup EXIT

echo "============================================"
echo "  Kafka Pipeline: wpgen → wparse → Kafka → wfusion → alerts"
echo "============================================"
echo ""

# 1. Start Kafka
echo "1> Starting Kafka..."
docker-compose up -d
sleep 5
echo "   Kafka ready at localhost:9092"
echo ""

# 2. Ensure TCP configs (wpgen → tcp_sink, wparse → tcp_src)
cd wparse && rm -rf .run
cat > conf/wpgen.toml <<EOF
version = "1.0"

[generator]
count = 5000
speed = 5000
parallel = 1

[output]
connect = "tcp_sink"

[output.params]
addr = "127.0.0.1"
port = "$PORT"
framing = "line"

[logging]
level = "info"
output = "file"
file_path = "../data/logs"
EOF

cat > topology/sources/wpsrc.toml <<EOF
[[sources]]
key = "tcp_1"
enable = true
connect = "tcp_src"
tags = []

[sources.params]
addr = "127.0.0.1"
port = "$PORT"
framing = "line"
EOF

cd ..
echo ""

# 3. Use a fresh consumer group to always replay from beginning
GROUP_ID="wfusion_$(date +%s)"
sed -i '' "s/group_id = .*/group_id = \"$GROUP_ID\"/" wfusion/topology/sources/kafka_nginx.toml

echo "3> wfusion: starting daemon, consuming from Kafka..."
cd wfusion
rm -rf ../data/alerts
mkdir -p ../data/alerts
wfusion run --config conf/wfusion.toml &
WFUSION_PID=$!
sleep 3
cd ..
echo "   group_id=$GROUP_ID"
echo "   wfusion PID=$WFUSION_PID"
echo ""

# 4. wparse listens on TCP, wpgen sends, wparse → Kafka (Arrow)
echo "4> wparse: listening on TCP :$PORT, then wpgen sending..."
cd wparse
wparse batch -p -n "$LINE_CNT" -S 1 &
WPARSE_PID=$!
sleep 2
wpgen sample -n "$LINE_CNT" > /dev/null 2>&1
wait "$WPARSE_PID" 2>/dev/null || true
cd ..
echo "   → TCP → wparse → Kafka (wp_nginx_logs)"
echo ""

# 5. Graceful shutdown to flush windows and write alerts
echo "5> flushing wfusion windows..."
kill "$WFUSION_PID" 2>/dev/null || true
wait "$WFUSION_PID" 2>/dev/null || true
sleep 1

# 6. Show local alert files
echo ""
echo "wfusion alerts (local files):"
for f in data/alerts/*.ndjson; do
    name=$(basename "$f")
    size=$(wc -c < "$f" | tr -d ' ')
    if [ "$size" -gt 0 ]; then
        echo "  $name ($size bytes)"
        cat "$f" | python3 -m json.tool 2>/dev/null || cat "$f"
    else
        echo "  $name (empty)"
    fi
done
echo ""
