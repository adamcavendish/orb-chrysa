#!/usr/bin/env bash
# Production-style mirror/proxy cache workflow smoke test for a running
# layerhouse registry.
#
# Requires a live layerhouse compose cluster, Docker daemon, ORAS, curl, and jq.
# Defaults target the local compose cluster at localhost:5050 and a disposable
# upstream registry container attached to the compose network.
set -euo pipefail

REGISTRY="${REGISTRY:-localhost:5050}"
SCHEME="${SCHEME:-http}"
COMPOSE_NETWORK="${COMPOSE_NETWORK:-layerhouse_default}"
UPSTREAM_IMAGE="${UPSTREAM_IMAGE:-registry:2}"
RUN_ID="${RUN_ID:-$(date +%s)}"
EVIDENCE_ROOT="${EVIDENCE_ROOT:-/tmp}"
WORK="${WORK:-$EVIDENCE_ROOT/orb-mirror-proxy-$RUN_ID}"
POLL_TIMEOUT_SECS="${POLL_TIMEOUT_SECS:-180}"
POLL_INTERVAL_SECS="${POLL_INTERVAL_SECS:-3}"

UPSTREAM_NAME="${UPSTREAM_NAME:-layerhouse-upstream-$RUN_ID}"
UPSTREAM_HOST_REGISTRY=""
UPSTREAM_ORB_REGISTRY="$UPSTREAM_NAME:5000"
ORB_CONTAINER="${ORB_CONTAINER:-layerhouse-layerhouse-0-1}"

MIRROR_RULE="qa-mirror-$RUN_ID"
MIRROR_REPO="qa/mirror-$RUN_ID"
MIRROR_UPSTREAM_REPO="upstream/mirror-src"

CACHE_RULE="qa-cache-$RUN_ID"
CACHE_REPO="qa/cache-$RUN_ID"
CACHE_UPSTREAM_REPO="upstream/cache-src"

DOCKER_ROOT_CACHE_RULE="qa-docker-root-$RUN_ID"
DOCKER_ROOT_REPO="qa/docker-root-$RUN_ID"
DOCKER_ROOT_LOCAL_REPO="$DOCKER_ROOT_REPO/library/alpine"
DOCKER_LIBRARY_CACHE_RULE="qa-docker-library-$RUN_ID"
DOCKER_LIBRARY_REPO="qa/docker-library-$RUN_ID"
DOCKER_LIBRARY_LOCAL_REPO="$DOCKER_LIBRARY_REPO/alpine"
DOCKER_UPSTREAM_REPO="library/alpine"
DOCKER_TAG="3"
DOCKER_CHILD_TAG="layerhouse-smoke-$RUN_ID"

WARM_RULE="qa-cache-warm-$RUN_ID"
WARM_REPO="qa/cache-warm-$RUN_ID"
WARM_UPSTREAM_REPO="upstream/cache-warm"

PUSH_RULE="qa-push-$RUN_ID"
PUSH_SRC_REPO="qa/push-src-$RUN_ID"
PUSH_DST_UPSTREAM_REPO="upstream/push-dst"

PROXY_VALIDATION_RULE="qa-proxy-validation-$RUN_ID"
PROXY_DIRECT_RULE="qa-proxy-direct-$RUN_ID"
PROXY_SECRET_RULE="qa-proxy-secret-$RUN_ID"
PROXY_SECRET_CACHE="qa-proxy-secret-cache-$RUN_ID"

ORAS_TRANSPORT_FLAGS=()
if [ "$SCHEME" = "http" ]; then
    ORAS_TRANSPORT_FLAGS=(--plain-http)
fi

log() {
    printf '\n==> %s\n' "$*"
}

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $1" >&2
        exit 127
    fi
}

api_url() {
    printf '%s://%s%s' "$SCHEME" "$REGISTRY" "$1"
}

upstream_host_ref() {
    printf '%s/%s:%s' "$UPSTREAM_HOST_REGISTRY" "$1" "$2"
}

orb_ref() {
    printf '%s/%s:%s' "$REGISTRY" "$1" "$2"
}

delete_repo() {
    local repo="$1"
    curl -sS -X DELETE "$(api_url "/api/v1/repositories/$repo")" >/dev/null || true
}

delete_rule() {
    local id="$1"
    curl -sS -X DELETE "$(api_url "/api/v1/admin/mirror/rules/$id")" >/dev/null || true
}

delete_cache() {
    local id="$1"
    curl -sS -X DELETE "$(api_url "/api/v1/admin/proxy-cache/$id")" >/dev/null || true
}

