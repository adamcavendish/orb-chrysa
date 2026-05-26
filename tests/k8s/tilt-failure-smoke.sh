#!/usr/bin/env bash
set -euo pipefail

RUN_ID="${RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
RUN_SLUG="$(printf '%s' "$RUN_ID" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9-' '-')"
CLUSTER="${KIND_CLUSTER_NAME:-orb-chrysa-tilt}"
NAMESPACE="${ORB_NAMESPACE:-orb-chrysa-tilt}"
REGISTRY_ENDPOINT="${REGISTRY_ENDPOINT:-localhost:32050}"
NODE_PULL_ENDPOINT="${NODE_PULL_ENDPOINT:-orb-chrysa.$NAMESPACE.svc.cluster.local:5050}"
KANIDM_HOST_PORT="${KANIDM_HOST_PORT:-8443}"
KANIDM_URL="${KANIDM_URL:-https://localhost:$KANIDM_HOST_PORT}"
EVIDENCE_ROOT="${EVIDENCE_ROOT:-target/tilt/evidence}"
WORK="$EVIDENCE_ROOT/$RUN_ID-failure"
SMOKE_REPO="${SMOKE_REPO:-qa/tilt-failure-$RUN_ID}"
SMOKE_IMAGE="$REGISTRY_ENDPOINT/$SMOKE_REPO:probe"
NODE_PULL_IMAGE="$NODE_PULL_ENDPOINT/$SMOKE_REPO:probe"
SMOKE_BASE_IMAGE="${SMOKE_BASE_IMAGE:-busybox:1.36}"
SMOKE_BASE_IMAGE_FALLBACK="${SMOKE_BASE_IMAGE_FALLBACK:-alpine:3.21}"
LOCAL_STATUS_PORT="${LOCAL_STATUS_PORT:-32450}"
PAUSED_TASKS="$WORK/paused-tasks.txt"
PORT_FORWARD_PID=""
TRUST_REMOVED=0
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# shellcheck source=tests/k8s/lib.sh
source "$SCRIPT_DIR/lib.sh"

umask 077
mkdir -p "$WORK/dockerctx"
touch "$PAUSED_TASKS"
chmod -R go-rwx "$WORK"

cluster_status() {
    local_curl --cacert "$CA" -fsS \
        -H "Authorization: Bearer $CI_TOKEN" \
        "https://$REGISTRY_ENDPOINT/api/v1/admin/cluster/status"
}

