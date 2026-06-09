# Mirror And Proxy Production Workflow Test Plan

**Date**: 2026-05-26
**Type**: Runtime product test plan
**Source**: Mirror/proxy cache runtime behavior and production OCI workflow QA
**Scope**: End-to-end mirror rule execution, proxy-cache pull-through, Docker
manifest-list pull-through, warm-up, push mirror behavior, outbound proxy
validation, local upstream registry isolation, and cleanup.

---

## Product Contract Summary

Mirror and Proxy Cache must work with real OCI registries and real OCI clients.
Mirror rules create jobs that actively synchronize tags between layerhouse and
an upstream registry. Proxy Cache serves upstream content through a local prefix,
caches misses, and can warm selected tags. Both paths must honor credentials,
plain HTTP, and supported outbound proxy settings while keeping secrets out of
public admin responses.

This plan uses a disposable local `registry:2` container as the upstream so it
does not depend on Docker Hub availability, rate limits, or mutable public
images during execution.

## Safety Boundary

Run this only against disposable prefixes:

- `qa/mirror-*` for pull mirror rules and mirrored repositories.
- `qa/cache-*` for proxy-cache rules and cached repositories.
- `qa/push-*` for push mirror source/destination repositories.
- `upstream/*` inside the disposable upstream registry.

Cleanup must delete only rules, proxy-cache entries, repositories, and upstream
containers created by the current run ID.

## Tooling

Required local tools:

- `docker` for the disposable upstream registry container.
- `oras` for pushing/pulling OCI artifacts.
- `curl` and `jq` for admin API and Distribution API assertions.

The executable regression script is:

```bash
tests/production/mirror-proxy-workflow.sh
```

It assumes a running layerhouse compose cluster at `localhost:5050`, writes
evidence to `/tmp/orb-mirror-proxy-{RUN_ID}`, and joins the upstream registry
container to the `layerhouse_default` Docker network. Override `REGISTRY`,
`SCHEME`, `COMPOSE_NETWORK`, `UPSTREAM_IMAGE`, `RUN_ID`, or `EVIDENCE_ROOT` for
other environments.

Treat this as a local or self-hosted pre-release workflow rather than a
default hosted GitHub Actions job. It needs a live compose cluster, Docker
network access, ORAS, curl, jq, and a disposable upstream registry container.
Hosted CI may run it only when those capabilities are explicitly provisioned;
otherwise use `just check` for the normal CI-quality gate and run
`just production-smoke` manually before release.

## Features Tested

| Feature | Tests | Priority |
|---------|-------|----------|
| Local upstream registry isolation | MP1 | P0 |
| Pull mirror trigger and job run | MP2 | P0 |
| Proxy cache pull-through and cached hit | MP3 | P0 |
| Docker manifest-list proxy-cache matrix | MP3a | P0 |
| Proxy cache warm now job | MP4 | P0 |
| Push mirror trigger and upstream publication | MP5 | P1 |
| Outbound proxy validation | MP6 | P0 |
| Secret redaction | MP7 | P0 |
| Cleanup and cluster health | MP8 | P0 |

## Tests

### MP1. Local Upstream Registry Isolation

**Steps**:
1. Start a disposable upstream `registry:2` container on the compose network.
2. Expose it on a random localhost port for host-side seeding.
3. Verify `/v2/` from the host.
4. Configure layerhouse rules with the Docker-network registry name, not
   `localhost`.

**Expected**:
- Host can seed artifacts through `localhost:{random_port}`.
- layerhouse containers can reach the same registry by container name.
- No Docker Hub pulls happen during mirror/proxy workflow execution after the
  upstream registry image is present locally.

### MP2. Pull Mirror Trigger And Job Run

**Steps**:
1. Seed upstream repository `upstream/mirror-src` with tags `v1` and `v2`.
2. Create a manual pull mirror rule:
   - `local_prefix=qa/mirror-{RUN_ID}`
   - `upstream_registry={upstream_container}:5000`
   - `upstream_prefix=upstream/mirror-src`
   - `strategy={"type":"pattern","pattern":"v*"}`
   - `plain_http=true`
   - direct outbound proxy