cleanup() {
    local status=$?
    log "Cleanup disposable mirror/proxy workflow data"
    delete_rule "$MIRROR_RULE"
    delete_rule "$PUSH_RULE"
    delete_rule "$PROXY_VALIDATION_RULE"
    delete_rule "$PROXY_DIRECT_RULE"
    delete_rule "$PROXY_SECRET_RULE"
    delete_cache "$CACHE_RULE"
    delete_cache "$DOCKER_ROOT_CACHE_RULE"
    delete_cache "$DOCKER_LIBRARY_CACHE_RULE"
    delete_cache "$WARM_RULE"
    delete_cache "$PROXY_SECRET_CACHE"
    delete_repo "$MIRROR_REPO"
    delete_repo "$CACHE_REPO"
    delete_repo "$DOCKER_ROOT_LOCAL_REPO"
    delete_repo "$DOCKER_LIBRARY_LOCAL_REPO"
    delete_repo "$WARM_REPO"
    delete_repo "$PUSH_SRC_REPO"
    if [ -n "$UPSTREAM_HOST_REGISTRY" ]; then
        docker image rm \
            "$UPSTREAM_HOST_REGISTRY/$DOCKER_UPSTREAM_REPO:$DOCKER_CHILD_TAG" \
            "$UPSTREAM_HOST_REGISTRY/$DOCKER_UPSTREAM_REPO:$DOCKER_TAG" \
            "$REGISTRY/$DOCKER_ROOT_LOCAL_REPO:$DOCKER_TAG" \
            "$REGISTRY/$DOCKER_LIBRARY_LOCAL_REPO:$DOCKER_TAG" \
            >/dev/null 2>&1 || true
    fi
    docker rm -f "$UPSTREAM_NAME" >/dev/null 2>&1 || true
    if [ "$status" -eq 0 ]; then
        echo "PASS mirror/proxy production workflow. Evidence: $WORK"
    else
        echo "FAIL mirror/proxy production workflow. Evidence: $WORK" >&2
    fi
    exit "$status"
}
trap cleanup EXIT

json_put() {
    local url="$1"
    local body_file="$2"
    curl -fsS -X PUT \
        -H 'Content-Type: application/json' \
        --data-binary "@$body_file" \
        "$url"
}

curl_status() {
    local output="$1"
    shift
    curl -sS -o "$output" -w '%{http_code}' "$@"
}

wait_for_http() {
    local url="$1"
    local label="$2"
    local deadline=$(( $(date +%s) + 30 ))
    while [ "$(date +%s)" -le "$deadline" ]; do
        if curl -fsS "$url" >/dev/null 2>"$WORK/$label-ready.err"; then
            return 0
        fi
        sleep 1
    done
    echo "ERROR: timed out waiting for $label at $url" >&2
    cat "$WORK/$label-ready.err" >&2 || true
    return 1
}

wait_for_container_http() {
    local container="$1"
    local url="$2"
    local label="$3"
    local deadline=$(( $(date +%s) + 30 ))
    while [ "$(date +%s)" -le "$deadline" ]; do
        if docker exec "$container" curl -fsS "$url" >/dev/null 2>"$WORK/$label-ready.err"; then
            return 0
        fi
        sleep 1
    done
    echo "ERROR: timed out waiting for $label at $url from $container" >&2
    cat "$WORK/$label-ready.err" >&2 || true
    return 1
}

wait_for_api_jq() {
    local path="$1"
    local output="$2"
    local filter="$3"
    local label="$4"
    local deadline=$(( $(date +%s) + 30 ))
    local status
    while [ "$(date +%s)" -le "$deadline" ]; do
        status="$(curl_status "$output" "$(api_url "$path")")"
        if [ "$status" = "200" ] && jq -e "$filter" "$output" >/dev/null; then
            return 0
        fi
        sleep 1
    done
    echo "ERROR: timed out waiting for $label via $path" >&2
    printf 'last_http_status=%s\n' "$status" >&2
    cat "$output" >&2 || true
    return 1
}

wait_for_job() {
    local job_id="$1"
    local label="$2"
    local deadline=$(( $(date +%s) + POLL_TIMEOUT_SECS ))
    local runs_file="$WORK/$label-runs.json"
    local run_file="$WORK/$label-final-run.json"
    local job_file="$WORK/$label-final-job.json"

    while [ "$(date +%s)" -le "$deadline" ]; do
        local runs_status
        runs_status="$(curl_status "$runs_file" "$(api_url "/api/v1/admin/mirror/jobs/$job_id/runs?limit=5")")"
        if [ "$runs_status" != "200" ]; then
            sleep "$POLL_INTERVAL_SECS"
            continue
        fi
        local status
        status="$(jq -r '.[-1].status // empty' "$runs_file")"
        if [ -n "$status" ] && [ "$status" != "Running" ]; then
            jq '.[-1]' "$runs_file" | tee "$run_file" >/dev/null
            curl -fsS "$(api_url "/api/v1/admin/mirror/jobs/$job_id")" \
                | tee "$job_file" >/dev/null
            if [ "$status" = "Succeeded" ]; then
                return 0
            fi
            echo "ERROR: job $job_id ended with status $status" >&2
            cat "$run_file" >&2
            return 1
        fi
        sleep "$POLL_INTERVAL_SECS"
    done

    echo "ERROR: timed out waiting for job $job_id ($label)" >&2
    curl -fsS "$(api_url "/api/v1/admin/mirror/jobs/$job_id")" | tee "$job_file" >&2 || true
    curl -fsS "$(api_url "/api/v1/admin/mirror/jobs/$job_id/runs?limit=5")" | tee "$runs_file" >&2 || true
    return 1
}

