#!/usr/bin/env bash
set -euo pipefail

REGISTRY_ENDPOINT="${REGISTRY_ENDPOINT:-localhost:32050}"
NAMESPACE="${ORB_NAMESPACE:-orb-chrysa-tilt}"
SECRET_NAME="${SERVER_TLS_SECRET:-orb-chrysa-server-tls}"
EVIDENCE_ROOT="${EVIDENCE_ROOT:-target/tilt}"
CA_FILE=""
RESTART_DOCKERD=1
VERIFY=1
WAIT_KUBERNETES=0
HELPER_IMAGE="${HOST_DOCKER_TRUST_HELPER_IMAGE:-alpine:3.21}"

usage() {
    cat <<'USAGE'
Usage: tests/k8s/tilt/host-docker-trust.sh [options]

Install the Tilt Orb Chrysa server CA into the host Docker daemon trust path.

Options:
  --registry-endpoint HOST:PORT  Registry endpoint to trust. Default: localhost:32050
  --namespace NAME               Namespace containing the server TLS Secret. Default: orb-chrysa-tilt
  --secret NAME                  Server TLS Secret name. Default: orb-chrysa-server-tls
  --ca-file PATH                 Use an existing CA file instead of reading Kubernetes
  --no-restart                   Do not restart/reload dockerd if verification still fails
  --no-verify                    Install files only; skip Docker daemon verification
  --wait-kubernetes              Wait for the Kubernetes API and Orb Chrysa Secret after Docker restart
  -h, --help                     Show this help

OrbStack note:
  OrbStack exposes /etc/docker/certs.d from ~/.docker/certs.d, but the Docker
  daemon's token-fetch path also needs the CA in the OrbStack VM system trust
  bundle. This script installs both and restarts dockerd when needed.

Environment:
  HOST_DOCKER_TRUST_HELPER_IMAGE  Helper image for OrbStack VM updates. Default: alpine:3.21
USAGE
}

log() {
    printf '=== %s ===\n' "$*"
}

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $1" >&2
        exit 127
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --registry-endpoint)
            REGISTRY_ENDPOINT="$2"
            shift 2
            ;;
        --namespace)
            NAMESPACE="$2"
            shift 2
            ;;
        --secret)
            SECRET_NAME="$2"
            shift 2
            ;;
        --ca-file)
            CA_FILE="$2"
            shift 2
            ;;
        --no-restart)
            RESTART_DOCKERD=0
            shift
            ;;
        --no-verify)
            VERIFY=0
            shift
            ;;
        --wait-kubernetes)
            WAIT_KUBERNETES=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

need docker
need curl
need base64

WORK="$EVIDENCE_ROOT/host-docker-trust"
mkdir -p "$WORK"
CA="$WORK/orb-chrysa-server-ca.crt"

if [ -n "$CA_FILE" ]; then
    cp "$CA_FILE" "$CA"
else
    need kubectl
    log "Extracting CA from $NAMESPACE/$SECRET_NAME"
    kubectl -n "$NAMESPACE" get secret "$SECRET_NAME" -o jsonpath='{.data.ca\.crt}' | base64 -d > "$CA"
fi

if [ ! -s "$CA" ]; then
    echo "ERROR: CA file is empty: $CA" >&2
    exit 1
fi
CA_ABS="$(cd "$(dirname "$CA")" && pwd)/$(basename "$CA")"

slug="$(printf '%s' "$REGISTRY_ENDPOINT" | tr -c 'A-Za-z0-9._-' '-')"
docker_cert_dir="$HOME/.docker/certs.d/$REGISTRY_ENDPOINT"

install_user_certs_d() {
    log "Installing Docker certs.d CA at $docker_cert_dir"
    mkdir -p "$docker_cert_dir"
    cp "$CA" "$docker_cert_dir/ca.crt"
    chmod 0644 "$docker_cert_dir/ca.crt"
}

wait_for_docker() {
    for _ in $(seq 1 60); do
        if docker info >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    echo "ERROR: Docker daemon did not become available" >&2
    return 1
}

wait_for_registry() {
    for _ in $(seq 1 60); do
        if curl --cacert "$CA" -fsS "https://$REGISTRY_ENDPOINT/readyz" >/dev/null 2>&1; then
            return 0
        fi
        sleep 2
    done
    echo "ERROR: registry https://$REGISTRY_ENDPOINT/readyz did not become ready" >&2
    return 1
}

wait_for_kubernetes_api() {
    need kubectl

    log "Waiting for Kubernetes API recovery"
    for _ in $(seq 1 90); do
        if kubectl get --raw=/readyz >/dev/null 2>&1 \
            && kubectl -n "$NAMESPACE" get secret "$SECRET_NAME" >/dev/null 2>&1; then
            return 0
        fi
        sleep 2
    done

    echo "ERROR: Kubernetes API did not recover after Docker trust setup" >&2
    return 1
}

