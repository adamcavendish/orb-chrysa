# Scale-Up Test Plan — orb-chrysa

**Date**: 2026-05-22
**Type**: Test Plan
**Branch**: master
**Scope**: Expanding from standalone (1 voter) to multi-replica (2+ voters)
**Framework**: docker-compose — transition from standalone to 3-node cluster

---

## Architecture Summary

The upgrade path is: start a single node, then add replicas. New nodes discover the
existing leader via DNS and join. No manual reconfiguration needed — it's the same
`join_cluster` flow that handles cold-start cluster formation.

### Key flows

| Step | Who | What happens |
|------|-----|-------------|
| 1. Standalone | Node 0 | Self-bootstraps, voters=[1], leader |
| 2. New node starts | Node 1 (node_id=2) | DNS discovery finds node 0, `request_join` → leader |
| 3. Leader adds learner | Node 0 | `add_learner(2, blocking=true)` — replicates state to new node |
| 4. Leader promotes | Node 0 | `change_membership(AddVoterIds([2]))` — joint consensus [1]→[1,2] |
| 5. Repeat | Node 2 (node_id=3) | Same flow for third node → voters=[1,2,3] |

### Potential failure modes

- **Stale snapshot on new node**: New node might have S3 data from a prior cluster incarnation. `verify_or_rejoin` should handle this.
- **Pending config change**: If standalone leader has a stale pending change, join is blocked. Our `is_stale_config_change` detection covers this.
- **Quorum during transition**: 1→2 voters: joint consensus needs old config (1 node) to agree. 2→3 voters: needs 2/2 old config + 2/3 new config.
- **Node ID collision**: New nodes must have unique node_ids derived from hostname ordinals.

---

## Features Tested