push_oras_file() {
    local registry="$1"
    local repo="$2"
    local tag="$3"
    local file="$4"
    local media_type="$5"
    local output="$6"
    (
        cd "$(dirname "$file")"
        oras push --plain-http --no-tty --format json \
            --artifact-type application/vnd.layerhouse.qa.v1 \
            "$registry/$repo:$tag" \
            "$(basename "$file"):$media_type"
    ) | tee "$output"
}

pull_oras_file() {
    local ref="$1"
    local out_dir="$2"
    oras pull "${ORAS_TRANSPORT_FLAGS[@]}" --no-tty -o "$out_dir" "$ref"
}

docker_oci_arch() {
    local arch
    arch="$(docker info --format '{{.Architecture}}')"
    case "$arch" in
        amd64|x86_64) echo "amd64" ;;
        arm64|aarch64) echo "arm64" ;;
        armv7l) echo "arm" ;;
        *) echo "$arch" ;;
    esac
}

docker_oci_variant() {
    local arch
    arch="$(docker info --format '{{.Architecture}}')"
    case "$arch" in
        armv7l) echo "v7" ;;
        *) echo "" ;;
    esac
}

manifest_head_digest() {
    local url="$1"
    local output="$2"
    curl -fsSI \
        -H 'Accept: application/vnd.docker.distribution.manifest.list.v2+json' \
        -H 'Accept: application/vnd.oci.image.index.v1+json' \
        "$url" | tee "$output" >/dev/null
    awk -F': ' 'tolower($1) == "docker-content-digest" { gsub(/\r/, "", $2); print $2 }' "$output" | tail -1
}

get_manifest_list() {
    local url="$1"
    local output="$2"
    curl -fsS \
        -H 'Accept: application/vnd.docker.distribution.manifest.list.v2+json' \
        -H 'Accept: application/vnd.oci.image.index.v1+json' \
        "$url" | tee "$output"
}

need curl
need jq
need oras
need docker
need cmp
need sed
need grep
need awk

mkdir -p \
    "$WORK/seed" \
    "$WORK/dockerctx" \
    "$WORK/mirror-v1" \
    "$WORK/mirror-v2" \
    "$WORK/cache-first" \
    "$WORK/cache-second" \
    "$WORK/warm-pull" \
    "$WORK/push-src-pull" \
    "$WORK/push-upstream-pull"

printf 'mirror v1 payload %s\n' "$RUN_ID" > "$WORK/seed/mirror-v1.txt"
printf 'mirror v2 payload %s\n' "$RUN_ID" > "$WORK/seed/mirror-v2.txt"
printf 'cache v1 payload %s\n' "$RUN_ID" > "$WORK/seed/cache-v1.txt"
printf 'warm payload %s\n' "$RUN_ID" > "$WORK/seed/warm.txt"
printf 'push release payload %s\n' "$RUN_ID" > "$WORK/seed/push-release.txt"

cat > "$WORK/summary.env" <<EOF
RUN_ID=$RUN_ID
REGISTRY=$REGISTRY
SCHEME=$SCHEME
COMPOSE_NETWORK=$COMPOSE_NETWORK
UPSTREAM_IMAGE=$UPSTREAM_IMAGE
UPSTREAM_NAME=$UPSTREAM_NAME
WORK=$WORK
MIRROR_RULE=$MIRROR_RULE
MIRROR_REPO=$MIRROR_REPO
CACHE_RULE=$CACHE_RULE
CACHE_REPO=$CACHE_REPO
DOCKER_ROOT_CACHE_RULE=$DOCKER_ROOT_CACHE_RULE
DOCKER_ROOT_LOCAL_REPO=$DOCKER_ROOT_LOCAL_REPO
DOCKER_LIBRARY_CACHE_RULE=$DOCKER_LIBRARY_CACHE_RULE
DOCKER_LIBRARY_LOCAL_REPO=$DOCKER_LIBRARY_LOCAL_REPO
DOCKER_UPSTREAM_REPO=$DOCKER_UPSTREAM_REPO
DOCKER_TAG=$DOCKER_TAG
WARM_RULE=$WARM_RULE
WARM_REPO=$WARM_REPO
PUSH_RULE=$PUSH_RULE
PUSH_SRC_REPO=$PUSH_SRC_REPO
EOF

log "Client versions"
{
    oras version
    docker version --format '{{.Client.Version}} client / {{.Server.Version}} server'
    curl --version | head -1
    jq --version
} | tee "$WORK/client-versions.txt"

log "Registry liveness and cluster health"
curl -fsS "$(api_url /v2/)" >/dev/null
curl -fsS "$(api_url /api/v1/admin/cluster/status)" \
    | tee "$WORK/cluster-before-full.json" \
    | jq '{leader_id, quorum, healthy_voters}' \
    | tee "$WORK/cluster-before.json"
jq -e '.leader_id != null and .healthy_voters >= .quorum' "$WORK/cluster-before-full.json" >/dev/null

log "Start disposable local upstream registry"
if ! docker image inspect "$UPSTREAM_IMAGE" >/dev/null 2>&1; then
    echo "ERROR: upstream image $UPSTREAM_IMAGE is not available locally." >&2
    echo "Pull or preload it once, then rerun. This workflow itself uses only the local upstream registry." >&2
    exit 1
