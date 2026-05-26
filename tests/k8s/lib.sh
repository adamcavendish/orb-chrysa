need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $1" >&2
        exit 127
    fi
}

record() {
    "$@" 2>&1 | tee -a "$WORK/commands.log"
}

retry() {
    local attempts="${RETRY_ATTEMPTS:-12}"
    local delay="${RETRY_DELAY_SECONDS:-5}"
    local status=0

    for attempt in $(seq 1 "$attempts"); do
        if "$@"; then
            return 0
        fi
        status=$?
        if [ "$attempt" -eq "$attempts" ]; then
            return "$status"
        fi
        echo "WARN: command failed, retrying in ${delay}s ($attempt/$attempts): $*" >&2
        sleep "$delay"
    done
}

record_retry() {
    retry "$@" 2>&1 | tee -a "$WORK/commands.log"
}

base64_one_line() {
    base64 | tr -d '\n'
}

local_curl() {
    curl --noproxy '*' "$@"
}

refresh_ci_bot_token() {
    local api_token
    local token_resp
    local ci_token
    local token_b64
    local api_token_b64
    local patch

    api_token="$(kubectl -n "$NAMESPACE" get secret orb-chrysa-test-auth -o jsonpath='{.data.ci_bot_api_token}' | base64 -d)"
    token_resp="$(local_curl -skf \
        -H "Content-Type: application/x-www-form-urlencoded" \
        --data-urlencode "grant_type=urn:ietf:params:oauth:grant-type:token-exchange" \
        --data-urlencode "client_id=orb-chrysa" \
        --data-urlencode "subject_token=$api_token" \
        --data-urlencode "subject_token_type=urn:ietf:params:oauth:token-type:access_token" \
        --data-urlencode "audience=orb-chrysa" \
        --data-urlencode "scope=openid profile email groups oci_admin" \
        "$KANIDM_URL/oauth2/token")"
    printf '%s\n' "$token_resp" > "$WORK/ci-bot-token-refresh-response.json"

    ci_token="$(printf '%s\n' "$token_resp" | jq -r '.id_token // empty')"
    if [ -z "$ci_token" ] || [ "$ci_token" = "null" ]; then
        echo "ERROR: failed to refresh ci-bot token" >&2
        cat "$WORK/ci-bot-token-refresh-response.json" >&2
        return 1
    fi

    token_b64="$(printf '%s' "$ci_token" | base64_one_line)"
    api_token_b64="$(printf '%s' "$api_token" | base64_one_line)"
    patch="$(jq -n \
        --arg token "$token_b64" \
        --arg api_token "$api_token_b64" \
        '{data:{ci_bot_token:$token, ci_bot_api_token:$api_token}}')"
    kubectl -n "$NAMESPACE" patch secret orb-chrysa-test-auth --type merge -p "$patch" >/dev/null

    printf '%s' "$ci_token"
}

create_pat() {
    local ci_token="$1"
    local name="${2:-tilt-smoke}"
    local resp="${3:-$WORK/pat-response.json}"

    local_curl --cacert "$CA" -fsS \
        -H "Authorization: Bearer $ci_token" \
        -H "Content-Type: application/json" \
        -d "{\"name\":\"$name\",\"scopes\":[\"repository:*:*\"],\"expires_in_days\":1}" \
        "https://$REGISTRY_ENDPOINT/api/v1/tokens" | tee "$resp" >/dev/null
    jq -r '.token' "$resp"
}

resolve_smoke_base_image() {
    if docker image inspect "$SMOKE_BASE_IMAGE" >/dev/null 2>&1; then
        printf '%s' "$SMOKE_BASE_IMAGE"
        return
    fi

    if [ -n "$SMOKE_BASE_IMAGE_FALLBACK" ] \
        && docker image inspect "$SMOKE_BASE_IMAGE_FALLBACK" >/dev/null 2>&1; then
        echo "WARN: using local smoke base image fallback: $SMOKE_BASE_IMAGE_FALLBACK" >&2
        printf '%s' "$SMOKE_BASE_IMAGE_FALLBACK"
        return
    fi

    printf '%s' "$SMOKE_BASE_IMAGE"
}

kind_containerd_push() {
    local image="$1"
    local password="$2"
    local node
    local image_tar

    node="$(kind get nodes --name "$CLUSTER" | head -1)"
    image_tar="$WORK/$(echo "$image" | tr '/:' '__').tar"

    echo "=== Falling back to kind containerd push for $image on $node ==="
    record docker save "$image" -o "$image_tar"
    echo "docker exec -i $node sh -c 'cat > /tmp/orb-chrysa-push-image.tar' < $image_tar" | tee -a "$WORK/commands.log"
    docker exec -i "$node" sh -c 'cat > /tmp/orb-chrysa-push-image.tar' < "$image_tar"
    record docker exec "$node" ls -lh /tmp/orb-chrysa-push-image.tar
    record docker exec "$node" ctr -n k8s.io images import /tmp/orb-chrysa-push-image.tar
    record docker exec "$node" ctr -n k8s.io images push --user "ci-bot:$password" "$image"
}

verify_node_pull() {
    local image="$1"
    local password="$2"
    local label="${3:-}"
    local node

    for node in $(kind get nodes --name "$CLUSTER"); do
        record docker exec "$node" crictl pull --creds "ci-bot:$password" "$image"
    done
    if [ -n "$label" ]; then
        echo "verified node pulls for $label" | tee -a "$WORK/commands.log"
    fi
}
