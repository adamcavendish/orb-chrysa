# Monitoring

## Prometheus Metrics

Available at `GET /metrics`.

| Metric | Labels | Description |
|--------|--------|-------------|
| `orb_chrysa_up` | none | Process liveness |
| `orb_chrysa_raft_leader` | none | 1 if this node is the leader, 0 otherwise |
| `orb_chrysa_raft_quorum` | none | Required healthy voters for quorum |
| `orb_chrysa_raft_healthy_voters` | none | Voters caught up with the leader |
| `orb_chrysa_gc_last_run_timestamp_seconds` | none | Last GC sweep timestamp |
| `orb_chrysa_gc_last_deleted_blobs` | none | Blobs deleted by the last GC sweep |
| `orb_chrysa_gc_last_delete_errors` | none | Delete errors from the last GC sweep |
| `orb_chrysa_auth_jwks_keys` | none | Number of keys currently available for JWT validation |
| `orb_chrysa_auth_jwks_cache_age_seconds` | none | Age of the current last-good JWKS material |
| `orb_chrysa_auth_jwks_stale_cache` | none | 1 when validation is using stale last-good JWKS because all configured endpoints are unreachable |
| `orb_chrysa_auth_jwks_refresh_failures_total` | none | Total failed JWKS refresh attempts |
| `orb_chrysa_auth_jwks_endpoint_info` | `endpoint` | Active issuer or JWKS endpoint used by the latest successful refresh |

## Logging

Human-readable logs are the default. Set `ORB_CHRYSA_LOG_FORMAT=json` for JSON logs; the Helm chart uses JSON logs by default.

```bash
# Debug logging
RUST_LOG=orb_chrysa_server=debug cargo run

# Specific module logging
RUST_LOG=orb_chrysa_server::raft=debug cargo run

# JSON format
ORB_CHRYSA_LOG_FORMAT=json orb-chrysa-server --config config.toml
```

### Log Levels

| Level | Usage |
|-------|-------|
| `error` | Fatal errors requiring attention |
| `warn` | Degraded but operational (e.g., JWKS refresh failed) |
| `info` | Normal operations (startup, leadership changes, snapshot builds) |
| `debug` | Detailed request/response tracing |

## Alerting

Recommended alerts:

| Alert | Condition | Severity |
|-------|-----------|----------|
| No leader | `orb_chrysa_raft_leader == 0` for all nodes > 30s | Critical |
| Quorum lost | `orb_chrysa_raft_healthy_voters < orb_chrysa_raft_quorum` | Critical |
| JWKS refresh failure | `orb_chrysa_auth_jwks_refresh_failures_total` increasing for > 5 min | Warning |
| Stale JWKS cache in use | `orb_chrysa_auth_jwks_stale_cache == 1` near the `jwks_max_stale_seconds` window | Warning |