fi
docker network inspect "$COMPOSE_NETWORK" >/dev/null
docker run -d --rm --name "$UPSTREAM_NAME" \
    --network "$COMPOSE_NETWORK" \
    -p 127.0.0.1::5000 \
    "$UPSTREAM_IMAGE" >/dev/null
UPSTREAM_PORT="$(docker port "$UPSTREAM_NAME" 5000/tcp | sed -E 's/.*:([0-9]+)$/\1/' | tail -1)"
UPSTREAM_HOST_REGISTRY="localhost:$UPSTREAM_PORT"
printf 'UPSTREAM_HOST_REGISTRY=%s\nUPSTREAM_ORB_REGISTRY=%s\n' \
    "$UPSTREAM_HOST_REGISTRY" "$UPSTREAM_ORB_REGISTRY" | tee -a "$WORK/summary.env"
wait_for_http "http://$UPSTREAM_HOST_REGISTRY/v2/" "upstream-host"
wait_for_container_http "$ORB_CONTAINER" "http://$UPSTREAM_ORB_REGISTRY/v2/" "upstream-container"

log "Seed upstream registry with mirror/proxy artifacts"
push_oras_file "$UPSTREAM_HOST_REGISTRY" "$MIRROR_UPSTREAM_REPO" "v1" \
    "$WORK/seed/mirror-v1.txt" "application/vnd.layerhouse.qa.mirror.v1+txt" \
    "$WORK/upstream-mirror-v1.json"
MIRROR_V1_DIGEST="$(jq -r '.digest' "$WORK/upstream-mirror-v1.json")"
push_oras_file "$UPSTREAM_HOST_REGISTRY" "$MIRROR_UPSTREAM_REPO" "v2" \
    "$WORK/seed/mirror-v2.txt" "application/vnd.layerhouse.qa.mirror.v2+txt" \
    "$WORK/upstream-mirror-v2.json"
MIRROR_V2_DIGEST="$(jq -r '.digest' "$WORK/upstream-mirror-v2.json")"
push_oras_file "$UPSTREAM_HOST_REGISTRY" "$CACHE_UPSTREAM_REPO" "v1" \
    "$WORK/seed/cache-v1.txt" "application/vnd.layerhouse.qa.cache.v1+txt" \
    "$WORK/upstream-cache-v1.json"
CACHE_V1_DIGEST="$(jq -r '.digest' "$WORK/upstream-cache-v1.json")"
push_oras_file "$UPSTREAM_HOST_REGISTRY" "$WARM_UPSTREAM_REPO" "warm" \
    "$WORK/seed/warm.txt" "application/vnd.layerhouse.qa.warm.v1+txt" \
    "$WORK/upstream-warm.json"
WARM_DIGEST="$(jq -r '.digest' "$WORK/upstream-warm.json")"
printf 'MIRROR_V1_DIGEST=%s\nMIRROR_V2_DIGEST=%s\nCACHE_V1_DIGEST=%s\nWARM_DIGEST=%s\n' \
    "$MIRROR_V1_DIGEST" "$MIRROR_V2_DIGEST" "$CACHE_V1_DIGEST" "$WARM_DIGEST" \
    | tee -a "$WORK/summary.env"

log "Seed upstream registry with Docker manifest-list image"
DOCKER_ARCH="$(docker_oci_arch)"
DOCKER_VARIANT="$(docker_oci_variant)"
DOCKER_UPSTREAM_CHILD_REF="$UPSTREAM_HOST_REGISTRY/$DOCKER_UPSTREAM_REPO:$DOCKER_CHILD_TAG"
DOCKER_UPSTREAM_INDEX_REF="$UPSTREAM_HOST_REGISTRY/$DOCKER_UPSTREAM_REPO:$DOCKER_TAG"
cat > "$WORK/dockerctx/Dockerfile" <<'DOCKERFILE'
FROM scratch
LABEL org.opencontainers.image.title="layerhouse docker proxy smoke"
COPY docker-smoke.txt /layerhouse-smoke.txt
DOCKERFILE
printf 'docker manifest-list payload %s\n' "$RUN_ID" > "$WORK/dockerctx/docker-smoke.txt"
docker build -t "$DOCKER_UPSTREAM_CHILD_REF" "$WORK/dockerctx" \
    | tee "$WORK/docker-build.log"
docker push "$DOCKER_UPSTREAM_CHILD_REF" \
    | tee "$WORK/docker-push-child.log"
docker buildx imagetools create --tag "$DOCKER_UPSTREAM_INDEX_REF" "$DOCKER_UPSTREAM_CHILD_REF" \
    | tee "$WORK/docker-imagetools-create.log"
docker manifest inspect --insecure "$DOCKER_UPSTREAM_INDEX_REF" \
    | tee "$WORK/docker-upstream-index.json" \
    | jq -e --arg arch "$DOCKER_ARCH" '.manifests[] | select(.platform.os == "linux" and .platform.architecture == $arch)' >/dev/null
