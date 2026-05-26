#!/usr/bin/env bash
set -euo pipefail

RUN_ID="${RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
CLUSTER="${KIND_CLUSTER_NAME:-orb-chrysa-tilt}"
NAMESPACE="${ORB_NAMESPACE:-orb-chrysa-tilt}"
REGISTRY_ENDPOINT="${REGISTRY_ENDPOINT:-localhost:32050}"
KANIDM_HOST_PORT="${KANIDM_HOST_PORT:-8443}"
KANIDM_URL="${KANIDM_URL:-https://localhost:$KANIDM_HOST_PORT}"
EVIDENCE_ROOT="${EVIDENCE_ROOT:-target/tilt/evidence}"
WORK="$EVIDENCE_ROOT/$RUN_ID-recovery"
SMOKE_REPO="${SMOKE_REPO:-qa/tilt-recovery-$RUN_ID}"
SMOKE_IMAGE="$REGISTRY_ENDPOINT/$SMOKE_REPO:before-restart"
SMOKE_BASE_IMAGE="${SMOKE_BASE_IMAGE:-busybox:1.36}"
SMOKE_BASE_IMAGE_FALLBACK="${SMOKE_BASE_IMAGE_FALLBACK:-alpine:3.21}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# shellcheck source=tests/k8s/lib.sh
source "$SCRIPT_DIR/lib.sh"

umask 077
mkdir -p "$WORK/dockerctx"
chmod -R go-rwx "$WORK"

cluster_status() {
    local_curl --cacert "$CA" -fsS \
        -H "Authorization: Bearer $CI_TOKEN" \
        "https://$REGISTRY_ENDPOINT/api/v1/admin/cluster/status"
}

assert_cluster_healthy() {
    local output="$1"

    for _ in $(seq 1 90); do
        if cluster_status > "$WORK/cluster-status-current.json" \
            && jq -e '.leader_id != null and .healthy_voters >= .quorum' "$WORK/cluster-status-current.json" >/dev/null; then
            cp "$WORK/cluster-status-current.json" "$output"
            return 0
        fi
        sleep 2
    done

    echo "ERROR: cluster did not report healthy quorum" >&2
    cat "$WORK/cluster-status-current.json" >&2 2>/dev/null || true
    return 1
}

collect_orb_logs() {
    local phase="$1"
    local pod

    mkdir -p "$WORK/logs-$phase"
    for pod in orb-chrysa-0 orb-chrysa-1 orb-chrysa-2; do
        kubectl -n "$NAMESPACE" logs "$pod" > "$WORK/logs-$phase/$pod-current.log" 2>&1 || true
        kubectl -n "$NAMESPACE" logs "$pod" --previous > "$WORK/logs-$phase/$pod-previous.log" 2>&1 || true
    done
}

assert_snapshot_restore_evidence() {
    cat "$WORK"/logs-after-rollout-restart/* > "$WORK/rollout-restart-logs.txt"

    if ! grep -Eq 'uploaded final snapshot to S3|uploaded raft snapshot to S3' "$WORK/rollout-restart-logs.txt"; then
        echo "ERROR: rollout restart logs did not show S3 snapshot upload" >&2
        exit 1
    fi

    if ! grep -Eq 'downloaded raft snapshot from S3|restoring state from S3 snapshot|resumed from S3 snapshot' "$WORK/rollout-restart-logs.txt"; then
        echo "ERROR: rollout restart logs did not show S3 snapshot restore" >&2
        exit 1
    fi
}

cleanup() {
    status=$?
    if [ "$status" -eq 0 ]; then
        echo "PASS Tilt recovery smoke. Evidence: $WORK"
    else
        echo "FAIL Tilt recovery smoke. Evidence: $WORK" >&2
    fi
    exit "$status"
}
trap cleanup EXIT

need base64
need curl
need docker
need grep
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

record kubectl -n "$NAMESPACE" rollout status statefulset/orb-chrysa --timeout=240s
WORK="$WORK/node-trust" REGISTRY_ENDPOINT="$REGISTRY_ENDPOINT" ORB_NAMESPACE="$NAMESPACE" tests/k8s/tilt/kind-node-trust.sh \
    2>&1 | tee -a "$WORK/commands.log"

CA="$WORK/orb-chrysa-ca.crt"
kubectl -n "$NAMESPACE" get secret orb-chrysa-server-tls -o jsonpath='{.data.ca\.crt}' | base64 -d > "$CA"
CI_TOKEN="$(refresh_ci_bot_token)"
PAT="$(create_pat "$CI_TOKEN" tilt-recovery-smoke)"
if [ -z "$PAT" ] || [ "$PAT" = "null" ]; then
    echo "ERROR: PAT response did not include a token" >&2
    exit 1
fi

assert_cluster_healthy "$WORK/cluster-status-initial.json"

BASE_IMAGE="$(resolve_smoke_base_image)"
printf 'tilt recovery smoke %s\n' "$RUN_ID" > "$WORK/dockerctx/hello.txt"
printf 'FROM %s\nCOPY hello.txt /hello.txt\n' "$BASE_IMAGE" > "$WORK/dockerctx/Dockerfile"
record docker build --provenance=false --sbom=false -t "$SMOKE_IMAGE" "$WORK/dockerctx"
kind_containerd_push "$SMOKE_IMAGE" "$PAT"
verify_node_pull "$SMOKE_IMAGE" "$PAT"

echo "=== Recovery: single pod restart rejoins membership ==="
record kubectl -n "$NAMESPACE" delete pod orb-chrysa-1 --wait=false
record kubectl -n "$NAMESPACE" rollout status statefulset/orb-chrysa --timeout=240s
assert_cluster_healthy "$WORK/cluster-status-after-pod-restart.json"
verify_node_pull "$SMOKE_IMAGE" "$PAT"

echo "=== Recovery: StatefulSet restart restores from S3 snapshot ==="
record kubectl -n "$NAMESPACE" rollout restart statefulset/orb-chrysa
record kubectl -n "$NAMESPACE" rollout status statefulset/orb-chrysa --timeout=360s
collect_orb_logs after-rollout-restart
assert_snapshot_restore_evidence
assert_cluster_healthy "$WORK/cluster-status-after-rollout-restart.json"
verify_node_pull "$SMOKE_IMAGE" "$PAT"
