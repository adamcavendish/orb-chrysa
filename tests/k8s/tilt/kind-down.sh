#!/usr/bin/env bash
set -euo pipefail

CLUSTER="${KIND_CLUSTER_NAME:-orb-chrysa-tilt}"

if command -v tilt >/dev/null 2>&1; then
    tilt down --context "kind-$CLUSTER" || true
fi

if command -v kind >/dev/null 2>&1 && kind get clusters | grep -qx "$CLUSTER"; then
    kind delete cluster --name "$CLUSTER"
fi

echo "deleted kind cluster $CLUSTER"
