# Introduction

orb-chrysa is a **Rust OCI container registry** with built-in Raft clustering and S3 blob
storage. It is a single binary with no external service dependencies — no PostgreSQL,
Redis, or etcd required.

## Why orb-chrysa?

orb-chrysa embeds distribution, consensus, and storage in a single process — no
external database or cache dependencies.

- **No external database.** Metadata stored in Raft state machine, not PostgreSQL
- **No Redis.** All coordination through Raft consensus
- **True HA.** Multi-node Raft clustering with automatic failover
- **Single binary.** ~30 MB, deploy anywhere

## Key Design Decisions

- **Raft for metadata, S3 for blobs.** Metadata changes go through Raft consensus;
  blob I/O goes directly to S3. This avoids the complexity and cost of distributed
  file systems.
- **Two crates, modules not crates.** `orb-chrysa-server` and `orb-chrysa-cli`.
  Internal boundaries use Rust modules, not separate crates.
- **Ephemeral Raft log.** redb-backed Raft log is lost on pod restart; state is
  recovered from S3 snapshots. No PersistentVolumeClaim needed.
- **DNS-based peer discovery.** No static peer list. Peers found via DNS lookup
  of the configured `discovery_dns` name. Node ID derived from hostname ordinal.
- **No backward compatibility.** Not yet deployed to production. Breaking changes
  to snapshot format, API, or config are acceptable.

## License

MIT OR Apache-2.0 (dual-licensed).