UPSTREAM_DOCKER_INDEX_DIGEST="$(manifest_head_digest \
    "http://$UPSTREAM_HOST_REGISTRY/v2/$DOCKER_UPSTREAM_REPO/manifests/$DOCKER_TAG" \
    "$WORK/docker-upstream-index.headers")"
[ -n "$UPSTREAM_DOCKER_INDEX_DIGEST" ]
printf 'DOCKER_ARCH=%s\nDOCKER_VARIANT=%s\nDOCKER_INDEX_DIGEST=%s\n' \
    "$DOCKER_ARCH" "$DOCKER_VARIANT" "$UPSTREAM_DOCKER_INDEX_DIGEST" \
    | tee -a "$WORK/summary.env"
docker image rm \
    "$DOCKER_UPSTREAM_CHILD_REF" \
    "$DOCKER_UPSTREAM_INDEX_REF" \
    >/dev/null 2>&1 || true

log "Create and trigger manual pull mirror rule"
cat > "$WORK/mirror-rule.json" <<JSON
{
  "id": "$MIRROR_RULE",
  "direction": "pull",
  "local_prefix": "$MIRROR_REPO",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": "$MIRROR_UPSTREAM_REPO",
  "schedule": null,
  "strategy": { "type": "pattern", "pattern": "v*" },
  "plain_http": true,
  "outbound_proxy": { "protocol": "none" },
  "username": null,
  "password": null,
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/mirror/rules/$MIRROR_RULE")" "$WORK/mirror-rule.json" \
    | tee "$WORK/mirror-rule-put.body" >/dev/null
wait_for_api_jq \
    "/api/v1/admin/mirror/rules/$MIRROR_RULE" \
    "$WORK/mirror-rule-public.json" \
    '.id == "'"$MIRROR_RULE"'" and .outbound_proxy.protocol == "none"' \
    "mirror rule visibility"
curl -fsS -X POST "$(api_url "/api/v1/admin/mirror/rules/$MIRROR_RULE/trigger")" \
    | tee "$WORK/mirror-trigger-job.json" \
    | jq -e '.kind == "mirror" and .rule_id == "'"$MIRROR_RULE"'"' >/dev/null
MIRROR_JOB_ID="$(jq -r '.id' "$WORK/mirror-trigger-job.json")"
wait_for_job "$MIRROR_JOB_ID" "mirror"
jq -e '.status == "Succeeded" and (.tags_synced | index("v1")) and (.tags_synced | index("v2"))' \
    "$WORK/mirror-final-run.json" >/dev/null
pull_oras_file "$(orb_ref "$MIRROR_REPO" "v1")" "$WORK/mirror-v1"
pull_oras_file "$(orb_ref "$MIRROR_REPO" "v2")" "$WORK/mirror-v2"
cmp "$WORK/seed/mirror-v1.txt" "$WORK/mirror-v1/mirror-v1.txt"
cmp "$WORK/seed/mirror-v2.txt" "$WORK/mirror-v2/mirror-v2.txt"
curl -fsS "$(api_url "/api/v1/repositories/$MIRROR_REPO/manifests")" \
    | tee "$WORK/mirror-manifests.json" \
    | jq -e --arg v1 "$MIRROR_V1_DIGEST" --arg v2 "$MIRROR_V2_DIGEST" \
        '(.manifests[] | select(.digest == $v1 and (.tags | index("v1")))) and (.manifests[] | select(.digest == $v2 and (.tags | index("v2"))))' >/dev/null

log "Create proxy cache and verify pull-through plus local cached hit"
cat > "$WORK/cache-rule.json" <<JSON
{
  "id": "$CACHE_RULE",
  "local_prefix": "$CACHE_REPO",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": "$CACHE_UPSTREAM_REPO",
  "warm_filters": [{ "type": "none" }],
  "warm_schedule": null,
  "plain_http": true,
  "outbound_proxy": { "protocol": "none" },
  "username": null,
  "password": null,
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/proxy-cache/$CACHE_RULE")" "$WORK/cache-rule.json" \
    | tee "$WORK/cache-rule-put.body" >/dev/null
wait_for_api_jq \
    "/api/v1/admin/proxy-cache/$CACHE_RULE" \
    "$WORK/cache-rule-public.json" \
    '.id == "'"$CACHE_RULE"'" and .outbound_proxy.protocol == "none"' \
    "proxy cache visibility"
pull_oras_file "$(orb_ref "$CACHE_REPO" "v1")" "$WORK/cache-first"
cmp "$WORK/seed/cache-v1.txt" "$WORK/cache-first/cache-v1.txt"
curl -fsS "$(api_url "/api/v1/repositories/$CACHE_REPO/manifests")" \
    | tee "$WORK/cache-manifests.json" \
    | jq -e --arg d "$CACHE_V1_DIGEST" '.manifests[] | select(.digest == $d and (.tags | index("v1")))' >/dev/null
docker pause "$UPSTREAM_NAME" >/dev/null
pull_oras_file "$(orb_ref "$CACHE_REPO" "v1")" "$WORK/cache-second"
docker unpause "$UPSTREAM_NAME" >/dev/null
cmp "$WORK/seed/cache-v1.txt" "$WORK/cache-second/cache-v1.txt"

log "Verify Docker proxy-cache manifest-list pull-through for root prefix"
cat > "$WORK/docker-root-cache-rule.json" <<JSON
{
  "id": "$DOCKER_ROOT_CACHE_RULE",
  "local_prefix": "$DOCKER_ROOT_REPO",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": " / ",
  "warm_filters": [{ "type": "none" }],
  "warm_schedule": null,
  "plain_http": true,
  "outbound_proxy": { "protocol": "none" },
  "username": null,
  "password": null,
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/proxy-cache/$DOCKER_ROOT_CACHE_RULE")" "$WORK/docker-root-cache-rule.json" \
    | tee "$WORK/docker-root-cache-rule-put.body" >/dev/null
wait_for_api_jq \
    "/api/v1/admin/proxy-cache/$DOCKER_ROOT_CACHE_RULE" \
    "$WORK/docker-root-cache-rule-public.json" \
    '.id == "'"$DOCKER_ROOT_CACHE_RULE"'" and .upstream_prefix == null and .outbound_proxy.protocol == "none"' \
    "docker root proxy cache visibility"
ROOT_HEAD_DIGEST="$(manifest_head_digest \
    "$(api_url "/v2/$DOCKER_ROOT_LOCAL_REPO/manifests/$DOCKER_TAG")" \
    "$WORK/docker-root-head.headers")"
test "$ROOT_HEAD_DIGEST" = "$UPSTREAM_DOCKER_INDEX_DIGEST"
get_manifest_list \
    "$(api_url "/v2/$DOCKER_ROOT_LOCAL_REPO/manifests/$DOCKER_TAG")" \
    "$WORK/docker-root-index.json" \
    | jq -e --arg arch "$DOCKER_ARCH" '.manifests[] | select(.platform.os == "linux" and .platform.architecture == $arch)' >/dev/null
curl -fsS "$(api_url "/api/v1/repositories/$DOCKER_ROOT_LOCAL_REPO/manifests")" \
    | tee "$WORK/docker-root-manifests.json" \
    | jq -e --arg d "$UPSTREAM_DOCKER_INDEX_DIGEST" '.manifests[] | select(.digest == $d and (.tags | index("'"$DOCKER_TAG"'")))' >/dev/null
docker pull "$REGISTRY/$DOCKER_ROOT_LOCAL_REPO:$DOCKER_TAG" \
    | tee "$WORK/docker-root-pull.log"
docker image rm "$REGISTRY/$DOCKER_ROOT_LOCAL_REPO:$DOCKER_TAG" >/dev/null 2>&1 || true
docker pause "$UPSTREAM_NAME" >/dev/null
docker pull "$REGISTRY/$DOCKER_ROOT_LOCAL_REPO:$DOCKER_TAG" \
    | tee "$WORK/docker-root-cached-pull.log"
docker unpause "$UPSTREAM_NAME" >/dev/null

log "Verify Docker proxy-cache manifest-list pull-through for /library/ prefix"
cat > "$WORK/docker-library-cache-rule.json" <<JSON
{
  "id": "$DOCKER_LIBRARY_CACHE_RULE",
  "local_prefix": "$DOCKER_LIBRARY_REPO",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": " /library/ ",
  "warm_filters": [{ "type": "none" }],
  "warm_schedule": null,
  "plain_http": true,
  "outbound_proxy": { "protocol": "none" },
  "username": null,
  "password": null,
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/proxy-cache/$DOCKER_LIBRARY_CACHE_RULE")" "$WORK/docker-library-cache-rule.json" \
    | tee "$WORK/docker-library-cache-rule-put.body" >/dev/null
wait_for_api_jq \
    "/api/v1/admin/proxy-cache/$DOCKER_LIBRARY_CACHE_RULE" \
    "$WORK/docker-library-cache-rule-public.json" \
    '.id == "'"$DOCKER_LIBRARY_CACHE_RULE"'" and .upstream_prefix == "library" and .outbound_proxy.protocol == "none"' \
    "docker library proxy cache visibility"
LIBRARY_HEAD_DIGEST="$(manifest_head_digest \
    "$(api_url "/v2/$DOCKER_LIBRARY_LOCAL_REPO/manifests/$DOCKER_TAG")" \
    "$WORK/docker-library-head.headers")"
test "$LIBRARY_HEAD_DIGEST" = "$UPSTREAM_DOCKER_INDEX_DIGEST"
get_manifest_list \
    "$(api_url "/v2/$DOCKER_LIBRARY_LOCAL_REPO/manifests/$DOCKER_TAG")" \
    "$WORK/docker-library-index.json" \
    | jq -e --arg arch "$DOCKER_ARCH" '.manifests[] | select(.platform.os == "linux" and .platform.architecture == $arch)' >/dev/null
curl -fsS "$(api_url "/api/v1/repositories/$DOCKER_LIBRARY_LOCAL_REPO/manifests")" \
    | tee "$WORK/docker-library-manifests.json" \
    | jq -e --arg d "$UPSTREAM_DOCKER_INDEX_DIGEST" '.manifests[] | select(.digest == $d and (.tags | index("'"$DOCKER_TAG"'")))' >/dev/null
docker pull "$REGISTRY/$DOCKER_LIBRARY_LOCAL_REPO:$DOCKER_TAG" \
    | tee "$WORK/docker-library-pull.log"

log "Create proxy cache warm rule and verify warm job"
cat > "$WORK/warm-rule.json" <<JSON
{
  "id": "$WARM_RULE",
  "local_prefix": "$WARM_REPO",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": "$WARM_UPSTREAM_REPO",
  "warm_filters": [{ "type": "pattern", "pattern": "warm" }],
  "warm_schedule": null,
  "plain_http": true,
  "outbound_proxy": { "protocol": "none" },
  "username": null,
  "password": null,
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/proxy-cache/$WARM_RULE")" "$WORK/warm-rule.json" \
    | tee "$WORK/warm-rule-put.body" >/dev/null
curl -fsS -X POST "$(api_url "/api/v1/admin/proxy-cache/$WARM_RULE/warm")" \
    | tee "$WORK/warm-trigger-job.json" \
    | jq -e '.kind == "proxy_cache" and .rule_id == "'"$WARM_RULE"'"' >/dev/null
WARM_JOB_ID="$(jq -r '.id' "$WORK/warm-trigger-job.json")"
wait_for_job "$WARM_JOB_ID" "warm"
jq -e '.status == "Succeeded" and (.tags_synced | index("warm"))' "$WORK/warm-final-run.json" >/dev/null
pull_oras_file "$(orb_ref "$WARM_REPO" "warm")" "$WORK/warm-pull"
cmp "$WORK/seed/warm.txt" "$WORK/warm-pull/warm.txt"

log "Create and trigger push mirror rule"
push_oras_file "$REGISTRY" "$PUSH_SRC_REPO" "release" \
    "$WORK/seed/push-release.txt" "application/vnd.layerhouse.qa.push.v1+txt" \
    "$WORK/orb-push-src.json"
PUSH_SRC_DIGEST="$(jq -r '.digest' "$WORK/orb-push-src.json")"
cat > "$WORK/push-rule.json" <<JSON
{
  "id": "$PUSH_RULE",
  "direction": "push",
  "local_prefix": "$PUSH_SRC_REPO",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": "$PUSH_DST_UPSTREAM_REPO",
  "schedule": null,
  "strategy": { "type": "pattern", "pattern": "release" },
  "plain_http": true,
  "outbound_proxy": { "protocol": "none" },
  "username": null,
  "password": null,
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/mirror/rules/$PUSH_RULE")" "$WORK/push-rule.json" \
    | tee "$WORK/push-rule-put.body" >/dev/null
curl -fsS -X POST "$(api_url "/api/v1/admin/mirror/rules/$PUSH_RULE/trigger")" \
    | tee "$WORK/push-trigger-job.json" \
    | jq -e '.kind == "mirror" and .rule_id == "'"$PUSH_RULE"'"' >/dev/null
PUSH_JOB_ID="$(jq -r '.id' "$WORK/push-trigger-job.json")"
wait_for_job "$PUSH_JOB_ID" "push"
jq -e '.status == "Succeeded" and (.tags_synced | index("release"))' "$WORK/push-final-run.json" >/dev/null
oras pull --plain-http --no-tty -o "$WORK/push-upstream-pull" \
    "$(upstream_host_ref "$PUSH_DST_UPSTREAM_REPO" "release")"
cmp "$WORK/seed/push-release.txt" "$WORK/push-upstream-pull/push-release.txt"
printf 'PUSH_SRC_DIGEST=%s\n' "$PUSH_SRC_DIGEST" | tee -a "$WORK/summary.env"

log "Validate outbound proxy protocol handling and secret redaction"
cat > "$WORK/proxy-validation-rule.json" <<JSON
{
  "id": "$PROXY_VALIDATION_RULE",
  "direction": "pull",
  "local_prefix": "qa/proxy-validation-$RUN_ID",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": "$MIRROR_UPSTREAM_REPO",
  "schedule": null,
  "strategy": { "type": "all" },
  "plain_http": true,
  "outbound_proxy": {
    "protocol": "http",
    "url": "http://127.0.0.1:3128",
    "username": "proxy-user",
    "password": "proxy-pass"
  },
  "username": null,
  "password": null,
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/mirror/rules/$PROXY_VALIDATION_RULE")" "$WORK/proxy-validation-rule.json" \
    | tee "$WORK/proxy-validation-put.body" >/dev/null
wait_for_api_jq \
    "/api/v1/admin/mirror/rules/$PROXY_VALIDATION_RULE" \
    "$WORK/proxy-validation-public.json" \
    '.outbound_proxy.protocol == "http" and .outbound_proxy.url == "http://127.0.0.1:3128" and .outbound_proxy.password_configured == true and (.outbound_proxy.password | not)' \
    "HTTP proxy validation rule visibility"

cat > "$WORK/proxy-https-rule.json" <<JSON
{
  "id": "$PROXY_VALIDATION_RULE",
  "direction": "pull",
  "local_prefix": "qa/proxy-validation-$RUN_ID",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": "$MIRROR_UPSTREAM_REPO",
  "schedule": null,
  "strategy": { "type": "all" },
  "plain_http": true,
  "outbound_proxy": {
    "protocol": "https",
    "url": "https://127.0.0.1:3129"
  },
  "username": null,
  "password": null,
  "created_at": 0
}
JSON
HTTPS_CODE="$(curl_status "$WORK/proxy-https.response" \
    -X PUT \
    -H 'Content-Type: application/json' \
    --data-binary "@$WORK/proxy-https-rule.json" \
    "$(api_url "/api/v1/admin/mirror/rules/$PROXY_VALIDATION_RULE")")"
printf 'HTTPS_PROXY_HTTP=%s\n' "$HTTPS_CODE" | tee -a "$WORK/summary.env"
[[ "$HTTPS_CODE" == "400" ]]
grep -qi 'aioduct' "$WORK/proxy-https.response"

cat > "$WORK/proxy-direct-rule.json" <<JSON
{
  "id": "$PROXY_DIRECT_RULE",
  "direction": "pull",
  "local_prefix": "qa/proxy-direct-$RUN_ID",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": "$MIRROR_UPSTREAM_REPO",
  "schedule": null,
  "strategy": { "type": "all" },
  "plain_http": true,
  "outbound_proxy": {
    "protocol": "none",
    "url": "http://127.0.0.1:3128",
    "username": "stale-user",
    "password": "stale-pass"
  },
  "username": null,
  "password": null,
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/mirror/rules/$PROXY_DIRECT_RULE")" "$WORK/proxy-direct-rule.json" \
    | tee "$WORK/proxy-direct-put.body" >/dev/null
wait_for_api_jq \
    "/api/v1/admin/mirror/rules/$PROXY_DIRECT_RULE" \
    "$WORK/proxy-direct-public.json" \
    '.outbound_proxy.protocol == "none" and .outbound_proxy.url == null and .outbound_proxy.username_configured == false and .outbound_proxy.password_configured == false' \
    "direct proxy rule visibility"

cat > "$WORK/proxy-secret-rule.json" <<JSON
{
  "id": "$PROXY_SECRET_RULE",
  "direction": "pull",
  "local_prefix": "qa/proxy-secret-$RUN_ID",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": "$MIRROR_UPSTREAM_REPO",
  "schedule": null,
  "strategy": { "type": "all" },
  "plain_http": true,
  "outbound_proxy": { "protocol": "none" },
  "username": "upstream-user",
  "password": "upstream-pass",
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/mirror/rules/$PROXY_SECRET_RULE")" "$WORK/proxy-secret-rule.json" \
    | tee "$WORK/proxy-secret-put.body" >/dev/null
wait_for_api_jq \
    "/api/v1/admin/mirror/rules/$PROXY_SECRET_RULE" \
    "$WORK/proxy-secret-public.json" \
    '.username_configured == true and .password_configured == true and (.password | not)' \
    "secret rule visibility"

cat > "$WORK/proxy-secret-cache.json" <<JSON
{
  "id": "$PROXY_SECRET_CACHE",
  "local_prefix": "qa/proxy-secret-cache-$RUN_ID",
  "upstream_registry": "$UPSTREAM_ORB_REGISTRY",
  "upstream_prefix": "$CACHE_UPSTREAM_REPO",
  "warm_filters": [{ "type": "none" }],
  "warm_schedule": null,
  "plain_http": true,
  "outbound_proxy": {
    "protocol": "socks5",
    "url": "socks5h://127.0.0.1:1080",
    "username": "proxy-user",
    "password": "proxy-pass"
  },
  "username": "cache-user",
  "password": "cache-pass",
  "created_at": 0
}
JSON
json_put "$(api_url "/api/v1/admin/proxy-cache/$PROXY_SECRET_CACHE")" "$WORK/proxy-secret-cache.json" \
    | tee "$WORK/proxy-secret-cache-put.body" >/dev/null
wait_for_api_jq \
    "/api/v1/admin/proxy-cache/$PROXY_SECRET_CACHE" \
    "$WORK/proxy-secret-cache-public.json" \
    '.username_configured == true and .password_configured == true and .outbound_proxy.protocol == "socks5" and .outbound_proxy.password_configured == true and (.password | not) and (.outbound_proxy.password | not)' \
    "secret proxy cache visibility"

log "Final job and cluster health"
curl -fsS "$(api_url /api/v1/admin/mirror/jobs)" \
    | tee "$WORK/mirror-jobs.json" \
    | jq -e --arg mirror "$MIRROR_JOB_ID" --arg warm "$WARM_JOB_ID" --arg push "$PUSH_JOB_ID" \
        '([.[].id] | index($mirror) and index($warm) and index($push))' >/dev/null
curl -fsS "$(api_url /api/v1/admin/cluster/status)" \
    | tee "$WORK/cluster-after-full.json" \
    | jq '{leader_id, quorum, healthy_voters}' \
    | tee "$WORK/cluster-after.json"
jq -e '.leader_id != null and .healthy_voters >= .quorum' "$WORK/cluster-after-full.json" >/dev/null
