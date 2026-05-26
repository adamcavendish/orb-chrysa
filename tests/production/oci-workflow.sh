#!/usr/bin/env bash
# Production-style OCI workflow smoke test for a running orb-chrysa registry.
#
# Requires a live registry, Docker daemon, ORAS, curl, and jq.
# Defaults target the local compose cluster at localhost:5050.
set -euo pipefail

REGISTRY="${REGISTRY:-localhost:5050}"
SCHEME="${SCHEME:-http}"
NODE_PORTS="${NODE_PORTS:-5050 5051 5052}"
EXPECT_BLOB_REDIRECT="${EXPECT_BLOB_REDIRECT:-auto}"
S3_PUBLIC_ENDPOINT="${S3_PUBLIC_ENDPOINT:-http://localhost:9000}"
RUN_ID="${RUN_ID:-$(date +%s)}"
EVIDENCE_ROOT="${EVIDENCE_ROOT:-/tmp}"
WORK="${WORK:-$EVIDENCE_ROOT/orb-oci-$RUN_ID}"

ORAS_REPO="qa/oci-oras-$RUN_ID"
DOCKER_REPO="qa/oci-docker-$RUN_ID"
FOLLOWER_REPO="qa/oci-follower-$RUN_ID"
NEGATIVE_REPO="qa/oci-negative-$RUN_ID"

ORAS_DIGEST=""
DOCKER_DIGEST=""
FOLLOWER_DIGEST=""
DOCKER_IMAGE_TAG="$REGISTRY/$DOCKER_REPO:blue"
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

delete_repo() {
    local repo="$1"
    curl -sS -X DELETE "$(api_url "/api/v1/repositories/$repo")" >/dev/null || true
}

cleanup() {
    local status=$?
    log "Cleanup disposable OCI workflow data"
    delete_repo "$ORAS_REPO"
    delete_repo "$DOCKER_REPO"
    delete_repo "$FOLLOWER_REPO"
    delete_repo "$NEGATIVE_REPO"
    docker image rm "$DOCKER_IMAGE_TAG" >/dev/null 2>&1 || true
    if [ "$status" -eq 0 ]; then
        echo "PASS production OCI workflow. Evidence: $WORK"
    else
        echo "FAIL production OCI workflow. Evidence: $WORK" >&2
    fi
    exit "$status"
}
trap cleanup EXIT

need curl
need jq
need oras
need docker
need cmp
need awk
need grep

mkdir -p "$WORK/oras-pull-tag" "$WORK/oras-pull-digest" "$WORK/follower-pull" "$WORK/dockerctx"
printf 'orb-chrysa OCI workflow payload %s\n' "$RUN_ID" > "$WORK/payload.txt"
printf 'follower-routed payload %s\n' "$RUN_ID" > "$WORK/follower.txt"
printf 'docker payload %s\n' "$RUN_ID" > "$WORK/dockerctx/hello.txt"
printf 'FROM scratch\nCOPY hello.txt /hello.txt\n' > "$WORK/dockerctx/Dockerfile"

cat > "$WORK/summary.env" <<EOF
RUN_ID=$RUN_ID
REGISTRY=$REGISTRY
SCHEME=$SCHEME
WORK=$WORK
ORAS_REPO=$ORAS_REPO
DOCKER_REPO=$DOCKER_REPO
FOLLOWER_REPO=$FOLLOWER_REPO
EXPECT_BLOB_REDIRECT=$EXPECT_BLOB_REDIRECT
S3_PUBLIC_ENDPOINT=$S3_PUBLIC_ENDPOINT
EOF

log "Client versions"
{
    oras version
    docker version --format '{{.Client.Version}} client / {{.Server.Version}} server'
    curl --version | head -1
    jq --version
} | tee "$WORK/client-versions.txt"

log "OCI1 registry liveness and cluster health"
curl -fsS "$(api_url /v2/)" >/dev/null
curl -fsS "$(api_url /api/v1/admin/cluster/status)" \
    | tee "$WORK/cluster-before-full.json" \
    | jq '{leader_id, quorum, healthy_voters}' \
    | tee "$WORK/cluster-before.json"
jq -e '.leader_id != null and .healthy_voters >= .quorum' "$WORK/cluster-before-full.json" >/dev/null