assert_cluster_healthy() {
    local output="$1"

    for _ in $(seq 1 60); do
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

install_node_trust() {
    WORK="$WORK/node-trust" REGISTRY_ENDPOINT="$REGISTRY_ENDPOINT" ORB_NAMESPACE="$NAMESPACE" tests/k8s/tilt/kind-node-trust.sh \
        2>&1 | tee -a "$WORK/commands.log"
}

install_node_pull_trust() {
    WORK="$WORK/node-pull-trust" REGISTRY_ENDPOINT="$NODE_PULL_ENDPOINT" ORB_NAMESPACE="$NAMESPACE" tests/k8s/tilt/kind-node-trust.sh \
        2>&1 | tee -a "$WORK/commands.log"
}

install_node_pull_host_alias() {
    local node="$1"
    local service_host="${NODE_PULL_ENDPOINT%:*}"
    local service_ip

    service_ip="$(kubectl -n "$NAMESPACE" get svc orb-chrysa -o jsonpath='{.spec.clusterIP}')"
    record docker exec "$node" sh -ec "grep -q ' $service_host' /etc/hosts || printf '%s %s\\n' '$service_ip' '$service_host' >> /etc/hosts"
}

restart_node_containerd() {
    local node="$1"

    record docker exec "$node" systemctl restart containerd
    for _ in $(seq 1 30); do
        if docker exec "$node" systemctl is-active --quiet containerd; then
            return 0
        fi
        sleep 1
    done
    echo "ERROR: containerd did not become active on $node" >&2
    return 1
}

pod_container_task() {
    local pod="$1"
    local node
    local container_id

    node="$(kubectl -n "$NAMESPACE" get pod "$pod" -o jsonpath='{.spec.nodeName}')"
    container_id="$(kubectl -n "$NAMESPACE" get pod "$pod" -o jsonpath='{.status.containerStatuses[?(@.name=="orb-chrysa")].containerID}' | sed 's|containerd://||')"
    if [ -z "$node" ] || [ -z "$container_id" ]; then
        echo "ERROR: could not resolve node/container for $pod" >&2
        return 1
    fi
    printf '%s %s\n' "$node" "$container_id"
}

pause_pod() {
    local pod="$1"
    local node
    local task

    read -r node task < <(pod_container_task "$pod")
    record docker exec "$node" ctr -n k8s.io tasks pause "$task"
    printf '%s %s %s\n' "$pod" "$node" "$task" >> "$PAUSED_TASKS"
}

resume_paused_tasks() {
    if [ ! -s "$PAUSED_TASKS" ]; then
        return
    fi

    while read -r _pod node task; do
        docker exec "$node" ctr -n k8s.io tasks resume "$task" >/dev/null 2>&1 || true
    done
    : > "$PAUSED_TASKS"
}

resume_all_paused_tasks() {
    local node

    for node in $(kind get nodes --name "$CLUSTER"); do
        docker exec "$node" sh -ec 'ctr -n k8s.io tasks ls | awk '"'"'$3 == "PAUSED" {print $1}'"'"' | while read -r id; do ctr -n k8s.io tasks resume "$id" || true; done' \
            >/dev/null 2>&1 || true
    done
}

start_pod0_port_forward() {
    local pod_fqdn="orb-chrysa.$NAMESPACE.svc.cluster.local"

    kubectl -n "$NAMESPACE" port-forward --address 127.0.0.1 pod/orb-chrysa-0 "$LOCAL_STATUS_PORT:5050" \
        > "$WORK/pod0-port-forward.log" 2>&1 &
    PORT_FORWARD_PID="$!"
    sleep 2
    echo "$pod_fqdn"
}

stop_pod0_port_forward() {
    if [ -n "$PORT_FORWARD_PID" ]; then
        kill "$PORT_FORWARD_PID" >/dev/null 2>&1 || true
        wait "$PORT_FORWARD_PID" >/dev/null 2>&1 || true
        PORT_FORWARD_PID=""
    fi
}

wait_for_quorum_loss() {
    local pod_fqdn
    local status_url
    local write_url
    local http_code
    local curl_status

    pod_fqdn="$(start_pod0_port_forward)"
    status_url="https://$pod_fqdn:$LOCAL_STATUS_PORT/api/v1/admin/cluster/status"
    write_url="https://$pod_fqdn:$LOCAL_STATUS_PORT/api/v1/tokens"

    for _ in $(seq 1 60); do
        if local_curl --cacert "$CA" \
            --resolve "$pod_fqdn:$LOCAL_STATUS_PORT:127.0.0.1" \
            -fsS -H "Authorization: Bearer $CI_TOKEN" \
            "$status_url" > "$WORK/two-pod-quorum-loss.json" 2>"$WORK/two-pod-quorum-loss.err" \
            && jq -e '(.leader_id == null) or (.healthy_voters < .quorum)' "$WORK/two-pod-quorum-loss.json" >/dev/null; then
            stop_pod0_port_forward
            return 0
        fi

        set +e
        http_code="$(local_curl --cacert "$CA" \
            --resolve "$pod_fqdn:$LOCAL_STATUS_PORT:127.0.0.1" \
            --max-time 5 \
            -sS -o "$WORK/two-pod-write-during-quorum-loss.json" \
            -w '%{http_code}' \
            -H "Authorization: Bearer $CI_TOKEN" \
            -H "Content-Type: application/json" \
            -d "{\"name\":\"tilt-quorum-loss-$RUN_SLUG\",\"scopes\":[\"repository:*:*\"],\"expires_in_days\":1}" \
            "$write_url" 2>"$WORK/two-pod-write-during-quorum-loss.err")"
        curl_status=$?
        set -e

        if [ "$curl_status" -ne 0 ] || [ "$http_code" = "000" ] || [ "$http_code" -ge 500 ]; then
            printf '%s\n' "$http_code" > "$WORK/two-pod-write-during-quorum-loss.http_code"
            stop_pod0_port_forward
            return 0
        fi

        printf '%s\n' "$http_code" > "$WORK/two-pod-write-during-quorum-loss.http_code"
        sleep 2
    done

    stop_pod0_port_forward
    echo "ERROR: two paused Orb Chrysa pods did not produce quorum-loss status or write failure" >&2
    cat "$WORK/two-pod-quorum-loss.json" >&2 2>/dev/null || true
    cat "$WORK/two-pod-write-during-quorum-loss.json" >&2 2>/dev/null || true
    return 1
}

cleanup() {
    status=$?
    set +e
    stop_pod0_port_forward
    resume_paused_tasks
    resume_all_paused_tasks
    if [ "$TRUST_REMOVED" -eq 1 ]; then
        install_node_trust >/dev/null 2>&1
        install_node_pull_trust >/dev/null 2>&1
        if [ -n "${NODE:-}" ]; then
            restart_node_containerd "$NODE" >/dev/null 2>&1 || true
        fi
    fi
    kubectl -n "$NAMESPACE" delete pod "orb-no-secret-$RUN_SLUG" "orb-with-secret-$RUN_SLUG" --ignore-not-found >/dev/null 2>&1
    kubectl -n "$NAMESPACE" delete secret "orb-chrysa-pull-$RUN_SLUG" --ignore-not-found >/dev/null 2>&1
    kubectl -n "$NAMESPACE" rollout status statefulset/orb-chrysa --timeout=180s >/dev/null 2>&1
    if [ "$status" -eq 0 ]; then
        echo "PASS Tilt failure smoke. Evidence: $WORK"
    else
        echo "FAIL Tilt failure smoke. Evidence: $WORK" >&2
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
need sed
need tee

{
    echo "RUN_ID=$RUN_ID"
    echo "CLUSTER=$CLUSTER"
    echo "NAMESPACE=$NAMESPACE"
    echo "REGISTRY_ENDPOINT=$REGISTRY_ENDPOINT"
    echo "NODE_PULL_ENDPOINT=$NODE_PULL_ENDPOINT"
    echo "KANIDM_URL=$KANIDM_URL"
    echo "SMOKE_IMAGE=$SMOKE_IMAGE"
    echo "NODE_PULL_IMAGE=$NODE_PULL_IMAGE"
} > "$WORK/summary.env"

record kubectl -n "$NAMESPACE" rollout status statefulset/orb-chrysa --timeout=240s
install_node_trust
# Push imports the test image into the first kind node. Use a different node
# for the trust-removal pull so a local image cache cannot mask TLS failure.
NODE="$(kind get nodes --name "$CLUSTER" | tail -1)"
install_node_pull_host_alias "$NODE"
install_node_pull_trust
restart_node_containerd "$NODE"

CA="$WORK/orb-chrysa-ca.crt"
kubectl -n "$NAMESPACE" get secret orb-chrysa-server-tls -o jsonpath='{.data.ca\.crt}' | base64 -d > "$CA"
CI_TOKEN="$(refresh_ci_bot_token)"
PAT="$(create_pat "$CI_TOKEN" tilt-failure-smoke)"
if [ -z "$PAT" ] || [ "$PAT" = "null" ]; then
    echo "ERROR: PAT response did not include a token" >&2
    exit 1
fi

assert_cluster_healthy "$WORK/cluster-status-initial.json"

BASE_IMAGE="$(resolve_smoke_base_image)"
printf 'tilt failure smoke %s\n' "$RUN_ID" > "$WORK/dockerctx/hello.txt"
printf 'FROM %s\nCOPY hello.txt /hello.txt\n' "$BASE_IMAGE" > "$WORK/dockerctx/Dockerfile"
record docker build --provenance=false --sbom=false -t "$SMOKE_IMAGE" "$WORK/dockerctx"
kind_containerd_push "$SMOKE_IMAGE" "$PAT"

echo "=== Failure: node trust removal blocks containerd pull ==="
record docker exec "$NODE" crictl rmi "$NODE_PULL_IMAGE" || true
record docker exec "$NODE" rm -rf "/etc/containerd/certs.d/$NODE_PULL_ENDPOINT"
restart_node_containerd "$NODE"
TRUST_REMOVED=1
set +e
docker exec "$NODE" crictl pull --creds "ci-bot:$PAT" "$NODE_PULL_IMAGE" > "$WORK/node-trust-missing.log" 2>&1
PULL_STATUS=$?
set -e
if [ "$PULL_STATUS" -eq 0 ]; then
    echo "ERROR: crictl pull succeeded after trust removal" >&2
    exit 1
fi
install_node_pull_trust
restart_node_containerd "$NODE"
TRUST_REMOVED=0
record docker exec "$NODE" crictl pull --creds "ci-bot:$PAT" "$NODE_PULL_IMAGE"

echo "=== Failure: missing imagePullSecret blocks Kubernetes pull ==="
kubectl -n "$NAMESPACE" delete pod "orb-no-secret-$RUN_SLUG" --ignore-not-found
record kubectl -n "$NAMESPACE" run "orb-no-secret-$RUN_SLUG" \
    --image="$SMOKE_IMAGE" \
    --image-pull-policy=Always \
    --restart=Never \
    --command -- /bin/sh -c 'cat /hello.txt'
for _ in $(seq 1 60); do
    reason="$(kubectl -n "$NAMESPACE" get pod "orb-no-secret-$RUN_SLUG" -o jsonpath='{.status.containerStatuses[0].state.waiting.reason}' 2>/dev/null || true)"
    if [ "$reason" = "ErrImagePull" ] || [ "$reason" = "ImagePullBackOff" ]; then
        kubectl -n "$NAMESPACE" describe pod "orb-no-secret-$RUN_SLUG" > "$WORK/missing-imagepullsecret-describe.txt"
        break
    fi
    sleep 2
done
if [ "${reason:-}" != "ErrImagePull" ] && [ "${reason:-}" != "ImagePullBackOff" ]; then
    echo "ERROR: missing imagePullSecret did not produce ErrImagePull/ImagePullBackOff" >&2
    kubectl -n "$NAMESPACE" get pod "orb-no-secret-$RUN_SLUG" -o yaml > "$WORK/missing-imagepullsecret-pod.yaml" || true
    exit 1
fi

kubectl -n "$NAMESPACE" create secret docker-registry "orb-chrysa-pull-$RUN_SLUG" \
    --docker-server="$REGISTRY_ENDPOINT" \
    --docker-username=ci-bot \
    --docker-password="$PAT" \
    --dry-run=client -o yaml | kubectl apply -f -
kubectl -n "$NAMESPACE" delete pod "orb-with-secret-$RUN_SLUG" --ignore-not-found
record kubectl -n "$NAMESPACE" run "orb-with-secret-$RUN_SLUG" \
    --image="$SMOKE_IMAGE" \
    --image-pull-policy=Always \
    --restart=Never \
    --overrides="{\"spec\":{\"imagePullSecrets\":[{\"name\":\"orb-chrysa-pull-$RUN_SLUG\"}]}}" \
    --command -- /bin/sh -c 'cat /hello.txt; sleep 5'
record kubectl -n "$NAMESPACE" wait --for=condition=Ready "pod/orb-with-secret-$RUN_SLUG" --timeout=180s

echo "=== Failure: one pod loss keeps quorum ==="
record kubectl -n "$NAMESPACE" delete pod orb-chrysa-2 --wait=false
record kubectl -n "$NAMESPACE" rollout status statefulset/orb-chrysa --timeout=240s
assert_cluster_healthy "$WORK/cluster-status-after-one-pod-loss.json"

echo "=== Failure: two paused pods lose quorum ==="
pause_pod orb-chrysa-1
pause_pod orb-chrysa-2
wait_for_quorum_loss
resume_paused_tasks
resume_all_paused_tasks
record kubectl -n "$NAMESPACE" wait --for=condition=Ready pod/orb-chrysa-1 pod/orb-chrysa-2 --timeout=180s
assert_cluster_healthy "$WORK/cluster-status-after-two-pod-restore.json"
