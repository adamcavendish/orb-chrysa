# Docker Compose Deployment

Production multi-replica orb-chrysa with RustFS for local S3-compatible storage.

## Files

| File | Description |
|---|---|
| `standalone.yml` | Single orb-chrysa node + RustFS |
| `cluster.yml` | Three-node Raft cluster + RustFS |
| `config/standalone.toml` | Config for standalone mode |
| `config/cluster.toml` | Config for cluster mode |

## Quick start

```bash
# Single node
docker compose -f deploy/compose/standalone.yml up -d

# Three-node cluster
docker compose -f deploy/compose/cluster.yml up -d
```

## Rolling updates

Named services (`orb-chrysa-0`, `orb-chrysa-1`, `orb-chrysa-2`) allow
rolling restarts without downtime:

```bash
docker compose -f deploy/compose/cluster.yml up -d --no-deps orb-chrysa-0
docker compose -f deploy/compose/cluster.yml up -d --no-deps orb-chrysa-1
docker compose -f deploy/compose/cluster.yml up -d --no-deps orb-chrysa-2
```

Each node gracefully leaves the Raft cluster on shutdown (uploads snapshot,
removes itself from membership), then the updated container rejoins on start.

## Configuration

Override defaults via environment variables:

```bash
RUSTFS_ACCESS_KEY=mykey RUSTFS_SECRET_KEY=mysecret \
  docker compose -f deploy/compose/cluster.yml up -d
```

For persistent configuration, copy `config/cluster.toml` and edit.