log "OCI2 ORAS push and pull by tag/digest"
(
    cd "$WORK"
    oras push "${ORAS_TRANSPORT_FLAGS[@]}" --no-tty --format json \
        --artifact-type application/vnd.orb-chrysa.qa.v1 \
        "$REGISTRY/$ORAS_REPO:alpha,beta" \
        "payload.txt:application/vnd.orb-chrysa.qa.payload.v1+txt"
) | tee "$WORK/oras-push.json"
ORAS_DIGEST="$(jq -r '.digest' "$WORK/oras-push.json")"
[[ "$ORAS_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]
printf 'ORAS_DIGEST=%s\n' "$ORAS_DIGEST" | tee -a "$WORK/summary.env"
oras pull "${ORAS_TRANSPORT_FLAGS[@]}" --no-tty -o "$WORK/oras-pull-tag" "$REGISTRY/$ORAS_REPO:alpha"
oras pull "${ORAS_TRANSPORT_FLAGS[@]}" --no-tty -o "$WORK/oras-pull-digest" "$REGISTRY/$ORAS_REPO@$ORAS_DIGEST"
cmp "$WORK/payload.txt" "$WORK/oras-pull-tag/payload.txt"
cmp "$WORK/payload.txt" "$WORK/oras-pull-digest/payload.txt"

log "OCI4 tag list, manifest reads, blob reads, and range reads"
curl -fsS "$(api_url "/v2/$ORAS_REPO/tags/list")" \
    | tee "$WORK/oras-tags-before.json" \
    | jq -e '.tags | index("alpha") and index("beta")' >/dev/null
curl -fsSI \
    -H 'Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.oci.artifact.manifest.v1+json' \
    "$(api_url "/v2/$ORAS_REPO/manifests/alpha")" \
    | tee "$WORK/oras-head-alpha.headers" >/dev/null
curl -fsS \
    -H 'Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.oci.artifact.manifest.v1+json' \
    "$(api_url "/v2/$ORAS_REPO/manifests/$ORAS_DIGEST")" \
    | tee "$WORK/oras-manifest.json" \
    | jq .mediaType >/dev/null
BLOB_DIGEST="$(jq -r '((.layers // .blobs // []) | first | .digest) // .config.digest // empty' "$WORK/oras-manifest.json")"
[[ "$BLOB_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]
printf 'ORAS_BLOB_DIGEST=%s\n' "$BLOB_DIGEST" | tee -a "$WORK/summary.env"
curl -fsSI "$(api_url "/v2/$ORAS_REPO/blobs/$BLOB_DIGEST")" \
    | tee "$WORK/blob-head.headers" >/dev/null
BLOB_FULL_URL="$(api_url "/v2/$ORAS_REPO/blobs/$BLOB_DIGEST")"
BLOB_FULL_CODE="$(
    curl -sS -D "$WORK/blob-full.headers" -o "$WORK/blob-full.bin" -w '%{http_code}' \
        "$BLOB_FULL_URL"
)"
printf 'BLOB_FULL_HTTP=%s\n' "$BLOB_FULL_CODE" | tee -a "$WORK/summary.env"
if [ "$BLOB_FULL_CODE" = "307" ]; then
    BLOB_REDIRECT_LOCATION="$(
        awk -F': ' 'tolower($1)=="location" {gsub("\r", "", $2); print $2}' \
            "$WORK/blob-full.headers" | tail -1
    )"
    [[ "$BLOB_REDIRECT_LOCATION" == "$S3_PUBLIC_ENDPOINT"* ]]
    grep -qi '^docker-content-digest:' "$WORK/blob-full.headers"
    grep -qi '^accept-ranges: bytes' "$WORK/blob-full.headers"
    curl -fsSL "$BLOB_REDIRECT_LOCATION" -o "$WORK/blob-full.bin"
    printf 'BLOB_REDIRECT=1\nBLOB_REDIRECT_LOCATION=%s\n' "$BLOB_REDIRECT_LOCATION" \
        | tee -a "$WORK/summary.env"
elif [ "$BLOB_FULL_CODE" = "200" ]; then
    printf 'BLOB_REDIRECT=0\n' | tee -a "$WORK/summary.env"
else
    echo "unexpected blob GET status: $BLOB_FULL_CODE" >&2
    exit 1
fi
if [ "$EXPECT_BLOB_REDIRECT" = "1" ] && [ "$BLOB_FULL_CODE" != "307" ]; then
    echo "expected blob redirect but received HTTP $BLOB_FULL_CODE" >&2
    exit 1
fi
if [ "$EXPECT_BLOB_REDIRECT" = "0" ] && [ "$BLOB_FULL_CODE" = "307" ]; then
    echo "expected proxied blob body but received redirect" >&2
    exit 1
fi
cmp "$WORK/payload.txt" "$WORK/blob-full.bin"
curl -fsS -i -H 'Range: bytes=0-9' "$(api_url "/v2/$ORAS_REPO/blobs/$BLOB_DIGEST")" \
    | tee "$WORK/blob-range.response" >/dev/null
grep -q '^HTTP/1.1 206' "$WORK/blob-range.response"

log "OCI5 and OCI10 dashboard digest consistency and copy-value invariants"
curl -fsS "$(api_url /api/v1/repositories?q=qa/oci)" \
    | tee "$WORK/repositories-qa.json" \
    | jq -e --arg repo "$ORAS_REPO" '.repositories[] | select(.name == $repo)' >/dev/null
DASH_DIGEST="$(
    curl -fsS "$(api_url "/api/v1/repositories/$ORAS_REPO/manifests")" \
        | tee "$WORK/oras-dashboard-manifests.json" \
        | jq -r --arg d "$ORAS_DIGEST" '.manifests[] | select(.digest == $d and (.tags | index("alpha")) and (.tags | index("beta"))) | .digest'
)"
[[ "$DASH_DIGEST" == "$ORAS_DIGEST" ]]
curl -fsS "$(api_url "/api/v1/repositories/$ORAS_REPO/manifests/$ORAS_DIGEST")" \
    | tee "$WORK/oras-dashboard-detail.json" \
    | jq -e --arg d "$ORAS_DIGEST" '.digest == $d and (.digest | length == 71)' >/dev/null
curl -fsS "$(api_url "/api/v1/repositories/$ORAS_REPO/manifests/$ORAS_DIGEST/raw")" \
    | tee "$WORK/oras-dashboard-raw.json" \
    | jq .mediaType >/dev/null

log "OCI3 Docker image build, push, pull, and inspect"
docker build -q -t "$DOCKER_IMAGE_TAG" "$WORK/dockerctx" | tee "$WORK/docker-build-image-id.txt"
docker push "$DOCKER_IMAGE_TAG" | tee "$WORK/docker-push.log"
DOCKER_DIGEST="$(
    curl -fsSI \
        -H 'Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' \
        "$(api_url "/v2/$DOCKER_REPO/manifests/blue")" \
        | awk -F': ' 'tolower($1)=="docker-content-digest" {gsub("\r", "", $2); print $2}' \
        | tail -1
)"
[[ "$DOCKER_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]
printf 'DOCKER_DIGEST=%s\n' "$DOCKER_DIGEST" | tee -a "$WORK/summary.env"
docker image rm "$DOCKER_IMAGE_TAG" >/dev/null
docker pull "$DOCKER_IMAGE_TAG" | tee "$WORK/docker-pull.log"
docker image inspect "$DOCKER_IMAGE_TAG" >/dev/null
curl -fsS "$(api_url "/api/v1/repositories/$DOCKER_REPO/manifests")" \
    | tee "$WORK/docker-dashboard-manifests.json" \
    | jq -e --arg d "$DOCKER_DIGEST" '.manifests[] | select(.digest == $d and (.tags | index("blue")))' >/dev/null

log "OCI9 follower-routed ORAS write and all-node reads"
LEADER_ID="$(jq -r '.leader_id' "$WORK/cluster-before-full.json")"
FOLLOWER_PORT=""
for port in $NODE_PORTS; do
    curl -fsS "$SCHEME://localhost:$port/api/v1/admin/cluster/status" >/dev/null
    if [ -z "$FOLLOWER_PORT" ] && [ "$port" != "$((5049 + LEADER_ID))" ]; then
        FOLLOWER_PORT="$port"
    fi
done
FOLLOWER_PORT="${FOLLOWER_PORT:-5050}"
(
    cd "$WORK"
    oras push "${ORAS_TRANSPORT_FLAGS[@]}" --no-tty --format json \
        --artifact-type application/vnd.orb-chrysa.qa.v1 \
        "localhost:$FOLLOWER_PORT/$FOLLOWER_REPO:from-follower" \
        "follower.txt:application/vnd.orb-chrysa.qa.payload.v1+txt"
) | tee "$WORK/follower-push.json"
FOLLOWER_DIGEST="$(jq -r '.digest' "$WORK/follower-push.json")"
[[ "$FOLLOWER_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]
printf 'FOLLOWER_DIGEST=%s\nFOLLOWER_PORT=%s\n' "$FOLLOWER_DIGEST" "$FOLLOWER_PORT" | tee -a "$WORK/summary.env"
oras pull "${ORAS_TRANSPORT_FLAGS[@]}" --no-tty -o "$WORK/follower-pull" "$REGISTRY/$FOLLOWER_REPO@$FOLLOWER_DIGEST"
cmp "$WORK/follower.txt" "$WORK/follower-pull/follower.txt"
for port in $NODE_PORTS; do
    curl -fsS "$SCHEME://localhost:$port/v2/$FOLLOWER_REPO/tags/list" \
        | jq -e '.tags | index("from-follower")' >/dev/null
    curl -fsS "$SCHEME://localhost:$port/api/v1/admin/cluster/status" \
        | jq -c --arg port "$port" '{port: $port, leader_id, healthy_voters}'
done | tee "$WORK/node-read-smoke.jsonl"

log "OCI6 tag delete and untagged digest behavior"
curl -fsS -X DELETE "$(api_url "/api/v1/repositories/$ORAS_REPO/manifests/$ORAS_DIGEST/tags/beta")" \
    -o "$WORK/delete-beta.body" \
    -w '%{http_code}\n' \
    | tee "$WORK/delete-beta.status" \
    | grep -q '^204$'
curl -fsS "$(api_url "/v2/$ORAS_REPO/tags/list")" \
    | tee "$WORK/oras-tags-after-beta.json" \
    | jq -e '.tags | index("alpha") and (index("beta") | not)' >/dev/null
curl -fsS -X DELETE "$(api_url "/api/v1/repositories/$ORAS_REPO/manifests/$ORAS_DIGEST/tags/alpha")" \
    -o "$WORK/delete-alpha.body" \
    -w '%{http_code}\n' \
    | tee "$WORK/delete-alpha.status" \
    | grep -q '^204$'
curl -fsS "$(api_url "/api/v1/repositories/$ORAS_REPO/manifests")" \
    | tee "$WORK/oras-dashboard-untagged.json" \
    | jq -e --arg d "$ORAS_DIGEST" '.manifests[] | select(.digest == $d and (.tags | length == 0))' >/dev/null

log "OCI7 digest delete"
curl -fsS -X DELETE "$(api_url "/api/v1/repositories/$ORAS_REPO/manifests/$ORAS_DIGEST")" \
    | tee "$WORK/delete-oras-digest.json" \
    | jq -e '.deleted_manifests >= 1 and .deleted_tags == 0' >/dev/null
ORAS_AFTER_CODE="$(
    curl -sS -o "$WORK/oras-after-delete.body" -w '%{http_code}' \
        "$(api_url "/v2/$ORAS_REPO/manifests/$ORAS_DIGEST")"
)"
printf 'ORAS_AFTER_DELETE_HTTP=%s\n' "$ORAS_AFTER_CODE" | tee -a "$WORK/summary.env"
[[ "$ORAS_AFTER_CODE" == "404" ]]

log "OCI8 repository cleanup"
curl -fsS -X DELETE "$(api_url "/api/v1/repositories/$DOCKER_REPO")" \
    | tee "$WORK/delete-docker-repo.json" \
    | jq -e '.deleted_manifests >= 1 and .deleted_tags >= 1' >/dev/null
curl -fsS -X DELETE "$(api_url "/api/v1/repositories/$FOLLOWER_REPO")" \
    | tee "$WORK/delete-follower-repo.json" \
    | jq -e '.deleted_manifests >= 1 and .deleted_tags >= 1' >/dev/null
curl -fsS -X DELETE "$(api_url "/api/v1/repositories/$ORAS_REPO")" \
    | tee "$WORK/delete-oras-repo.json" >/dev/null
curl -fsS "$(api_url /api/v1/repositories?q=qa/oci)" \
    | tee "$WORK/repositories-after-cleanup.json" \
    | jq -e --arg oras "$ORAS_REPO" --arg docker "$DOCKER_REPO" --arg follower "$FOLLOWER_REPO" \
        '([.repositories[].name] | index($oras) == null and index($docker) == null and index($follower) == null)' >/dev/null
docker image rm "$DOCKER_IMAGE_TAG" >/dev/null 2>&1 || true

log "OCI11 missing blob manifest rejection"
MISSING='sha256:0000000000000000000000000000000000000000000000000000000000000000'
cat > "$WORK/missing-blob-manifest.json" <<JSON
{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":"$MISSING","size":2},"layers":[]}
JSON
NEG_CODE="$(
    curl -sS -o "$WORK/missing-blob.response" -w '%{http_code}' \
        -X PUT \
        -H 'Content-Type: application/vnd.oci.image.manifest.v1+json' \
        --data-binary "@$WORK/missing-blob-manifest.json" \
        "$(api_url "/v2/$NEGATIVE_REPO/manifests/bad")"
)"
printf 'MISSING_BLOB_HTTP=%s\n' "$NEG_CODE" | tee -a "$WORK/summary.env"
[[ "$NEG_CODE" =~ ^(400|404)$ ]]
grep -q 'BLOB_UNKNOWN' "$WORK/missing-blob.response"

log "Final cluster health"
curl -fsS "$(api_url /api/v1/admin/cluster/status)" \
    | tee "$WORK/cluster-after-full.json" \
    | jq '{leader_id, quorum, healthy_voters}' \
    | tee "$WORK/cluster-after.json"
jq -e '.leader_id != null and .healthy_voters >= .quorum' "$WORK/cluster-after-full.json" >/dev/null
