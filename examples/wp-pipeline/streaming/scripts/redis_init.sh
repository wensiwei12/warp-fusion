#!/bin/sh
# Load weak password database into Redis.
#
# Redis data model:
#   - weak_passwords            SET of all hash_values
#   - wp:{hash_value}           HASH  {hash_value, password_masked, category, note}
#
# Prerequisites: Redis running, python3 available on host.
#
# Usage: ./scripts/redis_init.sh [redis-host] [redis-port]

HOST="${1:-localhost}"
PORT="${2:-6379}"
DATA_FILE="$(dirname "$0")/../data/weak_password_list.ndjson"

if [ ! -f "$DATA_FILE" ]; then
    echo "redis_init: data file not found at $DATA_FILE"
    exit 1
fi

echo "redis_init: loading $(wc -l < "$DATA_FILE" | tr -d ' ') weak passwords to ${HOST}:${PORT}..."

# Generate Redis commands from NDJSON, pipe via docker exec to avoid needing local redis-cli
if docker ps --filter "name=redis" --format "{{.Names}}" 2>/dev/null | grep -q redis; then
    python3 -c "
import json, sys
with open('${DATA_FILE}') as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        d = json.loads(line)
        hv = d['hash_value']
        masked = d['password_masked'].replace(\"'\", \"'\\\"'\\\"'\")
        note = d['note'].replace(\"'\", \"'\\\"'\\\"'\")
        cat = d['category'].replace(\"'\", \"'\\\"'\\\"'\")
        print(f\"SADD weak_passwords '{hv}'\")
        print(f\"HSET wp:{hv} hash_value '{hv}' password_masked '{masked}' category '{cat}' note '{note}'\")
" | docker exec -i redis redis-cli --pipe 2>&1
else
    python3 -c "
import json, sys
with open('${DATA_FILE}') as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        d = json.loads(line)
        hv = d['hash_value']
        masked = d['password_masked'].replace(\"'\", \"'\\\"'\\\"'\")
        note = d['note'].replace(\"'\", \"'\\\"'\\\"'\")
        cat = d['category'].replace(\"'\", \"'\\\"'\\\"'\")
        print(f\"SADD weak_passwords '{hv}'\")
        print(f\"HSET wp:{hv} hash_value '{hv}' password_masked '{masked}' category '{cat}' note '{note}'\")
" | redis-cli -h "$HOST" -p "$PORT" --pipe 2>&1
fi

echo "redis_init: done."
