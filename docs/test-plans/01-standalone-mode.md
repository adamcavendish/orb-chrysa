# Standalone Mode Test Plan — orb-chrysa

**Date**: 2026-05-22
**Type**: Test Plan
**Branch**: master
**Scope**: Single-node operation (no Raft peers)
**Framework**: Single docker container, local RustFS for S3

---

## Architecture Summary

- **Single node**: `orb-chrysa-0` (node_id=1), no peers
- **Self-bootstrap**: Node 0 bootstraps immediately as single-node cluster
- **Always leader**: No election, no quorum concerns
- **No DNS discovery**: No `discovery_dns` needed (or resolves only self)
- **S3 snapshot**: Same persistence path as multi-replica — upload on shutdown, restore on cold start
- **GC**: Runs unconditionally (single node is always leader)

### Key Endpoints

| Endpoint | Purpose |
|----------|---------|
| `/v2/` | OCI registry API |
| `/raft/status` | Cluster status (always `state: leader`, `voters: [{id:1}]`) |
| `/raft/join` | No-op (no peers to join) |
| `/raft/leave` | Rejected (last voter guard) |

---

## Features Tested

| Feature | Tests |
|---------|-------|
| Single-node startup and self-bootstrap | S1 |
| OCI push/pull without Raft peers | S2 |
| Cold restart from S3 snapshot | S3 |
| Graceful shutdown (snapshot upload, skip leave) | S4 |
| SIGKILL recovery | S5 |
| GC sweep on single node | S6 |
| Last voter guard | S7 |

## Features NOT Tested

These are explicitly not applicable to standalone mode, not hidden manual tests.
No executable manual plan is required here because each scenario contradicts the
single-node topology.

| Feature | Reason | Manual/Automation Status |
|---------|--------|--------------------------|
| Leader election | No peers — always leader | Not applicable in standalone mode |
| Data replication | No peers — nothing to replicate | Not applicable in standalone mode |
| Dynamic membership (join/leave) | No peers — join returns self-bootstrap, leave blocked | Not applicable in standalone mode |
| Follower reads | No followers | Not applicable in standalone mode |
| DNS discovery | Not configured / resolves only self | Not applicable in standalone mode |
| Network partition | Not applicable | Covered by multi-replica manual plan `K8S-MANUAL-PARTITION-01`, not standalone |

---

## Test Plan

### S1. Cold Start — Single Node Bootstraps

**Precondition**: Fresh RustFS, no S3 data.

**Steps**:
1. Start single container: `docker compose -f deploy/compose/standalone.yml up -d`
2. Wait for health
3. Check `/raft/status`

**Expected**:
- Node self-bootstraps immediately
- `state: leader`, `voters: [{id: 1}]`
- `leader_id: 1`
- No DNS discovery retries, no join loop

---

### S2. OCI Push/Pull — No Raft Replication Overhead

**Precondition**: Standalone node running.

**Steps**:
1. Push a manifest: `curl -X PUT .../manifests/latest`
2. Pull the manifest: `curl .../manifests/latest`
3. Check `/raft/status` for `last_applied_log`

**Expected**:
- Push succeeds (HTTP 201)
- Pull succeeds (HTTP 200, correct digest)
- `last_applied_log` increments (Raft commits locally, no replication step)
- No peer communication attempts in logs

---

### S3. Cold Restart — Restore from S3 Snapshot

**Precondition**: Standalone node with data pushed, graceful stop completed.

**Steps**:
1. `docker compose -f deploy/compose/standalone.yml stop`
2. `docker compose -f deploy/compose/standalone.yml up -d`
3. Check logs and `/raft/status`

**Expected**:
- Logs: `downloaded raft snapshot from S3`
- Logs: `restoring state from S3 snapshot`
- `has_restored=true` → `verify_or_rejoin` path
- `state: leader`, `voters: [{id: 1}]`
- All previously pushed manifests accessible
- `last_applied_log` restored from snapshot

---

### S4. Graceful Shutdown — Snapshot Upload, Skip Leave

