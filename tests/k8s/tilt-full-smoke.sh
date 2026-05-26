#!/usr/bin/env bash
set -euo pipefail

RUN_ID="${RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
CLUSTER="${KIND_CLUSTER_NAME:-orb-chrysa-tilt}"
NAMESPACE="${ORB_NAMESPACE:-orb-chrysa-tilt}"
S3_NAMESPACE="${RUSTFS_NAMESPACE:-orb-chrysa-tilt-s3}"
REGISTRY_ENDPOINT="${REGISTRY_ENDPOINT:-localhost:32050}"
KANIDM_HOST_PORT="${KANIDM_HOST_PORT:-8443}"
KANIDM_URL="${KANIDM_URL:-https://localhost:$KANIDM_HOST_PORT}"
EVIDENCE_ROOT="${EVIDENCE_ROOT:-target/tilt/evidence}"
WORK="$EVIDENCE_ROOT/$RUN_ID"
SMOKE_REPO="${SMOKE_REPO:-qa/tilt-smoke-$RUN_ID}"
SMOKE_IMAGE="$REGISTRY_ENDPOINT/$SMOKE_REPO:green"
PAT_IMAGE="$REGISTRY_ENDPOINT/$SMOKE_REPO:pat"
SMOKE_BASE_IMAGE="${SMOKE_BASE_IMAGE:-busybox:1.36}"
SMOKE_BASE_IMAGE_FALLBACK="${SMOKE_BASE_IMAGE_FALLBACK:-alpine:3.21}"
REQUIRE_HOST_DOCKER_PUSH="${REQUIRE_HOST_DOCKER_PUSH:-0}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# shellcheck source=tests/k8s/lib.sh
source "$SCRIPT_DIR/lib.sh"

umask 077
mkdir -p "$WORK/dockerctx"
chmod -R go-rwx "$WORK"

push_image() {
    local image="$1"
    local password="$2"
    local log="$WORK/docker-push-$(echo "$image" | tr '/:' '__').log"

    if docker push "$image" 2>&1 | tee -a "$WORK/commands.log" "$log"; then
        return 0
    fi

    if [ "$REQUIRE_HOST_DOCKER_PUSH" = "1" ]; then
        echo "ERROR: host Docker push is required for this smoke run; not falling back to kind containerd push" >&2
        return 1
    fi

    if grep -qi 'certificate signed by unknown authority' "$log"; then
        kind_containerd_push "$image" "$password"
    else
        return 1
    fi
}

wait_for_kubernetes_api_access() {
    local consecutive_ok=0

    for _ in $(seq 1 90); do
        if kubectl get --raw=/readyz >/dev/null 2>&1 \
            && kubectl auth can-i get deployments.apps -n kanidm >/dev/null 2>&1 \
            && kubectl auth can-i get pods -n "$NAMESPACE" >/dev/null 2>&1; then
            consecutive_ok=$((consecutive_ok + 1))
            if [ "$consecutive_ok" -ge 3 ]; then
                return 0
            fi
        else
            consecutive_ok=0
        fi
        sleep 2
    done

    return 1
}

cleanup() {
    status=$?
    if [ "$status" -eq 0 ]; then
        echo "PASS Tilt Helm smoke. Evidence: $WORK"
    else
        echo "FAIL Tilt Helm smoke. Evidence: $WORK" >&2
    fi
    exit "$status"
}
trap cleanup EXIT

need kubectl
need docker
need curl
need jq
need base64
need kind

{
    echo "RUN_ID=$RUN_ID"
    echo "CLUSTER=$CLUSTER"
    echo "NAMESPACE=$NAMESPACE"
    echo "S3_NAMESPACE=$S3_NAMESPACE"
    echo "REGISTRY_ENDPOINT=$REGISTRY_ENDPOINT"
    echo "KANIDM_URL=$KANIDM_URL"
    echo "SMOKE_IMAGE=$SMOKE_IMAGE"
    echo "PAT_IMAGE=$PAT_IMAGE"
    echo "REQUIRE_HOST_DOCKER_PUSH=$REQUIRE_HOST_DOCKER_PUSH"
} > "$WORK/summary.env"

{
    kubectl version --client=true
    docker version --format '{{.Client.Version}} client / {{.Server.Version}} server'
    kind version
    curl --version | head -1
    jq --version
} | tee "$WORK/versions.txt"

echo "=== Waiting for Kubernetes API access ==="
if ! wait_for_kubernetes_api_access; then
    echo "ERROR: Kubernetes API access did not recover after node trust setup" >&2
    exit 1
fi

