#!/usr/bin/env bash
set -euo pipefail

CLUSTER="${KIND_CLUSTER_NAME:-orb-chrysa-tilt}"
NAMESPACE="${ORB_NAMESPACE:-orb-chrysa-tilt}"
REGISTRY_ENDPOINT="${REGISTRY_ENDPOINT:-localhost:32050}"
WORK="${WORK:-target/tilt/node-trust}"
RESTART_CONTAINERD="${RESTART_CONTAINERD_FOR_TRUST:-0}"

mkdir -p "$WORK"

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $1" >&2
        exit 127
    fi
}

need docker
need kind
need kubectl
need base64

echo "=== Waiting for Orb Chrysa server TLS Secret ==="
for _ in $(seq 1 90); do
    if kubectl -n "$NAMESPACE" get secret orb-chrysa-server-tls >/dev/null 2>&1; then
        break
    fi
    sleep 2
done
kubectl -n "$NAMESPACE" get secret orb-chrysa-server-tls >/dev/null

CA="$WORK/ca.crt"
kubectl -n "$NAMESPACE" get secret orb-chrysa-server-tls -o jsonpath='{.data.ca\.crt}' | base64 -d > "$CA"

HOSTS="$WORK/hosts.toml"
cat > "$HOSTS" <<EOF
server = "https://$REGISTRY_ENDPOINT"

[host."https://$REGISTRY_ENDPOINT"]
  capabilities = ["pull", "resolve", "push"]
  ca = "/etc/containerd/certs.d/$REGISTRY_ENDPOINT/ca.crt"
EOF

for node in $(kind get nodes --name "$CLUSTER"); do
    echo "Installing containerd trust on $node"
    docker exec "$node" mkdir -p "/etc/containerd/certs.d/$REGISTRY_ENDPOINT"
    docker cp "$CA" "$node:/etc/containerd/certs.d/$REGISTRY_ENDPOINT/ca.crt"
    docker cp "$HOSTS" "$node:/etc/containerd/certs.d/$REGISTRY_ENDPOINT/hosts.toml"
    if [ "$RESTART_CONTAINERD" = "1" ]; then
        docker exec "$node" sh -ec 'pkill -SIGHUP containerd 2>/dev/null || true'
    fi
done

echo "containerd trust installed for $REGISTRY_ENDPOINT"