3. Trigger `POST /api/v1/admin/mirror/rules/{id}/trigger`.
4. Poll `/api/v1/admin/mirror/jobs/{job_id}/runs` until a terminal status.
5. Pull `qa/mirror-{RUN_ID}:v1` and `:v2` from layerhouse with ORAS.

**Expected**:
- Trigger returns a mirror sync job with the rule ID.
- The run reaches `Succeeded`.
- `tags_synced` includes `v1` and `v2`.
- Pulled bytes from layerhouse exactly match upstream seed bytes.
- Jobs are observable through mirror job APIs and are read-only history.

### MP3. Proxy Cache Pull-Through And Cached Hit

**Steps**:
1. Seed upstream repository `upstream/cache-src` with tag `v1`.
2. Create a proxy-cache rule:
   - `local_prefix=qa/cache-{RUN_ID}`
   - `upstream_registry={upstream_container}:5000`
   - `upstream_prefix=upstream/cache-src`
   - no warm schedule
   - direct outbound proxy
3. Pull `qa/cache-{RUN_ID}:v1` from layerhouse.
4. Pause or otherwise make the upstream registry unavailable.
5. Pull the same tag from layerhouse again.

**Expected**:
- First pull fetches upstream, stores manifest metadata and blob bytes locally,
  and returns the artifact to the client.
- Second pull succeeds from layerhouse local storage even when the upstream is
  unavailable.
- Dashboard repository/detail APIs show the cached digest under the local cache
  repository.

### MP3a. Docker Manifest-List Proxy-Cache Matrix

**Steps**:
1. Build a scratch Docker image locally without pulling a base image.
2. Push the native-platform child image to the disposable upstream registry as
   `library/alpine:{run_id}`.
3. Publish a Docker manifest list at upstream `library/alpine:3` that points to
   the child image.
4. Create a proxy-cache rule with:
   - `local_prefix=qa/docker-root-{RUN_ID}`
   - `upstream_registry={upstream_container}:5000`
   - `upstream_prefix=" / "`
   - `plain_http=true`
5. Send `HEAD /v2/qa/docker-root-{RUN_ID}/library/alpine/manifests/3` through
   layerhouse and compare `Docker-Content-Digest` with upstream.
6. Send `GET /v2/qa/docker-root-{RUN_ID}/library/alpine/manifests/3` through
   layerhouse and verify the dashboard manifest list records the upstream
   manifest-list digest.
7. Pull `localhost:5050/qa/docker-root-{RUN_ID}/library/alpine:3` with Docker.
8. Remove the local pulled image tag, pause the upstream registry, and pull the
   same layerhouse tag again.
9. Create a second proxy-cache rule with:
   - `local_prefix=qa/docker-library-{RUN_ID}`
   - `upstream_prefix=" /library/ "`
10. Send `HEAD /v2/qa/docker-library-{RUN_ID}/alpine/manifests/3` through
   layerhouse and compare `Docker-Content-Digest` with upstream.
11. Send `GET /v2/qa/docker-library-{RUN_ID}/alpine/manifests/3` through
   layerhouse and verify the dashboard manifest list records the upstream
   manifest-list digest.
12. Pull `localhost:5050/qa/docker-library-{RUN_ID}/alpine:3` with Docker.

**Expected**:
- The smoke uses a local manifest list and does not depend on Docker Hub.
- `upstream_prefix="/"` is normalized to root, so Docker Hub-style
  `library/alpine:3` pulls through the cache.
- `upstream_prefix="/library/"` is normalized to `library`, so the local short
  repository `alpine:3` maps to upstream `library/alpine:3`.
- `HEAD` on an uncached Docker manifest list succeeds and returns the upstream
  manifest-list digest without caching the manifest body.
- `GET` on the manifest list caches the manifest body and records the local tag
  before Docker host content-cache behavior can hide registry writes.
- Docker's real pull sequence can fetch the manifest list, selected child
  manifest, config blob, and layers through layerhouse.
- A second Docker pull succeeds while upstream is paused, proving the local
  cache has the selected child manifest and blobs.

### MP4. Proxy Cache Warm Now Job

