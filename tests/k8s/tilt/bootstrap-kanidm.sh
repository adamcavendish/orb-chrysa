#!/usr/bin/env bash
set -euo pipefail

KANIDM_NAMESPACE="${KANIDM_NAMESPACE:-kanidm}"
ORB_NAMESPACE="${ORB_NAMESPACE:-orb-chrysa-tilt}"
KANIDM_HOST_PORT="${KANIDM_HOST_PORT:-8443}"
KANIDM_URL="${KANIDM_URL:-https://localhost:$KANIDM_HOST_PORT}"
REGISTRY_ENDPOINT="${REGISTRY_ENDPOINT:-localhost:32050}"
WORK="${WORK:-target/tilt/kanidm}"
KANIDM_API_TOKEN_EXPIRY="${KANIDM_API_TOKEN_EXPIRY:-1893456000}"

umask 077
mkdir -p "$WORK"
chmod -R go-rwx "$WORK"

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $1" >&2
        exit 127
    fi
}

need kubectl
need curl
need base64
need jq

CURL="curl -sk"

kubectl -n "$KANIDM_NAMESPACE" rollout status deploy/kanidm --timeout=240s

echo "=== Waiting for Kanidm public endpoint ==="
for _ in $(seq 1 90); do
    if $CURL -f "$KANIDM_URL/status" >/dev/null 2>&1; then
        break
    fi
    sleep 2
done
$CURL -f "$KANIDM_URL/status" >/dev/null

kubectl -n "$KANIDM_NAMESPACE" wait \
    --for=condition=Ready \
    pod \
    -l app=kanidm \
    --timeout=240s
POD="$(kubectl -n "$KANIDM_NAMESPACE" get pod \
    -l app=kanidm \
    --field-selector=status.phase=Running \
    -o jsonpath='{.items[0].metadata.name}')"

recover_password() {
    local account="$1"
    local output
    output="$(kubectl -n "$KANIDM_NAMESPACE" exec "$POD" -- kanidmd recover-account "$account" -c /data/server.toml 2>&1 || true)"
    printf '%s\n' "$output" > "$WORK/recover-$account.log"
    printf '%s\n' "$output" | grep -oE 'new_password: [^ ]+' | head -1 | sed 's/new_password: //' | tr -d '"'
}

echo "=== Recovering idm_admin password ==="
ADMIN_PW="$(recover_password idm_admin)"
if [ -z "$ADMIN_PW" ]; then
    echo "ERROR: could not recover idm_admin password" >&2
    cat "$WORK/recover-idm_admin.log" >&2
    exit 1
fi

echo "=== Authenticating as idm_admin ==="
$CURL -D "$WORK/auth-headers" \
    -H "Content-Type: application/json" \
    -d '{"step":{"init":"idm_admin"}}' \
    "$KANIDM_URL/v1/auth" >/dev/null
SESSION_COOKIE="$(grep -i 'set-cookie:' "$WORK/auth-headers" | sed 's/[Ss]et-[Cc]ookie: //' | cut -d';' -f1 | tr -d '\r' | head -1)"

$CURL \
    -H "Content-Type: application/json" \
    -H "Cookie: $SESSION_COOKIE" \
    -d '{"step":{"begin":"password"}}' \
    "$KANIDM_URL/v1/auth" >/dev/null

TOKEN_RESP="$($CURL \
    -H "Content-Type: application/json" \
    -H "Cookie: $SESSION_COOKIE" \
    -d "{\"step\":{\"cred\":{\"password\":\"$ADMIN_PW\"}}}" \
    "$KANIDM_URL/v1/auth")"
