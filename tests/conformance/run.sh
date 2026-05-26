#!/usr/bin/env bash
# OCI Distribution Conformance Test Suite runner for orb-chrysa.
#
# Upstream docs:
#   https://github.com/opencontainers/distribution-spec/blob/v1.1.1/conformance/README.md
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
REQUESTED_REPORT_DIR="${OCI_REPORT_DIR:-$RESULTS_DIR}"
COMPOSE_FILE="$PROJECT_DIR/deploy/compose/standalone.yml"
COMPOSE_PROJECT="orb-chrysa-conformance"
CONFORMANCE_BIN="$SCRIPT_DIR/conformance.test"
DIST_SPEC_DIR="$SCRIPT_DIR/.distribution-spec"
DEFAULT_DISTRIBUTION_SPEC_REF="$(tr -d '[:space:]' < "$SCRIPT_DIR/distribution-spec.ref")"
DISTRIBUTION_SPEC_URL="${OCI_DISTRIBUTION_SPEC_URL:-https://github.com/opencontainers/distribution-spec.git}"
DISTRIBUTION_SPEC_REF="${OCI_DISTRIBUTION_SPEC_REF:-$DEFAULT_DISTRIBUTION_SPEC_REF}"
DISTRIBUTION_SPEC_REF_FILE="$DIST_SPEC_DIR/.orb-chrysa-ref"

if [ -z "$DISTRIBUTION_SPEC_REF" ]; then
    echo "ERROR: tests/conformance/distribution-spec.ref is empty" >&2
    exit 1
fi

cleanup() {
    echo "=== Cleaning up ==="
    docker compose -f "$COMPOSE_FILE" -p "$COMPOSE_PROJECT" down --volumes 2>/dev/null || true
}

trap cleanup EXIT

ensure_distribution_spec_source() {
    local current_ref=""
    current_ref="$(cat "$DISTRIBUTION_SPEC_REF_FILE" 2>/dev/null || true)"
    if [ -d "$DIST_SPEC_DIR/.git" ] && [ "$current_ref" = "$DISTRIBUTION_SPEC_REF" ]; then
        return
    fi

    rm -rf "$DIST_SPEC_DIR"
    git clone --depth 1 --branch "$DISTRIBUTION_SPEC_REF" "$DISTRIBUTION_SPEC_URL" "$DIST_SPEC_DIR"
    printf '%s\n' "$DISTRIBUTION_SPEC_REF" > "$DISTRIBUTION_SPEC_REF_FILE"
}

# Build conformance binary if missing or cached from a different upstream ref.
if [ ! -f "$CONFORMANCE_BIN" ] || [ "$(cat "$DISTRIBUTION_SPEC_REF_FILE" 2>/dev/null || true)" != "$DISTRIBUTION_SPEC_REF" ]; then
    echo "=== Building conformance test binary ==="
    echo "Distribution spec ref: $DISTRIBUTION_SPEC_REF"
    ensure_distribution_spec_source
    (cd "$DIST_SPEC_DIR/conformance" && CGO_ENABLED=0 go test -c -o "$CONFORMANCE_BIN" .)
    echo "Binary: $CONFORMANCE_BIN"
fi

echo "=== Building orb-chrysa ==="
(cd "$PROJECT_DIR" && docker compose -f "$COMPOSE_FILE" -p "$COMPOSE_PROJECT" build)

echo "=== Starting services ==="
docker compose -f "$COMPOSE_FILE" -p "$COMPOSE_PROJECT" up -d

echo "=== Waiting for orb-chrysa to be ready ==="
for i in $(seq 1 30); do
    if curl -sf http://localhost:5050/v2/ >/dev/null 2>&1; then
        echo "orb-chrysa is ready"
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "ERROR: orb-chrysa did not become ready"
        docker compose -f "$COMPOSE_FILE" -p "$COMPOSE_PROJECT" logs
        exit 1
    fi
    sleep 2
done

echo "=== Running conformance tests ==="
rm -rf "$RESULTS_DIR"
mkdir -p "$RESULTS_DIR"
if [ "$REQUESTED_REPORT_DIR" != "none" ]; then
    rm -rf "$REQUESTED_REPORT_DIR"
    mkdir -p "$REQUESTED_REPORT_DIR"
fi

export OCI_ROOT_URL="http://localhost:5050"
export OCI_NAMESPACE="conformance/test"
export OCI_CROSSMOUNT_NAMESPACE="conformance/crossmount"
export OCI_TEST_PULL="${OCI_TEST_PULL:-1}"
export OCI_TEST_PUSH="${OCI_TEST_PUSH:-1}"
export OCI_TEST_CONTENT_DISCOVERY="${OCI_TEST_CONTENT_DISCOVERY:-1}"
export OCI_TEST_CONTENT_MANAGEMENT="${OCI_TEST_CONTENT_MANAGEMENT:-1}"
export OCI_DELETE_MANIFEST_BEFORE_BLOBS="${OCI_DELETE_MANIFEST_BEFORE_BLOBS:-1}"
export OCI_HIDE_SKIPPED_WORKFLOWS="${OCI_HIDE_SKIPPED_WORKFLOWS:-1}"
export OCI_REPORT_DIR="$REQUESTED_REPORT_DIR"

set +e
"$CONFORMANCE_BIN" 2>&1 | tee "$RESULTS_DIR/conformance.log"
status="${PIPESTATUS[0]}"
set -e

if [ "$status" -eq 137 ] && [ "$REQUESTED_REPORT_DIR" != "none" ]; then
    cat > "$RESULTS_DIR/report-generation-killed.txt" <<EOF
The OCI conformance binary was killed with exit 137 while reports were enabled.
The runner re-executed the full conformance suite with OCI_REPORT_DIR=none.
Use conformance-no-report.log as evidence for the test result. The HTML/JUnit
report is missing because the local host/runtime killed report generation.
EOF

    echo "WARN: conformance report generation was killed; rerunning with OCI_REPORT_DIR=none" >&2
    export OCI_REPORT_DIR=none
    set +e
    "$CONFORMANCE_BIN" 2>&1 | tee "$RESULTS_DIR/conformance-no-report.log"
    status="${PIPESTATUS[0]}"
    set -e
fi

if [ "$status" -ne 0 ]; then
    exit "$status"
fi

echo ""
echo "=== Results ==="
if [ -f "$RESULTS_DIR/report.html" ]; then
    echo "HTML report: $RESULTS_DIR/report.html"
fi
if [ -f "$RESULTS_DIR/junit.xml" ]; then
    echo "JUnit report: $RESULTS_DIR/junit.xml"
fi
