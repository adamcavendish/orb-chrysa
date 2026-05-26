# OCI Conformance Test runner for CI.
#
# The conformance binary must be pre-built on the host (Docker networking
# blocks Go module downloads in China). Build it first:
#
#   git clone --depth 1 --branch "$(cat tests/conformance/distribution-spec.ref)" https://github.com/opencontainers/distribution-spec.git /tmp/dist-spec
#   cd /tmp/dist-spec/conformance
#   CGO_ENABLED=0 GOOS=linux go test -c -o conformance.test .
#   cp conformance.test tests/conformance/
#
# Upstream reference (check for version bumps):
#   https://github.com/opencontainers/distribution-spec/blob/v1.1.1/conformance/Dockerfile
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY conformance.test /usr/local/bin/conformance.test

ENTRYPOINT ["conformance.test"]
