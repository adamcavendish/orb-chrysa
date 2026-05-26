# Snapshot & Recovery

Raft snapshots are the mechanism for state recovery and cold-start. They are stored
in S3 and downloaded on startup.

## Snapshot Lifecycle

1. After 1000 log entries are committed, a snapshot is triggered
2. The state machine serializes its in-memory data to JSON
3. The snapshot is uploaded to `s3://<bucket>/raft-snapshots/<node-id>/latest.bin`
4. Log entries before the snapshot are compacted (redb `Compact()`)

## Cold Start Recovery

On pod restart, the ephemeral redb log is lost. Recovery proceeds as follows:

1. Download the latest snapshot from S3
2. Deserialize the state machine data
3. Seed the Raft vote from the snapshot's `last_applied_log`
4. Initialize the state machine with the restored data
5. Join the cluster — Raft replication catches up from the snapshot point

## Snapshot Version

The snapshot format has a version header (`SNAPSHOT_FORMAT_VERSION = 4`). Snapshots
with an older version are rejected. Since Orb Chrysa is not yet deployed to
production, breaking snapshot format changes are acceptable.

## Kubernetes Considerations

Because redb is ephemeral and state is restored from S3 snapshots:

- **No PersistentVolumeClaim needed** for the Raft log
- Pods can be rescheduled freely — state is recovered from S3
- Cold start adds a few seconds of recovery time
