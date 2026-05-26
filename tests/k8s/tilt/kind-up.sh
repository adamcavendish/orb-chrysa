#!/usr/bin/env bash
set -euo pipefail

CLUSTER="${KIND_CLUSTER_NAME:-orb-chrysa-tilt}"
CONTEXT="kind-$CLUSTER"
REGISTRY_NODE_PORT="${REGISTRY_NODE_PORT:-32050}"
KANIDM_NODE_PORT="${KANIDM_NODE_PORT:-30443}"
KANIDM_HOST_PORT="${KANIDM_HOST_PORT:-8443}"
LOAD_FIXTURE_IMAGES="${LOAD_TILT_FIXTURE_IMAGES:-1}"

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $1" >&2
        exit 127
    fi
}

need docker
need kind
need kubectl

docker_arch="$(docker info --format '{{.Architecture}}' 2>/dev/null || uname -m)"
case "$docker_arch" in
    aarch64 | arm64)
        default_image_platform="linux/arm64"
        ;;
    x86_64 | amd64)
        default_image_platform="linux/amd64"
        ;;
    *)
        default_image_platform="linux/$docker_arch"
        ;;
esac
LOAD_IMAGE_PLATFORM="${LOAD_TILT_IMAGE_PLATFORM:-$default_image_platform}"

pull_image() {
    local image="$1"

    for attempt in 1 2 3; do
        if docker pull --platform "$LOAD_IMAGE_PLATFORM" "$image"; then
            return 0
        fi
        if [ "$attempt" -eq 3 ]; then
            echo "ERROR: failed to pull fixture image after 3 attempts: $image" >&2
            return 1
        fi
        sleep 3
    done
}

load_fixture_image() {
    local image="$1"

    echo "Loading fixture image into kind: $image"
    if docker image inspect "$image" >/dev/null 2>&1; then
        echo "Using local fixture image: $image"
    else
        pull_image "$image"
    fi

    if kind load docker-image --name "$CLUSTER" "$image"; then
        return 0
    fi

    echo "WARN: kind failed to load local image $image; pulling $LOAD_IMAGE_PLATFORM and retrying" >&2
    pull_image "$image"
    kind load docker-image --name "$CLUSTER" "$image"
}

if kind get clusters | grep -qx "$CLUSTER"; then
    echo "kind cluster $CLUSTER already exists"
else
    config="$(mktemp)"
    cat > "$config" <<YAML
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
    extraPortMappings:
      - containerPort: $REGISTRY_NODE_PORT
        hostPort: $REGISTRY_NODE_PORT
        protocol: TCP
      - containerPort: $KANIDM_NODE_PORT
        hostPort: $KANIDM_HOST_PORT
        protocol: TCP
  - role: worker
  - role: worker
YAML
    kind create cluster --name "$CLUSTER" --config "$config"
    rm -f "$config"
fi

kubectl config use-context "$CONTEXT" >/dev/null
kubectl cluster-info --context "$CONTEXT" >/dev/null

if [ "$LOAD_FIXTURE_IMAGES" = "1" ]; then
    for image in \
        "${RUSTFS_IMAGE:-rustfs/rustfs:1.0.0-beta.2}" \
        "${RUSTFS_RC_IMAGE:-rustfs/rc:latest}" \
        "${KANIDM_IMAGE:-kanidm/server:1.10.3}"; do
        load_fixture_image "$image"
    done
    for image in \
        "${SMOKE_BASE_IMAGE:-busybox:1.36}" \
        "${SMOKE_BASE_IMAGE_FALLBACK:-alpine:3.21}"; do
        if ! load_fixture_image "$image"; then
            echo "WARN: continuing without preloaded optional smoke base image: $image" >&2
        fi
    done
    rustfs_namespace="${RUSTFS_NAMESPACE:-orb-chrysa-tilt-s3}"
    orb_namespace="${ORB_NAMESPACE:-orb-chrysa-tilt}"
    kubectl -n "$rustfs_namespace" delete job rustfs-init --ignore-not-found --wait=false 2>/dev/null || true
    kubectl -n "$rustfs_namespace" delete pod -l app=rustfs --ignore-not-found --wait=false 2>/dev/null || true
    kubectl -n "${KANIDM_NAMESPACE:-kanidm}" delete pod -l app=kanidm --ignore-not-found --wait=false 2>/dev/null || true
    kubectl -n "$orb_namespace" delete statefulset orb-chrysa --ignore-not-found --cascade=foreground --wait=true 2>/dev/null || true
    kubectl -n "$orb_namespace" delete pod -l app.kubernetes.io/name=orb-chrysa --ignore-not-found --wait=true 2>/dev/null || true
fi

if [ "${LOAD_TILT_APP_IMAGE:-0}" = "1" ]; then
    app_image="${ORB_CHRYSA_TILT_IMAGE:-orb-chrysa-server:tilt}"
    echo "Loading Tilt app image into kind: $app_image"
    if ! docker image inspect "$app_image" >/dev/null 2>&1; then
        echo "ERROR: local Tilt app image not found: $app_image" >&2
        echo "Build it with default tilt-ci or tag an existing image before setting LOAD_TILT_APP_IMAGE=1." >&2
        exit 1
    fi
    kind load docker-image --name "$CLUSTER" "$app_image"
fi

echo "ready: $CONTEXT"