| Feature | Tests |
|---------|-------|
| Standalone → 2-voter expansion | U1 |
| Standalone → 3-voter expansion | U2 |
| Data preserved through expansion | U3 |
| New node state catch-up (empty → full) | U4 |
| Full restart after expansion | U5 |
| Expansion during writes | U6 |
| Self-bootstrap race (new node shouldn't bootstrap if cluster exists) | U7 |

## Features NOT Tested

These are not covered by this automated compose-oriented scale-up plan. The
table states whether a separate manual plan exists or whether the gap is still
explicitly unplanned.

| Feature | Reason | Manual/Automation Status |
|---------|--------|--------------------------|
| 3→5+ voter expansion | Same mechanism as 1→3, just more nodes | Not automated here; no manual plan currently required unless 5+ voters become a production contract |
| Cross-datacenter expansion | Requires WAN latency simulation | Backlog/non-contract until WAN or multi-region clustering is a production contract |
| Rollback (scale down) | Compose uses explicit leave; Kubernetes uses StatefulSet desired-replica reconciliation plus preStop delay | Covered by `03-multi-replica-cluster.md` and `just tilt-scale-smoke` |
| K8s StatefulSet scale-up/down | Same Raft logic, different orchestration | Automated by `just tilt-scale-smoke`; evidence under `target/tilt/evidence/<run_id>-scale` |

---

## Test Plan

### U1. Standalone → 2-Voter Expansion

**Precondition**: Single node (orb-chrysa-0) running with data pushed. No S3 data for node 1.

**Steps**:
1. Start standalone node, push test manifest
2. Verify: `state: leader`, `voters: [{id: 1}]`
3. Start orb-chrysa-1 (node_id=2) alongside
4. Wait for join to complete (check logs)
5. Check `/raft/status` on both nodes

**Expected**:
- Node 1 logs: `successfully joined cluster`
- Node 0 logs: `node joined cluster node_id=2`
- Both nodes: `voters: [{id: 1}, {id: 2}]`, agree on leader
- `last_applied_log` consistent on both
- Test manifest accessible on both nodes
- Node 1's state matches node 0 (full catch-up via Raft replication)

---

### U2. Standalone → 3-Voter Expansion

**Precondition**: U1 complete (2-voter cluster). No S3 data for node 2.

**Steps**:
1. With 2-voter cluster running, start orb-chrysa-2 (node_id=3)
2. Wait for join
3. Check `/raft/status` on all nodes

**Expected**:
- Node 2 logs: `successfully joined cluster`
- All nodes: `voters: [{id: 1}, {id: 2}, {id: 3}]`
- `last_applied_log` consistent on all 3
- Test manifest accessible on all nodes

---

### U3. Data Preserved Through Expansion

**Precondition**: Standalone node with data: manifests, tags.

**Steps**:
1. Push multiple manifests to standalone node
2. Record tags and digests
3. Add nodes 1 and 2 (scale to 3 voters)
4. Verify all manifests and tags on all 3 nodes

**Expected**:
- All manifests accessible on all nodes with identical digests
- All tags resolve to correct digests
- No data loss, no corruption
- `last_applied_log` on new nodes catches up to leader

---

### U4. New Node State Catch-Up

**Precondition**: Standalone node with substantial data (50+ manifests).

**Steps**:
1. Push 50 unique manifests to standalone node
2. Start node 1 (fresh, no snapshot)
3. Monitor node 1's `/raft/status` during join
4. Compare state between nodes

**Expected**:
- Node 1 starts with `last_applied_log: null` (no state)
- `add_learner(blocking=true)` replicates all 50 manifests
- After join, `last_applied_log` equals leader's
- All 50 manifests accessible on node 1
- Join time scales with data volume but completes (may take seconds for 50 manifests)

---

### U5. Full Restart After Expansion

**Precondition**: 3-voter cluster (scaled up from standalone), graceful stop.

**Steps**:
1. After U2, `docker compose stop` (graceful shutdown, snapshots uploaded)
2. `docker compose up -d` (simultaneous restart)
3. Check cluster formation

**Expected**:
- All nodes restore from S3 snapshots
- `verify_or_rejoin`: quorum confirms membership
- `voters=[1,2,3]`, consistent `last_applied_log`
- Data intact on all nodes
- No self-bootstrap by node 0 (snapshot has cluster state)

---

### U6. Expansion During Active Writes

**Precondition**: Standalone node with write loop running.

**Steps**:
1. Start a script pushing manifests to standalone node in a loop
2. While writes are active, start node 1
3. After node 1 joins, start node 2
4. Stop write loop, verify final state

**Expected**:
- Writes continue to succeed during expansion (no downtime)
- New nodes catch up including manifests pushed during join
- Final `last_applied_log` consistent across all nodes
- No write failures due to joint consensus transition

---

### U7. New Node Doesn't Self-Bootstrap When Cluster Exists

**Precondition**: Standalone leader running.

**Steps**:
1. Start node 1 with `node_id=1` (same node ID as leader, simulating misconfiguration)
2. Or: start node 1 (node_id=2) but block DNS so it can't discover leader
3. Check behavior

**Expected (same ID)**:
- Node 1's join request gets `result: already_member` (leader sees node_id=1 already a voter)
- Node 1 does NOT self-bootstrap (node_id=1 but not ordinal-0, or join_cluster path)

**Expected (no DNS)**:
- Node 1 retries DNS discovery with exponential backoff
- `node_id != 1`, so does NOT self-bootstrap even when no peers reachable
- Logs: `no peers reachable for membership verification` (verify_or_rejoin) or continues retrying (join_cluster)
- When DNS restored, discovers and joins automatically

---

## Test Execution Priority

| Priority | Test | Why |
|----------|------|-----|
| P0 | U1 Standalone → 2-voter | Core expansion mechanism |
| P0 | U3 Data preserved | No data loss during scale-up |
| P1 | U2 Standalone → 3-voter | Full scale-up path |
| P1 | U4 State catch-up | Replication integrity for new nodes |
| P2 | U5 Full restart after expansion | Snapshot consistency after scale-up |
| P2 | U6 Expansion during writes | Zero-downtime scale-up |
| P3 | U7 No self-bootstrap race | Safety against misconfiguration |

---

## Traceability Matrix

### Tests → Design Decisions

| Test | Validates |
|------|-----------|
| U1, U2 | `handle_join` works for 1→N voter expansion (same code path as cold-start join) |
| U3 | Raft replication preserves all committed state during membership change |
| U4 | `add_learner(blocking=true)` correctly replicates full state to empty nodes |
| U5 | Post-expansion snapshots are consistent; `verify_or_rejoin` handles expanded cluster |
| U6 | Joint consensus doesn't block writes during transition |
| U7 | Node ID uniqueness enforced; ordinal-0 bootstrap guard prevents split-brain |

### Tests → Bugs Caught

| Test | Bug | Severity | Fixed |
|------|-----|----------|-------|
| U5 | Bug 1: Snapshot Membership Inconsistency | High | `2566404` |
| U1, U2 | Bug 2: Stale Config Change | Medium | `2566404` `44357c6` |
| U1, U2 | Bug 3: Learner Stuck After Re-join | Medium | `44357c6` |

---

## Prerequisites

- `docker compose` v2
- RustFS images available
- orb-chrysa Docker image built
- Ports 5050-5052 free on host
- `curl` and `jq` for CLI verification

## Running the Tests

```bash
# 1. Start standalone
docker compose -f deploy/compose/standalone.yml up -d --build
while ! curl -sf http://localhost:5050/v2/ >/dev/null; do sleep 1; done

# 2. Push test data
# (use ORAS, Docker, or direct curl)

# 3. Verify standalone
curl -s http://localhost:5050/raft/status | jq .
# → voters: [{id: 1}], state: leader

# 4. Add node 1
# Start orb-chrysa-1 (point to same RustFS, same config, discovery_dns resolves both)
docker compose -f docker-compose.scale-up.yml up -d orb-chrysa-1
sleep 5

# 5. Verify 2-voter cluster
curl -s http://localhost:5050/raft/status | jq '.voters'
# → [{id: 1}, {id: 2}]
curl -s http://localhost:5051/raft/status | jq '.voters'
# → [{id: 1}, {id: 2}]

# 6. Add node 2
docker compose -f docker-compose.scale-up.yml up -d orb-chrysa-2
sleep 5

# 7. Verify 3-voter cluster
curl -s http://localhost:5050/raft/status | jq '.voters'
# → [{id: 1}, {id: 2}, {id: 3}]
```

---

## Notes

- **No API call needed**: Expansion is automatic — new nodes discover the leader via DNS and call `request_join`. The operator just starts a new container.
- **Same config everywhere**: All nodes share identical config. `discovery_dns` resolves all nodes in the compose network.
- **New nodes start empty**: They have no redb log and no S3 snapshot (unless restarting from a prior run). `add_learner(blocking=true)` replicates the full state.
- **Shrink is orchestrator-specific**: Compose shrink uses `/raft/leave`; Kubernetes shrink uses the chart's StatefulSet desired-replica reconciler so multi-pod scale-down can commit one voter replacement before terminating pods exit.
