# Cluster Dashboard Test Plan

**Date**: 2026-05-25
**Type**: Product contract test plan
**Source**: Cluster dashboard product behavior, active UI mockups, and
implementation contracts
**Scope**: Cluster dashboard page, UI-oriented cluster APIs, voters/learners,
join/leave/remove actions, quorum guard behavior, and stale status handling.

---

## Product Contract Summary

The cluster dashboard is an operator view over Raft membership. It must explain
leader, voters, learners, health, quorum, and membership-change safety without
making destructive actions easy to perform accidentally.

## Features Tested

| Feature | Tests | Priority |
|---------|-------|----------|
| Cluster route and page chrome | C1 | P0 |
| Summary cards and topology | C2 | P1 |
| Voters table | C3 | P0 |
| Learners table | C4 | P1 |
| Cluster status API | C5 | P0 |
| Join node modal/API | C6 | P1 |
| Leave current node | C7 | P1 |
| Remove remote member | C8 | P1 |
| Quorum guard | C9 | P0 |
| Leader/follower dashboard behavior | C10 | P1 |
| Refresh failure and stale data | C11 | P0 |
| Locale/theme route smoke | C12 | P1 |

## Tests

### C1. Cluster Route And Page Chrome

**Steps**:
1. Start a 3-node compose cluster.
2. Load `#/cluster`.
3. Refresh the page.

**Expected**:
- Page title is Cluster Members.
- Freshness indicator is visible after successful API refresh.
- Summary health status appears, e.g. Cluster Operational.
- Refresh preserves route.

### C2. Summary Cards And Topology

**Steps**:
1. Load cluster dashboard with 3 voters and no learners.
2. Add or simulate a learner when supported.
3. Compare topology and table data.

**Expected**:
- Summary cards show Nodes, Leader, Voters, and Learners.
- Voters card includes quorum requirement.
- Topology shows leader and voters with address, role, status, and lag.
- Topology is a visual summary of table data, not separate source of truth.

### C3. Voters Table

**Steps**:
1. Inspect voters table in healthy cluster.
2. Stop one node.
3. Restart it.

**Expected**:
- Columns are Node ID, Address, Role, Status, Actions.
- Leader badge is distinct.
- Other voters use neutral voter badges.
- Status can show Healthy, Lagging, Unreachable, Leaving, or Removing.
- Leave appears for current node.
- Remove appears for remote voters.

### C4. Learners Table

**Steps**:
1. Load cluster page with no learners.
2. Start a joining node and observe learner state where available.

**Expected**:
- Columns are Node ID, Address, Role, Status, Actions.
- Empty state says:
  `No learners joined. New nodes appear here while they catch up before promotion to voter.`
- If explicit promotion is added, Promote appears only with clear readiness
  criteria.

### C5. Cluster Status API

**Steps**:
1. Call `GET /api/v1/admin/cluster/status`.
2. Compare to `/raft/status`.
3. Kill/restart nodes and re-check.

**Expected**:
- Response includes cluster id when available, leader id, term, quorum,
  healthy voters, updated_at, voters, and learners.
- Each member includes node id, address, role, status, commit index, and
  replication lag where available.
- API shape is UI-oriented and stable.

### C6. Join Node Modal And API

**Steps**:
1. Open Join Node modal.
2. Submit empty fields, duplicate node id, and unreachable address.
3. Submit a valid join request where supported.

**Expected**:
- Modal fields are Node ID and Address.
- Modal has Cancel and Join Node.
- Node ID must be unique.
- Address must be reachable by leader.
- Duplicate joins return structured conflict.
- Join failure leaves existing membership unchanged.
- Successful mutation returns updated cluster status when practical.

### C7. Leave Current Node

**Steps**:
1. Trigger Leave for the current node.
2. Cancel confirmation.
3. Confirm leave on follower.
4. Repeat with current leader where safe.

**Expected**:
- Confirmation names node id and address.
- If current node is leader, warning states leadership will transfer or a new
  election may occur.
- Cancel returns to normal row state.
- After successful leave, UI redirects to reachable leader if available.

### C8. Remove Remote Member

**Steps**:
1. Trigger Remove for a remote voter.
2. Cancel confirmation.
3. Confirm removal for an unreachable voter when quorum-safe.

**Expected**:
- Confirmation names node id and address.
- Confirmation states whether node is leader, voter, or learner.
- Warning explains quorum impact.
- Success updates voters/learners and summary cards.

### C9. Quorum Guard

**Steps**:
1. In a 3-voter cluster, stop one voter.
2. Attempt to remove a healthy voter.
3. Attempt to remove the unreachable voter.
4. Force stale/unknown status data and attempt removal.

**Expected**:
- Removing a healthy voter that would leave fewer healthy voters than new
  quorum requires is blocked.
- Removing an unreachable voter is allowed only if current cluster still has
  enough healthy voters to commit the change.
- If quorum safety cannot be determined, action is disabled with explanation.

### C10. Connected To Follower

**Steps**:
1. Load dashboard through a follower port.
2. Trigger join/remove/leave mutation where safe.

**Expected**:
- Reads work from follower.
- Mutations redirect or proxy to leader according to Raft routing behavior.
- UI handles leader redirect without duplicate mutation.

### C11. Refresh Failure And Stale Data

**Steps**:
1. Load cluster page with healthy data.
2. Break status endpoint or network.
3. Wait for refresh attempts.

**Expected**:
- Stale table and summary data remain visible.
- Error banner follows the shared dashboard contract.
- After three consecutive failures, full-page retry is shown.
- When refresh succeeds again, freshness and status update.

### C12. Locale And Theme Route Smoke

**Steps**:
1. With a healthy compose cluster and leader present, load `#/overview` and
   `#/cluster`.
2. Switch to Light theme.
3. Switch locale to Chinese and Arabic.
4. Refresh and deep-link back to `#/cluster`.

**Expected**:
- Cluster summary, voters table, learners table, quorum guard actions, and
  stale-data banners stay readable in light theme.
- Document `lang` and `dir` update for selected locale.
- Arabic RTL does not break topology cards, table actions, join/remove/leave
  modals, or quorum warnings.
- Topbar preference controls remain compact and do not wrap above the cluster
  summary at the tested viewport widths.
- Deep-linked cluster and overview routes emit no browser console errors while
  switching dark, light, system, English, Chinese, and Arabic.
- Locale/theme changes do not affect `/api/v1/admin/cluster/status` shape or
  leader detection.

## Runtime Cross-Checks

These product tests should be paired with `03-multi-replica-cluster.md` for:

- compose startup leader gate on ports 5050-5052
- leader election after crash
- quorum loss behavior
- graceful leave
- simultaneous restart
- DNS discovery and join loop
- snapshot restore

## Coverage Map

| Contract Area | Tests |
|---------------|-------|
| Cluster page | C1, C2 |
| Voters table | C3, C7, C8 |
| Learners table | C4 |
| Join node | C6 |
| Leave and remove | C7, C8, C9 |
| API design | C5-C8 |
| Edge cases | C9-C12 |