**Precondition**: Standalone node running.

**Steps**:
1. `docker compose -f deploy/compose/standalone.yml stop`
2. Check logs
3. Check S3 for snapshot

**Expected**:
- Logs: `uploaded final snapshot to S3`
- Logs: `not enough voters to maintain quorum after leave, skipping` (or `last voter, skipping leave`)
- Node does NOT attempt leave (no peers)
- Snapshot file exists in S3: `raft-snapshots/1/latest.bin`

---

### S5. SIGKILL Recovery

**Precondition**: Standalone node with data, unclean shutdown.

**Steps**:
1. `docker kill --signal=KILL orb-chrysa-standalone`
2. Restart: `docker compose -f deploy/compose/standalone.yml up -d`
3. Check logs and data

**Expected**:
- Node restores from last S3 snapshot (may be stale if snapshot wasn't recent)
- `has_restored=true` → `verify_or_rejoin`
- Since no peers reachable and `node_id==1`: self-bootstraps from restored state
- Logs: `re-bootstrapped cluster from restored snapshot`
- Data from snapshot restored; data after snapshot may be lost (no replication)

---

### S6. GC Sweep on Single Node

**Precondition**: Standalone node with unreferenced blobs in S3.

**Steps**:
1. Push a manifest, note the blobs
2. Delete the manifest (push a new manifest to the same tag, or use GC mechanism)
3. Wait for GC interval
4. Check if unreferenced blobs are deleted from S3

**Expected**:
- GC runs (no leader gate blocks — node is always leader)
- Logs: `GC sweep completed`
- Unreferenced blobs deleted from S3
- Referenced blobs preserved

---

### S7. Last Voter Guard

**Precondition**: Standalone node running.

**Steps**:
1. `curl -X POST http://localhost:5050/raft/leave -d '{"node_id": 1}'`

**Expected**:
- Response: `result: last_voter` (or the handler returns early before JSON)
- Node NOT removed from its own cluster
- `/raft/status` still shows `voters: [{id: 1}]`

---

## Test Execution Priority

| Priority | Test | Why |
|----------|------|-----|
| P0 | S1 Cold start | Basic single-node bootstrap |
| P0 | S2 OCI push/pull | Core registry functionality |
| P1 | S3 Cold restart | Snapshot integrity |
| P1 | S4 Graceful shutdown | Snapshot upload correctness |
| P2 | S5 SIGKILL recovery | Unclean shutdown resilience |
| P2 | S6 GC sweep | GC correctness |
| P3 | S7 Last voter guard | Safety check |

---

## Traceability Matrix

### Tests → Design Decisions

| Test | Validates |
|------|-----------|
| S1 | Self-bootstrap without peers, no DNS dependency |
| S3 | S3 snapshot as sole persistence (ephemeral redb lost) |
| S4 | Shutdown snapshot upload, last-voter guard prevents leave |
| S5 | Recovery from snapshot alone (no peer to replicate from) |
| S6 | GC always runs when single node (no leader gate) |

---

## Prerequisites

- `docker compose` v2
- RustFS images available
- orb-chrysa Docker image built
- Port 5050 free on host
- `curl` and `jq` for CLI verification

## Running the Tests

```bash
# Start standalone
docker compose -f deploy/compose/standalone.yml up -d --build

# Wait for health
while ! curl -sf http://localhost:5050/v2/ >/dev/null; do sleep 1; done
echo "ready"

# Check status
curl -s http://localhost:5050/raft/status | jq .

# Push test data
# (use ORAS, Docker, or direct curl)

# Graceful stop
docker compose -f deploy/compose/standalone.yml stop
```

---

## Notes

- **No peers**: The standalone compose file uses a single service with no DNS aliases to other nodes. The config should omit `discovery_dns` or set it to a name that resolves only to self.
- **Always leader**: All leader-gated features (GC, mirror scheduler, writes) are always active.
- **No PVC needed**: Same ephemeral redb + S3 snapshot architecture as multi-replica.