verify_docker_trust() {
    local output="$WORK/docker-trust-verify.log"
    local probe_image="$REGISTRY_ENDPOINT/orb-chrysa/host-docker-trust-probe:missing"
    local status

    log "Verifying host Docker daemon TLS trust"
    if docker pull "$probe_image" >"$output" 2>&1; then
        status=0
    else
        status=$?
    fi

    if [ "$status" -eq 0 ]; then
        cat "$output"
        return 0
    fi

    if grep -Eqi 'certificate signed by unknown authority|x509:' "$output"; then
        cat "$output"
        return 2
    fi

    if grep -Eqi 'unauthorized|authentication required|pull access denied|requested access|manifest unknown|not found|no basic auth credentials' "$output"; then
        cat "$output"
        echo "Docker reached the registry without a TLS trust error."
        return 0
    fi

    cat "$output"
    return 1
}

install_orbstack_system_ca() {
    log "Installing CA into OrbStack VM system trust"
    docker run --rm --privileged --pid=host -v /:/host \
        --mount "type=bind,src=$CA_ABS,dst=/tmp/orb-chrysa-ca.crt,readonly" \
        "$HELPER_IMAGE" sh -eu -c "
            mkdir -p /host/usr/local/share/ca-certificates
            cp /tmp/orb-chrysa-ca.crt /host/usr/local/share/ca-certificates/orb-chrysa-${slug}.crt
            chroot /host /usr/sbin/update-ca-certificates
        "
}

restart_orbstack_dockerd() {
    log "Restarting OrbStack dockerd"
    docker run -d --rm --privileged --pid=host "$HELPER_IMAGE" sh -eu -c '
        sleep 1
        pid="$(pidof dockerd || pgrep -x dockerd | head -1)"
        test -n "$pid"
        kill -TERM "$pid"
    ' >/dev/null
    sleep 2
    wait_for_docker
}

install_linux_engine_ca() {
    local system_dir="/etc/docker/certs.d/$REGISTRY_ENDPOINT"

    log "Installing Docker Engine CA at $system_dir"
    sudo mkdir -p "$system_dir"
    sudo cp "$CA" "$system_dir/ca.crt"
    sudo chmod 0644 "$system_dir/ca.crt"
}

restart_linux_dockerd() {
    log "Reloading/restarting Linux dockerd"
    if command -v systemctl >/dev/null 2>&1; then
        sudo systemctl reload docker 2>/dev/null || sudo systemctl restart docker
    else
        sudo service docker restart
    fi
    wait_for_docker
}

install_user_certs_d

docker_os="$(docker info --format '{{.OperatingSystem}}' 2>/dev/null || true)"
host_os="$(uname -s)"

if [ "$VERIFY" -eq 0 ]; then
    echo "Installed Docker certs.d CA. Verification skipped."
    exit 0
fi

wait_for_registry

set +e
verify_docker_trust
verify_status=$?
set -e

if [ "$verify_status" -eq 0 ]; then
    echo "Host Docker daemon already trusts $REGISTRY_ENDPOINT."
    exit 0
fi

if [ "$verify_status" -ne 2 ]; then
    echo "ERROR: Docker trust verification failed for a non-TLS reason." >&2
    exit "$verify_status"
fi

case "$docker_os:$host_os" in
    OrbStack:Darwin)
        install_orbstack_system_ca
        if [ "$RESTART_DOCKERD" -eq 1 ]; then
            restart_orbstack_dockerd
            wait_for_registry
        else
            echo "OrbStack needs dockerd restart before the new system CA is used." >&2
            echo "Re-run without --no-restart to restart OrbStack dockerd." >&2
            exit 1
        fi
        ;;
    *:Linux)
        install_linux_engine_ca
        if [ "$RESTART_DOCKERD" -eq 1 ]; then
            restart_linux_dockerd
            wait_for_registry
        else
            echo "Docker may need a daemon reload/restart before the new CA is used." >&2
            echo "Re-run without --no-restart to reload/restart dockerd." >&2
            exit 1
        fi
        ;;
    *)
        cat >&2 <<EOF
Installed $docker_cert_dir/ca.crt, but Docker still reports an x509 trust error.
Docker backend: ${docker_os:-unknown} on $host_os

Restart your Docker backend, then rerun this script. Docker Desktop may require
adding the CA to the OS trust store as well as ~/.docker/certs.d.
EOF
        exit 1
        ;;
esac

if [ "$WAIT_KUBERNETES" -eq 1 ]; then
    wait_for_kubernetes_api
fi

verify_docker_trust
echo "Host Docker daemon trusts $REGISTRY_ENDPOINT."
