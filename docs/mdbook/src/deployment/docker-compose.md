# Docker Compose

Multi-replica orb-chrysa with RustFS for local S3-compatible storage.

## Architecture

Named services (`orb-chrysa-0`, `orb-chrysa-1`, `orb-chrysa-2`) provide
stable hostnames for Raft peer discovery. RustFS provides the S3 API for
blob storage.

## Quick start

```bash
# Single node
docker compose -f deploy/compose/standalone.yml up -d

# Three-node cluster
docker compose -f deploy/compose/cluster.yml up -d
```

Check status:

```bash
curl http://localhost:5050/healthz
curl http://localhost:5050/raft/status | jq .
```

## Configuration

Compose files are in `deploy/compose/`:

| File | Description |
|---|---|
| `standalone.yml` | Single node + RustFS |
| `cluster.yml` | Three-node cluster + RustFS |
| `config/standalone.toml` | Server config for standalone |
| `config/cluster.toml` | Server config for cluster |

Override S3 credentials via environment:

```bash
RUSTFS_ACCESS_KEY=mykey RUSTFS_SECRET_KEY=mysecret \
  docker compose -f deploy/compose/cluster.yml up -d
```

## Rolling updates

Named services allow rolling restarts without downtime. Each node gracefully
leaves the Raft cluster on shutdown (uploads snapshot, removes itself from
membership), then the updated container rejoins on start.

```bash
# Update one node at a time
docker compose -f deploy/compose/cluster.yml up -d --no-deps orb-chrysa-0
sleep 10  # wait for rejoin
docker compose -f deploy/compose/cluster.yml up -d --no-deps orb-chrysa-1
sleep 10
docker compose -f deploy/compose/cluster.yml up -d --no-deps orb-chrysa-2
```

Quorum is maintained as long as at least 2 of 3 nodes are available during
the rollout.

## Networking

Nodes communicate over the default Compose network. The `discovery_dns`
setting must match the service name prefix:

```toml
[raft]
discovery_dns = "orb-chrysa"  # matches service names orb-chrysa-0, orb-chrysa-1, ...
```

## Limitations

- Compose does not orchestrate scale-up/down automatically. Add or remove
  named services manually.
- Host port mapping (`5050`, `5051`, `5052`) assumes no port conflicts.
  Use a reverse proxy (nginx, traefik) for production.