printf '%s\n' "$TOKEN_RESP" > "$WORK/admin-token-response.json"
BEARER="$(printf '%s\n' "$TOKEN_RESP" | grep -o '"success":"[^"]*"' | cut -d'"' -f4 || true)"
if [ -z "$BEARER" ]; then
    echo "ERROR: failed to get Kanidm bearer token" >&2
    cat "$WORK/admin-token-response.json" >&2
    exit 1
fi
AUTH="Authorization: Bearer $BEARER"

echo "=== Creating users, groups, and service accounts ==="
for user_info in \
    "admin|Admin User|admin@orb-chrysa.local" \
    "developer|Developer User|developer@orb-chrysa.local"; do
    name="$(printf '%s' "$user_info" | cut -d'|' -f1)"
    display="$(printf '%s' "$user_info" | cut -d'|' -f2)"
    email="$(printf '%s' "$user_info" | cut -d'|' -f3)"
    $CURL -f -H "$AUTH" \
        -H "Content-Type: application/json" \
        -d "{\"attrs\":{\"name\":[\"$name\"],\"displayname\":[\"$display\"],\"mail\":[\"$email\"]}}" \
        "$KANIDM_URL/v1/person" >/dev/null 2>&1 || true
done

for group in registry_admins registry_developers; do
    $CURL -f -H "$AUTH" \
        -H "Content-Type: application/json" \
        -d "{\"attrs\":{\"name\":[\"$group\"]}}" \
        "$KANIDM_URL/v1/group" >/dev/null 2>&1 || true
done

$CURL -f -H "$AUTH" \
    -H "Content-Type: application/json" \
    -d '{"attrs":{"name":["ci-bot"],"displayname":["CI Bot"],"entry_managed_by":["idm_admins"]}}' \
    "$KANIDM_URL/v1/service_account" >/dev/null 2>&1 || true
$CURL -f -H "$AUTH" \
    -H "Content-Type: application/json" \
    -d '["idm_admins"]' \
    "$KANIDM_URL/v1/service_account/ci-bot/_attr/entry_managed_by" -X PUT >/dev/null 2>&1 || true

ADMIN_USER_PW="$(recover_password admin || true)"
DEVELOPER_USER_PW="$(recover_password developer || true)"

echo "=== Creating groups and memberships ==="
$CURL -f -H "$AUTH" -H "Content-Type: application/json" \
    -d '["admin"]' \
    "$KANIDM_URL/v1/group/registry_admins/_attr/member" -X POST >/dev/null 2>&1 || true
$CURL -f -H "$AUTH" -H "Content-Type: application/json" \
    -d '["ci-bot"]' \
    "$KANIDM_URL/v1/group/registry_admins/_attr/member" -X POST >/dev/null 2>&1 || true
$CURL -f -H "$AUTH" -H "Content-Type: application/json" \
    -d '["developer"]' \
    "$KANIDM_URL/v1/group/registry_developers/_attr/member" -X POST >/dev/null 2>&1 || true

echo "=== Creating OAuth2 client ==="
# Kanidm attribute names are confusing, so map them through clearly-named vars:
#   oauth2_rs_origin         = the allowed OAuth2 redirect (callback) URL set
#   oauth2_rs_origin_landing = the Kanidm app-portal landing page
# OAUTH2_REDIRECT_URL MUST equal the server's [auth] redirect_uri.
OAUTH2_REDIRECT_URL="https://$REGISTRY_ENDPOINT/oauth2/callback"
OAUTH2_LANDING_URL="https://$REGISTRY_ENDPOINT"
$CURL -f -H "$AUTH" \
    -H "Content-Type: application/json" \
    -d "{\"attrs\":{\"name\":[\"orb-chrysa\"],\"displayname\":[\"Orb Chrysa Container Registry\"],\"oauth2_rs_origin\":[\"$OAUTH2_REDIRECT_URL\"],\"oauth2_rs_origin_landing\":[\"$OAUTH2_LANDING_URL\"]}}" \
    "$KANIDM_URL/v1/oauth2/_basic" >/dev/null 2>&1 || true

echo "=== Verifying OAuth2 redirect/landing mapping ==="
OAUTH2_GET_RESP="$($CURL -f -H "$AUTH" "$KANIDM_URL/v1/oauth2/orb-chrysa" 2>/dev/null || true)"
printf '%s\n' "$OAUTH2_GET_RESP" > "$WORK/oauth2-client.json"
echo "  oauth2_rs_origin=$(printf '%s' "$OAUTH2_GET_RESP" | jq -c '.attrs.oauth2_rs_origin // empty' 2>/dev/null || true)"
echo "  oauth2_rs_origin_landing=$(printf '%s' "$OAUTH2_GET_RESP" | jq -c '.attrs.oauth2_rs_origin_landing // empty' 2>/dev/null || true)"
# Landing is the bare root, so the callback URL can only appear in oauth2_rs_origin.
if ! printf '%s' "$OAUTH2_GET_RESP" | grep -qF "$OAUTH2_REDIRECT_URL"; then
    echo "ERROR: oauth2_rs_origin does not contain the redirect URL $OAUTH2_REDIRECT_URL" >&2
    cat "$WORK/oauth2-client.json" >&2
    exit 1
fi

$CURL -f -H "$AUTH" \
    -H "Content-Type: application/json" \
    -d '["openid","profile","email","groups","oci_admin"]' \
    "$KANIDM_URL/v1/oauth2/orb-chrysa/_scopemap/registry_admins" -X POST >/dev/null 2>&1 || true
$CURL -f -H "$AUTH" \
    -H "Content-Type: application/json" \
    -d '["openid","profile","email","groups","oci_push","oci_pull"]' \
    "$KANIDM_URL/v1/oauth2/orb-chrysa/_scopemap/registry_developers" -X POST >/dev/null 2>&1 || true

echo "=== Getting client secret and ci-bot token ==="
CLIENT_SECRET="$($CURL -f -H "$AUTH" "$KANIDM_URL/v1/oauth2/orb-chrysa/_basic_secret" | tr -d '"' | tr -d '\n')"
if [ -z "$CLIENT_SECRET" ] || [ "$CLIENT_SECRET" = "null" ]; then
    echo "ERROR: failed to get OAuth client secret" >&2
    exit 1
fi

API_TOKEN_RESP="$($CURL -f -H "$AUTH" \
    -H "Content-Type: application/json" \
    -d "{\"label\":\"orb-chrysa-tilt\",\"expiry\":$KANIDM_API_TOKEN_EXPIRY,\"read_write\":false}" \
    "$KANIDM_URL/v1/service_account/ci-bot/_api_token" 2>&1 || true)"
printf '%s\n' "$API_TOKEN_RESP" > "$WORK/ci-bot-api-token-response.json"
CI_API_TOKEN="$(printf '%s\n' "$API_TOKEN_RESP" | jq -r 'if type == "string" then . else (.token // .api_token // empty) end' 2>/dev/null || true)"
if [ -z "$CI_API_TOKEN" ] || [ "$CI_API_TOKEN" = "null" ]; then
    echo "ERROR: failed to generate ci-bot API token" >&2
    cat "$WORK/ci-bot-api-token-response.json" >&2
    exit 1
fi

TOKEN_EXCHANGE_RESP="$($CURL -f \
    -H "Content-Type: application/x-www-form-urlencoded" \
    --data-urlencode "grant_type=urn:ietf:params:oauth:grant-type:token-exchange" \
    --data-urlencode "client_id=orb-chrysa" \
    --data-urlencode "subject_token=$CI_API_TOKEN" \
    --data-urlencode "subject_token_type=urn:ietf:params:oauth:token-type:access_token" \
    --data-urlencode "audience=orb-chrysa" \
    --data-urlencode "scope=openid profile email groups oci_admin" \
    "$KANIDM_URL/oauth2/token" 2>&1 || true)"
printf '%s\n' "$TOKEN_EXCHANGE_RESP" > "$WORK/ci-bot-token-exchange-response.json"
CI_TOKEN="$(printf '%s\n' "$TOKEN_EXCHANGE_RESP" | jq -r '.id_token // empty' 2>/dev/null || true)"
if [ -z "$CI_TOKEN" ] || [ "$CI_TOKEN" = "null" ]; then
    echo "ERROR: failed to exchange ci-bot API token for an OAuth id token" >&2
    cat "$WORK/ci-bot-token-exchange-response.json" >&2
    exit 1
fi

SIGNING_KEY_B64="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
ENCRYPTION_KEY_B64="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
TOKEN_SIGNING_KEYS="[\"$SIGNING_KEY_B64\"]"

echo "=== Writing Kubernetes Secrets ==="
kubectl create namespace "$ORB_NAMESPACE" --dry-run=client -o yaml | kubectl apply -f -
kubectl -n "$ORB_NAMESPACE" create secret generic orb-chrysa-auth \
    --from-literal=client_secret="$CLIENT_SECRET" \
    --from-literal=token_signing_keys="$TOKEN_SIGNING_KEYS" \
    --from-literal=session_encryption_key="$ENCRYPTION_KEY_B64" \
    --dry-run=client -o yaml | kubectl apply -f -
kubectl -n "$ORB_NAMESPACE" create secret generic orb-chrysa-test-auth \
    --from-literal=ci_bot_token="$CI_TOKEN" \
    --from-literal=ci_bot_api_token="$CI_API_TOKEN" \
    --from-literal=admin_password="$ADMIN_USER_PW" \
    --from-literal=developer_password="$DEVELOPER_USER_PW" \
    --dry-run=client -o yaml | kubectl apply -f -

printf '%s\n' "$CI_TOKEN" > "$WORK/ci-bot-token"
chmod 0600 "$WORK/ci-bot-token"
echo "Kanidm bootstrap complete"
