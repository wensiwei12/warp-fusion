#!/usr/bin/env bash
# Shared pre-check for wp-pipeline examples — resolves wfusion/wparse binaries
# and checks minimum versions. Source this from any run.sh:
#
#   source "$(dirname "${BASH_SOURCE[0]}")/../lib-check.sh"
#
# Sets: WFUSION_VER, WPARSE_VER

WF_BUILD_BASE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/target"

resolve_binary() {
    local n="$1"
    # Prefer release, fall back to debug (ensures latest build is used)
    for profile in release debug; do
        if [ -x "$WF_BUILD_BASE/$profile/$n" ]; then
            export PATH="$WF_BUILD_BASE/$profile:$PATH"
            return 0
        fi
    done
    command -v "$n" 2>/dev/null && return 0
    return 1
}

if ! resolve_binary wfusion; then
    echo "ERROR: wfusion not found (checked $WF_BUILD_BASE/{release,debug}/wfusion and PATH)" >&2
    exit 1
fi
if ! resolve_binary wparse; then
    echo "ERROR: wparse not found (checked $WF_BUILD_BASE/{release,debug}/wparse and PATH)" >&2
    exit 1
fi
if ! wfusion version --ge 0.1.0 >/dev/null 2>&1; then
    echo "ERROR: wfusion >= 0.1.0 required" >&2
    exit 1
fi
if ! wparse version --ge 0.25.0 >/dev/null 2>&1; then
    echo "ERROR: wparse >= 0.25.0 required" >&2
    exit 1
fi

WFUSION_VER=$(wfusion version 2>&1 | awk '{print $NF}')
WPARSE_VER=$(wparse version 2>&1)