record_retry kubectl -n cert-manager rollout status deploy/cert-manager --timeout=180s
record_retry kubectl -n cert-manager wait --for=condition=Ready certificate/orb-chrysa-ca --timeout=180s
record_retry kubectl -n "$S3_NAMESPACE" rollout status deploy/rustfs --timeout=180s
record_retry kubectl -n "$S3_NAMESPACE" wait --for=condition=complete job/rustfs-init --timeout=180s
record_retry kubectl -n kanidm rollout status deploy/kanidm --timeout=240s
record_retry kubectl -n "$NAMESPACE" rollout status statefulset/orb-chrysa --timeout=360s
kubectl -n "$NAMESPACE" get pods -o wide | tee "$WORK/orb-chrysa-pods.txt"

CA="$WORK/orb-chrysa-ca.crt"
kubectl -n "$NAMESPACE" get secret orb-chrysa-server-tls -o jsonpath='{.data.ca\.crt}' | base64 -d > "$CA"
CI_TOKEN="$(refresh_ci_bot_token)"

echo "=== Configure Docker trust for $REGISTRY_ENDPOINT ==="
if [ "$REQUIRE_HOST_DOCKER_PUSH" = "1" ]; then
    tests/k8s/tilt/host-docker-trust.sh \
        --registry-endpoint "$REGISTRY_ENDPOINT" \
        --ca-file "$CA" \
        --no-restart \
        --wait-kubernetes
else
    tests/k8s/tilt/host-docker-trust.sh \
        --registry-endpoint "$REGISTRY_ENDPOINT" \
        --ca-file "$CA" \
        --no-restart \
        --no-verify
fi

retry local_curl --cacert "$CA" -fsS "https://$REGISTRY_ENDPOINT/readyz" | tee "$WORK/readyz.txt"
retry local_curl --cacert "$CA" -fsS \
    -H "Authorization: Bearer $CI_TOKEN" \
    "https://$REGISTRY_ENDPOINT/api/v1/admin/cluster/status" \
    | tee "$WORK/cluster-status.json" \
    | jq '{leader_id, quorum, healthy_voters}'
jq -e '.leader_id != null and .healthy_voters >= .quorum' "$WORK/cluster-status.json" >/dev/null

HTTP_CODE="$(local_curl --cacert "$CA" -sS -o "$WORK/v2-unauth.txt" -w "%{http_code}" "https://$REGISTRY_ENDPOINT/v2/")"
if [ "$HTTP_CODE" != "401" ]; then
    echo "ERROR: expected unauthenticated /v2/ to return 401, got $HTTP_CODE" >&2
    exit 1
fi

BASE_IMAGE="$(resolve_smoke_base_image)"
printf 'hello from orb-chrysa tilt smoke %s\n' "$RUN_ID" > "$WORK/dockerctx/hello.txt"
printf 'FROM %s\nCOPY hello.txt /hello.txt\n' "$BASE_IMAGE" > "$WORK/dockerctx/Dockerfile"
record docker build --provenance=false --sbom=false -t "$SMOKE_IMAGE" "$WORK/dockerctx"

echo "=== Docker login and push with Kanidm ci-bot token ==="
printf '%s' "$CI_TOKEN" | record docker login "$REGISTRY_ENDPOINT" --username ci-bot --password-stdin
push_image "$SMOKE_IMAGE" "$CI_TOKEN"

echo "=== Create PAT and verify Docker push with PAT ==="
PAT_RESP="$WORK/pat-response.json"
PAT="$(create_pat "$CI_TOKEN" tilt-smoke "$PAT_RESP")"
if [ -z "$PAT" ] || [ "$PAT" = "null" ]; then
    echo "ERROR: PAT response did not include a token" >&2
    exit 1
fi
record docker tag "$SMOKE_IMAGE" "$PAT_IMAGE"
printf '%s' "$PAT" | record docker login "$REGISTRY_ENDPOINT" --username ci-bot --password-stdin
push_image "$PAT_IMAGE" "$PAT"

echo "=== Verify node containerd pulls ==="
verify_node_pull "$SMOKE_IMAGE" "$PAT"

echo "=== Verify Kubernetes image pull ==="
kubectl -n "$NAMESPACE" create secret docker-registry orb-chrysa-pull \
    --docker-server="$REGISTRY_ENDPOINT" \
    --docker-username=ci-bot \
    --docker-password="$PAT" \
    --dry-run=client -o yaml | kubectl apply -f -
kubectl -n "$NAMESPACE" delete pod "orb-smoke-$RUN_ID" --ignore-not-found
record kubectl -n "$NAMESPACE" run "orb-smoke-$RUN_ID" \
    --image="$SMOKE_IMAGE" \
    --restart=Never \
    --overrides='{"spec":{"imagePullSecrets":[{"name":"orb-chrysa-pull"}]}}' \
    --command -- /bin/sh -c 'cat /hello.txt; sleep 5'
record kubectl -n "$NAMESPACE" wait --for=condition=Ready "pod/orb-smoke-$RUN_ID" --timeout=180s
kubectl -n "$NAMESPACE" logs "pod/orb-smoke-$RUN_ID" | tee "$WORK/kubectl-run.log"