**Steps**:
1. Seed upstream repository `upstream/cache-warm` with tag `warm`.
2. Create a second proxy-cache rule with `warm_filters` pattern `warm`.
3. Call `POST /api/v1/admin/proxy-cache/{id}/warm`.
4. Poll `/api/v1/admin/mirror/jobs/{job_id}/runs` until terminal.
5. Pull `qa/cache-warm-{RUN_ID}:warm` from layerhouse.

**Expected**:
- Warm now returns a proxy-cache sync job.
- The run reaches `Succeeded`.
- `tags_synced` includes `warm`.
- The warmed artifact can be pulled without a preceding client miss.

### MP5. Push Mirror Trigger And Upstream Publication

**Steps**:
1. Push a local artifact to layerhouse repository `qa/push-src-{RUN_ID}:release`.
2. Create a manual push mirror rule:
   - `direction=push`
   - `local_prefix=qa/push-src-{RUN_ID}`
   - `upstream_registry={upstream_container}:5000`
   - `upstream_prefix=upstream/push-dst`
   - `strategy={"type":"pattern","pattern":"release"}`
3. Trigger the rule and poll the job run.
4. Pull `upstream/push-dst:release` directly from the upstream registry.

**Expected**:
- Push mirror run reaches `Succeeded`.
- The upstream registry receives the manifest and required blobs.
- Direct upstream pull bytes match the original local artifact.

### MP6. Outbound Proxy Validation

**Steps**:
1. PUT a mirror rule with `plain_http=true` against the disposable upstream
   registry.
2. PUT a proxy-cache rule with `insecure_tls=true` against an HTTPS registry
   test fixture that presents an untrusted certificate.
3. PUT a mirror rule using `outbound_proxy.protocol=http` and a syntactically
   valid proxy URL.
4. GET the rule and inspect public transport/proxy fields.
5. PUT a mirror rule or proxy-cache with both `plain_http=true` and
   `insecure_tls=true`.
6. PUT a mirror rule or proxy-cache with `outbound_proxy.protocol=https`.
7. PUT a direct proxy payload with stale URL/credentials included.

**Expected**:
- Plain HTTP upstream configuration uses `http://` for upstream registry calls.
- Insecure HTTPS upstream configuration keeps `https://` and accepts the
  untrusted upstream certificate.
- Public responses include `plain_http` and `insecure_tls` transport flags.
- API rejects conflicting Plain HTTP and Insecure HTTPS upstream modes.
- HTTP proxy configuration is accepted and returned without password values.
- HTTPS proxy is rejected with a structured validation error mentioning
  deferred `aioduct` HTTPS proxy support.
- Direct proxy clears URL, username, and password.
- UI must not offer HTTPS proxy until `aioduct` exposes a public HTTPS proxy
  constructor.

### MP7. Secret Redaction

**Steps**:
1. Create mirror and proxy-cache entries with upstream credentials and proxy
   credentials.
2. GET list and detail endpoints without `include_secrets`.
3. Inspect response JSON.

**Expected**:
- Public responses include `username_configured`, `password_configured`, and
  proxy credential configured booleans.
- Public responses do not include raw upstream passwords or proxy passwords.

### MP8. Cleanup And Cluster Health

**Steps**:
1. Delete test mirror rules and proxy-cache rules.
2. Delete disposable layerhouse repositories.
3. Stop/remove the upstream registry container.
4. Check `/api/v1/admin/cluster/status`.

**Expected**:
- Cleanup affects only the current run's `qa/*` prefixes and upstream
  container.
- Cluster still reports a non-null leader and enough healthy voters for quorum.
- No stale running mirror/proxy jobs remain for deleted test rules.

## Runtime Evidence To Record

For every run, record:

- Commit SHA and cluster image build date.
- ORAS, Docker, curl, and jq versions.
- Upstream container name, image, host port, and compose network.
- Mirror/proxy rule IDs and local prefixes.
- Docker manifest-list digest and native platform used for Docker pull-through.
- Triggered job IDs and final run JSON.
- Source and mirrored/cached/pushed manifest digests.
- Cluster leader and healthy voter count before and after the workflow.
- Cleanup responses and any remaining disposable repositories.

## Reference Command

```bash
tests/production/mirror-proxy-workflow.sh
```
