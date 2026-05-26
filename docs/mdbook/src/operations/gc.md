# Garbage Collection

Orb Chrysa uses reference-count based garbage collection to clean up unreferenced
blobs from S3.

## How It Works

1. **Walk state machine** — iterate over all manifests in the Raft state machine
   and collect the set of referenced blob digests
2. **Compute unreferenced** — compare against blob reference counts
3. **Grace period check** — blobs uploaded within `grace_period_secs` are immune
4. **Commit tombstone** — write a delete request to Raft
5. **S3 DELETE** — after the tombstone is committed, issue the S3 DELETE

## Configuration

```toml
[gc]
interval_secs = 3600
grace_period_secs = 3600
dry_run = false
```

| Key | Description |
|-----|-------------|
| `interval_secs` | How often GC runs (default: 3600 = 1 hour) |
| `grace_period_secs` | Blobs younger than this are protected (default: 3600) |
| `dry_run` | If true, log what would be deleted without actually deleting |

## Grace Period

The grace period matches the upload session timeout (1 hour). Blobs that have been
uploaded but not yet referenced by a manifest are protected from premature deletion.

## Monitoring

GC status is available via the dashboard API:

```bash
curl http://localhost:5050/api/v1/admin/gc/status | jq
```

Response includes `last_run_at`, `scanned`, `deleted`, and `dry_run` fields.

## Notes

- GC walks the Raft state machine, not S3. It only considers blobs known to the
  metadata index
- Blob reference counts are updated on manifest push/delete
- The GC process is rate-limited to avoid impacting normal operations
