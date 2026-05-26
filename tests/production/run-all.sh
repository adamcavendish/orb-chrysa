#!/usr/bin/env bash
# Run the local production-style workflow suite against a live orb-chrysa
# registry. Each child script owns its disposable data and evidence directory.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITE_RUN_ID="${RUN_ID:-$(date +%s)}"

export REGISTRY="${REGISTRY:-localhost:5050}"
export SCHEME="${SCHEME:-http}"
export NODE_PORTS="${NODE_PORTS:-5050 5051 5052}"
export EVIDENCE_ROOT="${EVIDENCE_ROOT:-/tmp}"

log() {
    printf '\n==> %s\n' "$*"
}

run_workflow() {
    local name="$1"
    local script="$2"
    local run_id="$SUITE_RUN_ID-$name"
    log "Run $name workflow (RUN_ID=$run_id)"
    RUN_ID="$run_id" "$script"
}

log "Production workflow suite target: $SCHEME://$REGISTRY"
run_workflow "oci" "$SCRIPT_DIR/oci-workflow.sh"
run_workflow "mirror-proxy" "$SCRIPT_DIR/mirror-proxy-workflow.sh"
log "Production workflow suite passed (SUITE_RUN_ID=$SUITE_RUN_ID)"
