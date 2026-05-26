#!/usr/bin/env bash
set -euo pipefail

RUN_ID="${RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
CLUSTER="${KIND_CLUSTER_NAME:-orb-chrysa-tilt}"
NAMESPACE="${ORB_NAMESPACE:-orb-chrysa-tilt}"
REGISTRY_ENDPOINT="${REGISTRY_ENDPOINT:-localhost:32050}"
KANIDM_HOST_PORT="${KANIDM_HOST_PORT:-8443}"
KANIDM_URL="${KANIDM_URL:-https://localhost:$KANIDM_HOST_PORT}"
EVIDENCE_ROOT="${EVIDENCE_ROOT:-target/tilt/evidence}"
WORK="$EVIDENCE_ROOT/$RUN_ID-scale"
SMOKE_REPO="${SMOKE_REPO:-qa/tilt-scale-$RUN_ID}"
SMOKE_IMAGE="$REGISTRY_ENDPOINT/$SMOKE_REPO:scale"
SMOKE_BASE_IMAGE="${SMOKE_BASE_IMAGE:-busybox:1.36}"
SMOKE_BASE_IMAGE_FALLBACK="${SMOKE_BASE_IMAGE_FALLBACK:-alpine:3.21}"
RESTORE_REPLICAS="${RESTORE_REPLICAS:-3}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# shellcheck source=tests/k8s/lib.sh
source "$SCRIPT_DIR/lib.sh"

umask 077
mkdir -p "$WORK/dockerctx"
chmod -R go-rwx "$WORK"

cluster_status() {
    local_curl --cacert "$CA" --connect-timeout 5 --max-time 10 -fsS \
        -H "Authorization: Bearer $CI_TOKEN" \
        "https://$REGISTRY_ENDPOINT/api/v1/admin/cluster/status"
}

assert_cluster_size() {
    local expected="$1"
    local output="$2"

    for _ in $(seq 1 120); do
        if cluster_status > "$WORK/cluster-status-current.json" \
            && jq -e --argjson expected "$expected" \
                '.leader_id != null
                 and .healthy_voters >= .quorum
                 and (.voters | length) == $expected
                 and .healthy_voters == $expected' \
                "$WORK/cluster-status-current.json" >/dev/null; then
            cp "$WORK/cluster-status-current.json" "$output"
            return 0
        fi
        sleep 2
    done

    echo "ERROR: cluster did not converge to $expected healthy voters" >&2
    cat "$WORK/cluster-status-current.json" >&2 2>/dev/null || true
    return 1
}

wait_ready_replicas() {
    local expected="$1"
    local ready

    for _ in $(seq 1 180); do
        ready="$(kubectl -n "$NAMESPACE" get statefulset orb-chrysa -o jsonpath='{.status.readyReplicas}' 2>/dev/null || true)"
        ready="${ready:-0}"
        if [ "$ready" = "$expected" ]; then
            return 0
        fi
        sleep 2
    done

    echo "ERROR: StatefulSet did not reach $expected ready replicas" >&2
    kubectl -n "$NAMESPACE" get statefulset orb-chrysa -o yaml > "$WORK/statefulset-timeout.yaml" 2>&1 || true
    return 1
}

scale_orb() {
    local replicas="$1"
    local label="$2"

    echo "=== Scale Orb Chrysa to $replicas replicas ($label) ==="
    record kubectl -n "$NAMESPACE" scale statefulset/orb-chrysa --replicas="$replicas"
    record kubectl -n "$NAMESPACE" rollout status statefulset/orb-chrysa --timeout=420s
    wait_ready_replicas "$replicas"
    kubectl -n "$NAMESPACE" get pods -o wide | tee "$WORK/pods-$label.txt"
    assert_cluster_size "$replicas" "$WORK/cluster-status-$label.json"
    verify_node_pull "$SMOKE_IMAGE" "$PAT" "$label"
}

cleanup() {
    status=$?
    if [ "${RESTORE_ON_EXIT:-1}" = "1" ]; then
        kubectl -n "$NAMESPACE" scale statefulset/orb-chrysa --replicas="$RESTORE_REPLICAS" >/dev/null 2>&1 || true
    fi
    if [ "$status" -eq 0 ]; then
        echo "PASS Tilt scale smoke. Evidence: $WORK"
    else
        echo "FAIL Tilt scale smoke. Evidence: $WORK" >&2
    fi
    exit "$status"
}
trap cleanup EXIT

need base64
need curl
need docker
need jq
need kind
need kubectl

{
    echo "RUN_ID=$RUN_ID"
    echo "CLUSTER=$CLUSTER"
    echo "NAMESPACE=$NAMESPACE"
    echo "REGISTRY_ENDPOINT=$REGISTRY_ENDPOINT"
    echo "KANIDM_URL=$KANIDM_URL"
    echo "SMOKE_IMAGE=$SMOKE_IMAGE"
} > "$WORK/summary.env"

record kubectl -n "$NAMESPACE" rollout status statefulset/orb-chrysa --timeout=360s
TRUST_WORK="$WORK/node-trust"
mkdir -p "$TRUST_WORK"
chmod 0700 "$TRUST_WORK"
WORK="$TRUST_WORK" REGISTRY_ENDPOINT="$REGISTRY_ENDPOINT" ORB_NAMESPACE="$NAMESPACE" tests/k8s/tilt/kind-node-trust.sh \
    2>&1 | tee -a "$WORK/commands.log"

CA="$WORK/orb-chrysa-ca.crt"
kubectl -n "$NAMESPACE" get secret orb-chrysa-server-tls -o jsonpath='{.data.ca\.crt}' | base64 -d > "$CA"
CI_TOKEN="$(refresh_ci_bot_token)"
PAT="$(create_pat "$CI_TOKEN" tilt-scale-smoke)"
if [ -z "$PAT" ] || [ "$PAT" = "null" ]; then
    echo "ERROR: PAT response did not include a token" >&2
    exit 1
fi

assert_cluster_size 3 "$WORK/cluster-status-initial.json"

BASE_IMAGE="$(resolve_smoke_base_image)"
printf 'tilt scale smoke %s\n' "$RUN_ID" > "$WORK/dockerctx/hello.txt"
printf 'FROM %s\nCOPY hello.txt /hello.txt\n' "$BASE_IMAGE" > "$WORK/dockerctx/Dockerfile"
record docker build --provenance=false --sbom=false -t "$SMOKE_IMAGE" "$WORK/dockerctx"
kind_containerd_push "$SMOKE_IMAGE" "$PAT"
verify_node_pull "$SMOKE_IMAGE" "$PAT" initial

scale_orb 1 scale-1-first
scale_orb 3 scale-3
scale_orb 2 scale-2
scale_orb 1 scale-1-final
